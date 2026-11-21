# MacMonger

SpaceMonger 1.4 for macOS, written in Rust: a treemap that shows where the
space on a disk goes. Same rainbow colours, same layout, same way of working.

## Build and install

On a Mac with Rust installed (Apple Silicon, macOS 11 or later):

    ./build.sh install

This builds `target/MacMonger.app` and copies it to `/Applications`.
Use `./build.sh` without `install` to only build the bundle.

## Use

| Action | How |
| --- | --- |
| Open a disk | **Open** button: lists the startup disk and everything mounted under `/Volumes` (USB sticks, external disks), plus **Choose Folder…** |
| Make a folder the new 100% | double-click it, or select it and press **Zoom In** / Return |
| Go back up | **Zoom Out** (Backspace or Esc), **Zoom Full** |
| Show or hide free space | **Free Space** button |
| Go to a folder in Finder | select it and press **Show in Finder**; double-clicking a file reveals it in Finder |
| Rescan | **Reload** |

Clicking the title strip or frame of a folder selects the folder; clicking
inside selects what is under the mouse. The window title shows the selection's
path, its share of the disk and its size. Hovering shows path, size and date.
Right-click gives the same commands in a menu.

`MacMonger.app/Contents/MacOS/MacMonger /some/folder` scans a folder directly.

## Good to know

- **Full Disk Access.** macOS hides parts of the disk from apps (Mail, Photos,
  other users, system data). Without access those folders are skipped and the
  map is incomplete, and macOS asks for permission folder by folder. Grant it
  in System Settings → Privacy & Security → Full Disk Access. The app is
  ad-hoc signed, so macOS may ask again after a rebuild.
- **Sizes are allocated space**, not file length, in decimal units like Finder
  (1 GB = 1 000 000 000 bytes). Sparse files (for example Docker disk images)
  count for what they really occupy; hard links are counted once.
- **Startup disk.** The system volume and the Data volume are scanned as one
  disk. Local snapshots and purgeable space are not files, so files plus free
  space can add up to less than the disk size.
- Symlinks are not followed and other mounted volumes are not entered.
- iCloud folders that are not downloaded are not opened (that would download them).

## Differences from SpaceMonger 1.4

No Delete button, no Setup dialog, no zoom animation. "Run or Open" became
"Show in Finder". Labels use the system font at 10 pt instead of the 9 px
bitmap font, so a folder's title strip is one point taller.

## Development

The scanner, layout and formatting are plain Rust in `src/lib.rs` and run
anywhere (`cargo test`). The AppKit front end (`src/app.rs`, via `objc2`) only
builds on macOS. To look at a layout without a Mac:

    cargo run --release --example render -- /some/folder out.ppm 900 800

## Credits

Layout algorithm and colours are ported from
[SpaceMonger 1.4](https://github.com/seanofw/spacemonger1) by Sean Werkema (MIT).
