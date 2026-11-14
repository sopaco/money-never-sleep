//! `mns-help` binary: the standalone CHM-style help viewer for MNS.
//!
//! Content (markdown + screenshots) lives beside the executable in a `help/`
//! directory when installed; the markdown itself is embedded at compile time.

// Release builds on Windows are a windowed app: launching from the MNS tray
// must not flash a console window. Debug builds keep the console for logs.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(feature = "gui")]
fn main() {
    money_never_sleep::help::run();
}

#[cfg(not(feature = "gui"))]
fn main() {
    eprintln!("mns-help 需要 `gui` feature：请使用 --features gui 重新构建。");
    std::process::exit(1);
}
