//! The SpaceMonger 1.4 treemap layout, ported from `CFolderView::SizeFolders`
//! in the original source (MIT, (c) Sean Werkema).
//!
//! The items of a folder (sorted largest first) are dealt greedily into two
//! piles of roughly equal weight, the rectangle is cut across its longer side
//! in proportion to the piles, and each pile is subdivided again until a
//! single item is left or the rectangle is too small to be worth it. A folder
//! then lays out its own contents inside its frame, below its title strip.

use crate::tree::{Tree, NONE};

/// Width of a folder's frame.
pub const FRAME: i32 = 3;
/// Height of a folder's frame plus title strip (12 in the original, which
/// used a 9 px bitmap font; one more here for a 10 pt font).
pub const TITLE_H: i32 = 13;
/// Rectangles at or below this size are not subdivided or labelled
/// (the original's default "density" setting).
pub const MIN_W: i32 = 32;
pub const MIN_H: i32 = 24;

pub const B_DIR: u8 = 1;
pub const B_FREE: u8 = 2;

#[derive(Debug, Clone, PartialEq)]
pub struct DBox {
    /// Tree node shown by this box, or `NONE` for an anonymous filler that
    /// stands for items too small to draw.
    pub node: u32,
    /// Nesting depth, which picks the colour.
    pub depth: i32,
    pub flags: u8,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl DBox {
    pub fn is_dir(&self) -> bool {
        self.flags & B_DIR != 0
    }
    pub fn is_free(&self) -> bool {
        self.flags & B_FREE != 0
    }
}

struct Ctx<'a> {
    tree: &'a Tree,
    show_free: bool,
    out: Vec<DBox>,
}

/// Lays out the contents of `root` inside a view of `width` x `height`.
/// `depth` is `tree.depth(root)` (0 for the scan root),
/// so colours stay the same when zooming in.
pub fn build(
    tree: &Tree,
    root: u32,
    depth: i32,
    width: i32,
    height: i32,
    show_free: bool,
) -> Vec<DBox> {
    let mut ctx = Ctx {
        tree,
        show_free,
        out: Vec::new(),
    };
    if width > 1 && height > 1 {
        folder(&mut ctx, 0, 0, width - 1, height - 1, root, depth);
    }
    ctx.out
}

fn folder(ctx: &mut Ctx, x: i32, y: i32, w: i32, h: i32, dir: u32, depth: i32) {
    let items: Vec<u32> = ctx.tree.children(dir).collect();
    split(ctx, x, y, w, h, &items, depth);
}

fn weight(ctx: &Ctx, id: u32) -> u64 {
    let n = ctx.tree.node(id);
    if n.is_free() && !ctx.show_free {
        0
    } else {
        n.size
    }
}

fn split(ctx: &mut Ctx, x: i32, y: i32, w: i32, h: i32, items: &[u32], depth: i32) {
    let mut list1 = Vec::new();
    let mut list2 = Vec::new();
    let (mut sum1, mut sum2) = (0u128, 0u128);
    for &id in items {
        let size = weight(ctx, id) as u128;
        if size == 0 {
            continue;
        }
        if sum1 <= sum2 {
            list1.push(id);
            sum1 += size;
        } else {
            list2.push(id);
            sum2 += size;
        }
    }
    if sum1 + sum2 == 0 {
        return;
    }

    let (r1, r2) = if w > h {
        let s = (w as u128 * sum1 / (sum1 + sum2)) as i32;
        ((x, y, s, h), (x + s, y, w - s, h))
    } else {
        let s = (h as u128 * sum1 / (sum1 + sum2)) as i32;
        ((x, y, w, s), (x, y + s, w, h - s))
    };
    place(ctx, r1, &list1, depth);
    place(ctx, r2, &list2, depth);
}

fn place(ctx: &mut Ctx, (x, y, w, h): (i32, i32, i32, i32), list: &[u32], depth: i32) {
    if list.is_empty() {
        return;
    }
    let roomy = w > MIN_W && h > MIN_H;
    if list.len() > 1 && roomy {
        split(ctx, x, y, w, h, list, depth);
    } else if roomy {
        let id = list[0];
        let n = ctx.tree.node(id);
        if n.is_free() {
            ctx.out.push(DBox {
                node: id,
                depth: -1,
                flags: B_FREE,
                x,
                y,
                w,
                h,
            });
        } else if n.is_dir() {
            ctx.out.push(DBox {
                node: id,
                depth,
                flags: B_DIR,
                x,
                y,
                w,
                h,
            });
            folder(
                ctx,
                x + FRAME,
                y + TITLE_H,
                w - 2 * FRAME,
                h - TITLE_H - FRAME,
                id,
                depth + 1,
            );
        } else {
            ctx.out.push(DBox {
                node: id,
                depth,
                flags: 0,
                x,
                y,
                w,
                h,
            });
        }
    } else {
        ctx.out.push(DBox {
            node: NONE,
            depth,
            flags: 0,
            x,
            y,
            w,
            h,
        });
    }
}

/// The innermost named box under a point, as in the original: a click on a
/// folder's frame or title picks the folder, a click inside it picks whatever
/// is there. Fillers and the free-space block are not selectable.
pub fn hit_test(boxes: &[DBox], px: i32, py: i32) -> Option<usize> {
    for (i, b) in boxes.iter().enumerate() {
        if px > b.x && py > b.y && px < b.x + b.w && py < b.y + b.h {
            if b.is_dir() {
                let on_frame = px < b.x + FRAME
                    || py < b.y + TITLE_H
                    || px > b.x + b.w - FRAME
                    || py > b.y + b.h - FRAME;
                if on_frame {
                    return Some(i);
                }
            } else if b.node == NONE || b.is_free() {
                return None;
            } else {
                return Some(i);
            }
        }
    }
    None
}

/// The rainbow palette of SpaceMonger 1.4: (face, highlight, shadow) per
/// nesting depth, repeating every eight levels.
pub const PALETTE: [[u32; 3]; 8] = [
    [0xFF7F7F, 0xFFBFBF, 0xBF7F7F],
    [0xFFBF7F, 0xFFDFBF, 0xBF9F5F],
    [0xFFFF00, 0xFFFFBF, 0xBFBF3F],
    [0x7FFF7F, 0xBFFFBF, 0x7FBF7F],
    [0x7FFFFF, 0xDFFFFF, 0x7FBFBF],
    [0xBFBFFF, 0xDFDFFF, 0x9F9FFF],
    [0xBFBFBF, 0xDFDFDF, 0x9F9F9F],
    [0xFF7FFF, 0xFFBFFF, 0xBF7FBF],
];

/// Classic "3D face" grey, used for free space and the empty window.
pub const FACE_GREY: u32 = 0xD4D0C8;

pub fn colors(depth: i32) -> [u32; 3] {
    if depth < 0 {
        [FACE_GREY; 3]
    } else {
        PALETTE[(depth & 7) as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{F_DIR, F_FREE, ROOT};
    use std::path::PathBuf;

    /// root: big(600) dir{a(300) b(200) c(100)}, file(300), tiny(1), free(400)
    fn sample() -> Tree {
        let mut t = Tree::new(PathBuf::from("/x"));
        t.is_volume = true;
        t.free_space = 400;
        let first = t.nodes.len() as u32;
        let dir = t.push(b"dir", ROOT, F_DIR, 0, 0);
        t.push(b"file", ROOT, 0, 300, 0);
        t.push(b"tiny", ROOT, 0, 1, 0);
        t.push(b"", ROOT, F_FREE, 400, 0);
        t.set_children(ROOT, first, 4);
        let first = t.nodes.len() as u32;
        t.push(b"a", dir, 0, 300, 0);
        t.push(b"b", dir, 0, 200, 0);
        t.push(b"c", dir, 0, 100, 0);
        t.set_children(dir, first, 3);
        t.finalize();
        t
    }

    fn named<'a>(t: &Tree, boxes: &'a [DBox], name: &str) -> &'a DBox {
        boxes
            .iter()
            .find(|b| b.node != NONE && !b.is_free() && t.name(b.node) == name)
            .unwrap()
    }

    fn area(b: &DBox) -> i64 {
        b.w as i64 * b.h as i64
    }

    #[test]
    fn finalize_totals_exclude_free() {
        let t = sample();
        assert_eq!(t.node(ROOT).size, 901);
        assert_eq!(t.num_files, 5);
        assert_eq!(t.num_dirs, 1);
    }

    #[test]
    fn areas_are_proportional_and_inside_view() {
        let t = sample();
        let boxes = build(&t, ROOT, 0, 1001, 801, true);
        let total: i64 = 1000 * 800;
        let dir = named(&t, &boxes, "dir");
        let file = named(&t, &boxes, "file");
        let free = boxes.iter().find(|b| b.is_free()).unwrap();
        for (b, size) in [(dir, 600.0), (file, 300.0), (free, 400.0)] {
            let expect = total as f64 * size / 1301.0;
            let got = area(b) as f64;
            assert!(
                (got - expect).abs() / expect < 0.02,
                "{b:?} expected ~{expect}"
            );
        }
        for b in &boxes {
            assert!(
                b.x >= 0 && b.y >= 0 && b.x + b.w <= 1000 && b.y + b.h <= 800,
                "{b:?}"
            );
            assert!(b.w >= 0 && b.h >= 0);
        }
        assert_eq!(free.depth, -1);
        assert_eq!(dir.depth, 0);
        assert_eq!(named(&t, &boxes, "a").depth, 1);
    }

    #[test]
    fn top_level_boxes_tile_without_overlap() {
        let t = sample();
        let boxes = build(&t, ROOT, 0, 641, 481, true);
        let top: Vec<&DBox> = boxes.iter().filter(|b| b.depth <= 0).collect();
        let sum: i64 = top.iter().map(|b| area(b)).sum();
        assert_eq!(sum, 640 * 480);
        for (i, a) in top.iter().enumerate() {
            for b in &top[i + 1..] {
                let overlap =
                    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
                assert!(!overlap, "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn children_stay_inside_their_folder() {
        let t = sample();
        let boxes = build(&t, ROOT, 0, 900, 700, true);
        let dir = named(&t, &boxes, "dir").clone();
        for name in ["a", "b", "c"] {
            let c = named(&t, &boxes, name);
            assert!(c.x >= dir.x + FRAME && c.y >= dir.y + TITLE_H);
            assert!(c.x + c.w <= dir.x + dir.w - FRAME && c.y + c.h <= dir.y + dir.h - FRAME);
        }
    }

    #[test]
    fn hiding_free_space_gives_the_room_to_files() {
        let t = sample();
        let with = build(&t, ROOT, 0, 800, 600, true);
        let without = build(&t, ROOT, 0, 800, 600, false);
        assert!(without.iter().all(|b| !b.is_free()));
        assert!(area(named(&t, &without, "dir")) > area(named(&t, &with, "dir")));
    }

    #[test]
    fn zooming_keeps_depth_colours() {
        let t = sample();
        let dir = t.children(ROOT).find(|&c| t.name(c) == "dir").unwrap();
        let boxes = build(&t, dir, t.depth(dir), 800, 600, true);
        assert_eq!(boxes.len(), 3);
        assert!(boxes.iter().all(|b| b.depth == 1));
        assert_eq!(boxes.iter().map(area).sum::<i64>(), 799 * 599);
    }

    #[test]
    fn hit_testing_matches_the_original_rules() {
        let t = sample();
        let boxes = build(&t, ROOT, 0, 1000, 800, true);
        let dir = named(&t, &boxes, "dir").clone();
        let a = named(&t, &boxes, "a").clone();
        // Title strip selects the folder itself.
        let hit = hit_test(&boxes, dir.x + 10, dir.y + 5).unwrap();
        assert_eq!(boxes[hit], dir);
        // Inside selects the file.
        let hit = hit_test(&boxes, a.x + a.w / 2, a.y + a.h / 2).unwrap();
        assert_eq!(boxes[hit], a);
        // Free space is not selectable.
        let free = boxes.iter().find(|b| b.is_free()).unwrap();
        assert_eq!(
            hit_test(&boxes, free.x + free.w / 2, free.y + free.h / 2),
            None
        );
        assert_eq!(hit_test(&boxes, -5, -5), None);
    }

    #[test]
    fn tiny_views_and_empty_trees_do_not_panic() {
        let t = sample();
        for (w, h) in [
            (0, 0),
            (1, 1),
            (2, 2),
            (10, 500),
            (500, 10),
            (33, 25),
            (40, 30),
        ] {
            let boxes = build(&t, ROOT, 0, w, h, true);
            for b in &boxes {
                assert!(b.w >= 0 && b.h >= 0, "{w}x{h}: {b:?}");
            }
        }
        let empty = Tree::new(PathBuf::from("/empty"));
        assert!(build(&empty, ROOT, 0, 800, 600, true).is_empty());
    }

    #[test]
    fn huge_sizes_do_not_overflow() {
        let mut t = Tree::new(PathBuf::from("/x"));
        let first = t.nodes.len() as u32;
        t.push(b"a", ROOT, 0, u64::MAX / 4, 0);
        t.push(b"b", ROOT, 0, u64::MAX / 4, 0);
        t.push(b"c", ROOT, 0, u64::MAX / 8, 0);
        t.set_children(ROOT, first, 3);
        t.finalize();
        let boxes = build(&t, ROOT, 0, 30000, 20000, true);
        assert_eq!(boxes.len(), 3);
    }
}
