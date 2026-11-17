//! Embed per-binary icons into the Windows executables.
//!
//! GPUI's Windows backend registers the window class with the icon resource
//! `LoadImageW(GetModuleHandleW(None), MAKEINTRESOURCE(1), ...)`, i.e. the icon
//! compiled into *that exe*. `WindowOptions::icon` is X11-only and does not
//! apply. So the title-bar and taskbar icon of a binary is whatever ID-1 icon
//! resource we compile into it.
//!
//! Most binaries use `assets/app.ico`. `mns-help.exe` uses
//! `assets/icon_help.ico` instead — the help viewer is a separate process and
//! should not look like the main app in the taskbar. `mns-butler.exe` and
//! `mns-butlerd.exe` use `assets/icon_butler.ico`: they are the butler's own
//! processes, so Explorer / the task manager should show the butler face.
//! (The butler *window* lives inside `mns-gui.exe`, so it shares that process's
//! icon; its avatar is drawn in the GUI itself and is not a system icon.)
//!
//! Why not just call `WindowsResource::compile()` twice? Because that prints
//! *global* link directives (`rustc-link-lib=static=resource` on GNU,
//! `cargo:rustc-link-arg=resource.res` on MSVC), which apply to every binary in
//! the package — two icons would both claim resource ID 1. Instead we compile
//! two ID-1 resources and attach each with `cargo:rustc-link-arg-bin=<BIN>` so
//! every exe gets exactly one icon.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=assets/icon_help.ico");
    println!("cargo:rerun-if-changed=assets/icon_butler.ico");
    println!("cargo:rerun-if-changed=assets/app_icon.png");
    println!("cargo:rerun-if-changed=assets/icon_help.png");
    println!("cargo:rerun-if-changed=assets/icon_butler.png");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));

    let app_icon = manifest_dir.join("assets/app.ico");
    let help_icon = manifest_dir.join("assets/icon_help.ico");
    let butler_icon = manifest_dir.join("assets/icon_butler.ico");
    for icon in [&app_icon, &help_icon, &butler_icon] {
        assert!(
            icon.is_file(),
            "missing Windows icon resource: {} (regenerate with scripts/make-icons.swift / scripts/make-help-ico.swift)",
            icon.display()
        );
    }

    let target_env = env::var("CARGO_CFG_TARGET_ENV").expect("CARGO_CFG_TARGET_ENV");
    let app_obj = compile_icon(&target_env, &out_dir, &app_icon, "app");
    let help_obj = compile_icon(&target_env, &out_dir, &help_icon, "help");
    let butler_obj = compile_icon(&target_env, &out_dir, &butler_icon, "butler");

    // Attach exactly one icon to each binary. Keep this list in sync with the
    // [[bin]] targets in Cargo.toml: a binary with no link-arg gets Windows'
    // generic icon.
    for bin in ["mns", "mns-gui"] {
        println!("cargo:rustc-link-arg-bin={}={}", bin, app_obj.display());
    }
    for bin in ["mns-butler", "mns-butlerd"] {
        println!("cargo:rustc-link-arg-bin={}={}", bin, butler_obj.display());
    }
    println!("cargo:rustc-link-arg-bin=mns-help={}", help_obj.display());
}

/// Compile a .rc that declares `1 ICON "<path>"` into a linker input, returning
/// its path. GNU toolchains produce a COFF object (`windres`); MSVC toolchains
/// produce a resource file (`rc.exe`) that link.exe accepts directly.
fn compile_icon(target_env: &str, out_dir: &Path, icon: &Path, stem: &str) -> PathBuf {
    let rc_path = out_dir.join(format!("{stem}.rc"));
    // A single ID-1 ICON statement is all GPUI needs. Use an absolute path so
    // the resource compiler finds the .ico regardless of its cwd; escape any
    // backslashes for the .rc string literal.
    let icon_str = icon.to_string_lossy().replace('\\', "\\\\");
    std::fs::write(&rc_path, format!("1 ICON \"{}\"\n", icon_str))
        .expect("failed to write generated .rc");

    match target_env {
        "gnu" => {
            // Prefer an explicit WINDRES, then the usual MinGW cross prefix.
            // winresource derives the same prefix from TARGET/HOST; do it here
            // too so cross-compiling from macOS works without extra setup.
            let windres: OsString = env::var_os("WINDRES").unwrap_or_else(|| {
                let target = env::var("TARGET").unwrap_or_default();
                let prefix = match target.as_str() {
                    "x86_64-pc-windows-gnu" | "x86_64-pc-windows-gnullvm" => "x86_64-w64-mingw32-",
                    "i686-pc-windows-gnu" | "i586-pc-windows-gnu" => "i686-w64-mingw32-",
                    // LLVM-based toolchains (incl. aarch64) use llvm-windres.
                    _ if target.contains("gnullvm") || target.starts_with("aarch64") => "llvm-",
                    _ => "",
                };
                format!("{prefix}windres").into()
            });
            let obj = out_dir.join(format!("{stem}_icon.o"));
            let status = Command::new(&windres)
                .arg(&rc_path)
                .arg(&obj)
                .status()
                .unwrap_or_else(|e| panic!("failed to run {}: {e}", windres.to_string_lossy()));
            assert!(status.success(), "windres failed for {}", icon.display());
            obj
        }
        "msvc" => {
            // Native Windows uses rc.exe from the SDK (RC_PATH or PATH). On a
            // cross-compile host llvm-rc is the usual equivalent; it accepts
            // the same /fo and /nologo flags.
            let rc_exe: OsString = env::var_os("RC_PATH").unwrap_or_else(|| {
                if cfg!(windows) {
                    "rc.exe".into()
                } else {
                    "llvm-rc".into()
                }
            });
            let res = out_dir.join(format!("{stem}_icon.res"));
            let fo = format!("/fo{}", res.display());
            let status = Command::new(&rc_exe)
                .arg("/nologo")
                .arg(&fo)
                .arg(&rc_path)
                .status()
                .unwrap_or_else(|e| panic!("failed to run {}: {e}", rc_exe.to_string_lossy()));
            assert!(status.success(), "rc.exe failed for {}", icon.display());
            res
        }
        other => panic!("unsupported CARGO_CFG_TARGET_ENV for icon resources: {other}"),
    }
}
