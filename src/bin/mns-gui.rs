//! `mns-gui` binary. All behaviour lives in the shared library so the CLI and
//! the desktop shell can never drift into two implementations of the same rule.

fn main() {
    money_never_sleep::gui::run();
}
