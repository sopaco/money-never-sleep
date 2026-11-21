//! 管家状态的落盘。
//!
//! 全部是「小 JSON + 原子替换」：管家会在托盘里长期运行，写到一半被系统
//! 杀掉不能留下半截文件——读取方（下一次启动）会把半截 JSON 当成损坏状态，
//! 那等于丢掉用户的记忆与任务。因此先写 `.tmp` 再 rename。
//!
//! state / memory / tasks 有三个写者（GUI、`mns-butlerd`、`mns-butler`
//! CLI）。两道防线：
//!
//! 1. 临时文件名带 pid 与序号：两个进程同时写，谁也不会 rename 到对方
//!    写了一半的 `.tmp`；
//! 2. [`WriterLock`]：读-改-写全程持锁，后写的进程先看到先写者的修改，
//!    而不是拿自己启动时的旧快照整体覆盖。

use anyhow::{Context, Result};
use serde::{Serialize, de::DeserializeOwned};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

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

/// 临时文件名里的序号：同一进程连续写也不会复用同一个 `.tmp`。
static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 原子写一个 JSON 状态文件（父目录自动创建）。
///
/// 临时文件名带 pid 与序号：多写者场景下固定名 `xxx.json.tmp` 会被并发
/// 写者互相踩踏——A 刚写完自己的 `.tmp`，B 又往同一个名字里写，A 的
/// rename 送到的是 B 的半截内容。
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
    let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "state.json".to_owned());
    name.push_str(&format!(".{}.{}.tmp", std::process::id(), seq));
    path.with_file_name(name)
}

/// 跨进程写者互斥锁。
///
/// 持锁期间「读-改-写」不会被另一个进程插队。锁依附在打开的文件描述符
/// 上：进程崩溃时由系统自动释放，不需要过期清理。拿不到锁时调用方应
/// 降级为无锁写入（行为回到加锁之前），而不是拒绝写——送达路径不能
/// 因为锁而挂掉。
pub struct WriterLock {
    // 字段只在 Drop（解锁）时用：持有即持锁。
    _file: fs::File,
}

/// 拿写锁，最多等 `timeout`。
///
/// 返回 `None` 表示超时（另一个写者卡住了）或锁文件打不开：调用方降级
/// 为无锁写入并继续，与历史行为一致。
pub fn lock_writers(lock_path: &Path, timeout: Duration) -> Option<WriterLock> {
    if let Some(parent) = lock_path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        eprintln!(
            "管家状态写锁: 创建锁目录 {} 失败（{error}），本轮无锁写入",
            lock_path.display()
        );
        return None;
    }
    let file = match fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
    {
        Ok(file) => file,
        Err(error) => {
            eprintln!(
                "管家状态写锁: 打开写锁 {} 失败（{error}），本轮无锁写入",
                lock_path.display()
            );
            return None;
        }
    };
    let deadline = std::time::Instant::now() + timeout;
    loop {
        // std 的文件锁：Unix 走 flock，Windows 走 LockFileEx。
        match file.try_lock() {
            Ok(()) => return Some(WriterLock { _file: file }),
            Err(_) => {
                if std::time::Instant::now() >= deadline {
                    eprintln!(
                        "管家状态写锁: 写锁 {} 等待超时（另一写者未释放），本轮无锁写入",
                        lock_path.display()
                    );
                    return None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
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
        let dir =
            std::env::temp_dir().join(format!("mns-butler-store-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// 目录里残留的所有 `.tmp`：rename 成功后应当一个都没有。
    fn leftover_tmps(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok())
                    .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn 写入后能读回且不留临时文件() {
        let dir = scratch("roundtrip");
        let path = dir.join("state.json");
        assert!(
            read_json::<Sample>(&path).unwrap().is_none(),
            "缺失文件应返回 None"
        );

        write_json(&path, &Sample { value: 7 }).unwrap();
        let loaded: Sample = read_json(&path).unwrap().unwrap();
        assert_eq!(loaded.value, 7);
        assert!(leftover_tmps(&dir).is_empty(), "临时文件必须已被 rename");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 临时文件名互不相同() {
        // 固定名 xxx.json.tmp 会被并发写者踩踏：两次写必须用两个名字。
        let dir = scratch("tmp-unique");
        let path = dir.join("state.json");
        let first = tmp_path(&path);
        let second = tmp_path(&path);
        assert_ne!(first, second, "同一进程连续两次写不能复用 .tmp 名");

        write_json(&path, &Sample { value: 1 }).unwrap();
        let tmp = tmp_path(&path);
        let name = tmp.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("state.json.")
                && name.ends_with(".tmp")
                && name.contains(&std::process::id().to_string()),
            "临时名应带 pid：{name}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 并发写不会留下损坏文件() {
        // 多写者同时落盘：最终文件必须是其中某一次的完整内容，而不是
        // 两个进程写同一个 .tmp 拼出来的半截。
        let dir = scratch("concurrent");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        let mut handles = Vec::new();
        for writer in 0..4u32 {
            let path = path.clone();
            handles.push(std::thread::spawn(move || {
                for seq in 0..25u32 {
                    write_json(
                        &path,
                        &Sample {
                            value: writer * 100 + seq,
                        },
                    )
                    .unwrap();
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        let loaded: Sample = read_json(&path)
            .expect("并发写之后文件必须仍然可解析")
            .expect("文件应当存在");
        // 每个写者的值都是 `writer * 100 + seq`：拆开能落回合法区间，
        // 说明读到的是某一次完整写入，而不是两个进程各写一半拼出来的。
        let (writer, seq) = (loaded.value / 100, loaded.value % 100);
        assert!(
            (0..4).contains(&writer) && (0..25).contains(&seq),
            "读到的应当是某次完整写入的值，实际 {}",
            loaded.value
        );
        assert!(
            leftover_tmps(&dir).is_empty(),
            "所有写者的临时文件都应被自己的 rename 消费掉"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 写锁互斥且超时降级() {
        let dir = scratch("lock");
        let lock_path = dir.join("writers.lock");
        let held = lock_writers(&lock_path, Duration::from_millis(100)).expect("空目录应能拿到锁");

        // 已被持有时：短时间内拿不到，返回 None 而不是死等。
        let contested = lock_writers(&lock_path, Duration::from_millis(80));
        assert!(contested.is_none(), "锁被持有时应超时返回 None");

        drop(held);
        let reacquired = lock_writers(&lock_path, Duration::from_millis(500));
        assert!(reacquired.is_some(), "释放后应能重新拿到");

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
