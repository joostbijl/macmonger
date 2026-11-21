//! Mounted volumes the user can pick from the Open menu.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Volume {
    pub name: String,
    pub path: PathBuf,
}

/// The startup disk first, then everything mounted under /Volumes
/// (USB sticks, external disks, disk images, network shares).
pub fn list_volumes() -> Vec<Volume> {
    let mut boot_name = String::from("Macintosh HD");
    let mut others = Vec::new();

    if let Ok(rd) = std::fs::read_dir("/Volumes") {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                // /Volumes/<startup disk> is a symlink to "/".
                if std::fs::canonicalize(entry.path())
                    .map(|p| p.as_os_str() == "/")
                    .unwrap_or(false)
                {
                    boot_name = name;
                }
            } else if ft.is_dir() {
                others.push(Volume {
                    name,
                    path: entry.path(),
                });
            }
        }
    }
    others.sort_by_key(|v| v.name.to_lowercase());

    let mut out = vec![Volume {
        name: boot_name,
        path: PathBuf::from("/"),
    }];
    out.extend(others);
    out
}
