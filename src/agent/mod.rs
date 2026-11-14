//! ACP external-agent runtime shared by the GUI (and any future front end).
//!
//! # Design boundary
//!
//! MNS deliberately does **not** own an inference endpoint or an API key. It
//! only needs the path of an ACP-compatible CLI (first supported target:
//! `opencode`) and automatically appends the `acp` subcommand. Provider,
//! credentials and endpoint stay in the external agent's own configuration.
//!
//! One thing MNS does carry is an optional **model name**. The ACP surface has
//! no model selector — `opencode acp` has no `--model` flag and the protocol
//! version MNS speaks has no `session/set_model` request — so a session would
//! silently inherit the agent's global default. That default is a paid model
//! for opencode, which turns "free models work locally" into an opaque
//! `-32603 / provider.quota` the moment the GUI runs it. The field is a plain
//! string, never a fixed list: free model ids come and go, and only the user
//! knows which one their account may use today.
//!
//! # Autonomy
//!
//! A user submits **one goal**. MNS opens one ACP session, sends one prompt and
//! lets the agent plan, call tools and iterate until it reaches a stop reason.
//! Permission requests are resolved by a policy (auto-approved by default) so
//! the user watches progress instead of driving a chat turn by turn. The user
//! can still gate sensitive actions, resolve permissions and stop at any time.

mod runner;
pub mod routine;

pub(crate) use runner::cli_path;

use std::path::{Path, PathBuf};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// How ACP `session/request_permission` requests are handled.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionPolicy {
    /// Default. Auto-approve with the narrowest `allow_once` option, never
    /// `allow_always`, and record every approval in the timeline.
    #[default]
    Autonomous,
    /// Auto-approve read/search/think/fetch; require one-click approval for
    /// write, delete, move, execute and unknown tools.
    GuardWrites,
    /// Require one-click approval for every permission request.
    ConfirmAll,
}

impl PermissionPolicy {
    pub const ALL: [PermissionPolicy; 3] = [
        PermissionPolicy::Autonomous,
        PermissionPolicy::GuardWrites,
        PermissionPolicy::ConfirmAll,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PermissionPolicy::Autonomous => "自主运行",
            PermissionPolicy::GuardWrites => "保护写入",
            PermissionPolicy::ConfirmAll => "逐项确认",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            PermissionPolicy::Autonomous => "自动批准工具调用（仅 allow_once），全程记录",
            PermissionPolicy::GuardWrites => "写入/删除/执行命令需一键授权",
            PermissionPolicy::ConfirmAll => "每次工具调用都需一键授权",
        }
    }
}

/// Persisted GUI settings. The user only has to supply `agent_cli`.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AgentSettings {
    /// Absolute path (or name on PATH) of the external ACP agent CLI.
    pub agent_cli: String,
    /// Optional model id handed to the external agent, e.g.
    /// `opencode/deepseek-v4.1-flash-free`.
    ///
    /// Stored as free text on purpose: free/qualified model ids change far more
    /// often than this app ships, and the user's own account decides which ones
    /// are usable. Empty means "do not touch the agent's default".
    pub model: String,
    /// Extra CLI arguments. MNS always appends `acp` when it is missing.
    pub extra_args: Vec<String>,
    /// Session working directory. Empty means the launch directory.
    pub working_dir: String,
    pub permission_policy: PermissionPolicy,
    /// Run the daily review unattended.
    pub daily_review: bool,
    /// Local time of the daily review, `HH:MM`.
    pub daily_review_time: String,
    /// 登录系统时静默启动（只常驻托盘）。默认关闭，由用户在设置里开启；
    /// 系统侧登录项由 `gui::autostart` 写入/删除。
    pub launch_at_login: bool,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            agent_cli: detect_agent_cli(),
            model: String::new(),
            extra_args: Vec::new(),
            working_dir: String::new(),
            permission_policy: PermissionPolicy::Autonomous,
            daily_review: false,
            daily_review_time: "08:30".to_owned(),
            launch_at_login: false,
        }
    }
}

/// Where a turn came from. The daily review is not a user message: the UI shows
/// it as a scheduled run, and the prompt is assembled by MNS rather than typed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum GoalSource {
    #[default]
    Manual,
    DailyReview,
}

/// A structured conclusion returned by the agent inside a ```mns-decision
/// block.
///
/// This is the whole mns ↔ agent handshake: MNS owns storage and rendering, the
/// agent owns judgement. The agent never writes a file — it returns JSON, the
/// shell validates and persists it. `value` is kept untyped so an older GUI
/// still stores a newer agent's extra fields instead of rejecting the run.
#[derive(Clone, Debug)]
pub struct DecisionReport {
    pub raw: String,
    pub value: serde_json::Value,
}

impl DecisionReport {
    /// One-line headline for the collapsed card. Missing or non-string fields
    /// fall back rather than hiding the whole report.
    pub fn headline(&self) -> String {
        self.value
            .get("headline")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or("本轮未给出结论摘要")
            .to_owned()
    }

    pub fn verdict(&self) -> &str {
        self.value
            .get("verdict")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown")
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RunState {
    #[default]
    Idle,
    Starting,
    Running,
    WaitingPermission,
    Stopping,
    Finished,
    Failed,
    Cancelled,
}

impl RunState {
    pub fn label(self) -> &'static str {
        match self {
            RunState::Idle => "空闲",
            RunState::Starting => "正在连接 Agent",
            RunState::Running => "自主执行中",
            RunState::WaitingPermission => "等待授权",
            RunState::Stopping => "正在停止",
            RunState::Finished => "已完成",
            RunState::Failed => "失败",
            RunState::Cancelled => "已取消",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryKind {
    /// Something the user asked. Rendered as the user's own bubble, not as a
    /// trace row, so a session reads as a conversation.
    User,
    System,
    Thought,
    Message,
    Plan,
    ToolCall,
    ToolResult,
    Permission,
    Error,
}

impl EntryKind {
    pub fn label(self) -> &'static str {
        match self {
            EntryKind::User => "你",
            EntryKind::System => "系统",
            EntryKind::Thought => "思考",
            EntryKind::Message => "输出",
            EntryKind::Plan => "计划",
            EntryKind::ToolCall => "工具",
            EntryKind::ToolResult => "结果",
            EntryKind::Permission => "授权",
            EntryKind::Error => "错误",
        }
    }
}

#[derive(Clone, Debug)]
pub struct TimelineEntry {
    pub kind: EntryKind,
    pub text: String,
}

impl TimelineEntry {
    pub fn new(kind: EntryKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PermissionChoice {
    pub id: String,
    pub label: String,
    pub allow: bool,
}

#[derive(Clone, Debug)]
pub struct PermissionPrompt {
    pub id: u64,
    pub title: String,
    pub detail: String,
    pub options: Vec<PermissionChoice>,
}

/// Everything the UI needs, delivered on a single channel from the runtime.
#[derive(Clone, Debug)]
pub enum AgentEvent {
    State {
        state: RunState,
        detail: String,
    },
    Entry(TimelineEntry),
    /// A token-level chunk of a streamed message or thought. The UI must merge
    /// consecutive deltas of the same kind instead of rendering one row each.
    Delta {
        kind: EntryKind,
        text: String,
    },
    Permission(PermissionPrompt),
    PermissionResolved {
        id: u64,
    },
    /// One prompt finished. The session stays open, so the user can follow up.
    TurnFinished {
        stop_reason: String,
    },
    /// A ```mns-decision block parsed out of the finished turn.
    Decision(DecisionReport),
    /// The session itself ended; a follow-up needs a new run.
    Finished {
        stop_reason: String,
    },
    Failed {
        message: String,
    },
}

/// Commands from the UI to the runtime thread.
#[derive(Clone, Debug)]
pub enum AgentCommand {
    Start {
        goal: String,
        settings: AgentSettings,
        source: GoalSource,
    },
    /// A follow-up question in the running session. Reuses the same ACP
    /// session so the agent keeps its plan, tool history and context.
    Ask {
        text: String,
    },
    Stop,
    Grant {
        id: u64,
        option_id: String,
    },
    Deny {
        id: u64,
    },
    Shutdown,
}

/// Cheap, cloneable handle for sending commands to the runtime thread.
#[derive(Clone)]
pub struct AgentHandle {
    commands: UnboundedSender<AgentCommand>,
}

impl AgentHandle {
    pub fn send(&self, command: AgentCommand) -> Result<(), String> {
        self.commands
            .send(command)
            .map_err(|error| format!("Agent 运行时已停止：{error}"))
    }
}

/// GUI settings live beside the existing `~/.mns/config.toml` and `mns.db`.
/// Reuse the single application data root rather than repeating the path.
pub fn settings_dir() -> PathBuf {
    crate::config::app_data_dir()
}

pub fn settings_path() -> PathBuf {
    settings_dir().join("gui.toml")
}

pub fn load_settings() -> AgentSettings {
    let mut settings = match std::fs::read_to_string(settings_path()) {
        Ok(content) => toml::from_str::<AgentSettings>(&content).unwrap_or_default(),
        Err(_) => AgentSettings::default(),
    };
    if settings.agent_cli.trim().is_empty() {
        settings.agent_cli = detect_agent_cli();
    }
    settings
}

pub fn save_settings(settings: &AgentSettings) -> Result<(), String> {
    let path = settings_path();
    std::fs::create_dir_all(settings_dir())
        .map_err(|error| format!("无法创建配置目录：{error}"))?;
    let body =
        toml::to_string_pretty(settings).map_err(|error| format!("无法序列化配置：{error}"))?;
    // Write through a temp file + rename so a crash cannot truncate settings.
    let temp = path.with_extension("toml.tmp");
    std::fs::write(&temp, body).map_err(|error| format!("无法写入配置：{error}"))?;
    std::fs::rename(&temp, &path).map_err(|error| format!("无法保存配置：{error}"))?;
    Ok(())
}

/// Best-effort discovery of `opencode`, so most users never type anything.
pub fn detect_agent_cli() -> String {
    if let Some(home) = dirs::home_dir() {
        for relative in [
            ".opencode/bin/opencode",
            ".opencode/bin/opencode.exe",
            ".local/bin/opencode",
            "bin/opencode",
        ] {
            let candidate = home.join(relative);
            if is_file(&candidate) {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&paths) {
            for name in ["opencode", "opencode.exe"] {
                let candidate = directory.join(name);
                if is_file(&candidate) {
                    return candidate.to_string_lossy().into_owned();
                }
            }
        }
    }
    "opencode".to_owned()
}

fn is_file(path: &Path) -> bool {
    path.is_file()
}

/// Returns `(handle, event_receiver)`. The runtime owns a real OS thread with
/// its own Tokio runtime so ACP I/O never blocks the GPUI main thread.
pub fn start_runtime() -> (AgentHandle, UnboundedReceiver<AgentEvent>) {
    runner::start_runtime_impl()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_only_require_a_cli_path() {
        let settings = AgentSettings::default();
        assert!(
            !settings.agent_cli.is_empty(),
            "a usable CLI path must be pre-detected"
        );
        assert!(settings.extra_args.is_empty());
        assert_eq!(settings.permission_policy, PermissionPolicy::Autonomous);
        assert!(settings.working_dir.is_empty());
        assert!(
            settings.model.is_empty(),
            "an unset model must defer to the agent's own default"
        );
    }

    #[test]
    fn settings_round_trip_never_contains_an_endpoint_or_api_key() {
        let settings = AgentSettings {
            agent_cli: "/opt/bin/opencode".to_owned(),
            model: "opencode/deepseek-v4.1-flash-free".to_owned(),
            extra_args: vec!["--log-level".to_owned(), "warn".to_owned()],
            working_dir: "/tmp/mns-work".to_owned(),
            permission_policy: PermissionPolicy::GuardWrites,
            ..AgentSettings::default()
        };
        let encoded = toml::to_string_pretty(&settings).unwrap();
        assert!(!encoded.to_lowercase().contains("api_key"));
        assert!(!encoded.to_lowercase().contains("endpoint"));
        assert!(!encoded.to_lowercase().contains("base_url"));
        let decoded: AgentSettings = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.agent_cli, settings.agent_cli);
        assert_eq!(
            decoded.model, "opencode/deepseek-v4.1-flash-free",
            "the model name is user data, not a value MNS may rewrite"
        );
        assert_eq!(decoded.extra_args, settings.extra_args);
        assert_eq!(decoded.working_dir, settings.working_dir);
        assert_eq!(decoded.permission_policy, PermissionPolicy::GuardWrites);
    }

    #[test]
    fn malformed_or_partial_settings_fall_back_to_defaults() {
        let decoded: AgentSettings = toml::from_str("permission_policy = \"confirm_all\"").unwrap();
        assert_eq!(decoded.permission_policy, PermissionPolicy::ConfirmAll);
        assert!(!decoded.agent_cli.is_empty());
        assert!(decoded.extra_args.is_empty());
    }

    #[test]
    fn run_state_labels_are_stable_for_the_ui() {
        assert_eq!(RunState::Idle.label(), "空闲");
        assert_eq!(RunState::WaitingPermission.label(), "等待授权");
        assert_eq!(RunState::Finished.label(), "已完成");
    }
}
