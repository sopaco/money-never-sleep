//! GUI 与帮助窗口之间的「翻到第几页」请求。
//!
//! 帮助中心是独立进程，GUI 只能启动它、没法往里递参数；而用户点第二个 `?`
//! 时窗口往往已经开着，再启动一个实例在任何平台上都不是好答案（Linux/macOS
//! 根本没有单实例锁）。所以走一份极简的请求文件：
//!
//! - GUI 把目标 slug 写进 `~/.mns/help-request`（先写临时文件再改名，
//!   轮询方不会读到半截内容）；
//! - 帮助窗口每 400ms 看一眼这个文件的修改时间，变了就跳页并把自己提到前面；
//! - 冷启动同时把 slug 放进命令行，文件在启动前的写入会被 mtime 判为过期。
//!
//! 用文件而不是管道/命名事件，是因为它跨平台、可 `cat`、且失败模式是
//! 「停在当前页」而不是「窗口打不开」——帮助本身不该成为新的故障点。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 请求文件的位置：跟配置、报告放在同一个数据目录，排查时不用猜。
pub fn path() -> PathBuf {
    crate::config::app_data_dir().join("help-request")
}

/// 请求帮助窗口跳到 `slug`；空串表示「只把窗口提到前面，别翻页」。
pub fn request(slug: &str) -> std::io::Result<()> {
    request_at(&path(), slug)
}

/// [`request`] 的可注入路径版本：测试用它指向临时目录，不去踩真实请求。
pub fn request_at(path: &Path, slug: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let trimmed = slug.trim();
    let tmp = path.with_extension("tmp");
    let via_temp = || -> std::io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(trimmed.as_bytes())?;
        // 立刻落盘：另一个进程要在毫秒级看到它，不能等内核刷缓存。
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    };
    via_temp().or_else(|error| {
        // 先改名再覆盖在个别文件系统上会被拒；退化成直接写，宁可让轮询方
        // 有机会读到半截内容，也不要让「点问号没反应」。
        let _ = std::fs::remove_file(&tmp);
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            return Err(error);
        }
        std::fs::write(path, trimmed.as_bytes())
    })
}

/// 轮询器：只在文件被**改动过**时返回内容。
///
/// `since` 取构造时刻，用来把「进程启动前留下的请求」判为过期：冷启动那条
/// 路已经把目标页交给 argv 了，再按文件跳一次会把用户后来的选择覆盖掉。
pub struct Watcher {
    path: PathBuf,
    since: SystemTime,
    seen: Option<SystemTime>,
}

impl Default for Watcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Watcher {
    pub fn new() -> Self {
        Self::at(path())
    }

    /// [`Watcher::new`] 的可注入路径版本。
    pub fn at(path: PathBuf) -> Self {
        Self::since(path, SystemTime::now())
    }

    /// `since` 之前的请求视为已消费。
    ///
    /// 帮助窗口传的是**进程启动时刻**而不是轮询器创建时刻：窗口从启动到建好
    /// 轮询器要几百毫秒，那期间用户完全可能又点了另一个问号，按创建时刻切分
    /// 会把这次请求吞掉。
    pub fn since(path: PathBuf, since: SystemTime) -> Self {
        Self {
            path,
            since,
            seen: None,
        }
    }

    /// 有新请求时返回 slug（可能是空串，表示只置前）。
    pub fn poll(&mut self) -> Option<String> {
        let stamp = modified(&self.path)?;
        if stamp < self.since || self.seen == Some(stamp) {
            return None;
        }
        self.seen = Some(stamp);
        std::fs::read_to_string(&self.path)
            .ok()
            .map(|text| text.trim().to_owned())
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mns-deeplink-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        dir.join("help-request")
    }

    #[test]
    fn request_is_read_once() {
        let path = scratch("once");
        let mut watcher = Watcher::at(path.clone());
        assert_eq!(watcher.poll(), None, "还没有请求时不该返回内容");
        // 先让 watcher 的 `since` 落在一个确定的过去时刻，再写请求：某些文件
        // 系统的时间戳只精确到秒，同一秒内写入会被误判成「启动前的旧请求」。
        std::thread::sleep(Duration::from_millis(15));

        request_at(&path, "strategy-rebalance").expect("写入请求失败");
        // 文件系统时间戳的粒度不一定小于 1ms，等一下再读更稳。
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(watcher.poll().as_deref(), Some("strategy-rebalance"));
        assert_eq!(watcher.poll(), None, "同一个请求只该消费一次");

        // 空串是「只置前」：窗口要能收到，但不能当成一个页面去找。
        request_at(&path, "").expect("写入置前请求失败");
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(watcher.poll().as_deref(), Some(""));

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// 启动前写好的请求（冷启动那条路）必须被忽略，否则窗口会在用户翻到
    /// 别处之后又被拽回命令行指定的页。
    #[test]
    fn requests_from_before_startup_are_ignored() {
        let path = scratch("stale");
        request_at(&path, "finance-metrics").expect("写入请求失败");
        std::thread::sleep(Duration::from_millis(10));

        let mut watcher = Watcher::at(path.clone());
        assert_eq!(watcher.poll(), None, "启动前的请求不该再触发跳转");

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// 覆盖写不能失败：用户连续点两个 `?` 时，第二个必须能覆盖第一个。
    #[test]
    fn second_request_overwrites_the_first() {
        let path = scratch("overwrite");
        let mut watcher = Watcher::at(path.clone());
        std::thread::sleep(Duration::from_millis(15));

        request_at(&path, "desktop-plan").expect("写入请求失败");
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(watcher.poll().as_deref(), Some("desktop-plan"));

        request_at(&path, "finance-metrics").expect("覆盖请求失败");
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(watcher.poll().as_deref(), Some("finance-metrics"));

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
