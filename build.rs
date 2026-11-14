//! Embed the product mark into the Windows executable.
//!
//! GPUI's Windows backend registers the window class with the icon resource
//! `LoadImageW(GetModuleHandleW(None), MAKEINTRESOURCE(1), ...)`, i.e. the icon
//! compiled into the exe. Without it the title bar and taskbar fall back to the
//! generic Windows icon; `WindowOptions::icon` is X11-only and does not apply.

fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    winresource::WindowsResource::new()
        .set_icon("assets/app.ico")
        .compile()
        .expect("failed to embed assets/app.ico into the Windows executable");
}
