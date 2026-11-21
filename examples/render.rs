//! Developer tool: scan a folder and write the treemap (boxes only, no text)
//! as a PPM image, so the layout and colours can be checked without a Mac.
//!
//!     cargo run --release --example render -- /some/folder out.ppm 900 800

use macmonger::layout::{self, DBox};
use macmonger::scan::{scan, Progress};
use macmonger::tree::ROOT;
use std::io::Write;

struct Img {
    w: i32,
    h: i32,
    px: Vec<u8>,
}

impl Img {
    fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, rgb: u32) {
        for yy in y.max(0)..(y + h).min(self.h) {
            for xx in x.max(0)..(x + w).min(self.w) {
                let i = ((yy * self.w + xx) * 3) as usize;
                self.px[i] = (rgb >> 16) as u8;
                self.px[i + 1] = (rgb >> 8) as u8;
                self.px[i + 2] = rgb as u8;
            }
        }
    }
    fn frame(&mut self, x: i32, y: i32, w: i32, h: i32, tl: u32, br: u32) {
        self.fill(x, y, w, 1, tl);
        self.fill(x, y, 1, h, tl);
        self.fill(x, y + h - 1, w, 1, br);
        self.fill(x + w - 1, y, 1, h, br);
    }
    fn draw(&mut self, b: &DBox) {
        let (x, y, w, h) = (b.x, b.y, b.w + 1, b.h + 1);
        let [face, light, dark] = layout::colors(b.depth);
        if b.depth >= 0 {
            self.frame(x, y, w, h, 0, 0);
        }
        if w > 2 && h > 2 {
            self.frame(x + 1, y + 1, w - 2, h - 2, light, dark);
            self.fill(x + 2, y + 2, w - 4, h - 4, face);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = args.get(1).map(String::as_str).unwrap_or(".");
    let out = args.get(2).map(String::as_str).unwrap_or("treemap.ppm");
    let w: i32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(900);
    let h: i32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(800);

    let start = std::time::Instant::now();
    let tree = scan(std::path::Path::new(dir), &Progress::default()).expect("cannot scan");
    eprintln!(
        "{} files, {} folders, {} in {:.2?}",
        tree.num_files,
        tree.num_dirs,
        macmonger::format::size_string(tree.node(ROOT).size),
        start.elapsed()
    );
    let boxes = layout::build(&tree, ROOT, 0, w, h, true);
    eprintln!("{} boxes", boxes.len());

    let mut img = Img {
        w,
        h,
        px: vec![0xD4; (w * h * 3) as usize],
    };
    for b in &boxes {
        img.draw(b);
    }
    let mut f = std::fs::File::create(out).unwrap();
    write!(f, "P6\n{} {}\n255\n", w, h).unwrap();
    f.write_all(&img.px).unwrap();
}
