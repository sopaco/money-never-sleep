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

use super::{
    AgentCommand, AgentEvent, AgentHandle, AgentSettings, EntryKind, PermissionChoice,
    PermissionPolicy, PermissionPrompt, RunState, TimelineEntry, detect_agent_cli, settings_dir,
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
                        AgentCommand::Start { goal, settings } => {
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
                            let events = event_tx.clone();
                            let task_shared = shared.clone();
                            let task = tokio::spawn(async move {
                                run_session(goal, settings, events, task_shared, stop_rx).await;
                            });
                            session = Some(SessionHandle {
                                shared,
                                stop: Some(stop_tx),
                                task,
                            });
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
fn cli_path() -> Option<PathBuf> {
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

fn launch_config(settings: &AgentSettings, cli: Option<&Path>) -> AcpAgentConfig {
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

    let config = if base.chars().any(char::is_whitespace) {
        // Tolerate a command string such as `npx opencode`.
        let mut parts = base.split_whitespace();
        let command = parts.next().unwrap_or("opencode");
        AcpAgentConfig::new(command).args(parts).args(args)
    } else {
        AcpAgentConfig::new(base).args(args)
    };

    match cli {
        Some(cli) => {
            let existing = std::env::var("PATH").unwrap_or_default();
            config.envs(agent_env(cli, &existing))
        }
        None => config,
    }
}

fn resolve_cwd(settings: &AgentSettings) -> PathBuf {
    let trimmed = settings.working_dir.trim();
    if trimmed.is_empty() {
        std::env::current_dir().unwrap_or_else(|_| settings_dir())
    } else {
        PathBuf::from(trimmed)
    }
}

/// Inject the autonomy contract into the single prompt. This is what keeps the
/// interaction goal-driven rather than chat-driven.
///
/// The CLI path is part of the contract, not an afterthought. Without it the
/// agent burns its first turns on `find`, `ls /private/tmp` and reading
/// `Info.plist`; worse, it may decide the database file is the data source and
/// query `~/.mns/mns.db` with sqlite3. Naming the binary and forbidding the
/// storage layer keeps the agent on the one supported interface.
fn compose_goal(goal: &str, cwd: &Path, cli: Option<&Path>) -> String {
    let cli_rule = match cli {
        Some(cli) => {
            let cli = cli.display();
            format!(
                "- 数据入口只有 mns CLI，绝对路径 `{cli}`，已随本应用安装。\
                 账户、持仓、绩效、报告、定投计划一律通过它读取，例如 `{cli} portfolio`、\
                 `{cli} today`、`{cli} --help`。不要用 find / mdfind / ls 搜索 mns，\
                 也不要依赖 PATH 上的同名命令——那可能是旧版本。\n\
                 - 不要绕过 CLI 访问底层存储：禁止用 sqlite3 打开 `~/.mns/mns.db`，\
                 禁止解析 `~/.mns/config.toml` 的内部结构，禁止阅读 mns 源码。\
                 CLI 没有提供的数据，说明它不该由你获取。"
            )
        }
        None => "- 未检测到随应用安装的 mns CLI。任务若需要账户数据，请停止并告知用户，\
                 不要自行搜索文件系统，也不要改用其他数据源。"
            .to_owned(),
    };

    format!(
        "{goal}\n\n---\n【MNS 桌面端执行约定】\n\
         - 这是一个自主执行任务：请自行拆解步骤、连续调用所需工具，直到目标达成或确认无法继续；\
         不要逐步向用户提问，也不要等待用户补充信息。\n\
         - 工作目录：{cwd}\n\
         {cli_rule}\n\
         - 仅在该目录内读写；不要执行下单、交易、资金划转、删除仓库、推送远端或对外发送数据的操作。\n\
         - 模型、推理端点与凭据由你自己的配置文件管理（例如 opencode 的 provider 配置）。\
         MNS 不提供 URL 或 API Key，也不要向用户索要这些内容。\n\
         - 完成后用中文输出：已完成事项、涉及的文件或命令、以及未完成或有风险的事项。",
        cwd = cwd.display()
    )
}

async fn run_session(
    goal: String,
    settings: AgentSettings,
    events: EventTx,
    shared: Shared,
    mut stop_rx: oneshot::Receiver<()>,
) {
    emit_state(&events, RunState::Starting, "正在启动外部 Agent…");

    let cwd = resolve_cwd(&settings);
    if !cwd.is_dir() {
        let _ = events.send(AgentEvent::Failed {
            message: format!("工作目录不存在：{}，请在设置中修改。", cwd.display()),
        });
        return;
    }

    let cli = cli_path();
    let config = launch_config(&settings, cli.as_deref());
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

    let agent = AcpAgent::new(config);
    let prompt_text = compose_goal(&goal, &cwd, cli.as_deref());

    let events_for_updates = events.clone();
    let events_for_permissions = events.clone();
    let events_for_foreground = events.clone();
    let shared_for_permissions = shared.clone();
    let shared_for_foreground = shared.clone();
    let policy = settings.permission_policy;
    let mut translator = TimelineTranslator::default();

    let connection_future = Client
        .v1()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
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
            emit_state(
                &events_for_foreground,
                RunState::Running,
                "Agent 正在自主执行目标…",
            );

            let response = connection
                .send_request(PromptRequest::new(
                    session_id,
                    vec![ContentBlock::Text(TextContent::new(prompt_text))],
                ))
                .block_task()
                .await?;

            let reason = format!("{:?}", response.stop_reason);
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
                    "任务已取消"
                } else {
                    "本轮执行结束"
                },
            );
            let _ = events_for_foreground.send(AgentEvent::Finished {
                stop_reason: reason,
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
        let config = launch_config(&settings, None);
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
        let config = launch_config(&settings, None);
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
        let config = launch_config(&settings, None);
        assert_eq!(config.command(), std::path::Path::new("npx"));
        assert_eq!(config.arguments(), ["opencode", "acp"]);
    }

    #[test]
    fn launch_config_injects_the_cli_path_and_prepends_path() {
        let settings = AgentSettings {
            agent_cli: "opencode".to_owned(),
            ..AgentSettings::default()
        };
        let cli = Path::new("/Applications/MNS.app/Contents/MacOS/mns");
        let config = launch_config(&settings, Some(cli));

        assert_eq!(
            config.environment().get("MNS_BIN").map(String::as_str),
            Some("/Applications/MNS.app/Contents/MacOS/mns")
        );
        let path = config
            .environment()
            .get("PATH")
            .expect("the agent must receive PATH");
        assert!(
            path.starts_with("/Applications/MNS.app/Contents/MacOS:"),
            "the bundle directory must come first so a bare `mns` resolves to it, got {path}"
        );
    }

    #[test]
    fn agent_env_keeps_the_original_path_after_the_bundle() {
        let vars = agent_env(Path::new("/tmp/bin/mns"), "/usr/bin:/bin");
        assert_eq!(vars[0].0, "MNS_BIN");
        assert_eq!(vars[0].1, "/tmp/bin/mns");
        assert_eq!(vars[1].0, "PATH");
        assert_eq!(vars[1].1, "/tmp/bin:/usr/bin:/bin");

        let empty = agent_env(Path::new("/tmp/bin/mns"), "");
        assert_eq!(empty[1].1, "/tmp/bin", "an empty PATH must not gain a separator");
    }

    #[test]
    fn goal_prompt_carries_the_autonomy_contract() {
        let prompt = compose_goal(
            "检查数据",
            Path::new("/work"),
            Some(Path::new("/Applications/MNS.app/Contents/MacOS/mns")),
        );
        assert!(prompt.starts_with("检查数据"));
        assert!(prompt.contains("不要逐步向用户提问"));
        assert!(prompt.contains("连续调用所需工具"));
        assert!(prompt.contains("MNS 不提供 URL 或 API Key"));
        assert!(prompt.contains("/work"));
        assert!(prompt.contains("不要执行下单、交易"));
    }

    #[test]
    fn goal_prompt_names_the_cli_and_forbids_the_storage_layer() {
        let prompt = compose_goal(
            "生成今日报告",
            Path::new("/work"),
            Some(Path::new("/Applications/MNS.app/Contents/MacOS/mns")),
        );
        assert!(
            prompt.contains("/Applications/MNS.app/Contents/MacOS/mns"),
            "the agent must be told the absolute path instead of searching for it"
        );
        assert!(prompt.contains("不要用 find"), "search commands are ruled out");
        assert!(
            prompt.contains("sqlite3") && prompt.contains("~/.mns/mns.db"),
            "reading the database directly must be named as forbidden"
        );
        assert!(prompt.contains("禁止阅读 mns 源码"));
    }

    #[test]
    fn goal_prompt_without_a_cli_forbids_guessing() {
        let prompt = compose_goal("生成今日报告", Path::new("/work"), None);
        assert!(prompt.contains("未检测到随应用安装的 mns CLI"));
        assert!(prompt.contains("不要自行搜索文件系统"));
        assert!(
            !prompt.contains("sqlite3"),
            "no path is offered, so there is no storage rule to state"
        );
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
}
