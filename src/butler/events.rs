//! 本地行为事件日志（JSONL）。
//!
//! GUI 追加、管家只读。它记录的是**账户侧的客观动作**（打开应用、切页、
//! 打开买入弹层、护栏触发），不是管家的记忆，所以放在 `agent/events.jsonl`
//! 而不是 `butler/` 下。
//!
//! 三条底线：严格本地、可清除、首次启用明示。没有遥测，没有上传。

use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::events_path;
use super::store::{append_line, read_json, write_json};

/// 事件类型。刻意用字符串而不是枚举：旧版本 GUI 写的事件，新版本管家也要
/// 能读，未知类型只是不参与判定，不该让整行解析失败。
pub mod kind {
    pub const APP_OPEN: &str = "app_open";
    pub const PAGE_VIEW: &str = "page_view";
    pub const BUY_DIALOG: &str = "buy_dialog";
    pub const GUARD_HIT: &str = "guard_hit";
    pub const TRADE_RECORDED: &str = "trade_recorded";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehaviorEvent {
    pub at: DateTime<Local>,
    pub kind: String,
    /// 附加信息：页面 key、标的代码等。缺失即空对象。
    #[serde(default)]
    pub detail: serde_json::Value,
}

impl BehaviorEvent {
    pub fn new(kind: impl Into<String>, detail: serde_json::Value) -> Self {
        Self {
            at: Local::now(),
            kind: kind.into(),
            detail,
        }
    }

    pub fn page_view(page: impl Into<String>) -> Self {
        Self::new(kind::PAGE_VIEW, serde_json::json!({ "page": page.into() }))
    }
}

/// 用户对行为记录的开关。
///
/// 默认开启：管家若没有行为信号，只能靠账本事件，几乎无法判断「你是不是
/// 在盯盘」。但设置页必须明示它记什么、只在本地、可随时关闭与清除。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct BehaviorSettings {
    pub enabled: bool,
}

impl Default for BehaviorSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl BehaviorSettings {
    pub fn path() -> PathBuf {
        crate::config::app_data_dir()
            .join("agent")
            .join("behavior.json")
    }

    /// 读取失败按默认处理：埋点故障不能挡住应用启动。
    pub fn load() -> Self {
        read_json(&Self::path()).ok().flatten().unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        write_json(&Self::path(), self)
    }
}

/// 追加一条事件。失败不阻断 GUI：记账比埋点重要。
pub fn append(event: &BehaviorEvent) -> Result<()> {
    if !BehaviorSettings::load().enabled {
        return Ok(());
    }
    append_to(&events_path(), event)
}

pub fn append_to(path: &Path, event: &BehaviorEvent) -> Result<()> {
    let line = serde_json::to_string(event).context("序列化行为事件失败")?;
    append_line(path, &line)
}

/// 读全部事件。损坏行跳过而不是整份失败——埋点数据不值钱，管家不能因为它
/// 起不来。
pub fn read_all() -> Result<Vec<BehaviorEvent>> {
    read_from(&events_path())
}

pub fn read_from(path: &Path) -> Result<Vec<BehaviorEvent>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("读取 {} 失败", path.display())),
    };
    Ok(text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<BehaviorEvent>(line).ok())
        .collect())
}

/// 用户清除行为数据。
pub fn clear() -> Result<()> {
    let path = events_path();
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("删除 {} 失败", path.display())),
    }
}

/// 窗口内的行为画像，供开口判定第 2 条（高频查看）使用。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BehaviorWindow {
    pub opens: usize,
    pub page_views: BTreeMap<String, usize>,
    pub buy_dialogs: usize,
    pub guard_hits: usize,
}

impl BehaviorWindow {
    /// 某个页面在窗口内的查看次数。
    pub fn views(&self, page: &str) -> usize {
        self.page_views.get(page).copied().unwrap_or(0)
    }

    /// 「高频查看」：同一天把总览/计划看了 5 次以上。
    ///
    /// 阈值写成常量而不是散在调用点，是为了让「为什么这条算高频」有唯一答案。
    pub fn is_heavy_checking(&self, page: &str) -> bool {
        self.views(page) >= HEAVY_CHECK_VIEWS
    }
}

pub const HEAVY_CHECK_VIEWS: usize = 5;

/// 行为日志里的页面键 → 对用户可读的名称。日志用稳定英文键，话术用中文。
pub fn page_label(key: &str) -> &str {
    match key {
        "dashboard" => "总览",
        "portfolio" => "持仓",
        "plan" => "计划",
        "performance" => "复盘",
        "agent" => "助手",
        "settings" => "设置",
        other => other,
    }
}

/// 统计 `since` 之后的行为。
pub fn summarize(events: &[BehaviorEvent], since: DateTime<Local>) -> BehaviorWindow {
    let mut window = BehaviorWindow::default();
    for event in events.iter().filter(|event| event.at >= since) {
        match event.kind.as_str() {
            kind::APP_OPEN => window.opens += 1,
            kind::BUY_DIALOG => window.buy_dialogs += 1,
            kind::GUARD_HIT => window.guard_hits += 1,
            kind::PAGE_VIEW => {
                if let Some(page) = event.detail.get("page").and_then(|page| page.as_str()) {
                    *window.page_views.entry(page.to_owned()).or_insert(0) += 1;
                }
            }
            _ => {}
        }
    }
    window
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mns-butler-events-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("events.jsonl")
    }

    fn at(hour: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 11, 16, hour, 0, 0)
            .single()
            .unwrap()
    }

    #[test]
    fn 追加与读取往返() {
        let path = scratch("roundtrip");
        append_to(&path, &BehaviorEvent::page_view("dashboard")).unwrap();
        let mut event = BehaviorEvent::new(kind::BUY_DIALOG, serde_json::json!({}));
        event.at = at(10);
        append_to(&path, &event).unwrap();

        let rows = read_from(&path).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].kind, kind::PAGE_VIEW);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn 损坏行被跳过而不是拖垮整份日志() {
        let path = scratch("corrupt");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "{\"oops\"\n{\"at\":\"2026-11-16T10:00:00+08:00\",\"kind\":\"app_open\"}\n",
        )
        .unwrap();
        let rows = read_from(&path).unwrap();
        assert_eq!(rows.len(), 1);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn 高频查看按页面计数() {
        let path = scratch("heavy");
        let mut events = Vec::new();
        for _ in 0..HEAVY_CHECK_VIEWS {
            let mut event = BehaviorEvent::page_view("dashboard");
            event.at = at(10);
            events.push(event);
        }
        let mut other = BehaviorEvent::page_view("dashboard");
        other.at = at(10) - Duration::days(3);
        events.push(other);

        let window = summarize(&events, at(0));
        assert_eq!(window.views("dashboard"), HEAVY_CHECK_VIEWS);
        assert!(window.is_heavy_checking("dashboard"));
        assert!(!window.is_heavy_checking("plan"));

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
