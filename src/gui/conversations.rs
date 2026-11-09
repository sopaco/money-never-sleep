//! On-disk archive of assistant conversations.
//!
//! The live transcript (`AgentState::timeline`) used to be memory-only: hiding
//! the window kept it in a process snapshot, quitting the app lost it. This
//! module owns a small, version-tolerant file format under the single writable
//! app-data root. One conversation per JSON file avoids rewriting the whole
//! archive on every streamed turn and makes deletion a single `remove_file`.
//!
//! All I/O here is called from `jobs`' worker thread, never from GPUI's main
//! loop. The pure helpers (summaries, tags, fence stripping) stay testable
//! without touching the user's real `~/.mns` directory.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::agent::{DecisionReport, EntryKind, RunState, TimelineEntry};

const MAX_TITLE_CHARS: usize = 32;
const MAX_PREVIEW_CHARS: usize = 48;

/// Identifies the conversation currently being written. It travels with the
/// window snapshot so a re-created window keeps appending to the same file
/// instead of starting a duplicate archive entry.
#[derive(Clone, Debug, Default)]
pub(crate) struct ConversationMeta {
    pub id: String,
    pub created_at: String,
    /// "手动" or "每日巡检".
    pub source: String,
}

impl ConversationMeta {
    pub(crate) fn new(source: impl Into<String>) -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let millis = now_millis();
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed) & 0xffff;
        Self {
            id: format!("c{millis:013}-{sequence:04x}"),
            created_at: now_rfc3339(),
            source: source.into(),
        }
    }
}

/// A timeline entry on disk. Tags are owned by this archive, not by the
/// runtime's `EntryKind`: an entry kind can be renamed or added upstream
/// without invalidating files already written.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct StoredEntry {
    pub kind: String,
    pub text: String,
}

impl Default for StoredEntry {
    fn default() -> Self {
        Self {
            kind: "system".to_owned(),
            text: String::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ConversationRecord {
    pub id: String,
    pub title: String,
    pub preview: String,
    pub created_at: String,
    pub updated_at: String,
    /// Epoch milliseconds; the list sorts on this rather than on a formatted
    /// local-time string, which is not monotone across a timezone change.
    pub updated_ms: i64,
    pub source: String,
    /// How the run ended. A replay must not paint a failed session green, so
    /// the terminal state travels with the record instead of being assumed.
    /// Older files have no field and fall back to "finished".
    pub state: String,
    pub entries: Vec<StoredEntry>,
    /// Raw ```mns-decision JSON, so a replay can rebuild the conclusion card.
    pub decision: Option<String>,
}

impl Default for ConversationRecord {
    fn default() -> Self {
        Self {
            id: String::new(),
            title: String::new(),
            preview: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
            updated_ms: 0,
            source: String::new(),
            state: String::new(),
            entries: Vec::new(),
            decision: None,
        }
    }
}

/// What the drawer needs. The full transcript is only read when one is opened.
#[derive(Clone, Debug)]
pub(crate) struct ConversationSummary {
    pub id: String,
    pub title: String,
    pub preview: String,
    pub updated_at: String,
    pub message_count: usize,
    pub source: String,
}

// ── Pure helpers ────────────────────────────────────────────────────────

/// The ```mns-decision fence is a machine channel: MNS renders it as a card.
/// Leaving the raw JSON in prose (or in a history preview) would show the
/// reader a block of syntax they never asked for.
pub(crate) fn strip_decision_block(text: &str) -> String {
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
            // Streaming text can end mid-fence. Drop the incomplete tail.
            None => {
                rest = "";
            }
        }
    }
    let mut cleaned = out.trim_end().to_owned();
    cleaned.push('\n');
    cleaned
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

fn entry_tag(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::User => "user",
        EntryKind::System => "system",
        EntryKind::Thought => "thought",
        EntryKind::Message => "message",
        EntryKind::Plan => "plan",
        EntryKind::ToolCall => "tool_call",
        EntryKind::ToolResult => "tool_result",
        EntryKind::Permission => "permission",
        EntryKind::Error => "error",
    }
}

fn entry_kind(tag: &str) -> EntryKind {
    match tag {
        "user" => EntryKind::User,
        "thought" => EntryKind::Thought,
        "message" => EntryKind::Message,
        "plan" => EntryKind::Plan,
        "tool_call" => EntryKind::ToolCall,
        "tool_result" => EntryKind::ToolResult,
        "permission" => EntryKind::Permission,
        "error" => EntryKind::Error,
        // Unknown tags from a newer build degrade to a trace row instead of
        // making the whole archive unreadable.
        _ => EntryKind::System,
    }
}

pub(crate) fn restore_entries(entries: &[StoredEntry]) -> Vec<TimelineEntry> {
    entries
        .iter()
        .map(|entry| TimelineEntry::new(entry_kind(&entry.kind), entry.text.clone()))
        .collect()
}

/// Terminal state tags. A session that is still running when it is archived
/// (the 60s safety net) is stored as "running" and replays as finished: MNS
/// cannot know how it ended once the process is gone, and guessing "failed"
/// would be worse than saying the transcript is simply over.
fn state_tag(state: RunState) -> &'static str {
    match state {
        RunState::Failed => "failed",
        RunState::Cancelled => "cancelled",
        RunState::Idle | RunState::Starting | RunState::Running => "running",
        RunState::WaitingPermission | RunState::Stopping | RunState::Finished => "finished",
    }
}

pub(crate) fn state_from_tag(tag: &str) -> RunState {
    match tag {
        "failed" => RunState::Failed,
        "cancelled" => RunState::Cancelled,
        _ => RunState::Finished,
    }
}

pub(crate) fn decision_from_raw(raw: &str) -> Option<DecisionReport> {
    let value = serde_json::from_str::<serde_json::Value>(raw).ok()?;
    Some(DecisionReport {
        raw: raw.to_owned(),
        value,
    })
}

/// Build an archive row from the live timeline. Scheduled reviews show a short
/// label instead of the several-thousand-character prompt, so the title comes
/// from what the user actually saw.
pub(crate) fn build_record(
    meta: &ConversationMeta,
    timeline: &[TimelineEntry],
    decision: Option<&DecisionReport>,
    state: RunState,
) -> Option<ConversationRecord> {
    let has_content = timeline.iter().any(|entry| {
        matches!(
            entry.kind,
            EntryKind::User | EntryKind::Message | EntryKind::Error
        )
    });
    if !has_content || meta.id.is_empty() {
        return None;
    }

    let title = timeline
        .iter()
        .find(|entry| entry.kind == EntryKind::User)
        .map(|entry| one_line(&entry.text))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "未命名会话".to_owned());
    let preview = timeline
        .iter()
        .rev()
        .find(|entry| entry.kind == EntryKind::Message)
        .map(|entry| {
            truncate_chars(
                &one_line(&strip_decision_block(&entry.text)),
                MAX_PREVIEW_CHARS,
            )
        })
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "暂无助手回复".to_owned());

    Some(ConversationRecord {
        id: meta.id.clone(),
        title: truncate_chars(&title, MAX_TITLE_CHARS),
        preview,
        created_at: meta.created_at.clone(),
        updated_at: now_rfc3339(),
        updated_ms: now_millis() as i64,
        source: meta.source.clone(),
        state: state_tag(state).to_owned(),
        entries: timeline
            .iter()
            .map(|entry| StoredEntry {
                kind: entry_tag(entry.kind).to_owned(),
                text: entry.text.clone(),
            })
            .collect(),
        decision: decision.map(|report| report.raw.clone()),
    })
}

pub(crate) fn summaries_from(mut records: Vec<ConversationRecord>) -> Vec<ConversationSummary> {
    // A damaged or half-written file is skipped by `list()`, but records with
    // an empty id must not produce a clickable row either.
    records.retain(|record| !record.id.is_empty());
    records.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms));
    records
        .into_iter()
        .map(|record| {
            let message_count = record
                .entries
                .iter()
                .filter(|entry| matches!(entry.kind.as_str(), "user" | "message"))
                .count();
            ConversationSummary {
                id: record.id,
                title: record.title,
                preview: record.preview,
                updated_at: display_time(&record.updated_at),
                message_count,
                source: record.source,
            }
        })
        .collect()
}

pub(crate) fn display_time(rfc3339: &str) -> String {
    let Some(parsed) = chrono::DateTime::parse_from_rfc3339(rfc3339).ok() else {
        return rfc3339.to_owned();
    };
    let local = parsed.with_timezone(&chrono::Local);
    if local.date_naive() == chrono::Local::now().date_naive() {
        format!("今天 {}", local.format("%H:%M"))
    } else {
        local.format("%m-%d %H:%M").to_string()
    }
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|span| span.as_millis())
        .unwrap_or_default()
}

fn now_rfc3339() -> String {
    chrono::Local::now().to_rfc3339()
}

// ── Filesystem ──────────────────────────────────────────────────────────

/// `~/.mns/agent/conversations/`. Everything MNS writes resolves against the
/// app-data root, never the process cwd (a Finder-launched app starts at `/`).
pub(crate) fn store_dir() -> PathBuf {
    crate::agent::settings_dir()
        .join("agent")
        .join("conversations")
}

fn safe_id(id: &str) -> Result<&str, String> {
    let valid = !id.is_empty()
        && id.len() <= 96
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_');
    if valid {
        Ok(id)
    } else {
        Err("会话编号无效".to_owned())
    }
}

fn record_path(dir: &Path, id: &str) -> Result<PathBuf, String> {
    Ok(dir.join(format!("{}.json", safe_id(id)?)))
}

/// The four public entry points resolve the same app-data directory; the
/// `*_in(dir, …)` helpers keep the file logic testable against a temp folder
/// so unit tests never touch the user's real archive.
fn save_in(dir: &Path, record: &ConversationRecord) -> Result<(), String> {
    let id = safe_id(&record.id)?;
    std::fs::create_dir_all(dir).map_err(|error| format!("无法创建会话目录：{error}"))?;
    let body =
        serde_json::to_string_pretty(record).map_err(|error| format!("无法序列化会话：{error}"))?;
    // Temp + rename, the same pattern as `save_settings`: a crash mid-write
    // must leave the previous version intact rather than a truncated JSON.
    let path = dir.join(format!("{id}.json"));
    let temp = dir.join(format!("{id}.json.tmp"));
    std::fs::write(&temp, body).map_err(|error| format!("无法写入会话：{error}"))?;
    std::fs::rename(&temp, &path).map_err(|error| format!("无法保存会话：{error}"))?;
    Ok(())
}

fn load_in(dir: &Path, id: &str) -> Result<ConversationRecord, String> {
    let path = record_path(dir, id)?;
    let body = std::fs::read_to_string(&path).map_err(|error| format!("无法读取会话：{error}"))?;
    let record = serde_json::from_str::<ConversationRecord>(&body)
        .map_err(|error| format!("会话文件已损坏：{error}"))?;
    Ok(record)
}

fn delete_in(dir: &Path, id: &str) -> Result<(), String> {
    let path = record_path(dir, id)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("无法删除会话：{error}")),
    }
}

fn list_in(dir: &Path) -> Result<Vec<ConversationSummary>, String> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(dir).map_err(|error| format!("无法读取会话目录：{error}"))?;
    let mut records = Vec::new();
    for entry in entries.flatten() {
        let path: PathBuf = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") || !path.is_file() {
            continue;
        }
        // A `.tmp` leftover from an interrupted write has no .json extension;
        // unreadable or malformed records are skipped so one bad file cannot
        // hide every other conversation.
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(record) = serde_json::from_str::<ConversationRecord>(&body) {
            records.push(record);
        }
    }
    Ok(summaries_from(records))
}

pub(crate) fn save(record: &ConversationRecord) -> Result<(), String> {
    save_in(&store_dir(), record)
}

pub(crate) fn load(id: &str) -> Result<ConversationRecord, String> {
    load_in(&store_dir(), id)
}

pub(crate) fn delete(id: &str) -> Result<(), String> {
    delete_in(&store_dir(), id)
}

pub(crate) fn list() -> Result<Vec<ConversationSummary>, String> {
    list_in(&store_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: EntryKind, text: &str) -> TimelineEntry {
        TimelineEntry::new(kind, text)
    }

    #[test]
    fn decision_fence_is_removed_from_visible_prose() {
        let text = "今天不操作。\n\n```mns-decision\n{\"verdict\":\"hold\"}\n```";
        assert_eq!(strip_decision_block(text), "今天不操作。\n");
    }

    #[test]
    fn text_without_a_fence_is_untouched() {
        let text = "## 结论\n\n- 现金充足\n";
        assert_eq!(strip_decision_block(text), text);
    }

    #[test]
    fn a_half_written_fence_does_not_leak_json() {
        let text = "正在写：\n```mns-decision\n{\"verdict\":";
        assert_eq!(strip_decision_block(text), "正在写：\n");
    }

    #[test]
    fn entry_tags_round_trip_and_unknown_tags_degrade_to_system() {
        for kind in [
            EntryKind::User,
            EntryKind::System,
            EntryKind::Thought,
            EntryKind::Message,
            EntryKind::Plan,
            EntryKind::ToolCall,
            EntryKind::ToolResult,
            EntryKind::Permission,
            EntryKind::Error,
        ] {
            assert_eq!(entry_kind(entry_tag(kind)), kind);
        }
        assert_eq!(entry_kind("future_kind"), EntryKind::System);
    }

    #[test]
    fn record_round_trip_keeps_the_visible_transcript() {
        let meta = ConversationMeta::new("每日巡检");
        let timeline = vec![
            entry(EntryKind::User, "每日巡检 · 2026-11-09"),
            entry(EntryKind::System, "正在运行 mns today"),
            entry(
                EntryKind::Message,
                "今天不操作。\n\n```mns-decision\n{\"verdict\":\"hold\"}\n```",
            ),
        ];
        let decision =
            decision_from_raw("{\"verdict\":\"hold\",\"headline\":\"维持不动\"}").unwrap();
        let record = build_record(&meta, &timeline, Some(&decision), RunState::Finished).unwrap();
        assert_eq!(record.title, "每日巡检 · 2026-11-09");
        assert_eq!(record.preview, "今天不操作。");
        assert_eq!(record.source, "每日巡检");

        let encoded = serde_json::to_string(&record).unwrap();
        let decoded: ConversationRecord = serde_json::from_str(&encoded).unwrap();
        let restored = restore_entries(&decoded.entries);
        assert_eq!(restored.len(), 3);
        assert_eq!(restored[0].kind, EntryKind::User);
        assert_eq!(restored[2].text, timeline[2].text);
        assert_eq!(
            decision_from_raw(decoded.decision.as_deref().unwrap())
                .unwrap()
                .headline(),
            "维持不动"
        );
    }

    #[test]
    fn a_failed_run_replays_as_failed() {
        assert_eq!(state_tag(RunState::Failed), "failed");
        assert_eq!(state_from_tag("failed"), RunState::Failed);
        assert_eq!(state_from_tag("cancelled"), RunState::Cancelled);
        // Files written before the field existed, and tags from a newer build,
        // must still open instead of looking corrupted.
        assert_eq!(state_from_tag(""), RunState::Finished);
        assert_eq!(state_from_tag("future_state"), RunState::Finished);

        let meta = ConversationMeta::new("手动");
        let record = build_record(
            &meta,
            &[entry(EntryKind::User, "问一句")],
            None,
            RunState::Failed,
        )
        .unwrap();
        assert_eq!(record.state, "failed");
        let encoded = serde_json::to_string(&record).unwrap();
        assert!(encoded.contains("\"state\":\"failed\""));
    }

    #[test]
    fn empty_transcripts_are_not_archived() {
        let meta = ConversationMeta::new("手动");
        assert!(build_record(&meta, &[], None, RunState::Finished).is_none());
        assert!(
            build_record(
                &meta,
                &[entry(EntryKind::System, "正在准备…")],
                None,
                RunState::Finished
            )
            .is_none()
        );
    }

    #[test]
    fn summaries_are_newest_first_and_count_only_visible_messages() {
        let base = ConversationRecord {
            id: "c1".to_owned(),
            title: "较旧".to_owned(),
            updated_ms: 100,
            entries: vec![
                StoredEntry {
                    kind: "user".to_owned(),
                    text: "问".to_owned(),
                },
                StoredEntry {
                    kind: "tool_call".to_owned(),
                    text: "mns".to_owned(),
                },
                StoredEntry {
                    kind: "message".to_owned(),
                    text: "答".to_owned(),
                },
            ],
            ..Default::default()
        };
        let newer = ConversationRecord {
            id: "c2".to_owned(),
            title: "较新".to_owned(),
            updated_ms: 200,
            ..Default::default()
        };
        let rows = summaries_from(vec![base, newer]);
        assert_eq!(rows[0].title, "较新");
        assert_eq!(rows[1].message_count, 2);
    }

    #[test]
    fn ids_cannot_escape_the_archive_directory() {
        assert!(safe_id("c0000000000001-0001").is_ok());
        assert!(safe_id("../config.toml").is_err());
        assert!(safe_id("").is_err());
        assert!(safe_id("a/b").is_err());
    }

    #[test]
    fn truncated_titles_count_characters_not_bytes() {
        assert_eq!(truncate_chars("一二三四", 3), "一二三…");
        assert_eq!(truncate_chars("一二", 3), "一二");
    }

    #[test]
    fn archive_files_round_trip_and_bad_files_are_skipped() {
        let dir = std::env::temp_dir().join(format!(
            "mns-conversations-test-{}-{}",
            std::process::id(),
            now_millis()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let meta = ConversationMeta::new("手动");
        let timeline = vec![
            entry(EntryKind::User, "问一句"),
            entry(EntryKind::Message, "答一句"),
        ];
        let record = build_record(&meta, &timeline, None, RunState::Finished).unwrap();
        save_in(&dir, &record).unwrap();
        // An interrupted write and a corrupt file must not hide a good record.
        std::fs::write(dir.join("broken.json"), "{not json").unwrap();
        std::fs::write(dir.join(format!("{}.json.tmp", record.id)), "{}").unwrap();

        let rows = list_in(&dir).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, record.id);
        assert_eq!(rows[0].message_count, 2);

        let loaded = load_in(&dir, &record.id).unwrap();
        assert_eq!(loaded.title, "问一句");
        assert_eq!(restore_entries(&loaded.entries)[1].text, "答一句");

        delete_in(&dir, &record.id).unwrap();
        assert!(list_in(&dir).unwrap().is_empty());
        // Deleting an already-removed record is idempotent.
        delete_in(&dir, &record.id).unwrap();

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
