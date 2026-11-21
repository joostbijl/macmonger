//! MacMonger - a SpaceMonger 1.4 style disk space treemap for macOS.

#[cfg(target_os = "macos")]
mod app;

#[cfg(target_os = "macos")]
fn main() {
    app::run();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("MacMonger is a macOS application. On other systems only the core library and the `render` example are available.");
    std::process::exit(1);
}
