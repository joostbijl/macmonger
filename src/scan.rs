//! Parallel directory scanner.
//!
//! Rules:
//! * sizes are the bytes actually allocated on disk (`st_blocks * 512`), so
//!   sparse files, APFS clones' metadata and compressed files count for what
//!   they really cost;
//! * symlinks are never followed and other mounted volumes are not entered;
//! * a hard-linked file is counted once, and no directory is entered twice.
//!
//! macOS startup disk: "/" is a read-only system volume and the user's data
//! lives on a second volume mounted at /System/Volumes/Data, stitched into
//! "/" through firmlinks (/Users, /Applications, ...). Both volumes share one
//! APFS container, so both are scanned. /System/Volumes/Data itself is walked
//! last and only for directories that were not already reached through "/".

use std::collections::HashSet;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};

use crate::tree::{Tree, F_DIR, F_FREE, ROOT};

/// Shared between the scanning threads and the UI.
#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub bytes: AtomicU64,
    pub cancel: AtomicBool,
}

struct Raw {
    name: Vec<u8>,
    is_dir: bool,
    descend: bool,
    size: u64,
    mtime: i64,
}

struct State {
    tree: Tree,
    queue: Vec<(u32, PathBuf)>,
    /// Directory that must be walked after everything else (see module docs).
    deferred: Option<(u32, PathBuf)>,
    active: usize,
}

struct Ctx<'a> {
    state: Mutex<State>,
    wake: Condvar,
    /// (device, inode) of every directory entered and every multi-link file.
    seen: Mutex<HashSet<(u64, u64)>>,
    allowed_devs: Vec<u64>,
    defer_path: Option<PathBuf>,
    progress: &'a Progress,
}

/// Capacity and free bytes of the volume holding `path`.
pub fn volume_space(path: &Path) -> (u64, u64) {
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return (0, 0);
    };
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated string and `s` a valid out pointer.
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return (0, 0);
    }
    let unit = s.f_frsize as u64;
    (
        (s.f_blocks as u64).saturating_mul(unit),
        (s.f_bavail as u64).saturating_mul(unit),
    )
}

fn is_mount_point(path: &Path, dev: u64) -> bool {
    match path.parent() {
        None => true,
        Some(parent) => std::fs::metadata(parent)
            .map(|m| m.dev() != dev)
            .unwrap_or(true),
    }
}

/// Extra devices that belong to the same disk as "/" on macOS, and the
/// directory to walk last.
fn startup_disk_rules(root: &Path, allowed: &mut Vec<u64>) -> Option<PathBuf> {
    if !cfg!(target_os = "macos") || root != Path::new("/") {
        return None;
    }
    let volumes = Path::new("/System/Volumes");
    if let Ok(rd) = std::fs::read_dir(volumes) {
        for e in rd.flatten() {
            if let Ok(m) = e.metadata() {
                if m.is_dir() && !allowed.contains(&m.dev()) {
                    allowed.push(m.dev());
                }
            }
        }
    }
    let data = volumes.join("Data");
    data.is_dir().then_some(data)
}

#[cfg(target_os = "macos")]
fn is_dataless(md: &std::fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt as _;
    // SF_DATALESS: an iCloud/File Provider placeholder. Listing it would make
    // macOS download its contents.
    md.st_flags() & 0x4000_0000 != 0
}

#[cfg(not(target_os = "macos"))]
fn is_dataless(_md: &std::fs::Metadata) -> bool {
    false
}

fn read_dir(path: &Path, ctx: &Ctx) -> Vec<Raw> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(path) else {
        return out;
    };
    let (mut files, mut bytes) = (0u64, 0u64);
    for entry in rd {
        let Ok(entry) = entry else { continue };
        // DirEntry::metadata never follows symlinks.
        let Ok(md) = entry.metadata() else { continue };
        let size = md.blocks().saturating_mul(512);
        let name = entry.file_name().as_bytes().to_vec();
        if md.is_dir() {
            if !ctx.allowed_devs.contains(&md.dev()) {
                continue; // another volume mounted here
            }
            if !ctx.seen.lock().unwrap().insert((md.dev(), md.ino())) {
                continue; // already reached through another path
            }
            out.push(Raw {
                name,
                is_dir: true,
                descend: !is_dataless(&md),
                size,
                mtime: md.mtime(),
            });
        } else {
            if md.nlink() > 1 && !ctx.seen.lock().unwrap().insert((md.dev(), md.ino())) {
                continue; // second name of an already counted file
            }
            files += 1;
            bytes += size;
            out.push(Raw {
                name,
                is_dir: false,
                descend: false,
                size,
                mtime: md.mtime(),
            });
        }
    }
    ctx.progress.files.fetch_add(files, Ordering::Relaxed);
    ctx.progress.bytes.fetch_add(bytes, Ordering::Relaxed);
    ctx.progress.dirs.fetch_add(1, Ordering::Relaxed);
    out
}

fn worker(ctx: &Ctx) {
    loop {
        let (dir, path) = {
            let mut st = ctx.state.lock().unwrap();
            loop {
                if ctx.progress.cancel.load(Ordering::Relaxed) {
                    ctx.wake.notify_all();
                    return;
                }
                if let Some(job) = st.queue.pop() {
                    st.active += 1;
                    break job;
                }
                if st.active == 0 {
                    if let Some(job) = st.deferred.take() {
                        st.active += 1;
                        break job;
                    }
                    ctx.wake.notify_all();
                    return;
                }
                st = ctx.wake.wait(st).unwrap();
            }
        };

        let entries = read_dir(&path, ctx);

        let mut st = ctx.state.lock().unwrap();
        let first = st.tree.nodes.len() as u32;
        let mut count = 0u32;
        for e in &entries {
            let flags = if e.is_dir { F_DIR } else { 0 };
            let id = st.tree.push(&e.name, dir, flags, e.size, e.mtime);
            count += 1;
            if e.descend {
                let child = path.join(OsStr::from_bytes(&e.name));
                if ctx.defer_path.as_deref() == Some(child.as_path()) {
                    st.deferred = Some((id, child));
                } else {
                    st.queue.push((id, child));
                }
            }
        }
        if dir == ROOT && st.tree.is_volume && st.tree.free_space > 0 {
            let free = st.tree.free_space;
            st.tree.push(b"", ROOT, F_FREE, free, 0);
            count += 1;
        }
        st.tree.set_children(dir, first, count);
        st.active -= 1;
        drop(st);
        ctx.wake.notify_all();
    }
}

/// Scans `root` and returns the finished tree, or `None` when cancelled or
/// when `root` cannot be read at all.
pub fn scan(root: &Path, progress: &Progress) -> Option<Tree> {
    let md = std::fs::metadata(root).ok()?;
    if !md.is_dir() {
        return None;
    }
    let mut tree = Tree::new(root.to_path_buf());
    let (total, free) = volume_space(root);
    tree.total_space = total;
    tree.free_space = free;
    tree.is_volume = is_mount_point(root, md.dev());

    let mut allowed_devs = vec![md.dev()];
    let defer_path = startup_disk_rules(root, &mut allowed_devs);
    let mut seen = HashSet::new();
    seen.insert((md.dev(), md.ino()));

    let ctx = Ctx {
        state: Mutex::new(State {
            tree,
            queue: vec![(ROOT, root.to_path_buf())],
            deferred: None,
            active: 0,
        }),
        wake: Condvar::new(),
        seen: Mutex::new(seen),
        allowed_devs,
        defer_path,
        progress,
    };

    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| worker(&ctx));
        }
    });

    if progress.cancel.load(Ordering::Relaxed) {
        return None;
    }
    let mut tree = ctx.state.into_inner().unwrap().tree;
    tree.finalize();
    Some(tree)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("macmonger-test-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn find(tree: &Tree, parent: u32, name: &str) -> u32 {
        tree.children(parent)
            .find(|&c| tree.name(c) == name)
            .unwrap_or_else(|| panic!("no {name}"))
    }

    #[test]
    fn scans_sizes_links_and_order() {
        let root = temp_dir("scan");
        fs::create_dir_all(root.join("a/deep/er")).unwrap();
        fs::create_dir_all(root.join("b")).unwrap();
        fs::write(root.join("a/big.bin"), vec![1u8; 300_000]).unwrap();
        fs::write(root.join("a/deep/er/mid.bin"), vec![1u8; 100_000]).unwrap();
        fs::write(root.join("b/small.bin"), vec![1u8; 20_000]).unwrap();
        fs::write(root.join("top.bin"), vec![1u8; 50_000]).unwrap();
        fs::hard_link(root.join("a/big.bin"), root.join("b/big-again.bin")).unwrap();
        std::os::unix::fs::symlink(root.join("a"), root.join("loop")).unwrap();

        let progress = Progress::default();
        let tree = scan(&root, &progress).unwrap();
        assert!(!tree.is_volume);

        let a = find(&tree, ROOT, "a");
        let b = find(&tree, ROOT, "b");
        let deep = find(&tree, a, "deep");
        let er = find(&tree, deep, "er");
        let mid = find(&tree, er, "mid.bin");
        assert_eq!(tree.path(mid), root.join("a/deep/er/mid.bin"));
        assert_eq!(tree.depth(mid), 4);
        assert!(tree.node(mid).size >= 100_000);

        // The hard link is counted exactly once across the whole tree.
        let big_names = ["big.bin", "big-again.bin"];
        let count = (0..tree.nodes.len() as u32)
            .filter(|&i| big_names.contains(&tree.name(i).as_ref()))
            .count();
        assert_eq!(count, 1);

        // The symlink is a small file, not a second copy of "a".
        let link = find(&tree, ROOT, "loop");
        assert!(!tree.node(link).is_dir());

        // Directory sizes are the sum of their children, parents are intact
        // and siblings are sorted largest first.
        for i in 0..tree.nodes.len() as u32 {
            let kids: Vec<u32> = tree.children(i).collect();
            for w in kids.windows(2) {
                assert!(tree.node(w[0]).size >= tree.node(w[1]).size);
            }
            for &k in &kids {
                assert_eq!(tree.node(k).parent, i);
            }
        }
        let files_total: u64 = tree
            .nodes
            .iter()
            .filter(|n| !n.is_dir())
            .map(|n| n.size)
            .sum();
        assert!(tree.node(ROOT).size >= files_total);
        assert_eq!(tree.num_files, 5 - 1 + 1); // 4 regular files + symlink
        assert!(tree.node(a).size > tree.node(b).size || tree.node(b).size > 300_000);
        assert_eq!(progress.files.load(Ordering::Relaxed), tree.num_files);

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn cancelled_scan_returns_none() {
        let root = temp_dir("cancel");
        fs::write(root.join("x"), b"x").unwrap();
        let progress = Progress::default();
        progress.cancel.store(true, Ordering::Relaxed);
        assert!(scan(&root, &progress).is_none());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_root_returns_none() {
        assert!(scan(Path::new("/definitely/not/here"), &Progress::default()).is_none());
    }
}
