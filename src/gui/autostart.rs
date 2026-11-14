//! 开机自启动（登录时静默启动）。
//!
//! 只注册系统自己的登录入口，不引入常驻守护进程：
//! - Windows: `HKCU\...\CurrentVersion\Run` 下的一个 REG_SZ
//! - macOS:   `~/Library/LaunchAgents/com.mns.app.plist`
//! - Linux:   `~/.config/autostart/mns.desktop`
//!
//! 写入的命令都带 [`BACKGROUND_ARG`]，进程启动后只常驻托盘、不打开窗口；
//! 用户自己双击图标启动时没有这个参数，仍会正常打开主窗口。

use std::path::PathBuf;

#[cfg(windows)]
use super::command;

/// 静默启动参数。登录自启动项写入的就是它。
pub const BACKGROUND_ARG: &str = "--tray";

/// 本次进程是否由登录自启动拉起（只驻留托盘）。
pub fn launched_in_background() -> bool {
    std::env::args()
        .skip(1)
        .any(|arg| arg == BACKGROUND_ARG || arg == "--background")
}

fn current_exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|error| format!("无法定位程序路径：{error}"))
}

// ── Windows ────────────────────────────────────────────────────────────

#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const VALUE_NAME: &str = "MNS";

#[cfg(windows)]
pub fn is_enabled() -> bool {
    command("reg")
        .args(["query", RUN_KEY, "/v", VALUE_NAME])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[cfg(windows)]
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    if !enabled {
        // 值已不存在也视为关闭成功：设置项与系统状态保持一致即可。
        if !is_enabled() {
            return Ok(());
        }
        let output = command("reg")
            .args(["delete", RUN_KEY, "/v", VALUE_NAME, "/f"])
            .output()
            .map_err(|error| format!("无法执行 reg delete：{error}"))?;
        return reg_result(&output);
    }

    let exe = current_exe()?;
    let value = format!("\"{}\" {}", exe.display(), BACKGROUND_ARG);
    let output = command("reg")
        .args([
            "add",
            RUN_KEY,
            "/v",
            VALUE_NAME,
            "/t",
            "REG_SZ",
            "/d",
            &value,
            "/f",
        ])
        .output()
        .map_err(|error| format!("无法执行 reg add：{error}"))?;
    reg_result(&output)
}

#[cfg(windows)]
fn reg_result(output: &std::process::Output) -> Result<(), String> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    Err(if stderr.is_empty() {
        format!("注册表操作失败（退出码 {:?}）", output.status.code())
    } else {
        stderr.to_owned()
    })
}

// ── macOS ──────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn plist_path() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "无法获取用户主目录".to_owned())?;
    Ok(home
        .join("Library")
        .join("LaunchAgents")
        .join("com.mns.app.plist"))
}

#[cfg(target_os = "macos")]
#[allow(dead_code)]
pub fn is_enabled() -> bool {
    plist_path().map(|path| path.exists()).unwrap_or(false)
}

#[cfg(target_os = "macos")]
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let path = plist_path()?;
    if !enabled {
        if path.exists() {
            std::fs::remove_file(&path).map_err(|error| format!("无法删除登录项：{error}"))?;
        }
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|error| format!("无法创建登录项目录：{error}"))?;
    }
    let exe = current_exe()?;
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.mns.app</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
    <string>{}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
</dict>
</plist>
"#,
        escape_xml(&exe.to_string_lossy()),
        BACKGROUND_ARG
    );
    std::fs::write(&path, body).map_err(|error| format!("无法写入登录项：{error}"))
}

#[cfg(target_os = "macos")]
fn escape_xml(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ── Linux ──────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn desktop_path() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "无法获取用户主目录".to_owned())?;
    Ok(home.join(".config").join("autostart").join("mns.desktop"))
}

#[cfg(target_os = "linux")]
#[allow(dead_code)]
pub fn is_enabled() -> bool {
    desktop_path().map(|path| path.exists()).unwrap_or(false)
}

#[cfg(target_os = "linux")]
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let path = desktop_path()?;
    if !enabled {
        if path.exists() {
            std::fs::remove_file(&path).map_err(|error| format!("无法删除自启动项：{error}"))?;
        }
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|error| format!("无法创建自启动目录：{error}"))?;
    }
    let exe = current_exe()?;
    let body = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=MNS\n\
         Exec=\"{}\" {}\n\
         X-GNOME-Autostart-enabled=true\n\
         NoDisplay=true\n",
        exe.display(),
        BACKGROUND_ARG
    );
    std::fs::write(&path, body).map_err(|error| format!("无法写入自启动项：{error}"))
}

// ── 不支持的平台 ────────────────────────────────────────────────────────

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
pub fn is_enabled() -> bool {
    false
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
pub fn set_enabled(_enabled: bool) -> Result<(), String> {
    Err("当前平台不支持开机自启动".to_owned())
}
