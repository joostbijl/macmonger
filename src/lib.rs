//! MacMonger core: directory scanning, the SpaceMonger 1.4 treemap layout and
//! text formatting. Everything in here is platform independent so it can be
//! unit-tested anywhere; the AppKit front end lives in `src/app.rs`.

pub mod format;
pub mod layout;
pub mod scan;
pub mod tree;
pub mod volumes;
