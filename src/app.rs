//! AppKit front end: one window with a button bar and the treemap view.

use std::cell::{OnceCell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSAutoresizingMaskOptions,
    NSBackingStoreType, NSBezelStyle, NSBezierPath, NSButton, NSButtonType, NSCellImagePosition,
    NSColor, NSControlStateValueOff, NSControlStateValueOn, NSEvent, NSEventModifierFlags, NSFont,
    NSFontAttributeName, NSForegroundColorAttributeName, NSGraphicsContext, NSImage, NSMenu,
    NSMenuItem, NSModalResponseOK, NSOpenPanel, NSRectFill, NSStringDrawing, NSTrackingArea,
    NSTrackingAreaOptions, NSView, NSWindow, NSWindowDelegate, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSArray, NSAttributedStringKey, NSDictionary, NSNotification,
    NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSTimer, NSURL,
};

use macmonger::format::{bytes_string, date_string, group_digits, percent_string, size_string};
use macmonger::layout::{self, DBox, FACE_GREY, TITLE_H};
use macmonger::scan::{scan, Progress};
use macmonger::tree::{Tree, NONE, ROOT};
use macmonger::volumes::{list_volumes, Volume};

const APP_NAME: &str = "MacMonger";
const BAR_HEIGHT: f64 = 38.0;
const FONT_SIZE: f64 = 10.0;
const LINE: i32 = 12;

struct ScanJob {
    progress: Arc<Progress>,
    result: Receiver<Option<Tree>>,
    path: PathBuf,
}

struct State {
    tree: Option<Tree>,
    zoom_root: u32,
    show_free: bool,
    selected: Option<u32>,
    hover: Option<u32>,
    boxes: Vec<DBox>,
    layout_size: (i32, i32),
    scan: Option<ScanJob>,
    message: Option<String>,
    volumes: Vec<Volume>,
}

struct Buttons {
    reload: Retained<NSButton>,
    zoom_full: Retained<NSButton>,
    zoom_in: Retained<NSButton>,
    zoom_out: Retained<NSButton>,
    free: Retained<NSButton>,
    finder: Retained<NSButton>,
}

struct ViewIvars {
    state: RefCell<State>,
    buttons: OnceCell<Buttons>,
}

type Attrs = NSDictionary<NSAttributedStringKey, AnyObject>;

fn rgb(hex: u32) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        ((hex >> 16) & 0xFF) as f64 / 255.0,
        ((hex >> 8) & 0xFF) as f64 / 255.0,
        (hex & 0xFF) as f64 / 255.0,
        1.0,
    )
}

fn fill(color: &NSColor, x: i32, y: i32, w: i32, h: i32) {
    if w <= 0 || h <= 0 {
        return;
    }
    color.set();
    NSRectFill(NSRect::new(
        NSPoint::new(x as f64, y as f64),
        NSSize::new(w as f64, h as f64),
    ));
}

fn text_attrs(font: &NSFont, color: &NSColor) -> Retained<Attrs> {
    // SAFETY: the two statics are valid attribute keys provided by AppKit.
    let keys = unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] };
    let font: &AnyObject = font;
    let color: &AnyObject = color;
    NSDictionary::from_slices(&keys, &[font, color])
}

fn text_width(s: &NSString, attrs: &Attrs) -> i32 {
    // SAFETY: `attrs` holds an NSFont and an NSColor under their proper keys.
    unsafe { s.sizeWithAttributes(Some(attrs)) }.width.ceil() as i32
}

fn draw_text(s: &NSString, x: i32, y: i32, attrs: &Attrs) {
    // SAFETY: as in `text_width`.
    unsafe { s.drawAtPoint_withAttributes(NSPoint::new(x as f64, y as f64), Some(attrs)) };
}

/// Draws `s` horizontally centred in a box, or left-aligned when too wide.
fn draw_centered(s: &str, x: i32, w: i32, y: i32, attrs: &Attrs) {
    let s = NSString::from_str(s);
    let tw = text_width(&s, attrs);
    let tx = if tw > w - 2 { x + 2 } else { x + (w - tw) / 2 };
    draw_text(&s, tx, y, attrs);
}

define_class!(
    // SAFETY:
    // - NSView permits subclassing; the overridden methods keep their contracts.
    // - `TreemapView` does not implement `Drop`.
    #[unsafe(super(NSView, objc2_app_kit::NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "MMTreemapView"]
    #[ivars = ViewIvars]
    struct TreemapView;

    unsafe impl NSObjectProtocol for TreemapView {}

    impl TreemapView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, dirty: NSRect) {
            self.draw(dirty);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let hit = self.node_at(event);
            self.select(hit);
            if event.clickCount() == 2 {
                if let Some(id) = hit {
                    if self.is_dir(id) {
                        self.zoom_to(id);
                    } else {
                        self.show_in_finder_node(Some(id));
                    }
                }
            }
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            let hit = self.node_at(event);
            let tip = {
                let mut st = self.ivars().state.borrow_mut();
                if st.hover == hit {
                    return;
                }
                st.hover = hit;
                match (hit, st.tree.as_ref()) {
                    (Some(id), Some(tree)) => Some(tooltip(tree, id)),
                    _ => None,
                }
            };
            match tip {
                Some(t) => self.setToolTip(Some(&NSString::from_str(&t))),
                None => self.setToolTip(None),
            }
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.ivars().state.borrow_mut().hover = None;
            self.setToolTip(None);
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            match event.keyCode() {
                36 | 76 => self.zoom_in_selected(), // Return / Enter
                51 | 53 => self.zoom_out_one(),     // Backspace / Escape
                _ => unsafe { msg_send![super(self), keyDown: event] },
            }
        }

        #[unsafe(method_id(menuForEvent:))]
        fn menu_for_event(&self, event: &NSEvent) -> Option<Retained<NSMenu>> {
            let hit = self.node_at(event);
            self.select(hit);
            Some(self.context_menu())
        }

        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            self.validate(item)
        }

        // ----- actions (buttons, menus, timer) -----

        #[unsafe(method(openMenu:))]
        fn open_menu(&self, sender: &NSButton) {
            let mtm = self.mtm();
            let volumes = list_volumes();
            let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Open"));
            for (i, v) in volumes.iter().enumerate() {
                let title = if v.path == Path::new("/") {
                    format!("{}  (startup disk)", v.name)
                } else {
                    v.name.clone()
                };
                let item = self.menu_item(&title, sel!(openVolume:), "");
                item.setTag(i as isize);
                menu.addItem(&item);
            }
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            menu.addItem(&self.menu_item("Choose Folder\u{2026}", sel!(chooseFolder:), ""));
            self.ivars().state.borrow_mut().volumes = volumes;
            let below = NSPoint::new(0.0, sender.bounds().size.height + 4.0);
            menu.popUpMenuPositioningItem_atLocation_inView(None, below, Some(sender));
        }

        #[unsafe(method(openVolume:))]
        fn open_volume(&self, sender: &NSMenuItem) {
            let path = {
                let st = self.ivars().state.borrow();
                st.volumes.get(sender.tag() as usize).map(|v| v.path.clone())
            };
            if let Some(path) = path {
                self.start_scan(path);
            }
        }

        #[unsafe(method(chooseFolder:))]
        fn choose_folder(&self, _sender: Option<&AnyObject>) {
            let panel = NSOpenPanel::openPanel(self.mtm());
            panel.setCanChooseDirectories(true);
            panel.setCanChooseFiles(false);
            panel.setAllowsMultipleSelection(false);
            panel.setShowsHiddenFiles(true);
            panel.setMessage(Some(ns_string!("Choose a disk or folder to scan")));
            if panel.runModal() != NSModalResponseOK {
                return;
            }
            let path = panel.URLs().firstObject().and_then(|u| u.path());
            if let Some(path) = path {
                self.start_scan(PathBuf::from(path.to_string()));
            }
        }

        #[unsafe(method(reload:))]
        fn reload(&self, _sender: Option<&AnyObject>) {
            let path = {
                let st = self.ivars().state.borrow();
                st.tree
                    .as_ref()
                    .map(|t| t.root_path.clone())
                    .or_else(|| st.scan.as_ref().map(|s| s.path.clone()))
            };
            if let Some(path) = path {
                self.start_scan(path);
            }
        }

        #[unsafe(method(zoomFull:))]
        fn zoom_full(&self, _sender: Option<&AnyObject>) {
            let changed = {
                let mut st = self.ivars().state.borrow_mut();
                let changed = st.tree.is_some() && st.zoom_root != ROOT;
                if changed {
                    st.zoom_root = ROOT;
                    st.selected = None;
                }
                changed
            };
            if changed {
                self.relayout();
            }
        }

        #[unsafe(method(zoomIn:))]
        fn zoom_in(&self, _sender: Option<&AnyObject>) {
            self.zoom_in_selected();
        }

        #[unsafe(method(zoomOut:))]
        fn zoom_out(&self, _sender: Option<&AnyObject>) {
            self.zoom_out_one();
        }

        #[unsafe(method(toggleFree:))]
        fn toggle_free(&self, _sender: Option<&AnyObject>) {
            {
                let mut st = self.ivars().state.borrow_mut();
                st.show_free = !st.show_free;
            }
            self.relayout();
        }

        #[unsafe(method(showInFinder:))]
        fn show_in_finder(&self, _sender: Option<&AnyObject>) {
            let selected = self.ivars().state.borrow().selected;
            self.show_in_finder_node(selected);
        }

        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            let outcome = {
                let st = self.ivars().state.borrow();
                match st.scan.as_ref() {
                    None => return,
                    Some(job) => match job.result.try_recv() {
                        Ok(tree) => Some(tree),
                        Err(TryRecvError::Empty) => None,
                        Err(TryRecvError::Disconnected) => Some(None),
                    },
                }
            };
            match outcome {
                None => self.setNeedsDisplay(true), // refresh the progress text
                Some(tree) => self.finish_scan(tree),
            }
        }
    }
);

fn tooltip(tree: &Tree, id: u32) -> String {
    let n = tree.node(id);
    let mut s = format!(
        "{}\n{}  ({})",
        tree.path(id).display(),
        size_string(n.size),
        bytes_string(n.size)
    );
    if n.is_dir() {
        s.push_str(&format!("\n{} items", group_digits(n.child_count as u64)));
    }
    if n.mtime != 0 {
        s.push_str(&format!("\n{}", date_string(n.mtime)));
    }
    s
}

impl TreemapView {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ViewIvars {
            state: RefCell::new(State {
                tree: None,
                zoom_root: ROOT,
                show_free: true,
                selected: None,
                hover: None,
                boxes: Vec::new(),
                layout_size: (0, 0),
                scan: None,
                message: None,
                volumes: Vec::new(),
            }),
            buttons: OnceCell::new(),
        });
        // SAFETY: `initWithFrame:` is NSView's designated initialiser.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };

        let options = NSTrackingAreaOptions::MouseMoved
            | NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::ActiveInKeyWindow
            | NSTrackingAreaOptions::InVisibleRect;
        // SAFETY: the view owns the tracking area and outlives it.
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                frame,
                options,
                Some(&this),
                None,
            )
        };
        this.addTrackingArea(&area);
        this
    }

    /// Enables menu items and keeps the Free Space check mark in sync.
    fn validate(&self, item: &NSMenuItem) -> bool {
        let st = self.ivars().state.borrow();
        let Some(action) = item.action() else {
            return true;
        };
        let open = st.tree.is_some();
        let selected_dir = match (st.selected, st.tree.as_ref()) {
            (Some(id), Some(t)) => t.node(id).is_dir(),
            _ => false,
        };
        if action == sel!(toggleFree:) {
            item.setState(if st.show_free {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            true
        } else if action == sel!(zoomIn:) {
            selected_dir
        } else if action == sel!(zoomOut:) || action == sel!(zoomFull:) {
            open && st.zoom_root != ROOT
        } else if action == sel!(showInFinder:) {
            open
        } else if action == sel!(reload:) {
            open || st.scan.is_some()
        } else {
            true
        }
    }

    fn menu_item(&self, title: &str, action: Sel, key: &str) -> Retained<NSMenuItem> {
        // SAFETY: `action` is implemented by this view with a single object argument.
        unsafe {
            let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm()),
                &NSString::from_str(title),
                Some(action),
                &NSString::from_str(key),
            );
            item.setTarget(Some(self));
            item
        }
    }

    fn context_menu(&self) -> Retained<NSMenu> {
        let mtm = self.mtm();
        let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!(""));
        menu.addItem(&self.menu_item("Zoom In", sel!(zoomIn:), ""));
        menu.addItem(&self.menu_item("Zoom Out", sel!(zoomOut:), ""));
        menu.addItem(&self.menu_item("Zoom Full", sel!(zoomFull:), ""));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&self.menu_item("Show in Finder", sel!(showInFinder:), ""));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&self.menu_item("Reload", sel!(reload:), ""));
        menu.addItem(&self.menu_item("Show Free Space", sel!(toggleFree:), ""));
        menu
    }

    fn is_dir(&self, id: u32) -> bool {
        let st = self.ivars().state.borrow();
        st.tree
            .as_ref()
            .map(|t| t.node(id).is_dir())
            .unwrap_or(false)
    }

    /// The tree node under the mouse, if any.
    fn node_at(&self, event: &NSEvent) -> Option<u32> {
        let p = self.convertPoint_fromView(event.locationInWindow(), None);
        let st = self.ivars().state.borrow();
        if st.scan.is_some() {
            return None;
        }
        layout::hit_test(&st.boxes, p.x.floor() as i32, p.y.floor() as i32)
            .map(|i| st.boxes[i].node)
    }

    fn select(&self, id: Option<u32>) {
        {
            let mut st = self.ivars().state.borrow_mut();
            if st.selected == id {
                return;
            }
            st.selected = id;
        }
        self.setNeedsDisplay(true);
        self.refresh_chrome();
    }

    fn zoom_to(&self, id: u32) {
        {
            let mut st = self.ivars().state.borrow_mut();
            st.zoom_root = id;
            st.selected = None;
        }
        self.relayout();
    }

    fn zoom_in_selected(&self) {
        let target = {
            let st = self.ivars().state.borrow();
            match (st.selected, st.tree.as_ref()) {
                (Some(id), Some(t)) if t.node(id).is_dir() => Some(id),
                _ => None,
            }
        };
        if let Some(id) = target {
            self.zoom_to(id);
        }
    }

    fn zoom_out_one(&self) {
        let changed = {
            let mut st = self.ivars().state.borrow_mut();
            let parent = match st.tree.as_ref() {
                Some(t) if st.zoom_root != ROOT => t.node(st.zoom_root).parent,
                _ => NONE,
            };
            if parent == NONE {
                false
            } else {
                // Keep the folder we just left selected, so it is easy to find.
                st.selected = Some(st.zoom_root);
                st.zoom_root = parent;
                true
            }
        };
        if changed {
            self.relayout();
        }
    }

    /// Opens a folder in Finder, or reveals a file in its folder. With
    /// nothing selected, opens the folder currently filling the window.
    fn show_in_finder_node(&self, id: Option<u32>) {
        let (path, is_dir) = {
            let st = self.ivars().state.borrow();
            let Some(tree) = st.tree.as_ref() else { return };
            let id = id.unwrap_or(st.zoom_root);
            (tree.path(id), tree.node(id).is_dir())
        };
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let workspace = NSWorkspace::sharedWorkspace();
        if is_dir {
            workspace.openURL(&url);
        } else {
            workspace.activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
        }
    }

    fn start_scan(&self, path: PathBuf) {
        let progress = Arc::new(Progress::default());
        let (tx, rx) = channel();
        {
            let mut st = self.ivars().state.borrow_mut();
            if let Some(old) = st.scan.take() {
                old.progress.cancel.store(true, Ordering::Relaxed);
            }
            st.tree = None; // free the old tree before building the new one
            st.boxes.clear();
            st.zoom_root = ROOT;
            st.selected = None;
            st.hover = None;
            st.message = None;
            st.scan = Some(ScanJob {
                progress: progress.clone(),
                result: rx,
                path: path.clone(),
            });
        }
        std::thread::spawn(move || {
            let tree = scan(&path, &progress);
            let _ = tx.send(tree);
        });
        self.setToolTip(None);
        self.setNeedsDisplay(true);
        self.refresh_chrome();
    }

    fn finish_scan(&self, tree: Option<Tree>) {
        {
            let mut st = self.ivars().state.borrow_mut();
            let Some(job) = st.scan.take() else { return };
            if tree.is_none() {
                st.message = Some(format!("Could not read {}", job.path.display()));
            }
            st.tree = tree;
            st.zoom_root = ROOT;
            st.selected = None;
        }
        self.relayout();
    }

    /// Recomputes the boxes for the current size, zoom and free-space setting.
    fn relayout(&self) {
        {
            let mut st = self.ivars().state.borrow_mut();
            st.layout_size = (0, 0); // forces a rebuild in `draw`
            st.hover = None;
        }
        self.setToolTip(None);
        self.setNeedsDisplay(true);
        self.refresh_chrome();
    }

    /// Window title and button states, following SpaceMonger's title bar:
    /// "path - 12.3% - 1.2 GB" for a selection, otherwise totals.
    fn refresh_chrome(&self) {
        let st = self.ivars().state.borrow();
        let mut title = String::new();
        let mut selected_dir = false;
        if let Some(job) = st.scan.as_ref() {
            title = format!("Scanning {}  -  ", job.path.display());
        } else if let Some(tree) = st.tree.as_ref() {
            let whole = if tree.is_volume {
                tree.total_space
            } else {
                tree.node(ROOT).size
            };
            if let Some(id) = st.selected {
                let n = tree.node(id);
                selected_dir = n.is_dir();
                title = format!(
                    "{}  -  {}  -  {}  -  ",
                    tree.path(id).display(),
                    percent_string(n.size, whole),
                    size_string(n.size)
                );
            } else {
                let size = if st.zoom_root == ROOT {
                    whole
                } else {
                    tree.node(st.zoom_root).size
                };
                title = format!(
                    "{}  -  {} Total  -  ",
                    tree.path(st.zoom_root).display(),
                    size_string(size)
                );
                if tree.is_volume {
                    title.push_str(&format!("{} Free  -  ", size_string(tree.free_space)));
                }
            }
        }
        title.push_str(APP_NAME);
        if let Some(window) = self.window() {
            window.setTitle(&NSString::from_str(&title));
        }
        if let Some(b) = self.ivars().buttons.get() {
            let open = st.tree.is_some();
            let zoomed = open && st.zoom_root != ROOT;
            b.reload.setEnabled(open || st.scan.is_some());
            b.zoom_full.setEnabled(zoomed);
            b.zoom_out.setEnabled(zoomed);
            b.zoom_in.setEnabled(selected_dir);
            b.finder.setEnabled(open);
            b.free.setState(if st.show_free {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
    }

    fn draw(&self, dirty: NSRect) {
        let bounds = self.bounds();
        let (vw, vh) = (
            bounds.size.width.floor() as i32,
            bounds.size.height.floor() as i32,
        );
        let mut st = self.ivars().state.borrow_mut();
        let st = &mut *st;

        let grey = rgb(FACE_GREY);
        let black = rgb(0x000000);
        let white = rgb(0xFFFFFF);
        let font = NSFont::systemFontOfSize(FONT_SIZE);
        let black_text = text_attrs(&font, &black);
        let white_text = text_attrs(&font, &white);

        fill(&grey, 0, 0, vw + 1, vh + 1);

        if let Some(job) = st.scan.as_ref() {
            let p = &job.progress;
            let lines = [
                format!("Scanning {} \u{2026}", job.path.display()),
                format!(
                    "{} files in {} folders",
                    group_digits(p.files.load(Ordering::Relaxed)),
                    group_digits(p.dirs.load(Ordering::Relaxed))
                ),
                size_string(p.bytes.load(Ordering::Relaxed)),
            ];
            for (i, line) in lines.iter().enumerate() {
                draw_centered(
                    line,
                    0,
                    vw,
                    vh / 2 - 20 + i as i32 * (LINE + 2),
                    &black_text,
                );
            }
            return;
        }

        let Some(tree) = st.tree.as_ref() else {
            let line = st
                .message
                .clone()
                .unwrap_or_else(|| "Click Open to choose a disk or folder".to_string());
            draw_centered(&line, 0, vw, vh / 2 - LINE / 2, &black_text);
            return;
        };

        if st.layout_size != (vw, vh) {
            st.boxes = layout::build(
                tree,
                st.zoom_root,
                tree.depth(st.zoom_root),
                vw,
                vh,
                st.show_free,
            );
            st.layout_size = (vw, vh);
        }

        let palette: Vec<[Retained<NSColor>; 3]> = layout::PALETTE
            .iter()
            .map(|c| [rgb(c[0]), rgb(c[1]), rgb(c[2])])
            .collect();
        let (dx0, dy0) = (dirty.origin.x.floor() as i32, dirty.origin.y.floor() as i32);
        let (dx1, dy1) = (
            (dirty.origin.x + dirty.size.width).ceil() as i32,
            (dirty.origin.y + dirty.size.height).ceil() as i32,
        );
        // Text height of the label font, measured once.
        let text_h = unsafe { ns_string!("Xg").sizeWithAttributes(Some(&black_text)) }
            .height
            .ceil() as i32;

        for b in st.boxes.iter() {
            let (x, y, w, h) = (b.x, b.y, b.w + 1, b.h + 1);
            if x >= dx1 || y >= dy1 || x + w <= dx0 || y + h <= dy0 {
                continue;
            }
            let selected = b.node != NONE && !b.is_free() && st.selected == Some(b.node);

            // Black outline, bevel (light top/left, dark bottom/right), face.
            if b.depth >= 0 {
                fill(&black, x, y, w, h);
            }
            if w > 2 && h > 2 {
                if selected {
                    fill(&black, x + 1, y + 1, w - 2, h - 2);
                } else if b.depth < 0 {
                    fill(&grey, x, y, w, h);
                } else {
                    let [face, light, dark] = &palette[(b.depth & 7) as usize];
                    fill(dark, x + 1, y + 1, w - 2, h - 2);
                    fill(light, x + 1, y + 1, w - 3, h - 3);
                    fill(face, x + 2, y + 2, w - 4, h - 4);
                }
            }
            if b.node == NONE {
                continue;
            }

            NSGraphicsContext::saveGraphicsState_class();
            NSBezierPath::clipRect(NSRect::new(
                NSPoint::new(x as f64, y as f64),
                NSSize::new(w as f64, h as f64),
            ));
            if b.is_free() {
                let ty = y + (h - text_h) / 2;
                let lines = [
                    format!(
                        "<Free Space: {}>",
                        percent_string(tree.free_space, tree.total_space)
                    ),
                    format!("{} Free", size_string(tree.free_space)),
                    format!("Files Total:  {}", group_digits(tree.num_files)),
                    format!("Folders Total:  {}", group_digits(tree.num_dirs)),
                ];
                for (i, line) in lines.iter().enumerate() {
                    draw_centered(line, x, w, ty - 18 + i as i32 * LINE, &black_text);
                }
            } else {
                let attrs = if selected { &white_text } else { &black_text };
                let node = tree.node(b.node);
                let name = NSString::from_str(&tree.name(b.node));
                if b.is_dir() {
                    draw_text(&name, x + 3, y + (TITLE_H - text_h) / 2 + 1, attrs);
                } else {
                    let tw = text_width(&name, attrs);
                    let tx = if tw > w - 2 { x + 2 } else { x + (w - tw) / 2 };
                    let mut ty = if text_h > h - 2 {
                        y + 1
                    } else {
                        y + (h - text_h) / 2
                    };
                    if h >= 3 * LINE + 4 && w >= 48 {
                        // Room for the size and date below the name.
                        draw_centered(&bytes_string(node.size), x, w, ty, attrs);
                        draw_centered(&date_string(node.mtime), x, w, ty + LINE, attrs);
                        ty -= LINE;
                    }
                    draw_text(&name, tx, ty, attrs);
                }
            }
            NSGraphicsContext::restoreGraphicsState_class();
        }
    }
}

// ----------------------------------------------------------------------------

struct DelegateIvars {
    window: OnceCell<Retained<NSWindow>>,
    view: OnceCell<Retained<TreemapView>>,
    timer: OnceCell<Retained<NSTimer>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no `Drop` impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "MMAppDelegate"]
    #[ivars = DelegateIvars]
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            self.build_ui();
        }
    }

    unsafe impl NSWindowDelegate for AppDelegate {
        // Quit when the main window is closed. Done explicitly instead of via
        // applicationShouldTerminateAfterLastWindowClosed, which AppKit also
        // evaluates when helper windows (such as tooltips) go away.
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            NSApplication::sharedApplication(self.mtm()).terminate(None);
        }
    }
);

fn bar_button(
    mtm: MainThreadMarker,
    title: &str,
    symbol: &str,
    target: &TreemapView,
    action: Sel,
    x: &mut f64,
    bar: &NSView,
) -> Retained<NSButton> {
    // SAFETY: `target` implements `action` and outlives the button (both are
    // owned by the window for the lifetime of the app).
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(target),
            Some(action),
            mtm,
        )
    };
    button.setBezelStyle(NSBezelStyle::Toolbar);
    if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(symbol),
        None,
    ) {
        button.setImage(Some(&image));
        button.setImagePosition(NSCellImagePosition::ImageLeading);
    }
    button.sizeToFit();
    let size = button.frame().size;
    let width = size.width + 10.0;
    button.setFrame(NSRect::new(
        NSPoint::new(*x, ((BAR_HEIGHT - size.height) / 2.0).floor()),
        NSSize::new(width, size.height),
    ));
    *x += width + 6.0;
    bar.addSubview(&button);
    button
}

impl AppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars {
            window: OnceCell::new(),
            view: OnceCell::new(),
            timer: OnceCell::new(),
        });
        // SAFETY: plain NSObject init.
        unsafe { msg_send![super(this), init] }
    }

    fn build_ui(&self) {
        let mtm = self.mtm();
        let app = NSApplication::sharedApplication(mtm);

        let content = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1000.0, 760.0));
        // SAFETY: `setReleasedWhenClosed(false)` below makes manual ownership sound.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                content,
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable
                    | NSWindowStyleMask::Resizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str(APP_NAME));
        window.setContentMinSize(NSSize::new(480.0, 320.0));
        window.setDelegate(Some(ProtocolObject::from_ref(self)));

        let root = window.contentView().expect("window has a content view");
        let size = root.bounds().size;

        let view = TreemapView::new(
            mtm,
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(size.width, size.height - BAR_HEIGHT),
            ),
        );
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        root.addSubview(&view);

        let bar = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, size.height - BAR_HEIGHT),
                NSSize::new(size.width, BAR_HEIGHT),
            ),
        );
        bar.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        root.addSubview(&bar);

        let mut x = 10.0;
        let _open = bar_button(mtm, "Open", "folder", &view, sel!(openMenu:), &mut x, &bar);
        let reload = bar_button(
            mtm,
            "Reload",
            "arrow.clockwise",
            &view,
            sel!(reload:),
            &mut x,
            &bar,
        );
        x += 10.0;
        let zoom_full = bar_button(
            mtm,
            "Zoom Full",
            "arrow.up.left.and.arrow.down.right",
            &view,
            sel!(zoomFull:),
            &mut x,
            &bar,
        );
        let zoom_in = bar_button(
            mtm,
            "Zoom In",
            "plus.magnifyingglass",
            &view,
            sel!(zoomIn:),
            &mut x,
            &bar,
        );
        let zoom_out = bar_button(
            mtm,
            "Zoom Out",
            "minus.magnifyingglass",
            &view,
            sel!(zoomOut:),
            &mut x,
            &bar,
        );
        x += 10.0;
        let free = bar_button(
            mtm,
            "Free Space",
            "square.dashed",
            &view,
            sel!(toggleFree:),
            &mut x,
            &bar,
        );
        free.setButtonType(NSButtonType::PushOnPushOff);
        free.setState(NSControlStateValueOn);
        x += 10.0;
        let finder = bar_button(
            mtm,
            "Show in Finder",
            "magnifyingglass",
            &view,
            sel!(showInFinder:),
            &mut x,
            &bar,
        );

        let _ = view.ivars().buttons.set(Buttons {
            reload,
            zoom_full,
            zoom_in,
            zoom_out,
            free,
            finder,
        });

        app.setMainMenu(Some(&self.main_menu(&view)));

        window.center();
        window.setFrameAutosaveName(ns_string!("MacMongerMainWindow"));
        window.makeKeyAndOrderFront(None);
        window.makeFirstResponder(Some(&view));

        // Polls the background scan and refreshes the progress text.
        // SAFETY: the view implements `tick:` and lives as long as the app.
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                0.15,
                &view,
                sel!(tick:),
                None,
                true,
            )
        };

        view.refresh_chrome();
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);

        // `MacMonger /some/folder` on the command line scans straight away.
        if let Some(arg) = std::env::args().nth(1) {
            let path = PathBuf::from(arg);
            if path.is_dir() {
                view.start_scan(path);
            }
        }

        let _ = self.ivars().window.set(window);
        let _ = self.ivars().view.set(view);
        let _ = self.ivars().timer.set(timer);
    }

    fn main_menu(&self, view: &TreemapView) -> Retained<NSMenu> {
        let mtm = self.mtm();
        let bar = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!(""));

        let plain = |title: &str, action: Sel, key: &str| -> Retained<NSMenuItem> {
            // SAFETY: standard AppKit actions sent down the responder chain.
            unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(title),
                    Some(action),
                    &NSString::from_str(key),
                )
            }
        };
        let submenu = |title: &str, items: Vec<Retained<NSMenuItem>>| {
            let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
            for item in &items {
                menu.addItem(item);
            }
            let holder = NSMenuItem::new(mtm);
            holder.setSubmenu(Some(&menu));
            bar.addItem(&holder);
        };
        let sep = || NSMenuItem::separatorItem(mtm);

        let hide_others = plain("Hide Others", sel!(hideOtherApplications:), "h");
        hide_others.setKeyEquivalentModifierMask(
            NSEventModifierFlags::Command | NSEventModifierFlags::Option,
        );
        submenu(
            APP_NAME,
            vec![
                plain("About MacMonger", sel!(orderFrontStandardAboutPanel:), ""),
                sep(),
                plain("Hide MacMonger", sel!(hide:), "h"),
                hide_others,
                sep(),
                plain("Quit MacMonger", sel!(terminate:), "q"),
            ],
        );
        submenu(
            "File",
            vec![
                view.menu_item("Open Folder\u{2026}", sel!(chooseFolder:), "o"),
                view.menu_item("Reload", sel!(reload:), "r"),
                sep(),
                view.menu_item("Show in Finder", sel!(showInFinder:), "e"),
                sep(),
                plain("Close Window", sel!(performClose:), "w"),
            ],
        );
        submenu(
            "View",
            vec![
                view.menu_item("Zoom In", sel!(zoomIn:), "+"),
                view.menu_item("Zoom Out", sel!(zoomOut:), "-"),
                view.menu_item("Zoom Full", sel!(zoomFull:), "0"),
                sep(),
                view.menu_item("Show Free Space", sel!(toggleFree:), "f"),
            ],
        );
        submenu(
            "Window",
            vec![
                plain("Minimize", sel!(performMiniaturize:), "m"),
                plain("Zoom", sel!(performZoom:), ""),
            ],
        );
        bar
    }
}

/// A GUI app has no terminal, so record a panic where it can be found:
/// ~/Library/Logs/MacMonger-crash.log
fn install_crash_log() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(home) = std::env::var_os("HOME") {
            let path = PathBuf::from(home).join("Library/Logs/MacMonger-crash.log");
            let _ = std::fs::write(
                path,
                format!("MacMonger {}\n{}\n", env!("CARGO_PKG_VERSION"), info),
            );
        }
        previous(info);
    }));
}

pub fn run() {
    install_crash_log();
    let mtm = MainThreadMarker::new().expect("must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    let delegate = AppDelegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
}
