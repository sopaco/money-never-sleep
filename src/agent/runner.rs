//! The ACP runtime: one OS thread + Tokio runtime, isolated from GPUI.
//!
//! Responsibilities:
//! * Build the external agent launch command from a CLI path plus `acp`.
//! * Drive `initialize -> session/new -> session/prompt` in one long-running
//!   turn, so the agent plans and calls tools without user turns.
//! * Translate `session/update` notifications into a structured timeline.
//! * Act as the permission proxy: auto-approve by policy, or suspend on a
//!   oneshot until the UI resolves it.
//! * Honour stop with a protocol `session/cancel`, then force-terminate after
//!   a bounded grace period by dropping the connection (kills the child).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    CancelNotification, ContentBlock, InitializeRequest, NewSessionRequest, PermissionOptionKind,
    PlanEntryStatus, PromptRequest, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, SessionId, SessionNotification,
    SessionUpdate, TextContent, ToolCallContent, ToolCallStatus, ToolCallUpdate, ToolKind,
};
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, Client};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::workspace::{self, Binaries, Profile};
use super::{
    AgentCommand, AgentEvent, AgentHandle, AgentSettings, DecisionReport, EntryKind, GoalSource,
    PermissionChoice, PermissionPolicy, PermissionPrompt, RunState, TimelineEntry,
    detect_agent_cli,
};

type EventTx = UnboundedSender<AgentEvent>;

#[derive(Debug)]
enum Decision {
    Allow(String),
    Reject(String),
    Cancelled,
}

struct PendingPermission {
    tx: oneshot::Sender<Decision>,
    deny_id: String,
}

#[derive(Default)]
struct SessionShared {
    next_id: u64,
    pending: HashMap<u64, PendingPermission>,
    connection: Option<ConnectionTo<Agent>>,
    session_id: Option<SessionId>,
    stopped: bool,
}

type Shared = Arc<Mutex<SessionShared>>;

struct SessionHandle {
    shared: Shared,
    stop: Option<oneshot::Sender<()>>,
    /// Follow-up prompts for the live ACP session. Dropping it ends the turn
    /// loop; the connection then closes and the child process exits.
    ask: Option<mpsc::UnboundedSender<String>>,
    task: JoinHandle<()>,
}

pub(crate) fn start_runtime_impl() -> (AgentHandle, UnboundedReceiver<AgentEvent>) {
    let (command_tx, mut command_rx) = mpsc::unbounded_channel::<AgentCommand>();
    let (event_tx, event_rx) = mpsc::unbounded_channel::<AgentEvent>();

    std::thread::Builder::new()
        .name("mns-agent-runtime".to_owned())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = event_tx.send(AgentEvent::Failed {
                        message: format!("无法启动 Agent 运行时：{error}"),
                    });
                    return;
                }
            };

            runtime.block_on(async move {
                let mut session: Option<SessionHandle> = None;

                while let Some(command) = command_rx.recv().await {
                    match command {
                        AgentCommand::Start {
                            goal,
                            settings,
                            source,
                        } => {
                            if session.as_ref().is_some_and(|s| !s.task.is_finished()) {
                                emit_entry(
                                    &event_tx,
                                    EntryKind::Error,
                                    "已有任务在运行，请先停止当前任务。",
                                );
                                continue;
                            }
                            let shared: Shared = Arc::new(Mutex::new(SessionShared::default()));
                            let (stop_tx, stop_rx) = oneshot::channel();
                            let (ask_tx, ask_rx) = mpsc::unbounded_channel::<String>();
                            let events = event_tx.clone();
                            let task_shared = shared.clone();
                            let task = tokio::spawn(async move {
                                run_session(
                                    goal,
                                    source,
                                    settings,
                                    events,
                                    task_shared,
                                    stop_rx,
                                    ask_rx,
                                )
                                .await;
                            });
                            session = Some(SessionHandle {
                                shared,
                                stop: Some(stop_tx),
                                ask: Some(ask_tx),
                                task,
                            });
                        }
                        AgentCommand::Ask { text } => {
                            let Some(active) = session.as_ref() else {
                                emit_entry(
                                    &event_tx,
                                    EntryKind::Error,
                                    "当前没有会话，请先开始一个任务，再追问。",
                                );
                                continue;
                            };
                            if active.task.is_finished() {
                                emit_entry(
                                    &event_tx,
                                    EntryKind::Error,
                                    "上一个会话已结束，请重新开始一个任务。",
                                );
                                continue;
                            }
                            let delivered = active
                                .ask
                                .as_ref()
                                .is_some_and(|ask| ask.send(text).is_ok());
                            if !delivered {
                                emit_entry(&event_tx, EntryKind::Error, "追问未能送达 Agent。");
                            }
                        }
                        AgentCommand::Stop => {
                            if let Some(active) = session.as_mut() {
                                {
                                    let mut guard =
                                        active.shared.lock().unwrap_or_else(|e| e.into_inner());
                                    guard.stopped = true;
                                    // A cancelled turn must answer every pending
                                    // permission request with `cancelled`.
                                    for (_, pending) in guard.pending.drain() {
                                        let _ = pending.tx.send(Decision::Cancelled);
                                    }
                                    if let (Some(connection), Some(session_id)) =
                                        (&guard.connection, &guard.session_id)
                                    {
                                        let _ = connection.send_notification(
                                            CancelNotification::new(session_id.clone()),
                                        );
                                    }
                                }
                                if let Some(stop) = active.stop.take() {
                                    let _ = stop.send(());
                                }
                            } else {
                                emit_entry(&event_tx, EntryKind::System, "当前没有运行中的任务。");
                            }
                        }
                        AgentCommand::Grant { id, option_id } => {
                            resolve(&session, id, Decision::Allow(option_id));
                            let _ = event_tx.send(AgentEvent::PermissionResolved { id });
                        }
                        AgentCommand::Deny { id } => {
                            resolve(&session, id, Decision::Reject(String::new()));
                            let _ = event_tx.send(AgentEvent::PermissionResolved { id });
                        }
                        AgentCommand::Shutdown => {
                            if let Some(active) = session.as_mut() {
                                let mut guard =
                                    active.shared.lock().unwrap_or_else(|e| e.into_inner());
                                guard.stopped = true;
                                for (_, pending) in guard.pending.drain() {
                                    let _ = pending.tx.send(Decision::Cancelled);
                                }
                            }
                            if let Some(active) = session.take() {
                                active.task.abort();
                            }
                            break;
                        }
                    }
                }
            });
        })
        .expect("failed to spawn the MNS agent runtime thread");

    (
        AgentHandle {
            commands: command_tx,
        },
        event_rx,
    )
}

fn resolve(session: &Option<SessionHandle>, id: u64, decision: Decision) {
    let Some(active) = session else { return };
    let mut guard = active.shared.lock().unwrap_or_else(|e| e.into_inner());
    let Some(pending) = guard.pending.remove(&id) else {
        return;
    };
    let decision = match decision {
        Decision::Reject(empty) if empty.is_empty() => Decision::Reject(pending.deny_id.clone()),
        other => other,
    };
    let _ = pending.tx.send(decision);
}

fn emit_state(events: &EventTx, state: RunState, detail: impl Into<String>) {
    let _ = events.send(AgentEvent::State {
        state,
        detail: detail.into(),
    });
}

fn emit_entry(events: &EventTx, kind: EntryKind, text: impl Into<String>) {
    let _ = events.send(AgentEvent::Entry(TimelineEntry::new(kind, text)));
}

/// Absolute path of the mns CLI that ships with this app.
///
/// The bundle puts `mns` beside `mns-gui`, so the directory of the running
/// executable is authoritative. That is the whole point: an agent started by
/// this GUI should call one known path, not search the filesystem. The
/// installed-bundle fallback covers `cargo run`, where the GUI lives under
/// `target/debug` and may not have a sibling CLI.
pub fn mns_bin() -> Option<PathBuf> {
    let name = if cfg!(windows) { "mns.exe" } else { "mns" };

    if let Some(from_env) = std::env::var_os("MNS_BIN") {
        let candidate = PathBuf::from(from_env);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        let sibling = dir.join(name);
        if sibling.is_file() {
            return Some(sibling);
        }
    }

    #[cfg(target_os = "macos")]
    {
        let mut roots = vec![PathBuf::from("/Applications")];
        if let Some(home) = dirs::home_dir() {
            roots.push(home.join("Applications"));
        }
        for root in roots {
            let candidate = root.join("MNS.app/Contents/MacOS").join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

/// Absolute path of the butler CLI that ships beside `mns`.
///
/// `MNS_BUTLER_BIN` wins for tests and unusual installs. Otherwise the sibling
/// of the `mns` binary is authoritative, so the two always come from the same
/// bundle. `None` is a real state: the workspace skill says the butler is
/// unavailable instead of printing a path that cannot run.
pub fn butler_bin() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "mns-butler.exe"
    } else {
        "mns-butler"
    };

    if let Some(from_env) = std::env::var_os("MNS_BUTLER_BIN") {
        let candidate = PathBuf::from(from_env);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    if let Some(dir) = mns_bin().and_then(|mns| mns.parent().map(Path::to_path_buf)) {
        let sibling = dir.join(name);
        if sibling.is_file() {
            return Some(sibling);
        }
    }

    #[cfg(target_os = "macos")]
    {
        let mut roots = vec![PathBuf::from("/Applications")];
        if let Some(home) = dirs::home_dir() {
            roots.push(home.join("Applications"));
        }
        for root in roots {
            let candidate = root.join("MNS.app/Contents/MacOS").join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

/// Environment handed to the external agent.
///
/// Two variables, both aimed at the same failure: an agent that does not know
/// where the CLI is will search for it, and may end up reading the SQLite file
/// instead. `MNS_BIN` names the binary, and prepending its directory to `PATH`
/// makes a bare `mns` resolve to the shipped version rather than a stale copy
/// installed elsewhere.
fn agent_env(cli: &Path, existing_path: &str) -> Vec<(String, String)> {
    let mut vars = vec![("MNS_BIN".to_owned(), cli.display().to_string())];

    if let Some(dir) = cli.parent() {
        let separator = if cfg!(windows) { ";" } else { ":" };
        let dir = dir.display();
        let path = if existing_path.is_empty() {
            dir.to_string()
        } else {
            format!("{dir}{separator}{existing_path}")
        };
        vars.push(("PATH".to_owned(), path));
    }

    vars
}

/// One V2 permission rule: `{action, resource, effect}`.
///
/// V2 rule names matter: `shell` replaces V1 `bash`, `subagent` replaces `task`,
/// and the `edit` action covers `edit`, `write` and `patch`. Action names are
/// free strings in the schema, so a future plugin action still validates.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Rule {
    action: &'static str,
    /// `String` rather than `&'static str`: the guard is generated from the
    /// installed CLI path, and quoting/`mns.exe` variants would otherwise need
    /// dozens of hand-written constants.
    resource: String,
    effect: &'static str,
}

impl Rule {
    fn deny(action: &'static str, resource: impl Into<String>) -> Self {
        Self {
            action,
            resource: resource.into(),
            effect: "deny",
        }
    }
}

/// One-line permission note in the timeline.
///
/// Capability wording on purpose: it says what is enabled and that everything
/// else follows the agent default. "Read-only" or "all tools denied" would
/// contradict the policy after the capability-first change.
const PERMISSION_NOTE: &str = "权限：已启用 mns 账本写入保护（其他工具按 Agent 默认）";

/// Best-effort ledger guard shared by both profiles.
///
/// Capability comes first: the task page is an agent workspace, and the butler
/// is allowed to run commands, browse and use skills. This list is **not a
/// sandbox** — it only stops the obvious accident of asking an agent to run a
/// command that writes the ledger:
///
/// * `init`, `add`, `buy`, `sell`, `price`, `remove`, `sentiment`, `report`,
///   `today`, `update-prices`, `cash set|deposit|withdraw`,
///   `backtest update-data`.
///
/// `config` is deliberately out of scope: it guards strategy knobs, not the
/// ledger, and changing one is a visible action the user can take in the
/// settings page. Its subcommand shapes also proved unpredictable across
/// opencode builds — a `config * *` rule was verified to swallow the
/// one-argument read `config thresholds.fear` — so no rule is better than a
/// rule that misfires on a documented read.
///
/// Three deliberate limits:
/// * Exact spellings only (bare `mns`, the real absolute path, the quoted
///   absolute path, and the `.exe` forms). A leading `*` would also match
///   `rm -rf ~/mns cash`, turning a guard into an allowlist.
/// * Appended after nothing else: opencode's default is `allow`, so a deny is
///   the only rule MNS needs for ordinary work to keep working.
/// * `OPENCODE_CONFIG_CONTENT` is only read by opencode. With another ACP agent
///   the guard does not apply at all; the prompt still asks it not to write.
fn ledger_guard(cli: &Path) -> Vec<Rule> {
    let mut rules = Vec::new();
    for command in [
        "init",
        "add",
        "buy",
        "sell",
        "price",
        "remove",
        "sentiment",
        "report",
        "today",
        "update-prices",
        "cash set",
        "cash deposit",
        "cash withdraw",
        "backtest update-data",
    ] {
        deny_mns(&mut rules, cli, command);
    }
    rules
}

/// Every spelling of the CLI a shell command may start with.
///
/// Bare `mns` (PATH is redirected to the bundle), the real absolute path, the
/// quoted absolute path (the workspace contract quotes it), and the Windows
/// `.exe` forms.
fn mns_heads(cli: &Path) -> Vec<String> {
    let path = cli.display().to_string();
    vec![
        "mns".to_owned(),
        path.clone(),
        format!("\"{path}\""),
        "mns.exe".to_owned(),
        "\"mns.exe\"".to_owned(),
    ]
}

/// Deny one mns write command in every spelling the shell may hand us.
///
/// Two patterns per spelling: the bare command and the form with arguments.
/// opencode's matcher does not treat a trailing ` *` as optional, so a single
/// `mns today *` rule would let a bare `mns today` through. The guard only
/// names mns write commands; `rm -rf ~` in a compound command is left to the
/// agent's own default policy — this is a guardrail, not a sandbox.
fn deny_mns(rules: &mut Vec<Rule>, cli: &Path, command: &str) {
    for head in mns_heads(cli) {
        rules.push(Rule::deny("shell", format!("{head} {command}")));
        rules.push(Rule::deny("shell", format!("{head} {command} *")));
    }
}

fn permission_rules(_profile: Profile, cli: Option<&Path>) -> Vec<Rule> {
    // With no CLI the guard has nothing to name, and opencode keeps its
    // default allow policy. That is the intended capability-first behavior:
    // the prompt still tells the agent not to touch the ledger.
    cli.map(ledger_guard).unwrap_or_default()
}

/// Optional overrides handed to the external agent as `OPENCODE_CONFIG_CONTENT`.
///
/// Why an env var instead of an argument: `opencode acp` accepts no `--model`
/// flag, and the ACP version this crate speaks has no `session/set_model`
/// request, so a session started by the GUI would otherwise inherit the
/// agent's global default — which for opencode is a paid model, surfacing as an
/// opaque `-32603 / provider.quota` when the account has no balance.
///
/// `OPENCODE_CONFIG_CONTENT` is opencode's inline config layer: it is parsed as
/// one more config document that merges over the user's own file *for this
/// child process only*. Verified end to end for `model`; the same channel
/// carries `permissions`, which is the only place a deny rule can be enforced.
///
/// Permissions are always injected: even a task session appends its policy,
/// otherwise the field could silently disappear whenever the model is empty.
fn agent_config_env(
    settings: &AgentSettings,
    cli: Option<&Path>,
    profile: Profile,
) -> (String, String) {
    let model = settings.model.trim();
    let rules = permission_rules(profile, cli);
    let existing = std::env::var("OPENCODE_CONFIG_CONTENT").ok();
    (
        "OPENCODE_CONFIG_CONTENT".to_owned(),
        merge_agent_config(existing.as_deref(), model, &rules),
    )
}

/// Merge the model and permission rules into an inline opencode config.
///
/// Existing keys are preserved: a user who exported `OPENCODE_CONFIG_CONTENT`
/// keeps their theme, providers and earlier permission rules. New rules are
/// appended because opencode resolves the *last* matching rule — appended
/// rules therefore win over earlier ones.
fn merge_agent_config(existing: Option<&str>, model: &str, rules: &[Rule]) -> String {
    let mut content = existing
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();

    if !model.is_empty() {
        content.insert(
            "model".to_owned(),
            serde_json::Value::String(model.to_owned()),
        );
    }

    let permissions = content
        .entry("permissions".to_owned())
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    if !permissions.is_array() {
        // A non-array value cannot be extended; replacing it is the only way to
        // keep the deny rules enforceable (V1 `permission` is a different key
        // and is left untouched).
        *permissions = serde_json::Value::Array(Vec::new());
    }
    if let Some(array) = permissions.as_array_mut() {
        for rule in rules {
            array.push(serde_json::json!({
                "action": rule.action,
                "resource": &rule.resource,
                "effect": rule.effect,
            }));
        }
    }

    serde_json::Value::Object(content).to_string()
}

fn launch_config(settings: &AgentSettings, cli: Option<&Path>, profile: Profile) -> AcpAgentConfig {
    let base = if settings.agent_cli.trim().is_empty() {
        detect_agent_cli()
    } else {
        settings.agent_cli.trim().to_owned()
    };

    let mut args = settings.extra_args.clone();
    // MNS owns the `acp` subcommand; the user only configures the CLI path.
    if !args.iter().any(|arg| arg == "acp") {
        args.push("acp".to_owned());
    }

    // A command string such as `npx opencode` is split on whitespace, but a
    // path may contain spaces too: `D:\Program Files (x86)\QuickBin\opencode.exe`
    // split into `D:\Program` + `Files …` points at a file that does not exist,
    // and the spawn fails with `os error 2` before any ACP traffic. Paths always
    // contain a separator, so that is what tells the two cases apart.
    let config = if base.chars().any(char::is_whitespace) && !base.contains(['/', '\\']) {
        let mut parts = base.split_whitespace();
        let command = parts.next().unwrap_or("opencode");
        AcpAgentConfig::new(command).args(parts).args(args)
    } else {
        AcpAgentConfig::new(base).args(args)
    };

    let mut vars: Vec<(String, String)> = vec![agent_config_env(settings, cli, profile)];
    if let Some(cli) = cli {
        let existing = std::env::var("PATH").unwrap_or_default();
        vars.extend(agent_env(cli, &existing));
    }

    config.envs(vars)
}

fn resolve_cwd(settings: &AgentSettings) -> PathBuf {
    let trimmed = settings.working_dir.trim();
    let cwd = if trimmed.is_empty() {
        // 固定工作区，而不是 `std::env::current_dir()`。GUI 从 Finder 启动时
        // cwd 是只读的 `/`，沿用它会让 agent 的每次写文件都报 os error 30。
        crate::config::app_data_dir()
            .join("agent")
            .join("workspace")
    } else {
        PathBuf::from(trimmed)
    };
    let _ = std::fs::create_dir_all(&cwd);
    cwd
}

/// Build the first prompt for a session.
///
/// The CLI path is part of the contract, not an afterthought. Without it the
/// agent burns its first turns on `find`, `ls /private/tmp` and reading
/// `Info.plist`; worse, it may decide the database file is the data source and
/// query `~/.mns/mns.db` with sqlite3. Naming the binary and forbidding the
/// storage layer keeps the agent on the one supported interface.
///
/// Two different contracts live here, and the split is the point:
///
/// - `Manual` / `DailyReview` are tasks. They keep the original autonomy
///   contract (plan, call tools until done, report what was completed, return
///   a `mns-decision` block). The assistant page and the scheduled review
///   depend on it, so its text must not drift.
/// - `Conversation` is the butler window. It is an ACP session too, but the
///   user is chatting: the autonomy contract made the model narrate in-progress
///   work and append `action:` fields, which is exactly what a butler must not
///   do. It gets a short dialogue contract instead.
fn compose_goal(
    goal: &str,
    source: GoalSource,
    cwd: &Path,
    cli: Option<&Path>,
    managed: bool,
) -> String {
    let cli_rule = cli_rule(cli, managed);

    let contract = match source {
        GoalSource::Manual | GoalSource::DailyReview => format!(
            "【MNS 桌面端执行约定】\n\
             - 这是一个自主执行任务：请自行拆解步骤、连续调用所需工具，直到目标达成或确认无法继续；\
             不要逐步向用户提问，也不要等待用户补充信息。\n\
             - 工作目录：{cwd}\n\
             {cli_rule}\n\
             - 可以使用 shell、联网、编辑与子代理；需要账户数据时用 mns 的只读子命令\
             （portfolio / cash / performance / plan / guard / history / market）。\n\
             - MNS 只加了一条 best-effort 护栏：会改账本的 mns 子命令和 `init`\
             （buy / sell / cash set / today 等）在 opencode 下会被拒绝；\
             其余子命令（含 `config params` 等查询）照常可用。不要再尝试绕过。\
             换其他 ACP agent 时这条护栏不生效，请自己遵守。\n\
             - 工作目录可写；但不要把结论另存成记录文件，最终结论仍通过回答里的 mns-decision 块回传，\
             由 MNS 统一保存与展示。\n\
             - 不要执行下单、交易、资金划转、删除仓库、推送远端或对外发送数据的操作。\n\
             - 模型名可能由 MNS 设置指定；推理端点与凭据始终由你自己的配置文件管理（例如 opencode 的 provider 配置）。\
             MNS 不提供 URL 或 API Key，也不要向用户索要这些内容。\n\
             - 完成后用中文输出：已完成事项、涉及的文件或命令、以及未完成或有风险的事项。",
            cwd = cwd.display()
        ),
        GoalSource::Conversation => format!(
            "【MNS 桌面端对话约定】\n\
             - 这是一次对话，不是任务：不要拆解步骤，不要写「我先查一下」「接下来我会…」这类过程旁白。\n\
             - 账户事实已经注入在上下文里：够答就直接答；缺数据可以用 mns 只读命令查，查不到就直说没有。\n\
             - 可以执行命令、联网、使用技能；不要写任意文件，也不要动账本；\
             管家状态与到点任务只用 mns-butler 改。\n\
             - 管家状态与到点任务用 mns-butler（路径和命令清单见工作目录 AGENTS.md 与 mns-butler 技能）。\n\
             - 提醒只能靠真的建任务：`mns-butler task add \"<到点检查什么>\" --at \"YYYY-MM-DD HH:MM\"`；\
             先用 `date \"+%Y-%m-%d %H:%M\"` 取当前时间再算。看到「已接受」才能说记下了；\
             不要用 sleep / osascript / display notification 假装提醒。\n\
             - 回执要说清前提：到点提醒需要 MNS 开着（托盘也算）；静音时段、主动开口关闭或额度用完都会压住。\n\
             - 用中文回答，像真人聊天：先给结论，再给一两句理由；不要逐条汇报「已完成事项 / 涉及命令」。\n\
             - 不要输出 mns-decision 块，也不要在正文里写 action、effective、monthly_amount 之类的字段。\n\
             - 工作目录：{cwd}。\n\
             - 不要执行下单、交易、资金划转、删除仓库、推送远端或对外发送数据的操作。\n\
             - 「记住 / 忘掉」由管家界面在消息进入你之前解析并落盘；不要调用 remember / forget，\
             也不要声称自己写了记忆。\n\
             - 模型名可能由 MNS 设置指定；推理端点与凭据始终由你自己的配置文件管理（例如 opencode 的 provider 配置）。\
             MNS 不提供 URL 或 API Key，也不要向用户索要这些内容。",
            cwd = cwd.display()
        ),
    };

    format!("{goal}\n\n---\n{contract}")
}

/// The one line of standing rules the prompt keeps.
///
/// When the managed workspace was written, the details live in `AGENTS.md` and
/// the prompt only needs to name the binary, keep the storage ban, and point
/// at the contract. A user-chosen `working_dir` gets the full fallback: MNS
/// does not write into someone else's repository, so the prompt must carry
/// everything the agent needs.
fn cli_rule(cli: Option<&Path>, managed: bool) -> String {
    let Some(cli) = cli else {
        return "- 未检测到随应用安装的 mns CLI。任务若需要账户数据，请停止并告知用户，\
                不要自行搜索文件系统，也不要改用其他数据源。"
            .to_owned();
    };
    let cli = cli.display();
    let entry = format!(
        "- 数据入口只有 mns CLI，绝对路径 `{cli}`，已随本应用安装。\
         账户、持仓、绩效、报告、定投计划一律通过它读取。\
         不要用 find / mdfind / ls 搜索 mns，也不要依赖 PATH 上的同名命令——那可能是旧版本。"
    );
    let storage = "- 不要绕过 CLI 访问底层存储：禁止用 sqlite3 打开 `~/.mns/mns.db`，\
                   禁止解析 `~/.mns/config.toml` 的内部结构，禁止阅读 mns 源码。\
                   CLI 没有提供的数据，说明它不该由你获取。";
    if managed {
        format!("{entry}\n{storage}\n- 工作约定、可用技能与只读命令清单见工作目录下的 AGENTS.md。")
    } else {
        format!(
            "{entry}\n{storage}\n- 常用只读示例：`{cli} portfolio`、`{cli} performance`、`{cli} --help`。\n\
             `mns today` / `mns report` 会写入快照与报告，不要运行；需要刷新时请用户自行运行。"
        )
    }
}

/// Pull the last ```mns-decision block out of an assistant message.
///
/// Fenced blocks are the only channel the agent has back to MNS. Requiring a
/// labelled fence — rather than "any JSON in the prose" — keeps ordinary
/// examples and tool output from being mistaken for a conclusion.
pub fn extract_decision(text: &str) -> Option<DecisionReport> {
    const FENCE: &str = "```mns-decision";
    let mut found = None;
    let mut rest = text;
    while let Some(start) = rest.find(FENCE) {
        let after = &rest[start + FENCE.len()..];
        let after = after.trim_start_matches(['\r', '\n']);
        let Some(end) = after.find("```") else {
            break;
        };
        let body = after[..end].trim();
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
            found = Some(DecisionReport {
                raw: body.to_owned(),
                value,
            });
        }
        rest = &after[end + 3..];
    }
    found
}

async fn run_session(
    goal: String,
    source: GoalSource,
    settings: AgentSettings,
    events: EventTx,
    shared: Shared,
    mut stop_rx: oneshot::Receiver<()>,
    ask_rx: mpsc::UnboundedReceiver<String>,
) {
    emit_state(&events, RunState::Starting, "正在启动外部 Agent…");

    let cwd = resolve_cwd(&settings);
    if !cwd.is_dir() {
        let _ = events.send(AgentEvent::Failed {
            message: format!("工作目录不存在：{}，请在设置中修改。", cwd.display()),
        });
        return;
    }

    // The profile decides both the workspace contract and the permission set,
    // so it has to exist before the launch environment is built.
    let profile = match source {
        GoalSource::Conversation => Profile::Butler,
        GoalSource::Manual | GoalSource::DailyReview => Profile::Task,
    };

    let cli = mns_bin();
    let config = launch_config(&settings, cli.as_deref(), profile);
    emit_entry(
        &events,
        EntryKind::System,
        format!(
            "启动 {} {}",
            config.command().display(),
            config.arguments().join(" ")
        ),
    );
    // Surface the injected path in the timeline: if an agent still goes looking
    // for the binary, the user can see whether it was told where it lives.
    match &cli {
        Some(path) => emit_entry(
            &events,
            EntryKind::System,
            format!("mns CLI：{}", path.display()),
        ),
        None => emit_entry(
            &events,
            EntryKind::System,
            "未检测到 mns CLI：本次任务不提供账户数据入口",
        ),
    }
    // Naming the model in the timeline is the fastest way to answer "did my
    // setting actually reach the agent?" when a provider rejects the call.
    emit_entry(
        &events,
        EntryKind::System,
        match settings.model.trim() {
            "" => "模型：沿用外部 Agent 的默认模型".to_owned(),
            model => format!("模型：{model}"),
        },
    );
    // The guard is invisible in the UI otherwise, and "why did it refuse that
    // command" deserves an answer in the run log.
    emit_entry(&events, EntryKind::System, PERMISSION_NOTE);

    // Write the standing contract before the agent starts. Only MNS-owned
    // workspaces are touched; a user-chosen directory keeps its own rules and
    // gets the full rule set in the prompt instead.
    let butler = butler_bin();
    let managed = match workspace::bootstrap(
        &cwd,
        profile,
        Binaries {
            mns: cli.as_deref(),
            butler: butler.as_deref(),
        },
    ) {
        Ok(managed) => managed,
        Err(error) => {
            emit_entry(
                &events,
                EntryKind::Error,
                format!("工作区约定未写入，本次改用 prompt 兜底：{error}"),
            );
            false
        }
    };

    let agent = AcpAgent::new(config);
    let prompt_text = compose_goal(&goal, source, &cwd, cli.as_deref(), managed);

    let events_for_updates = events.clone();
    let events_for_permissions = events.clone();
    let events_for_foreground = events.clone();
    let shared_for_permissions = shared.clone();
    let shared_for_foreground = shared.clone();
    let policy = settings.permission_policy;
    let mut translator = TimelineTranslator::default();
    // The structured conclusion lives in the assistant's prose, so the whole
    // message stream is accumulated per turn and parsed when the turn ends.
    // Tool output and thinking are deliberately excluded: a `mns-decision`
    // block echoed inside a tool result must not be mistaken for the answer.
    let message_buffer: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let buffer_for_updates = message_buffer.clone();

    let connection_future = Client
        .v1()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                if let SessionUpdate::AgentMessageChunk(chunk) = &notification.update {
                    if let ContentBlock::Text(text) = &chunk.content {
                        if let Ok(mut guard) = buffer_for_updates.lock() {
                            guard.push_str(&text.text);
                        }
                    }
                }
                for event in translator.translate(notification.update) {
                    let _ = events_for_updates.send(event);
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _cx| {
                let outcome = permission_outcome(
                    &request,
                    policy,
                    &shared_for_permissions,
                    &events_for_permissions,
                )
                .await;
                responder.respond(RequestPermissionResponse::new(outcome))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, move |connection: ConnectionTo<Agent>| async move {
            {
                let mut guard = shared_for_foreground
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                guard.connection = Some(connection.clone());
            }

            let _init = connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            emit_entry(&events_for_foreground, EntryKind::System, "ACP 握手完成");

            let created = connection
                .send_request(NewSessionRequest::new(cwd.clone()))
                .block_task()
                .await?;
            let session_id = created.session_id;
            {
                let mut guard = shared_for_foreground
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                guard.session_id = Some(session_id.clone());
            }
            let mut queue: Vec<String> = vec![prompt_text];
            // The closure owns the receiver; binding it `mut` here is what
            // allows the loop to wait for a follow-up after a turn ends.
            let mut ask_rx = ask_rx;

            loop {
                let text = match queue.pop() {
                    Some(text) => text,
                    None => match ask_rx.recv().await {
                        Some(text) => text,
                        None => break,
                    },
                };

                if let Ok(mut guard) = message_buffer.lock() {
                    guard.clear();
                }
                emit_state(&events_for_foreground, RunState::Running, "Agent 正在分析…");

                let response = connection
                    .send_request(PromptRequest::new(
                        session_id.clone(),
                        vec![ContentBlock::Text(TextContent::new(text))],
                    ))
                    .block_task()
                    .await?;

                let reason = format!("{:?}", response.stop_reason);

                // One turn can contain several blocks; the last valid one wins,
                // which lets the agent revise its own conclusion while answering.
                if let Some(report) = message_buffer
                    .lock()
                    .ok()
                    .and_then(|guard| extract_decision(&guard))
                {
                    let _ = events_for_foreground.send(AgentEvent::Decision(report));
                }

                let cancelled = matches!(
                    response.stop_reason,
                    agent_client_protocol::v1::schema::StopReason::Cancelled
                );
                emit_state(
                    &events_for_foreground,
                    if cancelled {
                        RunState::Cancelled
                    } else {
                        RunState::Finished
                    },
                    if cancelled {
                        "本轮已取消"
                    } else {
                        "本轮结束，可以继续追问"
                    },
                );
                let _ = events_for_foreground.send(AgentEvent::TurnFinished {
                    stop_reason: reason,
                });
                if cancelled {
                    break;
                }
            }

            let _ = events_for_foreground.send(AgentEvent::Finished {
                stop_reason: "会话结束".to_owned(),
            });
            Ok::<(), agent_client_protocol::Error>(())
        });

    let mut connection_future = Box::pin(connection_future);

    let first = tokio::select! {
        result = &mut connection_future => Some(result),
        _ = &mut stop_rx => None,
    };

    let final_result = match first {
        Some(result) => Some(result),
        None => {
            // `session/cancel` was already sent. Give the agent a bounded
            // window to settle, then drop the connection (kills the process).
            emit_state(
                &events,
                RunState::Stopping,
                "已发送取消请求，等待 Agent 收尾…",
            );
            let grace = tokio::time::sleep(Duration::from_secs(5));
            tokio::pin!(grace);
            let settled = tokio::select! {
                result = &mut connection_future => Some(result),
                _ = &mut grace => None,
            };
            if settled.is_none() {
                emit_state(&events, RunState::Cancelled, "Agent 未响应取消，已结束进程");
            }
            settled
        }
    };

    match final_result {
        Some(Ok(())) => {}
        Some(Err(error)) => {
            let _ = events.send(AgentEvent::Failed {
                message: format!("{error:?}"),
            });
        }
        None => {}
    }
}

/// Does this command reach into the mns storage layer instead of the CLI?
///
/// Deliberately narrow: it needs both a SQLite client (or the database file
/// name) and a `.mns` path. A broad "no sqlite3" rule would break unrelated
/// work in the agent's own workspace, and a broad "no .mns" rule would break
/// nothing useful but look arbitrary. This pair is exactly the observed
/// violation: `sqlite3 file:/Users/…/.mns/mns.db?mode=ro "SELECT …"`.
fn touches_cli_storage(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    let sqlite = lower.contains("sqlite3") || lower.contains("mns.db");
    let inside_dot_mns = lower.contains(".mns/") || lower.contains(".mns\\");
    sqlite && inside_dot_mns
}

/// Decide an ACP permission request. Auto-approved by default; otherwise
/// suspend on a oneshot until the UI answers (or the turn is cancelled).
async fn permission_outcome(
    request: &RequestPermissionRequest,
    policy: PermissionPolicy,
    shared: &Shared,
    events: &EventTx,
) -> RequestPermissionOutcome {
    let kind = request.tool_call.fields.kind.unwrap_or(ToolKind::Other);
    let title = request
        .tool_call
        .fields
        .title
        .clone()
        .or_else(|| request.tool_call.fields.name.clone())
        .unwrap_or_else(|| "工具调用".to_owned());

    // The prompt tells the agent to use the CLI; this is what enforces it when
    // the agent ignores that. Rejecting here is visible in the timeline, so the
    // user sees why a step was refused rather than watching it fail silently.
    if touches_cli_storage(&title) {
        emit_entry(
            events,
            EntryKind::Error,
            format!("已拒绝直接访问 mns 存储：{title}。账户数据只能通过 mns CLI 读取。"),
        );
        let deny = request
            .options
            .iter()
            .find(|option| {
                matches!(
                    option.kind,
                    PermissionOptionKind::RejectOnce | PermissionOptionKind::RejectAlways
                )
            })
            .map(|option| option.option_id.0.to_string());
        return match deny {
            Some(id) => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id)),
            None => RequestPermissionOutcome::Cancelled,
        };
    }

    let allow = request
        .options
        .iter()
        .find(|option| option.kind == PermissionOptionKind::AllowOnce)
        .or_else(|| {
            request
                .options
                .iter()
                .find(|option| option.kind == PermissionOptionKind::AllowAlways)
        });
    let Some(allow) = allow else {
        emit_entry(
            events,
            EntryKind::Error,
            format!("{title}：Agent 未提供允许选项，已取消"),
        );
        return RequestPermissionOutcome::Cancelled;
    };
    let allow_id = allow.option_id.0.to_string();
    let deny_id = request
        .options
        .iter()
        .find(|option| {
            matches!(
                option.kind,
                PermissionOptionKind::RejectOnce | PermissionOptionKind::RejectAlways
            )
        })
        .map(|option| option.option_id.0.to_string())
        .unwrap_or_default();

    let needs_confirmation = match policy {
        PermissionPolicy::Autonomous => false,
        PermissionPolicy::GuardWrites => matches!(
            kind,
            ToolKind::Edit
                | ToolKind::Delete
                | ToolKind::Move
                | ToolKind::Execute
                | ToolKind::Other
        ),
        PermissionPolicy::ConfirmAll => true,
    };

    if !needs_confirmation {
        emit_entry(
            events,
            EntryKind::Permission,
            format!("自动批准 · {title}（{kind:?}，仅本次）"),
        );
        return RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(allow_id));
    }

    let (id, receiver) = {
        let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
        if guard.stopped {
            return RequestPermissionOutcome::Cancelled;
        }
        let id = guard.next_id + 1;
        guard.next_id = id;
        let (tx, receiver) = oneshot::channel();
        guard.pending.insert(id, PendingPermission { tx, deny_id });
        (id, receiver)
    };

    let options = request
        .options
        .iter()
        .map(|option| PermissionChoice {
            id: option.option_id.0.to_string(),
            label: option.name.clone(),
            allow: matches!(
                option.kind,
                PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways
            ),
        })
        .collect();

    emit_state(
        events,
        RunState::WaitingPermission,
        format!("等待授权：{title}"),
    );
    let _ = events.send(AgentEvent::Permission(PermissionPrompt {
        id,
        title: title.clone(),
        detail: format!("工具类型：{kind:?}；会话 {}", request.session_id.0),
        options,
    }));

    let decision = match tokio::time::timeout(Duration::from_secs(300), receiver).await {
        Ok(Ok(decision)) => decision,
        _ => Decision::Cancelled,
    };
    let _ = events.send(AgentEvent::PermissionResolved { id });
    emit_state(events, RunState::Running, "继续执行");

    match decision {
        Decision::Allow(id) | Decision::Reject(id) if !id.is_empty() => {
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id))
        }
        _ => RequestPermissionOutcome::Cancelled,
    }
}

fn text_of(block: ContentBlock) -> Option<String> {
    match block {
        ContentBlock::Text(text) => Some(text.text),
        _ => None,
    }
}

fn kind_label(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "读取",
        ToolKind::Edit => "编辑",
        ToolKind::Delete => "删除",
        ToolKind::Move => "移动",
        ToolKind::Search => "检索",
        ToolKind::Execute => "执行命令",
        ToolKind::Think => "内部思考",
        ToolKind::Fetch => "拉取数据",
        ToolKind::SwitchMode => "切换模式",
        ToolKind::Other => "工具",
        // `ToolKind` is non-exhaustive: a newer agent may add kinds.
        _ => "工具",
    }
}

fn state_label(status: ToolCallStatus) -> &'static str {
    match status {
        ToolCallStatus::Pending => "待执行",
        ToolCallStatus::InProgress => "执行中",
        ToolCallStatus::Completed => "完成",
        ToolCallStatus::Failed => "失败",
        _ => "未知状态",
    }
}

fn plan_label(status: PlanEntryStatus) -> &'static str {
    match status {
        PlanEntryStatus::Pending => "待开始",
        PlanEntryStatus::InProgress => "进行中",
        PlanEntryStatus::Completed => "已完成",
        _ => "未知状态",
    }
}

/// Flatten whitespace and bound the length so one tool output cannot push the
/// rest of the timeline off screen.
fn compact(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= limit {
        flat
    } else {
        let head: String = flat.chars().take(limit).collect();
        format!("{head}…")
    }
}

fn tool_output(content: &[ToolCallContent]) -> Option<String> {
    content.iter().find_map(|item| match item {
        ToolCallContent::Content(inner) => text_of(inner.content.clone())
            .map(|text| compact(&text, 200))
            .filter(|text| !text.is_empty()),
        _ => None,
    })
}

#[derive(Default)]
struct ToolState {
    label: String,
    kind: ToolKind,
    /// A row already describes this call, so status chatter can be dropped.
    announced: bool,
    /// The terminal row has been emitted; later updates are duplicates.
    reported: bool,
}

/// Turns ACP session updates into a small number of readable rows.
///
/// ACP streams token chunks and emits several `tool_call_update` messages per
/// tool call. Rendering each message directly produces a wall of duplicated
/// rows, so this type keeps just enough state to coalesce streams and collapse
/// tool lifecycles into "announced" + "finished".
#[derive(Default)]
struct TimelineTranslator {
    tools: HashMap<String, ToolState>,
    /// Kind of the row currently receiving chunks. A whitespace-only chunk is
    /// only meaningful when it continues an open sentence.
    open_stream: Option<EntryKind>,
}

impl TimelineTranslator {
    fn translate(&mut self, update: SessionUpdate) -> Vec<AgentEvent> {
        match update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                self.stream(EntryKind::Message, chunk.content)
            }
            SessionUpdate::AgentThoughtChunk(chunk) => {
                self.stream(EntryKind::Thought, chunk.content)
            }
            other => {
                self.open_stream = None;
                self.translate_row(other)
            }
        }
    }

    fn stream(&mut self, kind: EntryKind, block: ContentBlock) -> Vec<AgentEvent> {
        let Some(text) = text_of(block).filter(|text| !text.is_empty()) else {
            return Vec::new();
        };
        // A lone space arrives as its own chunk in streamed text. It separates
        // words, but on its own would render as an empty row.
        if text.trim().is_empty() && self.open_stream != Some(kind) {
            return Vec::new();
        }
        self.open_stream = Some(kind);
        vec![AgentEvent::Delta { kind, text }]
    }

    fn translate_row(&mut self, update: SessionUpdate) -> Vec<AgentEvent> {
        match update {
            SessionUpdate::ToolCall(call) => {
                let id = call.tool_call_id.0.to_string();
                let label = compact(&call.title, 80);
                // Most agents announce a call with the tool name only and send
                // the real command in a later update, sometimes already marked
                // in-progress. Announce only when the title says something the
                // tool name does not.
                let generic = label.is_empty()
                    || call
                        .name
                        .as_deref()
                        .is_some_and(|name| name.eq_ignore_ascii_case(call.title.trim()));
                let announce = !generic;

                self.tools.insert(
                    id,
                    ToolState {
                        label: if label.is_empty() {
                            "工具调用".to_owned()
                        } else {
                            label.clone()
                        },
                        kind: call.kind,
                        announced: announce,
                        reported: false,
                    },
                );

                if announce {
                    vec![AgentEvent::Entry(TimelineEntry::new(
                        EntryKind::ToolCall,
                        format!("{} · {}", kind_label(call.kind), label),
                    ))]
                } else {
                    Vec::new()
                }
            }
            SessionUpdate::ToolCallUpdate(update) => self.tool_update(update),
            SessionUpdate::Plan(plan) => plan
                .entries
                .into_iter()
                .map(|entry| {
                    AgentEvent::Entry(TimelineEntry::new(
                        EntryKind::Plan,
                        format!("{} · {}", plan_label(entry.status), entry.content),
                    ))
                })
                .collect(),
            // User echoes, mode/config/usage notices and unstable extensions do
            // not need a dedicated row in the P0 timeline.
            _ => Vec::new(),
        }
    }

    fn tool_update(&mut self, update: ToolCallUpdate) -> Vec<AgentEvent> {
        let id = update.tool_call_id.0.to_string();
        let fields = update.fields;

        let (label, kind, announced, reported, title_changed) = {
            let state = self.tools.entry(id.clone()).or_default();
            let mut title_changed = false;
            if let Some(title) = fields
                .title
                .as_deref()
                .map(str::trim)
                .filter(|title| !title.is_empty())
            {
                let next = compact(title, 80);
                title_changed = next != state.label;
                state.label = next;
            } else if let Some(name) = fields
                .name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
            {
                let next = compact(name, 80);
                title_changed = next != state.label;
                state.label = next;
            }
            if let Some(kind) = fields.kind {
                state.kind = kind;
            }
            (
                state.label.clone(),
                state.kind,
                state.announced,
                state.reported,
                title_changed,
            )
        };

        // A label-only update carries no progress: fold it into the state.
        let Some(status) = fields.status else {
            return Vec::new();
        };

        let label = if label.trim().is_empty() {
            "工具调用".to_owned()
        } else {
            label
        };

        if matches!(status, ToolCallStatus::Pending | ToolCallStatus::InProgress) {
            if announced || (!title_changed && status == ToolCallStatus::Pending) {
                return Vec::new();
            }
            if let Some(state) = self.tools.get_mut(&id) {
                state.announced = true;
            }
            return vec![AgentEvent::Entry(TimelineEntry::new(
                EntryKind::ToolCall,
                format!("{} · {}", kind_label(kind), label),
            ))];
        }

        if reported {
            return Vec::new();
        }
        if let Some(state) = self.tools.get_mut(&id) {
            state.reported = true;
        }

        let output = fields.content.as_deref().and_then(tool_output);
        let text = match (status, output) {
            (ToolCallStatus::Completed, Some(output)) => {
                format!("{label} · 完成 — {output}")
            }
            (ToolCallStatus::Completed, None) => format!("{label} · 完成"),
            (_, Some(output)) => format!("{label} · 失败 — {output}"),
            (_, None) => format!("{label} · 失败（{}）", state_label(status)),
        };

        // Keep the tombstone: agents re-send the terminal status, and a fresh
        // state would report the same tool twice. One small struct per call is
        // cheaper than a duplicate row in the user's timeline.
        vec![AgentEvent::Entry(TimelineEntry::new(
            EntryKind::ToolResult,
            text,
        ))]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::v1::schema::{
        ContentChunk, Plan, PlanEntry, PlanEntryPriority, PlanEntryStatus, ToolCall, ToolCallId,
        ToolCallUpdateFields,
    };

    fn tool_update(id: &str, title: Option<&str>, status: Option<ToolCallStatus>) -> SessionUpdate {
        let mut fields = ToolCallUpdateFields::new();
        if let Some(title) = title {
            fields = fields.title(title.to_owned());
        }
        if let Some(status) = status {
            fields = fields.status(status);
        }
        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(id.to_owned(), fields))
    }

    #[test]
    fn launch_config_appends_acp_exactly_once() {
        let settings = AgentSettings {
            agent_cli: "/opt/bin/opencode".to_owned(),
            ..AgentSettings::default()
        };
        let config = launch_config(&settings, None, Profile::Task);
        assert_eq!(config.command(), std::path::Path::new("/opt/bin/opencode"));
        assert_eq!(config.arguments(), ["acp"]);
    }

    #[test]
    fn launch_config_respects_extra_args_and_existing_acp() {
        let settings = AgentSettings {
            agent_cli: "opencode".to_owned(),
            extra_args: vec![
                "--log-level".to_owned(),
                "warn".to_owned(),
                "acp".to_owned(),
            ],
            ..AgentSettings::default()
        };
        let config = launch_config(&settings, None, Profile::Task);
        assert_eq!(
            config.arguments(),
            ["--log-level", "warn", "acp"],
            "'acp' must not be appended twice"
        );
    }

    #[test]
    fn launch_config_tolerates_a_command_string() {
        let settings = AgentSettings {
            agent_cli: "npx opencode".to_owned(),
            ..AgentSettings::default()
        };
        let config = launch_config(&settings, None, Profile::Task);
        assert_eq!(config.command(), std::path::Path::new("npx"));
        assert_eq!(config.arguments(), ["opencode", "acp"]);
    }

    #[test]
    fn launch_config_keeps_a_path_with_spaces_intact() {
        let settings = AgentSettings {
            agent_cli: r"D:\Program Files (x86)\QuickBin\opencode.exe".to_owned(),
            ..AgentSettings::default()
        };
        let config = launch_config(&settings, None, Profile::Task);
        assert_eq!(
            config.command(),
            std::path::Path::new(r"D:\Program Files (x86)\QuickBin\opencode.exe"),
            "a Windows path with spaces is one executable, not a command string"
        );
        assert_eq!(config.arguments(), ["acp"]);

        let mac = AgentSettings {
            agent_cli: "/Applications/My Agents/opencode".to_owned(),
            ..AgentSettings::default()
        };
        let config = launch_config(&mac, None, Profile::Task);
        assert_eq!(
            config.command(),
            std::path::Path::new("/Applications/My Agents/opencode")
        );
        assert_eq!(config.arguments(), ["acp"]);
    }

    #[test]
    fn launch_config_injects_the_cli_path_and_prepends_path() {
        let settings = AgentSettings {
            agent_cli: "opencode".to_owned(),
            ..AgentSettings::default()
        };
        let cli = Path::new("/Applications/MNS.app/Contents/MacOS/mns");
        let config = launch_config(&settings, Some(cli), Profile::Task);

        assert_eq!(
            config.environment().get("MNS_BIN").map(String::as_str),
            Some("/Applications/MNS.app/Contents/MacOS/mns")
        );
        let path = config
            .environment()
            .get("PATH")
            .expect("the agent must receive PATH");
        let separator = if cfg!(windows) { ";" } else { ":" };
        assert!(
            path.starts_with(&format!("/Applications/MNS.app/Contents/MacOS{separator}")),
            "the bundle directory must come first so a bare `mns` resolves to it, got {path}"
        );
    }

    #[test]
    fn launch_config_forwards_the_typed_model_as_inline_config() {
        let settings = AgentSettings {
            agent_cli: "opencode".to_owned(),
            model: "  opencode/deepseek-v4.1-flash-free  ".to_owned(),
            ..AgentSettings::default()
        };
        let config = launch_config(&settings, None, Profile::Task);
        let raw = config
            .environment()
            .get("OPENCODE_CONFIG_CONTENT")
            .expect("a typed model must reach the agent");
        let value: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(
            value["model"], "opencode/deepseek-v4.1-flash-free",
            "whitespace is trimmed; the id itself is passed through untouched"
        );
    }

    #[test]
    fn launch_config_keeps_the_deny_rules_even_without_a_model() {
        let settings = AgentSettings {
            agent_cli: "opencode".to_owned(),
            ..AgentSettings::default()
        };
        let config = launch_config(&settings, Some(test_cli()), Profile::Task);
        let raw = config
            .environment()
            .get("OPENCODE_CONFIG_CONTENT")
            .expect("the deny rules must reach the agent even when no model is set");
        let value: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert!(
            value["permissions"]
                .as_array()
                .is_some_and(|rules| !rules.is_empty()),
            "task sessions still carry their command deny list"
        );
        let fresh: serde_json::Value = serde_json::from_str(&merge_agent_config(
            None,
            "",
            &permission_rules(Profile::Task, Some(test_cli())),
        ))
        .unwrap();
        assert!(
            fresh.get("model").is_none(),
            "an empty model must not be invented"
        );
    }

    #[test]
    fn timeline_note_talks_about_capability_not_prohibition() {
        assert_eq!(
            PERMISSION_NOTE,
            "权限：已启用 mns 账本写入保护（其他工具按 Agent 默认）"
        );
        for stale in ["只读", "全部禁用", "拒绝工具", "无工具"] {
            assert!(
                !PERMISSION_NOTE.contains(stale),
                "the timeline must not describe the old fail-closed policy"
            );
        }
    }

    #[test]
    fn both_profiles_only_carry_the_ledger_guard() {
        for profile in [Profile::Task, Profile::Butler] {
            let rules = permission_rules(profile, Some(test_cli()));
            assert!(!rules.is_empty(), "{profile:?} must still guard the ledger");
            assert!(
                rules
                    .iter()
                    .all(|rule| rule.action == "shell" && rule.effect == "deny"),
                "{profile:?} may only add shell deny rules"
            );
            assert!(
                rules.iter().all(|rule| !rule.resource.starts_with('*')),
                "a leading wildcard would turn the guard into a prefix hole"
            );
            assert!(
                !rules.iter().any(|rule| rule.resource == "*"),
                "there must be no blanket shell deny"
            );
            // `config` is out of scope: it guards strategy knobs, not the
            // ledger, and a `config * *` rule was verified to swallow the
            // one-argument read `config thresholds.fear`.
            assert!(
                rules.iter().all(|rule| !rule.resource.contains(" config")),
                "no rule may mention `mns config`: its read shapes must stay usable"
            );
            for action in [
                "edit",
                "write",
                "webfetch",
                "websearch",
                "subagent",
                "execute",
                "skill",
            ] {
                assert!(
                    !rules.iter().any(|rule| rule.action == action),
                    "{profile:?} must keep opencode's default for {action}"
                );
            }
        }
        assert_eq!(
            permission_rules(Profile::Task, Some(test_cli())),
            permission_rules(Profile::Butler, Some(test_cli())),
            "both profiles share the same best-effort guard"
        );
    }

    #[test]
    fn write_commands_are_denied_in_every_spelling() {
        for command in [
            "mns init",
            "mns init --force",
            "mns add 000001",
            "mns buy 007605 100 1.0",
            "mns sell 007605 1",
            "mns price 007605 1.0",
            "mns remove 007605",
            "mns sentiment",
            "mns report",
            "mns today",
            "mns update-prices",
            "mns cash set 0",
            "mns cash deposit 1000",
            "mns cash withdraw 1000",
            "mns backtest update-data",
            "/Applications/MNS.app/Contents/MacOS/mns buy 007605 100 1.0",
            "\"/Applications/MNS.app/Contents/MacOS/mns\" cash set 0",
            "/Applications/MNS.app/Contents/MacOS/mns backtest update-data",
            "/Applications/MNS.app/Contents/MacOS/mns today",
            "\"/Applications/MNS.app/Contents/MacOS/mns\" report",
            "mns.exe buy 007605 100 1.0",
            "\"mns.exe\" init --force",
            "mns.exe today",
            // A denied subcommand anywhere in a compound command is enough.
            "mns portfolio && mns buy 1",
            "mns cash; mns cash set 0",
        ] {
            assert!(
                denied(Profile::Task, command),
                "{command} must be denied by the ledger guard"
            );
            assert!(
                denied(Profile::Butler, command),
                "the butler uses the same guard: {command}"
            );
        }
    }

    #[test]
    fn reads_and_default_tools_stay_available() {
        for command in [
            "mns portfolio",
            "mns cash",
            "mns performance",
            "mns guard",
            "mns market",
            "mns history 5",
            "mns history --limit 10",
            "mns config",
            "mns config params",
            "mns config params --json",
            "mns config thresholds.fear",
            "/Applications/MNS.app/Contents/MacOS/mns config params --json",
            "mns plan --monthly 3000",
            "mns --help",
            "mns --version",
            "/Applications/MNS.app/Contents/MacOS/mns portfolio",
            "\"/Applications/MNS.app/Contents/MacOS/mns\" plan --monthly 3000",
            "/Applications/MNS.app/Contents/MacOS/mns config params",
            "mns.exe portfolio",
            // Two reads may be chained; every subcommand is checked.
            "mns portfolio && mns cash",
            "mns plan --monthly 3000; mns cash",
            // Everything else is opencode's default, not MNS's business.
            "rm -rf /tmp/x",
            "curl https://example.com",
            "osascript -e 'display notification 1'",
            "sleep 5",
        ] {
            assert!(
                !denied(Profile::Task, command),
                "{command} is not a ledger write and must stay usable"
            );
        }
    }

    fn test_cli() -> &'static Path {
        Path::new("/Applications/MNS.app/Contents/MacOS/mns")
    }

    #[test]
    fn without_a_cli_the_guard_has_nothing_to_name() {
        assert!(permission_rules(Profile::Task, None).is_empty());
        assert!(permission_rules(Profile::Butler, None).is_empty());
    }

    fn denied(profile: Profile, command: &str) -> bool {
        let rules = permission_rules(profile, Some(test_cli()));
        shell_segments(command)
            .iter()
            .any(|segment| resolve_one(&rules, "shell", segment.trim()) == Some("deny"))
    }

    /// Minimal V2 matcher used by the tests: `*` spans `/`, `?` is one char,
    /// and a pattern ending in ` *` also matches the bare command.
    fn rule_matches(pattern: &str, command: &str) -> bool {
        // Faithful to a plain glob: no implicit optional argument, which is why
        // the guard writes both the bare and the ` *` spelling.
        wildcard(pattern, command)
    }

    fn wildcard(pattern: &str, text: &str) -> bool {
        let pattern: Vec<char> = pattern.chars().collect();
        let text: Vec<char> = text.chars().collect();
        let (mut p, mut t) = (0usize, 0usize);
        let (mut star, mut mark) = (None, 0usize);
        while t < text.len() {
            if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
                p += 1;
                t += 1;
            } else if p < pattern.len() && pattern[p] == '*' {
                star = Some(p);
                p += 1;
                mark = t;
            } else if let Some(s) = star {
                p = s + 1;
                mark += 1;
                t = mark;
            } else {
                return false;
            }
        }
        while p < pattern.len() && pattern[p] == '*' {
            p += 1;
        }
        p == pattern.len()
    }

    fn resolve_one(rules: &[Rule], action: &str, resource: &str) -> Option<&'static str> {
        let mut effect = None;
        for rule in rules {
            if rule.action == action && rule_matches(&rule.resource, resource) {
                effect = Some(rule.effect);
            }
        }
        effect
    }

    /// Split a shell command the way opencode's scanner does for the cases in
    /// these tests: outside quotes, on `;`, `&&`, `||`, `|` and newlines.
    /// `$(…)` and backticks stay in one segment.
    fn shell_segments(command: &str) -> Vec<String> {
        let chars: Vec<char> = command.chars().collect();
        let mut segments = Vec::new();
        let mut current = String::new();
        let mut quote: Option<char> = None;
        let mut depth = 0usize;
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if let Some(q) = quote {
                current.push(c);
                if c == q {
                    quote = None;
                }
                i += 1;
                continue;
            }
            match c {
                '\'' | '"' => {
                    quote = Some(c);
                    current.push(c);
                }
                '`' => {
                    current.push(c);
                    quote = Some('`');
                }
                '(' => {
                    depth += 1;
                    current.push(c);
                }
                ')' => {
                    depth = depth.saturating_sub(1);
                    current.push(c);
                }
                _ if depth > 0 => current.push(c),
                ';' | '\n' => {
                    if !current.trim().is_empty() {
                        segments.push(std::mem::take(&mut current));
                    }
                }
                '&' | '|' => {
                    if !current.trim().is_empty() {
                        segments.push(std::mem::take(&mut current));
                    }
                    // `&&` / `||` are one operator, not an empty segment.
                    if i + 1 < chars.len() && chars[i + 1] == c {
                        i += 1;
                    }
                }
                _ => current.push(c),
            }
            i += 1;
        }
        if !current.trim().is_empty() {
            segments.push(current);
        }
        segments
    }

    #[test]
    fn merge_agent_config_preserves_user_keys_and_appends_rules() {
        let existing = r#"{"theme":"dark","model":"old/model","permissions":[{"action":"read","resource":"*.env","effect":"deny"}]}"#;
        let merged = merge_agent_config(
            Some(existing),
            "new/model",
            &permission_rules(Profile::Task, Some(test_cli())),
        );
        let value: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(value["theme"], "dark", "unrelated keys survive");
        assert_eq!(value["model"], "new/model", "the settings field wins");

        let permissions = value["permissions"].as_array().unwrap();
        assert_eq!(
            permissions[0]["resource"], "*.env",
            "the user's earlier rule stays first"
        );
        assert!(
            permissions[1..]
                .iter()
                .all(|rule| rule["action"] == "shell"),
            "our guard is appended after the user's rule and only touches the shell"
        );
        assert_eq!(
            permissions[1]["resource"], "mns init",
            "the first guard entry names the bare write command"
        );
        let last = permissions.last().unwrap();
        assert_eq!(last["effect"], "deny");
        assert_eq!(last["resource"], "\"mns.exe\" backtest update-data *");
    }

    #[test]
    fn merge_agent_config_survives_broken_inline_json() {
        let merged = merge_agent_config(
            Some("not json"),
            "",
            &permission_rules(Profile::Butler, Some(test_cli())),
        );
        let value: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert!(value.get("model").is_none());
        assert_eq!(
            value["permissions"].as_array().unwrap().len(),
            permission_rules(Profile::Butler, Some(test_cli())).len()
        );
    }

    #[test]
    fn agent_env_keeps_the_original_path_after_the_bundle() {
        let vars = agent_env(Path::new("/tmp/bin/mns"), "/usr/bin:/bin");
        let separator = if cfg!(windows) { ";" } else { ":" };
        assert_eq!(vars[0].0, "MNS_BIN");
        assert_eq!(vars[0].1, "/tmp/bin/mns");
        assert_eq!(vars[1].0, "PATH");
        assert_eq!(vars[1].1, format!("/tmp/bin{separator}/usr/bin:/bin"));

        let empty = agent_env(Path::new("/tmp/bin/mns"), "");
        assert_eq!(
            empty[1].1, "/tmp/bin",
            "an empty PATH must not gain a separator"
        );
    }

    #[test]
    fn goal_prompt_carries_the_autonomy_contract() {
        for source in [GoalSource::Manual, GoalSource::DailyReview] {
            let prompt = compose_goal(
                "检查数据",
                source,
                Path::new("/work"),
                Some(Path::new("/Applications/MNS.app/Contents/MacOS/mns")),
                true,
            );
            assert!(prompt.starts_with("检查数据"));
            assert!(prompt.contains("这是一个自主执行任务"));
            assert!(prompt.contains("不要逐步向用户提问"));
            assert!(prompt.contains("连续调用所需工具"));
            assert!(prompt.contains("MNS 不提供 URL 或 API Key"));
            assert!(prompt.contains("/work"));
            assert!(prompt.contains("不要执行下单、交易"));
            assert!(prompt.contains("工作目录可写"));
            assert!(!prompt.contains("工作目录只读"));
            assert!(prompt.contains("mns-decision"));
            assert!(prompt.contains("已完成事项"));
        }
    }

    #[test]
    fn conversation_prompt_drops_the_task_contract() {
        let prompt = compile_conversation_goal();
        assert!(
            prompt.contains("这是一次对话，不是任务"),
            "the butler window must not read like an execution order"
        );
        assert!(prompt.contains("先给结论"), "answer first, then reasons");
        assert!(
            !prompt.contains("这是一个自主执行任务"),
            "the autonomy contract is exactly what made butler replies report-like"
        );
        assert!(!prompt.contains("不要逐步向用户提问"));
        assert!(!prompt.contains("连续调用所需工具"));
        assert!(
            !prompt.contains("完成后用中文输出"),
            "no completion report in a conversation"
        );
        assert!(
            prompt.contains("不要输出 mns-decision"),
            "the machine channel is only for task runs"
        );
        assert!(
            prompt.contains("不要执行下单、交易"),
            "safety rules still apply"
        );
        assert!(
            prompt.contains("账户事实已经注入") && prompt.contains("查不到就直说没有"),
            "the butler answers from injected facts, with the CLI as a fallback"
        );
        assert!(
            prompt.contains("mns-butler task add"),
            "a reminder must become a real due task"
        );
        assert!(
            prompt.contains("可以执行命令、联网"),
            "capability first: the butler is allowed to use tools"
        );
        assert!(
            !prompt.contains("不要调用工具") && !prompt.contains("不要执行命令"),
            "the old blanket tool ban must be gone"
        );
        assert!(
            !prompt.contains("仅在该目录内读写"),
            "the old read-write sentence contradicts the deny policy"
        );
    }

    #[test]
    fn conversation_prompt_has_no_action_fields() {
        let prompt = compile_conversation_goal();
        assert!(prompt.contains("action、effective、monthly_amount"));
        assert!(
            !prompt.contains("action: "),
            "the contract may name the fields but must not teach the syntax"
        );
    }

    fn compile_conversation_goal() -> String {
        compose_goal(
            "明天 A 股怎么看",
            GoalSource::Conversation,
            Path::new("/work"),
            Some(Path::new("/Applications/MNS.app/Contents/MacOS/mns")),
            true,
        )
    }

    #[test]
    fn goal_prompt_names_the_cli_and_forbids_the_storage_layer() {
        let prompt = compose_goal(
            "生成今日报告",
            GoalSource::Manual,
            Path::new("/work"),
            Some(Path::new("/Applications/MNS.app/Contents/MacOS/mns")),
            true,
        );
        assert!(
            prompt.contains("/Applications/MNS.app/Contents/MacOS/mns"),
            "the agent must be told the absolute path instead of searching for it"
        );
        assert!(
            prompt.contains("不要用 find"),
            "search commands are ruled out"
        );
        assert!(
            prompt.contains("sqlite3") && prompt.contains("~/.mns/mns.db"),
            "reading the database directly must be named as forbidden"
        );
        assert!(prompt.contains("禁止阅读 mns 源码"));
    }

    #[test]
    fn goal_prompt_without_a_cli_forbids_guessing() {
        let prompt = compose_goal(
            "生成今日报告",
            GoalSource::Manual,
            Path::new("/work"),
            None,
            true,
        );
        assert!(prompt.contains("未检测到随应用安装的 mns CLI"));
        assert!(prompt.contains("不要自行搜索文件系统"));
        assert!(
            !prompt.contains("sqlite3"),
            "no path is offered, so there is no storage rule to state"
        );
    }

    #[test]
    fn managed_prompt_points_at_agents_md_instead_of_listing_commands() {
        let prompt = compose_goal(
            "生成今日报告",
            GoalSource::Manual,
            Path::new("/work"),
            Some(Path::new("/Applications/MNS.app/Contents/MacOS/mns")),
            true,
        );
        assert!(prompt.contains("AGENTS.md"));
        assert!(
            !prompt.contains("--help"),
            "the command catalogue lives in the skill, not the prompt"
        );
    }

    #[test]
    fn custom_working_dir_keeps_the_full_command_examples() {
        let prompt = compose_goal(
            "生成今日报告",
            GoalSource::Manual,
            Path::new("/work"),
            Some(Path::new("/Applications/MNS.app/Contents/MacOS/mns")),
            false,
        );
        assert!(
            prompt.contains("--help"),
            "no AGENTS.md is written for this directory"
        );
        assert!(
            prompt.contains("会写入"),
            "the fallback must not present `mns today` as read-only"
        );
        assert!(!prompt.contains("AGENTS.md"));
    }

    #[test]
    fn plan_entries_become_readable_rows() {
        let plan = Plan::new(vec![PlanEntry::new(
            "生成报告",
            PlanEntryPriority::High,
            PlanEntryStatus::InProgress,
        )]);
        let rows = TimelineTranslator::default().translate(SessionUpdate::Plan(plan));
        assert_eq!(rows.len(), 1);
        match &rows[0] {
            AgentEvent::Entry(entry) => {
                assert_eq!(entry.kind, EntryKind::Plan);
                assert_eq!(entry.text, "进行中 · 生成报告");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn streamed_chunks_become_deltas_not_rows() {
        let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new("已完成")));
        let rows = TimelineTranslator::default().translate(SessionUpdate::AgentMessageChunk(chunk));
        assert_eq!(rows.len(), 1);
        match &rows[0] {
            AgentEvent::Delta { kind, text } => {
                assert_eq!(*kind, EntryKind::Message);
                assert_eq!(text, "已完成");
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let blank = ContentChunk::new(ContentBlock::Text(TextContent::new("   ")));
        assert!(
            TimelineTranslator::default()
                .translate(SessionUpdate::AgentThoughtChunk(blank))
                .is_empty(),
            "a whitespace-only chunk must not open a row"
        );

        let mut translator = TimelineTranslator::default();
        translator.translate(SessionUpdate::AgentThoughtChunk(ContentChunk::new(
            ContentBlock::Text(TextContent::new("Planning")),
        )));
        let separator = translator.translate(SessionUpdate::AgentThoughtChunk(ContentChunk::new(
            ContentBlock::Text(TextContent::new(" ")),
        )));
        assert_eq!(
            separator.len(),
            1,
            "a space between streamed words must be preserved once a row is open"
        );
    }

    #[test]
    fn generic_tool_name_waits_for_the_real_command() {
        let mut translator = TimelineTranslator::default();
        let call = ToolCall::new(ToolCallId::new("call-1"), "shell")
            .kind(ToolKind::Execute)
            .name("shell")
            .status(ToolCallStatus::InProgress);
        assert!(
            translator
                .translate(SessionUpdate::ToolCall(call))
                .is_empty(),
            "a bare tool name must not produce a row, even when already running"
        );

        let announced = translator.translate(tool_update(
            "call-1",
            Some("ls -1 | head -n 5"),
            Some(ToolCallStatus::InProgress),
        ));
        assert_eq!(announced.len(), 1);
        match &announced[0] {
            AgentEvent::Entry(entry) => {
                assert_eq!(entry.kind, EntryKind::ToolCall);
                assert_eq!(entry.text, "执行命令 · ls -1 | head -n 5");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn repeated_status_updates_collapse_into_one_result() {
        let mut translator = TimelineTranslator::default();
        let call = ToolCall::new(ToolCallId::new("call-2"), "读取 Cargo.toml").kind(ToolKind::Read);
        let first = translator.translate(SessionUpdate::ToolCall(call));
        assert_eq!(first.len(), 1, "a descriptive title is announced at once");

        for _ in 0..3 {
            assert!(
                translator
                    .translate(tool_update(
                        "call-2",
                        None,
                        Some(ToolCallStatus::InProgress)
                    ))
                    .is_empty(),
                "in-progress chatter must not add rows"
            );
        }

        let done =
            translator.translate(tool_update("call-2", None, Some(ToolCallStatus::Completed)));
        assert_eq!(done.len(), 1);
        match &done[0] {
            AgentEvent::Entry(entry) => {
                assert_eq!(entry.kind, EntryKind::ToolResult);
                assert_eq!(entry.text, "读取 Cargo.toml · 完成");
            }
            other => panic!("unexpected event: {other:?}"),
        }

        assert!(
            translator
                .translate(tool_update("call-2", None, Some(ToolCallStatus::Completed)))
                .is_empty(),
            "a finished call must not be reported twice"
        );
        assert!(
            translator
                .tools
                .get("call-2")
                .is_some_and(|state| state.reported),
            "the terminal state is remembered as a tombstone"
        );
    }

    #[test]
    fn storage_access_is_detected_without_blanket_bans() {
        assert!(touches_cli_storage(
            "sqlite3 file:/Users/x/.mns/mns.db?mode=ro&immutable=1"
        ));
        assert!(touches_cli_storage("sqlite3 ~/.mns/mns.db"));
        assert!(
            !touches_cli_storage("/Applications/MNS.app/Contents/MacOS/mns portfolio"),
            "the CLI itself must never be treated as storage access"
        );
        assert!(
            !touches_cli_storage("sqlite3 /tmp/other.db"),
            "an unrelated database in the workspace is out of scope"
        );
        assert!(
            !touches_cli_storage("ls ~/.mns"),
            "listing the directory is not a storage read"
        );
    }

    #[test]
    fn decision_block_is_parsed_out_of_prose() {
        let text = "分析如下：现金够用。\n\n```mns-decision\n{\"verdict\":\"hold\",\"headline\":\"今天不操作\"}\n```\n";
        let report = extract_decision(text).expect("the fenced block must parse");
        assert_eq!(report.verdict(), "hold");
        assert_eq!(report.headline(), "今天不操作");
    }

    #[test]
    fn an_unlabelled_or_invalid_block_is_ignored() {
        assert!(extract_decision("没有任何代码块").is_none());
        assert!(
            extract_decision("```json\n{\"verdict\":\"hold\"}\n```").is_none(),
            "only a mns-decision fence is a conclusion"
        );
        assert!(
            extract_decision("```mns-decision\n{不是合法 JSON}\n```").is_none(),
            "malformed JSON must not become a card"
        );
    }

    #[test]
    fn the_last_valid_decision_wins() {
        let text = "```mns-decision\n{\"verdict\":\"wait\"}\n```\n修正：\n```mns-decision\n{\"verdict\":\"hold\"}\n```";
        assert_eq!(extract_decision(text).unwrap().verdict(), "hold");
    }

    #[test]
    fn configured_working_dir_wins_over_the_launch_directory() {
        let settings = AgentSettings {
            working_dir: "/tmp/mns-agent-work".to_owned(),
            ..AgentSettings::default()
        };
        assert_eq!(
            resolve_cwd(&settings),
            std::path::PathBuf::from("/tmp/mns-agent-work")
        );
    }

    #[test]
    fn default_workspace_sits_in_writable_app_data() {
        let cwd = resolve_cwd(&AgentSettings::default());
        assert_eq!(
            cwd,
            crate::config::app_data_dir()
                .join("agent")
                .join("workspace")
        );
        assert!(
            cwd.is_dir(),
            "workspace must exist for the agent to write into"
        );
        assert_ne!(cwd, std::path::PathBuf::from("/"));
    }
}
