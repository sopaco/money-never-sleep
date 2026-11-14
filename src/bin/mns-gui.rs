//! `mns-gui` binary. All behaviour lives in the shared library so the CLI and
//! the desktop shell can never drift into two implementations of the same rule.

// Release builds on Windows are a windowed app: launching the installed
// shortcut must not leave a console window behind. Debug builds keep the
// console so `cargo run --bin mns-gui --features gui` still shows its logs.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    money_never_sleep::gui::run();
}
