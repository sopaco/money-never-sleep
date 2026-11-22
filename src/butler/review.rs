//! 巡检：排程、当天是否跑过、以及巡检报告的唯一存储。
//!
//! 这个模块是**非 feature 门控**的：`mns-butlerd`（agent 构建）、GUI 窗口
//! 进程与 `mns` 主程序都能用它。
//!
//! 两件事分开看：
//!
//! - **排程**：`schedule()` 是纯函数，回答「下一次该在什么时候巡检」。它读
//!   一个只有日期的小文件（`agent/last-review`），不读报告、不碰数据库。
//! - **报告**：`agent/reviews/<id>.json` 一份一个文件。定时巡检与手动巡检
//!   写同一个目录，巡检页的历史和管家窗口的 `@` 引用读同一个目录——不再有
//!   「GUI 一套、管家一套」两份历史。
//!
//! 旧 `src/gui/conversations.rs` 的会话存档不再读取；用户已有的
//! `agent/conversations/` 文件保持原样，不迁移、不删除。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Datelike, Local, TimeZone};
use serde::{Deserialize, Serialize};

use super::store::{read_json, write_json};

/// `last-review` 的文件名。内容只有一行日期（`YYYY-MM-DD`）。
pub const REVIEW_FILE: &str = "last-review";

/// 当天巡检失败后的重试记录。成功或跨天后自动失效。
pub const REVIEW_ATTEMPT_FILE: &str = "last-review-attempt.json";

/// 当天最多尝试几次巡检（首次 + 两次重试）。
pub const MAX_REVIEW_ATTEMPTS: u32 = 3;

/// 失败后隔多久再试。短到当天还有机会补救，长到不会把模型当成重试器。
pub const REVIEW_RETRY_MINUTES: i64 = 30;

/// 一次排好的巡检。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyReview {
    pub at: DateTime<Local>,
    /// `review:<目标日期>`：同一天最多一次，用于唤醒去重。
    pub id: String,
}

/// 巡检报告。一次运行一份，定时与手动共用。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewReport {
    pub id: String,
    /// `YYYY-MM-DD`，报告归属的日期（不是文件时间）。
    pub date: String,
    pub created_at: String,
    pub updated_ms: i64,
    /// `每日巡检` / `手动巡检`。
    pub source: String,
    /// `running` / `finished` / `failed` / `cancelled`。
    pub state: String,
    pub title: String,
    pub preview: String,
    /// 已剔除 ```mns-decision 围栏的正文。
    pub body: String,
    /// 原始 `mns-decision` JSON，回放时用来重建结论卡片。
    pub decision: Option<String>,
}

impl Default for ReviewReport {
    fn default() -> Self {
        Self {
            id: String::new(),
            date: String::new(),
            created_at: String::new(),
            updated_ms: 0,
            source: String::new(),
            state: String::new(),
            title: String::new(),
            preview: String::new(),
            body: String::new(),
            decision: None,
        }
    }
}

impl ReviewReport {
    /// 新报告：分配 id、写入创建时间与标题。
    pub fn new(source: impl Into<String>, date: impl Into<String>) -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let millis = now_millis();
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed) & 0xffff;
        let date = date.into();
        Self {
            id: format!("r{millis:013}-{sequence:04x}"),
            title: format!("巡检 · {date}"),
            date,
            created_at: now_rfc3339(),
            updated_ms: millis,
            source: source.into(),
            state: "running".to_owned(),
            preview: String::new(),
            body: String::new(),
            decision: None,
        }
    }

    /// 收尾：写入正文/结论/状态，并刷新预览与时间戳。
    pub fn finish(
        &mut self,
        state: impl Into<String>,
        body: impl Into<String>,
        decision: Option<String>,
    ) {
        self.state = state.into();
        self.body = strip_decision_block(&body.into());
        self.preview = preview_of(&self.body);
        self.decision = decision;
        self.updated_ms = now_millis();
    }

    pub fn summary(&self) -> ReviewSummary {
        ReviewSummary {
            id: self.id.clone(),
            date: self.date.clone(),
            title: self.title.clone(),
            preview: self.preview.clone(),
            source: self.source.clone(),
            state: self.state.clone(),
            updated_at: format_local_ms(self.updated_ms),
        }
    }
}

/// 历史列表与 `@` 选择器需要的最小信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewSummary {
    pub id: String,
    pub date: String,
    pub title: String,
    pub preview: String,
    pub source: String,
    pub state: String,
    pub updated_at: String,
}

/// 跨进程传的引用：只带 id 与标题，正文由接收方按 id 读取。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRef {
    pub id: String,
    pub title: String,
}

// ── 排程 ────────────────────────────────────────────────────────────────

/// `agent/last-review`：只记录「哪一天已经巡检过」。
pub fn last_review_path() -> PathBuf {
    crate::config::app_data_dir()
        .join("agent")
        .join(REVIEW_FILE)
}

/// 读上次巡检日期。空文件或读失败都当没跑过——宁可多跑一次，也别因为
/// 一个坏文件把巡检永久停掉。
pub fn read_last() -> Option<String> {
    read_last_at(&last_review_path())
}

/// 写上次巡检日期（`YYYY-MM-DD`）。
pub fn write_last(date: &str) -> std::io::Result<()> {
    write_last_at(&last_review_path(), date)
}

fn read_last_at(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_owned())
    }
}

fn write_last_at(path: &Path, date: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, date.trim())?;
    std::fs::rename(&tmp, path)
}

/// 当天失败过的巡检尝试。
///
/// 没有它，一次超时就要么当天不再补跑（用户看到「巡检没执行」），要么被
/// agenda 立刻重排成死循环。退避时刻落盘，守护进程重启也不会丢。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewAttempt {
    /// `YYYY-MM-DD`。
    pub date: String,
    /// 当天已经失败的次数。
    pub failures: u32,
    /// 下一次允许重试的时刻（本地时区毫秒时间戳）。
    pub retry_at_ms: i64,
}

impl ReviewAttempt {
    /// 当天是否已经把次数用完。
    pub fn exhausted(&self, today: &str) -> bool {
        self.date == today && self.failures >= MAX_REVIEW_ATTEMPTS
    }
}

pub fn attempt_path() -> PathBuf {
    crate::config::app_data_dir()
        .join("agent")
        .join(REVIEW_ATTEMPT_FILE)
}

/// 读当天的失败记录。坏文件当没有：宁可多跑一次，也别把巡检永久停掉。
pub fn read_attempt() -> Option<ReviewAttempt> {
    read_json(&attempt_path()).ok().flatten()
}

/// 记一次失败并写下退避时刻。
///
/// 写盘失败时退化成「今天跑过」：否则 agenda 会立刻再拉起一轮，变成烧模型
/// 的死循环。返回的计数用于决定还要不要重试、以及要不要落一份失败报告。
pub fn record_attempt_failure(now: DateTime<Local>, date: &str) -> ReviewAttempt {
    let mut attempt = read_attempt().unwrap_or_default();
    if attempt.date != date {
        attempt = ReviewAttempt {
            date: date.to_owned(),
            failures: 0,
            retry_at_ms: 0,
        };
    }
    attempt.failures = attempt.failures.saturating_add(1);
    attempt.retry_at_ms =
        (now + chrono::Duration::minutes(REVIEW_RETRY_MINUTES)).timestamp_millis();
    if let Err(error) = write_json(&attempt_path(), &attempt) {
        eprintln!("巡检重试记录写入失败：{error:#}");
        let _ = write_last(date);
    }
    attempt
}

/// 巡检成功：清掉当天的失败记录。
pub fn clear_attempt() {
    let path = attempt_path();
    if let Err(error) = std::fs::remove_file(&path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("巡检重试记录清理失败：{error}");
    }
}

/// 下一次该在什么时候巡检。
///
/// 规则很短：今天还没跑且已经过点，就现在补跑；还没到点就今天那个时刻；
/// 今天跑过就明天。当天失败过则按 `attempt.retry_at_ms` 退避重试，次数
/// 用尽后与「今天跑过」一样排明天。`None` 只表示「没开启」或「时间配置
/// 不合法」。
pub fn schedule(
    now: DateTime<Local>,
    enabled: bool,
    time: &str,
    last: Option<&str>,
    attempt: Option<&ReviewAttempt>,
) -> Option<DailyReview> {
    if !enabled {
        return None;
    }
    let (hour, minute) = parse_clock(time)?;
    let today = now.date_naive();
    let today_text = today.format("%Y-%m-%d").to_string();

    let done_today = last.is_some_and(|last| last.trim() == today_text)
        || attempt.is_some_and(|attempt| attempt.exhausted(&today_text));
    if done_today {
        let tomorrow = today.succ_opt()?;
        let at = local_at(tomorrow, hour, minute).unwrap_or(now);
        return Some(DailyReview {
            at,
            id: format!("review:{}", tomorrow.format("%Y-%m-%d")),
        });
    }

    let mut at = local_at(today, hour, minute).unwrap_or(now).max(now);
    // 当天失败过：等退避时刻，别把失败立刻重排成第二轮。
    if let Some(attempt) = attempt.filter(|attempt| attempt.date == today_text && attempt.failures > 0)
        && let Some(retry_at) = Local
            .timestamp_millis_opt(attempt.retry_at_ms)
            .single()
    {
        at = at.max(retry_at);
    }
    Some(DailyReview {
        at,
        id: format!("review:{today_text}"),
    })
}

/// `HH:MM` → `(时, 分)`。接受 `8:30` 与 `08:30`。
fn parse_clock(time: &str) -> Option<(u32, u32)> {
    let (hour, minute) = time.trim().split_once(':')?;
    let hour: u32 = hour.parse().ok()?;
    let minute: u32 = minute.parse().ok()?;
    (hour < 24 && minute < 60).then_some((hour, minute))
}

fn local_at(day: chrono::NaiveDate, hour: u32, minute: u32) -> Option<DateTime<Local>> {
    day.and_hms_opt(hour, minute, 0)
        .and_then(|naive| Local.from_local_datetime(&naive).single())
}

// ── 报告存储 ────────────────────────────────────────────────────────────

/// `~/.mns/agent/reviews`。
pub fn store_dir() -> PathBuf {
    crate::config::app_data_dir().join("agent").join("reviews")
}

/// 报告 id 只允许 `[A-Za-z0-9-]`：它会被拼进文件名，不能带路径分隔符。
pub fn is_safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 96 && id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
}

pub fn save(report: &ReviewReport) -> Result<(), String> {
    save_in(&store_dir(), report)
}

pub fn load(id: &str) -> Result<ReviewReport, String> {
    load_in(&store_dir(), id)
}

pub fn delete(id: &str) -> Result<(), String> {
    delete_in(&store_dir(), id)
}

/// 按更新时间倒序，最多 `limit` 条。坏文件跳过，不因一个文件让整个列表失败。
pub fn list(limit: usize) -> Result<Vec<ReviewSummary>, String> {
    list_in(&store_dir(), limit)
}

/// `@` 选择器用：只保留成功且有正文的报告。
pub fn recent_refs(limit: usize) -> Vec<ReviewRef> {
    list(limit)
        .unwrap_or_default()
        .into_iter()
        .filter(|row| row.state == "finished" && !row.preview.is_empty())
        .take(limit)
        .map(|row| ReviewRef {
            id: row.id,
            title: row.title,
        })
        .collect()
}

fn save_in(dir: &Path, report: &ReviewReport) -> Result<(), String> {
    if !is_safe_id(&report.id) {
        return Err(format!("巡检报告 id 不合法：{}", report.id));
    }
    write_json(&dir.join(format!("{}.json", report.id)), report)
        .map_err(|error| format!("巡检报告写入失败：{error:#}"))
}

fn load_in(dir: &Path, id: &str) -> Result<ReviewReport, String> {
    if !is_safe_id(id) {
        return Err(format!("巡检报告 id 不合法：{id}"));
    }
    read_json::<ReviewReport>(&dir.join(format!("{id}.json")))
        .map_err(|error| format!("巡检报告读取失败：{error:#}"))?
        .ok_or_else(|| "巡检报告不存在".to_owned())
}

fn delete_in(dir: &Path, id: &str) -> Result<(), String> {
    if !is_safe_id(id) {
        return Err(format!("巡检报告 id 不合法：{id}"));
    }
    match std::fs::remove_file(dir.join(format!("{id}.json"))) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("巡检报告删除失败：{error}")),
    }
}

fn list_in(dir: &Path, limit: usize) -> Result<Vec<ReviewSummary>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("巡检目录读取失败：{error}")),
    };
    let mut rows = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        // 读到坏文件就跳过：历史列表不能因为一份半截 JSON 整个打不开。
        if let Ok(Some(report)) = read_json::<ReviewReport>(&path) {
            if report.id.is_empty() || !is_safe_id(&report.id) {
                continue;
            }
            rows.push(report);
        }
    }
    rows.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms));
    rows.truncate(limit);
    Ok(rows.iter().map(ReviewReport::summary).collect())
}

// ── 纯函数 ──────────────────────────────────────────────────────────────

/// 把 ```mns-decision 围栏从正文里拿掉。结论卡片走 `decision` 字段，
/// 正文里再留一段 JSON 只会让读者看到语法。
pub fn strip_decision_block(text: &str) -> String {
    const FENCE: &str = "```mns-decision";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let Some(start) = rest.find(FENCE) else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let after = &rest[start + FENCE.len()..];
        match after.find("```") {
            Some(end) => {
                rest = after[end + 3..].trim_start_matches(['\r', '\n']);
            }
            // 流式正文可能停在围栏中间：丢掉没写完的尾巴。
            None => {
                rest = "";
            }
        }
    }
    out.trim_end().to_owned()
}

fn preview_of(body: &str) -> String {
    truncate_chars(&one_line(body), 48)
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(text: &str, limit: usize) -> String {
    let mut out: String = text.chars().take(limit).collect();
    if text.chars().count() > limit {
        out.push('…');
    }
    out
}

pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn now_rfc3339() -> String {
    Local::now().to_rfc3339()
}

fn format_local_ms(ms: i64) -> String {
    match Local.timestamp_millis_opt(ms).single() {
        Some(at) => at.format("%Y-%m-%d %H:%M").to_string(),
        None => String::new(),
    }
}

/// 报告里的日期：本地时间。
pub fn today_text() -> String {
    Local::now().date_naive().format("%Y-%m-%d").to_string()
}

/// 报告正文的日期（用于 prompt 里说「今天是…」）。
pub fn weekday_text(day: &str) -> String {
    match chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d") {
        Ok(date) => format!(
            "周{}",
            ["一", "二", "三", "四", "五", "六", "日"]
                [date.weekday().num_days_from_monday() as usize]
        ),
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at(day: u32, hour: u32, minute: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 11, day, hour, minute, 0)
            .single()
            .unwrap()
    }

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("mns-review-{name}-{}", std::process::id()))
            .join("reviews")
    }

    #[test]
    fn 未开启或时间非法时不排() {
        let now = at(21, 9, 0);
        assert_eq!(schedule(now, false, "08:30", None, None), None);
        assert_eq!(schedule(now, true, "25:00", None, None), None);
        assert_eq!(schedule(now, true, "08:70", None, None), None);
        assert_eq!(schedule(now, true, "八点半", None, None), None);
    }

    #[test]
    fn 今天未跑且未到点时排今天那个时刻() {
        let review = schedule(at(21, 7, 0), true, "08:30", None, None).unwrap();
        assert_eq!(review.at, at(21, 8, 30));
        assert_eq!(review.id, "review:2026-11-21");
    }

    #[test]
    fn 今天未跑且已过点时立刻补跑() {
        let now = at(21, 10, 0);
        let review = schedule(now, true, "08:30", None, None).unwrap();
        assert_eq!(review.at, now);
        assert_eq!(review.id, "review:2026-11-21");
    }

    #[test]
    fn 今天跑过就排明天() {
        let review = schedule(at(21, 10, 0), true, "08:30", Some("2026-11-21"), None).unwrap();
        assert_eq!(review.at, at(22, 8, 30));
        assert_eq!(review.id, "review:2026-11-22");
    }

    #[test]
    fn 当天失败按退避时刻重试() {
        let now = at(21, 10, 0);
        let attempt = ReviewAttempt {
            date: "2026-11-21".to_owned(),
            failures: 1,
            retry_at_ms: at(21, 10, 30).timestamp_millis(),
        };
        let review = schedule(now, true, "08:30", None, Some(&attempt)).unwrap();
        assert_eq!(review.at, at(21, 10, 30), "失败后等退避时刻，不立刻重跑");
        assert_eq!(review.id, "review:2026-11-21");
    }

    #[test]
    fn 重试次数用尽就排明天() {
        let now = at(21, 10, 0);
        let attempt = ReviewAttempt {
            date: "2026-11-21".to_owned(),
            failures: MAX_REVIEW_ATTEMPTS,
            retry_at_ms: at(21, 10, 30).timestamp_millis(),
        };
        let review = schedule(now, true, "08:30", None, Some(&attempt)).unwrap();
        assert_eq!(review.at, at(22, 8, 30));
        assert_eq!(review.id, "review:2026-11-22");
    }

    #[test]
    fn last_review_读写往返() {
        let path = std::env::temp_dir()
            .join(format!("mns-review-last-{}", std::process::id()))
            .join(REVIEW_FILE);
        let _ = std::fs::remove_file(&path);
        assert_eq!(read_last_at(&path), None);
        write_last_at(&path, "2026-11-21\n").unwrap();
        assert_eq!(read_last_at(&path).as_deref(), Some("2026-11-21"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn 报告保存读取列表与删除() {
        let dir = scratch("store");
        let _ = std::fs::remove_dir_all(&dir);
        let mut first = ReviewReport::new("手动巡检", "2026-11-21");
        first.finish(
            "finished",
            "本周不操作。\n```mns-decision\n{\"action\":\"hold\"}\n```\n",
            Some("{\"action\":\"hold\"}".to_owned()),
        );
        let mut second = ReviewReport::new("每日巡检", "2026-11-20");
        second.finish("finished", "低配 6.2pp。", None);
        second.updated_ms = first.updated_ms - 1;

        save_in(&dir, &first).unwrap();
        save_in(&dir, &second).unwrap();
        let loaded = load_in(&dir, &first.id).unwrap();
        assert_eq!(loaded.state, "finished");
        assert!(
            !loaded.body.contains("mns-decision"),
            "正文里不能留机器围栏"
        );
        assert_eq!(loaded.decision.as_deref(), Some("{\"action\":\"hold\"}"));

        let rows = list_in(&dir, 10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, first.id, "按更新时间倒序");

        delete_in(&dir, &first.id).unwrap();
        assert_eq!(list_in(&dir, 10).unwrap().len(), 1);
        assert!(load_in(&dir, &first.id).is_err());
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn 坏文件跳过而不是让列表失败() {
        let dir = scratch("bad");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("r-broken.json"), "{not json").unwrap();
        let mut good = ReviewReport::new("手动巡检", "2026-11-21");
        good.id = "r-good".to_owned();
        good.finish("finished", "正文", None);
        save_in(&dir, &good).unwrap();
        let rows = list_in(&dir, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "r-good");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn 报告_id_不允许路径分隔符() {
        let dir = scratch("path");
        let mut report = ReviewReport::new("手动巡检", "2026-11-21");
        report.id = "../escape".to_owned();
        assert!(save_in(&dir, &report).is_err());
        assert!(load_in(&dir, "../escape").is_err());
        assert!(load_in(&dir, "").is_err());
    }

    #[test]
    fn 围栏剔除保留正文与结尾换行规则() {
        assert_eq!(
            strip_decision_block("结论。\n\n```mns-decision\n{}\n```\n"),
            "结论。"
        );
        assert_eq!(strip_decision_block("没有围栏"), "没有围栏");
        assert_eq!(
            strip_decision_block("半截\n```mns-decision\n{\"a\":"),
            "半截"
        );
    }

    #[test]
    fn 预览是单行且截断() {
        let body = "第一行\n第二行 ".to_owned() + &"字".repeat(80);
        let preview = preview_of(&body);
        assert!(!preview.contains('\n'));
        assert!(preview.chars().count() <= 49);
    }

    #[test]
    fn 日期与星期文案() {
        assert_eq!(
            weekday_text("2026-11-20"),
            "周五",
            "{}",
            NaiveDate::from_ymd_opt(2026, 11, 20).unwrap()
        );
        assert_eq!(weekday_text("坏日期"), "");
    }
}
