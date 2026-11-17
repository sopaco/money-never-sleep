//! 管家状态的落盘。
//!
//! 全部是「小 JSON + 原子替换」：管家会在托盘里长期运行，写到一半被系统
//! 杀掉不能留下半截文件——读取方（下一次启动）会把半截 JSON 当成损坏状态，
//! 那等于丢掉用户的记忆与任务。因此先写 `.tmp` 再 rename。

use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// 读一个 JSON 状态文件。不存在时返回 `None`（首次启动不是错误）。
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("读取 {} 失败", path.display()));
        }
    };
    if text.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&text)
        .map(Some)
        .with_context(|| format!("解析 {} 失败", path.display()))
}

/// 原子写一个 JSON 状态文件（父目录自动创建）。
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建目录 {} 失败", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(value).context("序列化管家状态失败")?;
    let tmp = tmp_path(path);
    fs::write(&tmp, text).with_context(|| format!("写入 {} 失败", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("替换 {} 失败", path.display()))?;
    Ok(())
}

/// 追加一行 JSONL。用于行为事件日志——单行原子追加，崩溃最多丢最后一行。
pub fn append_line(path: &Path, line: &str) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建目录 {} 失败", parent.display()))?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("打开 {} 失败", path.display()))?;
    writeln!(file, "{line}").with_context(|| format!("追加 {} 失败", path.display()))?;
    Ok(())
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "state.json".to_owned());
    name.push_str(".tmp");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Sample {
        value: u32,
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mns-butler-store-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn 写入后能读回且不留临时文件() {
        let dir = scratch("roundtrip");
        let path = dir.join("state.json");
        assert!(read_json::<Sample>(&path).unwrap().is_none(), "缺失文件应返回 None");

        write_json(&path, &Sample { value: 7 }).unwrap();
        let loaded: Sample = read_json(&path).unwrap().unwrap();
        assert_eq!(loaded.value, 7);
        assert!(!dir.join("state.json.tmp").exists(), "临时文件必须已被 rename");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 半截_json_报错而不是当成空状态() {
        let dir = scratch("corrupt");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        fs::write(&path, "{\"value\": 1").unwrap();
        assert!(
            read_json::<Sample>(&path).is_err(),
            "损坏状态必须显式失败，不能静默重置用户记忆"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 追加行不会覆盖已有事件() {
        let dir = scratch("append");
        let path = dir.join("events.jsonl");
        append_line(&path, "{\"a\":1}").unwrap();
        append_line(&path, "{\"a\":2}").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 2);
        let _ = fs::remove_dir_all(&dir);
    }
}
