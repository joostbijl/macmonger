//! Compact in-memory file tree. A system disk easily holds millions of files,
//! so nodes live in one flat arena (40 bytes each) and all names share a
//! single byte buffer. The children of a directory are always contiguous.

use std::borrow::Cow;
use std::ffi::OsStr;
use std::ops::Range;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

pub const NONE: u32 = u32::MAX;
pub const ROOT: u32 = 0;

pub const F_DIR: u16 = 1;
/// The pseudo entry that represents the free space of the volume.
pub const F_FREE: u16 = 2;

#[derive(Clone, Debug)]
pub struct Node {
    /// Bytes on disk. For directories: the sum of everything inside.
    pub size: u64,
    /// Modification time, seconds since the Unix epoch.
    pub mtime: i64,
    name_off: u32,
    pub parent: u32,
    pub first_child: u32,
    pub child_count: u32,
    name_len: u16,
    pub flags: u16,
}

impl Node {
    pub fn is_dir(&self) -> bool {
        self.flags & F_DIR != 0
    }
    pub fn is_free(&self) -> bool {
        self.flags & F_FREE != 0
    }
}

pub struct Tree {
    pub nodes: Vec<Node>,
    names: Vec<u8>,
    pub root_path: PathBuf,
    /// Capacity and free space of the volume the root lives on.
    pub total_space: u64,
    pub free_space: u64,
    /// True when the root is a mount point (a whole disk was opened).
    pub is_volume: bool,
    pub num_files: u64,
    pub num_dirs: u64,
}

impl Tree {
    pub fn new(root_path: PathBuf) -> Tree {
        let mut t = Tree {
            nodes: Vec::new(),
            names: Vec::new(),
            root_path,
            total_space: 0,
            free_space: 0,
            is_volume: false,
            num_files: 0,
            num_dirs: 0,
        };
        t.push(b"", NONE, F_DIR, 0, 0);
        t
    }

    /// Appends a node and returns its id. Callers must add all children of a
    /// directory in one uninterrupted run (see `set_children`).
    pub fn push(&mut self, name: &[u8], parent: u32, flags: u16, size: u64, mtime: i64) -> u32 {
        let id = self.nodes.len() as u32;
        let name = &name[..name.len().min(u16::MAX as usize)];
        self.nodes.push(Node {
            size,
            mtime,
            name_off: self.names.len() as u32,
            parent,
            first_child: NONE,
            child_count: 0,
            name_len: name.len() as u16,
            flags,
        });
        self.names.extend_from_slice(name);
        id
    }

    pub fn set_children(&mut self, dir: u32, first: u32, count: u32) {
        let n = &mut self.nodes[dir as usize];
        n.first_child = if count == 0 { NONE } else { first };
        n.child_count = count;
    }

    pub fn node(&self, id: u32) -> &Node {
        &self.nodes[id as usize]
    }

    pub fn name_bytes(&self, id: u32) -> &[u8] {
        let n = &self.nodes[id as usize];
        &self.names[n.name_off as usize..n.name_off as usize + n.name_len as usize]
    }

    pub fn name(&self, id: u32) -> Cow<'_, str> {
        String::from_utf8_lossy(self.name_bytes(id))
    }

    pub fn children(&self, id: u32) -> Range<u32> {
        let n = &self.nodes[id as usize];
        if n.child_count == 0 {
            0..0
        } else {
            n.first_child..n.first_child + n.child_count
        }
    }

    /// Number of ancestors; the root has depth 0.
    pub fn depth(&self, mut id: u32) -> i32 {
        let mut d = 0;
        while self.nodes[id as usize].parent != NONE {
            id = self.nodes[id as usize].parent;
            d += 1;
        }
        d
    }

    /// Full filesystem path of a node.
    pub fn path(&self, id: u32) -> PathBuf {
        let mut chain = Vec::new();
        let mut cur = id;
        while cur != NONE && cur != ROOT {
            chain.push(cur);
            cur = self.nodes[cur as usize].parent;
        }
        let mut p = self.root_path.clone();
        for &c in chain.iter().rev() {
            p.push(OsStr::from_bytes(self.name_bytes(c)));
        }
        p
    }

    /// Called once after scanning: totals directory sizes and sorts every
    /// directory's children largest-first (the layout relies on that order).
    pub fn finalize(&mut self) {
        // Children always have a higher index than their parent, so one
        // reverse pass propagates sizes all the way up.
        for i in (1..self.nodes.len()).rev() {
            let (size, parent, free) = {
                let n = &self.nodes[i];
                (n.size, n.parent, n.is_free())
            };
            if !free && parent != NONE {
                self.nodes[parent as usize].size += size;
            }
        }
        // Sorting moves nodes around inside their sibling run, but every node
        // stays inside that run, so walking all indices still visits each
        // directory exactly once.
        for i in 0..self.nodes.len() {
            let n = &self.nodes[i];
            if n.child_count > 1 {
                let (a, b) = (
                    n.first_child as usize,
                    (n.first_child + n.child_count) as usize,
                );
                self.nodes[a..b].sort_unstable_by_key(|n| std::cmp::Reverse(n.size));
            }
        }
        // Re-point children at the (possibly moved) parents.
        for i in 0..self.nodes.len() {
            for c in self.children(i as u32) {
                self.nodes[c as usize].parent = i as u32;
            }
        }
        self.num_dirs = self.nodes.iter().skip(1).filter(|n| n.is_dir()).count() as u64;
        self.num_files = self
            .nodes
            .iter()
            .filter(|n| !n.is_dir() && !n.is_free())
            .count() as u64;
    }
}
