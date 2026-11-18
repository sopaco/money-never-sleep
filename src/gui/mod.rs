//! MNS desktop shell: a Fluent-inspired window over the shared engine.
//!
//! The shell owns presentation and interaction only. Every number it shows
//! comes from `crate::app`; every investment rule resolves to the same engine
//! modules the CLI uses. Background work runs in `jobs` and in the ACP runtime.

mod autostart;
mod butler;
mod chart;
// Theme and small building blocks are shared with the mns-help viewer.
pub(crate) mod components;
pub(crate) mod conversations;
pub(crate) mod help_links;
mod jobs;
mod mood;
mod pages;
pub(crate) mod theme;

use chrono::Datelike;
use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::component::input::{Input, InputState, TextareaState};
use gpui_kit::*;
use gpui_tray::{Icon, Tray};

use crate::agent::{
    AgentCommand, AgentEvent, AgentHandle, AgentSettings, DecisionReport, EntryKind, GoalSource,
    PermissionPolicy, PermissionPrompt, RunState, TimelineEntry, load_settings, routine,
    save_settings, start_runtime,
};
use crate::app::{BuyGuardrail, PlanView, Snapshot};
use crate::notify;
use crate::quote::FundCandidate;

/// 「缺标的」在 GUI 侧的下一步动作。引擎只存原因，去处由各界面拼：
/// 持仓页一定有「添加标的」入口（空态与非空态各一处），计划页没有；
/// 总览页只在空账户起始卡里给一个。所以这里指向持仓页，而不是本页。
pub(crate) const NO_INSTRUMENT_ACTION: &str = "，去持仓页添加标的";

/// 把引擎原因转成 GUI 展示文本：缺标的时补上「去哪儿补」，其余原因原样返回。
pub(crate) fn hold_reason_text(reason: &str) -> String {
    if reason == crate::strategy::NO_INSTRUMENT_REASON {
        format!("{reason}{NO_INSTRUMENT_ACTION}")
    } else {
        reason.to_owned()
    }
}

use components::*;
use conversations::{ConversationMeta, ConversationRecord, ConversationSummary};
use jobs::{Event as JobEvent, Jobs, Request};
use theme::*;

/// 构造 GUI 派生的子进程。
///
/// Release 版 GUI 是 windows 子系统程序，自身没有控制台；此时直接 spawn 一个
/// 控制台程序（`reg.exe`、`mns.exe`），Windows 会为它新建一个终端窗口，命令
/// 一结束窗口就消失——用户看到的就是「一闪而过的黑框」。统一从这里构造并
/// 带上 `CREATE_NO_WINDOW`，子进程复用不存在的控制台而不是新建一个。
pub(crate) fn command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    #[allow(unused_mut)]
    let mut command = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

actions!(
    mns_gui,
    [
        ShowMainWindow,
        RunDefaultGoal,
        StopCurrentTask,
        QuitApplication,
        StopButlerDaemon,
        CloseAgentPanel,
        CloseMainWindow,
        ShowAbout,
        RefreshData,
        OpenPlanPage,
        OpenAgentPage,
        OpenButlerWindow,
        RunTodayReview,
        OpenHelpCenter
    ]
);

const DEFAULT_GOAL: &str = "今天需要操作吗？帮我看一眼持仓和市场风险。";

/// Secondary surfaces on the assistant page. Both used to be inline cards
/// below the conversation, which forced a second scroll region and pushed the
/// composer off screen. One panel at a time keeps the page a single surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentPanel {
    Trace,
    Settings,
    /// Archive of past conversations: open for a read-only replay, or delete.
    History,
}

/// Feedback is transient, never a permanent layer over the content. A new
/// message restarts this window; the id keeps an older timer from hiding it.
const TOAST_DURATION: std::time::Duration = std::time::Duration::from_millis(2500);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Page {
    #[default]
    Dashboard,
    Portfolio,
    Plan,
    Performance,
    Agent,
    Settings,
}

impl Page {
    pub(crate) const ALL: [Page; 6] = [
        Page::Dashboard,
        Page::Portfolio,
        Page::Plan,
        Page::Performance,
        Page::Agent,
        Page::Settings,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Page::Dashboard => "总览",
            Page::Portfolio => "持仓",
            Page::Plan => "计划",
            Page::Performance => "复盘",
            Page::Agent => "助手",
            Page::Settings => "设置",
        }
    }

    /// 稳定机器键。行为日志用它而不是中文标签：标签会改，日志要长期可读。
    pub(crate) fn key(self) -> &'static str {
        match self {
            Page::Dashboard => "dashboard",
            Page::Portfolio => "portfolio",
            Page::Plan => "plan",
            Page::Performance => "performance",
            Page::Agent => "agent",
            Page::Settings => "settings",
        }
    }

    /// Fluent navigation always carries a glyph; the label alone reads as a
    /// menu, not as an app.
    pub(crate) fn icon(self) -> gpui_kit::assets::IconName {
        use gpui_kit::assets::IconName;
        match self {
            Page::Dashboard => IconName::LayoutDashboard,
            Page::Portfolio => IconName::Wallet,
            Page::Plan => IconName::CalendarCheck,
            Page::Performance => IconName::ChartLine,
            Page::Agent => IconName::Sparkles,
            Page::Settings => IconName::Settings,
        }
    }

    /// 顶栏问号指向的帮助页：回答「这一屏怎么用」。
    ///
    /// 复盘页没有同名文档，它的内容（曲线、回撤、流水）对应的其实是
    /// 「收益与风险指标」，所以落到理财百科而不是硬造一页。
    pub(crate) fn help_link(self) -> help_links::HelpLink {
        match self {
            Page::Dashboard => help_links::OVERVIEW,
            Page::Portfolio => help_links::PORTFOLIO,
            Page::Plan => help_links::PLAN,
            Page::Performance => help_links::REVIEW,
            Page::Agent => help_links::AGENT,
            Page::Settings => help_links::SETTINGS,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TradeSide {
    Buy,
    Sell,
}

impl TradeSide {
    pub(crate) fn label(self) -> &'static str {
        match self {
            TradeSide::Buy => "记录买入",
            TradeSide::Sell => "记录卖出",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CashKind {
    Deposit,
    Withdraw,
}

pub(crate) struct TradeForm {
    pub side: TradeSide,
    pub code: Entity<InputState>,
    pub shares: Entity<InputState>,
    pub price: Entity<InputState>,
    pub error: Option<String>,
}

pub(crate) struct CashForm {
    pub kind: CashKind,
    pub amount: Entity<InputState>,
    pub note: Entity<InputState>,
    pub error: Option<String>,
}

/// F2 买入冷静期层：买入触发护栏（计划外/超额/过热）后，先让用户把理由
/// 写下来再落库。保留已填的交易参数，确认时直接提交；「返回修改」时
/// 回填买入表单。不阻断——写完理由交易照常记录。
pub(crate) struct GuardrailForm {
    pub code: String,
    /// 解析后的交易参数，确认时直接提交。
    pub shares: f64,
    pub price: f64,
    /// 原样保留用户输入，「返回修改」时回填，不让用户重打数字。
    pub shares_text: String,
    pub price_text: String,
    pub amount: f64,
    pub verdict: BuyGuardrail,
    pub reason: Entity<InputState>,
    pub error: Option<String>,
}

pub(crate) struct AssetForm {
    /// 搜索框：代码或名称。
    pub query: Entity<InputState>,
    /// 已发出的关键词。worker 结果回来时按它对齐：慢响应不能覆盖后一次搜索。
    pub searched: Option<String>,
    pub searching: bool,
    pub results: Vec<FundCandidate>,
    /// 选中的候选。gpui-component 的 `InputState` 没有公开 setter，回填只能
    /// 靠这份权威选择，不能把文字塞回输入框。
    pub selected: Option<FundCandidate>,
    /// 手填代码是一等入口：不搜索也能直接添加。
    pub code: Entity<InputState>,
    pub name: Entity<InputState>,
    /// 三选一；None = 用户还没表态，提交会被退回。
    pub category: Option<&'static str>,
    pub error: Option<String>,
}

/// 三条腿的短标签。「添加标的」的 chip 与建议文案共用一套说法。
pub(crate) fn category_label(key: &str) -> &'static str {
    match key {
        "us_stocks" => "美股",
        "cn_stocks" => "A 股",
        "counter_cyclical" => "逆周期",
        _ => "未归类",
    }
}

pub(crate) enum Dialog {
    Trade(TradeForm),
    /// 买入冷静期（F2）：Trade 提交时先过护栏判定，触发才进这一层。
    Guardrail(GuardrailForm),
    Cash(CashForm),
    AddAsset(AssetForm),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToastTone {
    Success,
    Error,
}

pub(crate) struct Toast {
    /// Monotonic sequence number. The dismissal task only clears the toast
    /// that scheduled it, so a newer message is never hidden by an older one.
    pub id: u64,
    pub text: String,
    pub tone: ToastTone,
}

#[derive(Clone, Default)]
pub(crate) struct AgentSnapshot {
    pub state: Option<RunState>,
    pub detail: String,
    pub timeline: Vec<TimelineEntry>,
    pub pending: Option<PermissionPrompt>,
    pub error: Option<String>,
}

/// Agent panel state. Kept separate from the account data so an agent run can
/// stream while the user browses positions without clobbering either.
pub(crate) struct AgentState {
    pub state: RunState,
    pub detail: String,
    pub timeline: Vec<TimelineEntry>,
    pub streaming: Option<EntryKind>,
    pub pending: Option<PermissionPrompt>,
    pub error: Option<String>,
    pub started: bool,
    /// Latest structured conclusion. Kept beside the timeline rather than in
    /// it: the card is rendered from JSON, and the same run can revise it.
    pub decision: Option<DecisionReport>,
    /// ACP session is alive and a follow-up can be delivered.
    pub open: bool,
}

impl AgentState {
    fn new(restore: Option<AgentSnapshot>) -> Self {
        let restore = restore.unwrap_or_default();
        let state = restore.state.unwrap_or_default();
        let detail = if restore.detail.trim().is_empty() {
            "尚未提交目标".to_owned()
        } else {
            restore.detail
        };
        Self {
            state,
            detail,
            timeline: restore.timeline,
            streaming: None,
            pending: restore.pending,
            error: restore.error,
            started: false,
            decision: None,
            open: false,
        }
    }

    pub(crate) fn is_running(&self) -> bool {
        matches!(
            self.state,
            RunState::Starting
                | RunState::Running
                | RunState::WaitingPermission
                | RunState::Stopping
        )
    }

    fn snapshot(&self) -> AgentSnapshot {
        AgentSnapshot {
            state: Some(self.state),
            detail: self.detail.clone(),
            timeline: self.timeline.clone(),
            pending: self.pending.clone(),
            error: self.error.clone(),
        }
    }

    fn push(&mut self, kind: EntryKind, text: impl Into<String>) {
        self.streaming = None;
        self.timeline.push(TimelineEntry::new(kind, text));
        trim(&mut self.timeline);
    }

    fn append_delta(&mut self, kind: EntryKind, text: String) {
        if self.streaming == Some(kind) {
            if let Some(last) = self.timeline.last_mut() {
                last.text.push_str(&text);
                return;
            }
        }
        self.streaming = Some(kind);
        self.timeline.push(TimelineEntry::new(kind, text));
        trim(&mut self.timeline);
    }

    fn apply(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::State { state, detail } => {
                self.state = state;
                self.detail = detail;
            }
            AgentEvent::Entry(entry) => {
                self.streaming = None;
                self.timeline.push(entry);
                trim(&mut self.timeline);
            }
            AgentEvent::Delta { kind, text } => self.append_delta(kind, text),
            AgentEvent::Permission(prompt) => self.pending = Some(prompt),
            AgentEvent::PermissionResolved { id } => {
                if self.pending.as_ref().is_some_and(|p| p.id == id) {
                    self.pending = None;
                }
            }
            AgentEvent::TurnFinished { stop_reason } => {
                self.state = if self.state == RunState::Cancelled {
                    RunState::Cancelled
                } else {
                    RunState::Finished
                };
                self.detail = if self.state == RunState::Cancelled {
                    "已停止".to_owned()
                } else {
                    "本轮结束，可以继续追问".to_owned()
                };
                let _ = stop_reason;
                self.pending = None;
                self.open = self.state != RunState::Cancelled;
            }
            AgentEvent::Decision(report) => {
                self.decision = Some(report);
            }
            AgentEvent::Finished { stop_reason } => {
                self.open = false;
                self.detail = "会话已结束".to_owned();
                let _ = stop_reason;
                self.pending = None;
            }
            AgentEvent::Failed { message } => {
                self.state = RunState::Failed;
                self.detail = "任务失败".to_owned();
                self.error = Some(message.clone());
                self.push(EntryKind::Error, message);
                self.pending = None;
                self.open = false;
            }
        }
    }
}

fn trim(timeline: &mut Vec<TimelineEntry>) {
    if timeline.len() > 400 {
        timeline.drain(..timeline.len() - 400);
    }
}

/// `HH:MM`, zero padded. The scheduler compares strings, so `9:5` would never
/// fire — reject it here instead of silently accepting a time that cannot run.
fn valid_clock_time(raw: &str) -> bool {
    let mut parts = raw.split(':');
    let (Some(hours), Some(minutes), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    if hours.len() != 2 || minutes.len() != 2 {
        return false;
    }
    matches!(
        (hours.parse::<u32>(), minutes.parse::<u32>()),
        (Ok(h), Ok(m)) if h < 24 && m < 60
    )
}

/// Marker for the last scheduled review, stored next to the decision archive so
/// a restart cannot trigger the same day twice.
fn last_daily_path() -> std::path::PathBuf {
    crate::agent::settings_dir()
        .join("agent")
        .join("last-review")
}

fn read_last_daily() -> Option<String> {
    let text = std::fs::read_to_string(last_daily_path()).ok()?;
    let text = text.trim().to_owned();
    if text.is_empty() { None } else { Some(text) }
}

fn write_last_daily(date: &str) {
    let path = last_daily_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, date);
}

#[derive(Clone, Default)]
struct ShellSnapshot {
    agent: AgentSnapshot,
    /// Metadata of the conversation still being appended to, so a re-created
    /// window does not start a second archive file for the same session.
    conversation: Option<ConversationMeta>,
}

/// A past conversation shown in place of the live one. The ACP context lives
/// in the external agent, which MNS never re-opens, so a replay is explicitly
/// read-only rather than pretending the model remembers the thread.
#[derive(Clone, Debug)]
pub(crate) struct ViewedConversation {
    pub id: String,
    pub title: String,
}

struct GuiState {
    window: Option<AnyWindowHandle>,
    /// Id of `window`, so the closed observer can ignore a window that is
    /// being replaced (autostart creates a hidden one, then rebuilds it).
    window_id: Option<WindowId>,
    /// 启动时创建的是隐藏窗口（登录自启动只驻留托盘）。这类窗口没有跨平台
    /// 的“显示”接口，用户点「打开 MNS」时按可见参数重建。
    window_hidden: bool,
    view: Option<Entity<AppShell>>,
    /// 管家独立窗口与视图。与主窗口各存一份句柄，关闭其中一个不影响另一个。
    butler_window: Option<AnyWindowHandle>,
    butler_view: Option<Entity<butler::ButlerView>>,
    /// 最近一条已授权但用户还没在窗口里看到的管家结论，用于托盘菜单。
    butler_notice: Option<String>,
    /// 管家守护进程是否在跑。在跑时巡检归它，GUI 只负责算事实与显示。
    butler_daemon: bool,
    /// 守护进程那条连接的 id；断开时要能辨认出是它，而不是别的临时连接。
    butler_daemon_conn: Option<u64>,
    tray: Option<Tray>,
    jobs: Option<Jobs>,
    snapshot: Option<ShellSnapshot>,
    /// Last brief actually pushed into the native menu. `refresh_menu` always
    /// bumps its generation, so an unchanged brief must not be pushed again or
    /// macOS rebuilds the NSMenu (and closes an open one) once per minute.
    last_tray_brief: Option<TrayBrief>,
}

impl Global for GuiState {}

pub(crate) struct AppShell {
    pub(crate) page: Page,
    pub(crate) data: Option<Snapshot>,
    pub(crate) load_error: Option<String>,
    pub(crate) loading: bool,
    pub(crate) refreshing: bool,
    pub(crate) toast: Option<Toast>,
    /// Incremented for every message; see `Toast::id`.
    toast_seq: u64,
    pub(crate) dialog: Option<Dialog>,
    pub(crate) agent: AgentState,
    pub(crate) agent_settings: AgentSettings,
    pub(crate) jobs: Jobs,
    pub(crate) goal: Entity<TextareaState>,
    pub(crate) followup: Entity<TextareaState>,
    pub(crate) cli: Entity<InputState>,
    pub(crate) cwd: Entity<InputState>,
    /// Free-text model id. Empty defers to the external agent's own default.
    pub(crate) model: Entity<InputState>,
    pub(crate) daily_time: Entity<InputState>,
    pub(crate) monthly: Entity<InputState>,
    /// 本地行为记录是否开启（设置页可关，见 `butler::events`）。
    pub(crate) behavior_enabled: bool,
    /// 管家主动开口是否开启（设置页可关，落到 `butler/state.json` 的策略里）。
    pub(crate) butler_proactive: bool,
    /// 管家频控的编辑框。数值落在 `config.toml` 的 `[butler]` 节。
    pub(crate) butler_daily: Entity<InputState>,
    pub(crate) butler_weekly: Entity<InputState>,
    pub(crate) butler_dedupe: Entity<InputState>,
    pub(crate) butler_quiet_start: Entity<InputState>,
    pub(crate) butler_quiet_end: Entity<InputState>,
    /// 记账成功后等新快照到达再唤醒管家；记的是那条行为日志的毫秒时间戳。
    pending_ledger_wake: Option<String>,
    /// Date (`YYYY-MM-DD`) of the last scheduled review this process started.
    /// Set when the run is dispatched, not when it finishes: a failing review
    /// must not retry every minute for the rest of the day.
    pub(crate) last_daily: Option<String>,
    /// The one open secondary surface (execution trace or advanced setup).
    /// `None` means the conversation owns the whole page.
    pub(crate) agent_panel: Option<AgentPanel>,
    /// Tracks the single scrollable conversation surface so a new answer can
    /// follow the tail without the user fighting a nested scrollbar.
    pub(crate) conversation_scroll: ScrollHandle,
    /// The drawer takes focus while open: its own key context then owns Esc
    /// instead of the composer input behind it.
    pub(crate) agent_panel_focus: FocusHandle,
    /// The "About MNS" panel is reachable from the macOS app menu, so it has
    /// to work even when the window was hidden rather than closed.
    about_open: bool,
    /// Archive index shown in the history drawer. Only summaries are held in
    /// memory; full transcripts are read on demand.
    pub(crate) history: Vec<ConversationSummary>,
    pub(crate) history_loaded: bool,
    pub(crate) history_error: Option<String>,
    /// Row awaiting a second click before its file is removed.
    pub(crate) history_pending_delete: Option<String>,
    /// Metadata for the live session, `None` before the first goal and after
    /// the user deletes the record of the session they are still in.
    current_conversation: Option<ConversationMeta>,
    /// True when the transcript changed since the last archive write. The
    /// minute tick uses it to flush a still-streaming answer without writing
    /// the same file on every token or when nothing happened at all.
    conversation_dirty: bool,
    /// `Some` while a past conversation is being replayed in the chat surface.
    pub(crate) viewing_history: Option<ViewedConversation>,
    /// Whether the dashboard's "最近报告" card shows the full archived text.
    /// Collapsed by default: the card previews the header; the full weekly
    /// ritual text is one click away.
    pub(crate) report_expanded: bool,
    /// Live agent state parked while the chat surface shows a replay. Incoming
    /// runtime events are folded into this, never into the replay.
    live_agent: Option<Box<AgentState>>,
}

impl AppShell {
    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        jobs: Jobs,
        restore: Option<ShellSnapshot>,
    ) -> Self {
        let agent_settings = load_settings();
        let conversation = restore
            .as_ref()
            .and_then(|snapshot| snapshot.conversation.clone());
        let goal = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("想问什么？例如「今天需要操作吗」")
                .default_value(DEFAULT_GOAL)
        });
        let cli = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("opencode 可执行文件路径")
                .default_value(agent_settings.agent_cli.clone())
        });
        let followup = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder("接着问，例如：黄金这一腿为什么不动？")
        });
        let cwd = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("留空则使用启动目录")
                .default_value(agent_settings.working_dir.clone())
        });
        let model = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("例如 opencode/deepseek-v4.1-flash-free")
                .default_value(agent_settings.model.clone())
        });
        let daily_time = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("08:30")
                .default_value(agent_settings.daily_review_time.clone())
        });

        // A config read is a single small TOML file; loading the whole account
        // here would put a database read on the UI thread.
        let config = crate::config::AppConfig::load().ok();
        let monthly_value = config
            .as_ref()
            .map(|config| {
                if config.cashflow.monthly > 0.0 {
                    format!("{:.0}", config.cashflow.monthly)
                } else {
                    String::new()
                }
            })
            .unwrap_or_default();
        let monthly = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("例如 3000")
                .default_value(monthly_value)
        });

        // 管家频控：启动时读一次配置。保存后由 worker 重写 config.toml 并
        // 刷新快照；守护进程下一次判定会自己重新加载，不需要重启。
        let butler_policy = config
            .as_ref()
            .map(|config| config.butler.clone())
            .unwrap_or_default();
        let butler_daily = cx.new(|cx| {
            InputState::new(window, cx).default_value(butler_policy.daily_cap.to_string())
        });
        let butler_weekly = cx.new(|cx| {
            InputState::new(window, cx).default_value(butler_policy.weekly_cap.to_string())
        });
        let butler_dedupe = cx.new(|cx| {
            InputState::new(window, cx).default_value(butler_policy.dedupe_days.to_string())
        });
        let butler_quiet_start = cx.new(|cx| {
            InputState::new(window, cx).default_value(butler_policy.quiet_start_hour.to_string())
        });
        let butler_quiet_end = cx.new(|cx| {
            InputState::new(window, cx).default_value(butler_policy.quiet_end_hour.to_string())
        });

        let shell = Self {
            page: std::env::var("MNS_GUI_PAGE")
                .ok()
                .and_then(|key| {
                    Page::ALL
                        .into_iter()
                        .find(|page| page.label() == key.trim())
                })
                .unwrap_or(Page::Dashboard),
            data: None,
            load_error: None,
            loading: true,
            refreshing: false,
            toast: None,
            toast_seq: 0,
            dialog: None,
            agent: AgentState::new(restore.map(|s| s.agent)),
            agent_settings,
            jobs,
            goal,
            followup,
            cli,
            cwd,
            model,
            daily_time,
            monthly,
            butler_daily,
            butler_weekly,
            butler_dedupe,
            butler_quiet_start,
            butler_quiet_end,
            behavior_enabled: crate::butler::events::BehaviorSettings::load().enabled,
            butler_proactive: crate::butler::Butler::load()
                .map(|butler| butler.state.policy.enabled)
                .unwrap_or(true),
            pending_ledger_wake: None,
            last_daily: read_last_daily(),
            agent_panel: None,
            conversation_scroll: ScrollHandle::new(),
            agent_panel_focus: cx.focus_handle(),
            about_open: false,
            history: Vec::new(),
            history_loaded: false,
            history_error: None,
            history_pending_delete: None,
            current_conversation: conversation,
            conversation_dirty: false,
            viewing_history: None,
            report_expanded: false,
            live_agent: None,
        };
        // 启动即记一条：管家要能区分「今天开过应用」和「今天完全没看」。
        // 开关关闭时 `append` 自己会跳过，这里不必再判一次。窗口重建也属于
        // 同一次启动，所以用 `Once` 保证每个进程只记一次。
        static APP_OPEN_ONCE: std::sync::Once = std::sync::Once::new();
        APP_OPEN_ONCE.call_once(|| {
            let _ = crate::butler::events::append(&crate::butler::events::BehaviorEvent::new(
                crate::butler::events::kind::APP_OPEN,
                serde_json::json!({}),
            ));
        });
        shell
    }

    fn agent_snapshot(&self) -> ShellSnapshot {
        // A replay must never become the restored "current" conversation when
        // a window is rebuilt on another platform.
        let agent = self
            .live_agent
            .as_deref()
            .map(AgentState::snapshot)
            .unwrap_or_else(|| self.agent.snapshot());
        ShellSnapshot {
            agent,
            conversation: self.current_conversation.clone(),
        }
    }

    /// Show feedback that goes away on its own. Each message owns its own
    /// dismissal task; the id check keeps an older timer from hiding a newer
    /// message when several actions happen back to back.
    fn show_toast(&mut self, text: impl Into<String>, tone: ToastTone, cx: &mut Context<Self>) {
        self.toast_seq = self.toast_seq.wrapping_add(1);
        let id = self.toast_seq;
        self.toast = Some(Toast {
            id,
            text: text.into(),
            tone,
        });
        cx.spawn(async move |weak, async_cx| {
            async_cx.background_executor().timer(TOAST_DURATION).await;
            let _ = weak.update(async_cx, |shell, cx| {
                if shell.toast.as_ref().is_some_and(|toast| toast.id == id) {
                    shell.toast = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Clicking the message dismisses it immediately; the timer is harmless
    /// because it only acts on a live toast with the same id.
    fn dismiss_toast(&mut self, cx: &mut Context<Self>) {
        if self.toast.take().is_some() {
            cx.notify();
        }
    }

    /// Keep the newest answer in view, but never yank the page away from
    /// someone who scrolled up to re-read an earlier turn. `force` is for a
    /// turn the user just started themselves.
    fn follow_conversation(&self, force: bool) {
        let handle = &self.conversation_scroll;
        let near_bottom = handle.max_offset().y + handle.offset().y <= px(96.);
        if force || near_bottom {
            handle.scroll_to_bottom();
        }
    }

    pub(crate) fn navigate(&mut self, page: Page, cx: &mut Context<Self>) {
        if self.page != page {
            let _ = crate::butler::events::append(
                &crate::butler::events::BehaviorEvent::page_view(page.key()),
            );
        }
        self.page = page;
        // A drawer belongs to the page that opened it; leaving must not leave
        // it hanging around when the user comes back.
        self.agent_panel = None;
        cx.notify();
    }

    pub(crate) fn toggle_report(&mut self, cx: &mut Context<Self>) {
        self.report_expanded = !self.report_expanded;
        cx.notify();
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        if let Err(error) = self.jobs.send(Request::Refresh) {
            self.refreshing = false;
            self.show_toast(error, ToastTone::Error, cx);
        }
        cx.notify();
    }

    pub(crate) fn reload(&mut self) {
        let _ = self.jobs.send(Request::Load);
    }

    // ── Conversation archive ───────────────────────────────────────────

    /// Ask the worker for the archive index: on startup and whenever the
    /// history drawer is raised, so an unattended daily review shows up too.
    pub(crate) fn load_history(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = self.jobs.send(Request::LoadConversations) {
            self.history_error = Some(error.clone());
            self.show_toast(error, ToastTone::Error, cx);
        }
    }

    /// Upsert the live session. Deliberately called at low-frequency
    /// boundaries only — writing on every streamed delta would rewrite the
    /// whole transcript for each token.
    fn archive_current(&mut self, cx: &mut Context<Self>) {
        if self.viewing_history.is_some() {
            return;
        }
        let Some(meta) = self.current_conversation.clone() else {
            return;
        };
        let Some(record) = conversations::build_record(
            &meta,
            &self.agent.timeline,
            self.agent.decision.as_ref(),
            self.agent.state,
        ) else {
            return;
        };
        match self.jobs.send(Request::SaveConversation(Box::new(record))) {
            Ok(()) => self.conversation_dirty = false,
            Err(error) => self.show_toast(error, ToastTone::Error, cx),
        }
    }

    pub(crate) fn open_history_conversation(&mut self, id: String, cx: &mut Context<Self>) {
        if self.agent.is_running() || self.agent.pending.is_some() {
            self.show_toast(
                "助手正在工作，结束后再查看历史记录。".to_owned(),
                ToastTone::Error,
                cx,
            );
            cx.notify();
            return;
        }
        // Switching between replays must not bump the live session's
        // `updated_at` and reorder the list underneath the user.
        if self.viewing_history.is_none() {
            self.archive_current(cx);
        }
        if let Err(error) = self.jobs.send(Request::LoadConversation { id }) {
            self.show_toast(error, ToastTone::Error, cx);
        }
        cx.notify();
    }

    fn show_history_record(&mut self, record: ConversationRecord, cx: &mut Context<Self>) {
        if self.live_agent.is_none() {
            let live = std::mem::replace(&mut self.agent, AgentState::new(None));
            self.live_agent = Some(Box::new(live));
        }
        self.agent.timeline = conversations::restore_entries(&record.entries);
        self.agent.decision = record
            .decision
            .as_deref()
            .and_then(conversations::decision_from_raw);
        self.agent.state = conversations::state_from_tag(&record.state);
        self.agent.detail = "历史会话（只读）".to_owned();
        self.agent.open = false;
        self.agent.streaming = None;
        self.agent.pending = None;
        self.agent.error = None;
        self.viewing_history = Some(ViewedConversation {
            id: record.id.clone(),
            title: record.title.clone(),
        });
        self.agent_panel = None;
        self.history_pending_delete = None;
        self.follow_conversation(true);
        cx.notify();
    }

    pub(crate) fn exit_history_view(&mut self, cx: &mut Context<Self>) {
        if self.viewing_history.take().is_none() {
            return;
        }
        if let Some(live) = self.live_agent.take() {
            self.agent = *live;
        }
        // Turns that arrived while the replay was on screen were folded into
        // the parked state; flush them now that the live session is visible.
        self.archive_current(cx);
        self.follow_conversation(false);
        cx.notify();
    }

    pub(crate) fn request_delete_history(&mut self, id: String, cx: &mut Context<Self>) {
        self.history_pending_delete = Some(id);
        cx.notify();
    }

    pub(crate) fn cancel_delete_history(&mut self, cx: &mut Context<Self>) {
        self.history_pending_delete = None;
        cx.notify();
    }

    pub(crate) fn confirm_delete_history(&mut self, id: String, cx: &mut Context<Self>) {
        self.history_pending_delete = None;
        // Deleting the record of the live session must not let the next turn
        // silently recreate it; a new goal opens a fresh archive entry.
        if self
            .current_conversation
            .as_ref()
            .is_some_and(|meta| meta.id == id)
        {
            self.current_conversation = None;
        }
        if self
            .viewing_history
            .as_ref()
            .is_some_and(|view| view.id == id)
        {
            self.exit_history_view(cx);
        }
        if let Err(error) = self.jobs.send(Request::DeleteConversation { id }) {
            self.show_toast(error, ToastTone::Error, cx);
        }
        cx.notify();
    }

    pub(crate) fn start_agent(&mut self, cx: &mut Context<Self>) {
        let goal = self.goal.read(cx).value().trim().to_string();
        if goal.is_empty() {
            self.show_toast(
                "先写一句目标，再启动 Agent。".to_owned(),
                ToastTone::Error,
                cx,
            );
            cx.notify();
            return;
        }
        self.begin_session(goal.clone(), goal, GoalSource::Manual, cx);
    }

    /// Open one ACP session. `display` is what the user sees (a short label for
    /// scheduled runs), `prompt` is what the agent receives.
    fn begin_session(
        &mut self,
        display: String,
        prompt: String,
        source: GoalSource,
        cx: &mut Context<Self>,
    ) {
        self.persist_agent_settings(cx);
        // A menu action or an unattended review can arrive while a replay is
        // on screen: restore the live session before archiving and replacing it.
        if self.viewing_history.is_some() {
            self.exit_history_view(cx);
        }
        self.archive_current(cx);
        self.current_conversation = Some(ConversationMeta::new(match source {
            GoalSource::Manual => "手动",
            GoalSource::DailyReview => "每日巡检",
            // Not reached from this shell today (the butler window owns its
            // own session), but the enum is exhaustive on purpose: a new
            // source must decide what the archive calls it.
            GoalSource::Conversation => "管家对话",
        }));
        self.agent.timeline.clear();
        self.agent.streaming = None;
        self.agent.pending = None;
        self.agent.error = None;
        self.agent.decision = None;
        self.agent.started = true;
        self.agent.open = true;
        self.agent.state = RunState::Starting;
        self.agent.detail = match source {
            GoalSource::Manual => "目标已提交，正在连接外部 Agent".to_owned(),
            GoalSource::DailyReview => "正在准备今日巡检".to_owned(),
            GoalSource::Conversation => "正在连接管家 Agent".to_owned(),
        };
        // The user's own words stay in the transcript; a scheduled run shows a
        // label instead of dumping a several-thousand-character prompt.
        self.agent.push(EntryKind::User, display);
        if let Err(error) = self.agent_send(AgentCommand::Start {
            goal: prompt,
            settings: self.agent_settings.clone(),
            source,
        }) {
            self.agent.state = RunState::Failed;
            self.agent.open = false;
            self.agent.error = Some(error.clone());
            self.agent.push(EntryKind::Error, error);
        }
        // Persist the opening line immediately: a crash mid-run should still
        // leave a dated, titled conversation rather than nothing at all.
        self.archive_current(cx);
        self.follow_conversation(true);
        cx.notify();
    }

    /// Answer a follow-up inside the live session.
    pub(crate) fn ask_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.agent.is_running() {
            return;
        }
        if !self.agent.open {
            self.show_toast(
                "会话已结束，请重新开始一个任务。".to_owned(),
                ToastTone::Error,
                cx,
            );
            cx.notify();
            return;
        }
        let question = self.followup.read(cx).value().trim().to_string();
        if question.is_empty() {
            return;
        }

        self.agent.push(EntryKind::User, question.clone());
        self.agent.error = None;
        self.agent.streaming = None;
        self.agent.state = RunState::Running;
        self.agent.detail = "Agent 正在回答追问…".to_owned();
        if let Err(error) = self.agent_send(AgentCommand::Ask {
            text: routine::followup_prompt(&question),
        }) {
            self.agent.state = RunState::Failed;
            self.agent.error = Some(error.clone());
            self.agent.push(EntryKind::Error, error);
        }
        self.follow_conversation(true);
        self.archive_current(cx);
        // Rebuilding the state is the only reset that does not depend on an
        // editor-specific clear() API; the old value would otherwise linger in
        // the box and be re-sent by a second click.
        self.followup = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder("接着问，例如：黄金这一腿为什么不动？")
        });
        cx.notify();
    }

    /// Kick off the daily review: the CLI produces the brief, then the session
    /// opens with it. Nothing is written by the agent.
    pub(crate) fn start_daily_review(&mut self, refresh: bool, cx: &mut Context<Self>) {
        if self.agent.is_running() {
            return;
        }
        self.agent.detail = "正在生成今日报告…".to_owned();
        self.agent.state = RunState::Starting;
        self.agent.push(
            EntryKind::System,
            if refresh {
                "正在运行 mns today，生成今日数据…"
            } else {
                "正在读取最近一次报告…"
            },
        );
        if let Err(error) = self.jobs.send(Request::AgentBrief { refresh }) {
            self.agent.state = RunState::Failed;
            self.agent.error = Some(error.clone());
            self.agent.push(EntryKind::Error, error);
        }
        cx.notify();
    }

    /// Scheduled entry point. Dispatches at most once per calendar day, and
    /// only when a review is enabled.
    pub(crate) fn maybe_start_daily(&mut self, cx: &mut Context<Self>) {
        if !self.agent_settings.daily_review || self.agent.is_running() {
            return;
        }
        let now = chrono::Local::now();
        let today = now.format("%Y-%m-%d").to_string();
        if self.last_daily.as_deref() == Some(today.as_str()) {
            return;
        }
        let target = self.agent_settings.daily_review_time.trim();
        if target.is_empty() || now.format("%H:%M").to_string().as_str() < target {
            return;
        }
        self.last_daily = Some(today.clone());
        // Persisted so restarting the app mid-day does not fire the review a
        // second time. A duplicate run means duplicate trade suggestions, which
        // is worse than a missed one.
        write_last_daily(&today);
        self.start_daily_review(true, cx);
    }

    pub(crate) fn stop_agent(&mut self, cx: &mut Context<Self>) {
        let _ = self.agent_send(AgentCommand::Stop);
        self.agent
            .push(EntryKind::System, "已请求停止，等待协议确认");
        cx.notify();
    }

    pub(crate) fn apply_profile(&mut self, id: String, cx: &mut Context<Self>) {
        if let Err(error) = self.jobs.send(Request::SetProfile(id)) {
            self.show_toast(error, ToastTone::Error, cx);
        }
        cx.notify();
    }

    pub(crate) fn apply_weekday(&mut self, weekday: u32, cx: &mut Context<Self>) {
        if let Err(error) = self.jobs.send(Request::SetWeekday(weekday)) {
            self.show_toast(error, ToastTone::Error, cx);
        }
        cx.notify();
    }

    pub(crate) fn save_monthly(&mut self, cx: &mut Context<Self>) {
        let raw = self.monthly.read(cx).value().trim().to_owned();
        match parse_number(&raw) {
            Some(amount) if amount >= 0.0 => {
                if let Err(error) = self.jobs.send(Request::SetMonthly(amount)) {
                    self.show_toast(error, ToastTone::Error, cx);
                }
            }
            _ => {
                self.show_toast(
                    "每月定投金额必须是非负数字".to_owned(),
                    ToastTone::Error,
                    cx,
                );
            }
        }
        cx.notify();
    }

    /// 保存管家频控。参数先在这里做粗校验，真正的合法性（每周 ≥ 每天、
    /// 静音 0-24）由 `app::set_butler_policy` 与 `AppConfig::validate` 兜住，
    /// 这样错误信息能落在 toast 上，而不是等下次启动才暴露。
    pub(crate) fn save_butler_policy(&mut self, cx: &mut Context<Self>) {
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        let daily = value(&self.butler_daily).parse::<usize>();
        let weekly = value(&self.butler_weekly).parse::<usize>();
        let dedupe = value(&self.butler_dedupe).parse::<i64>();
        let quiet_start = value(&self.butler_quiet_start).parse::<u32>();
        let quiet_end = value(&self.butler_quiet_end).parse::<u32>();
        let (Ok(daily), Ok(weekly), Ok(dedupe), Ok(quiet_start), Ok(quiet_end)) =
            (daily, weekly, dedupe, quiet_start, quiet_end)
        else {
            self.show_toast("频控参数必须是非负整数".to_owned(), ToastTone::Error, cx);
            cx.notify();
            return;
        };
        if let Err(error) = self.jobs.send(Request::SetButlerPolicy {
            daily_cap: daily,
            weekly_cap: weekly,
            dedupe_days: dedupe,
            quiet_start_hour: quiet_start,
            quiet_end_hour: quiet_end,
        }) {
            self.show_toast(error, ToastTone::Error, cx);
        }
        cx.notify();
    }

    fn agent_send(&self, command: AgentCommand) -> Result<(), String> {
        let handle = self
            .agent_handle()
            .ok_or_else(|| "Agent 运行时不可用".to_owned())?;
        handle.send(command)
    }

    fn agent_handle(&self) -> Option<AgentHandle> {
        AGENT.with(|cell| cell.borrow().clone())
    }

    fn persist_agent_settings(&mut self, cx: &mut Context<Self>) {
        self.agent_settings.agent_cli = self.cli.read(cx).value().trim().to_owned();
        self.agent_settings.working_dir = self.cwd.read(cx).value().trim().to_owned();
        self.agent_settings.model = self.model.read(cx).value().trim().to_owned();
        if let Err(error) = save_settings(&self.agent_settings) {
            self.agent.error = Some(error);
        }
    }

    /// One drawer at a time. Clicking the same trigger again closes it, so the
    /// user never has to hunt for a close button.
    pub(crate) fn toggle_agent_panel(
        &mut self,
        panel: AgentPanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.agent_panel == Some(panel) {
            self.close_agent_panel(window, cx);
            return;
        }
        self.agent_panel = Some(panel);
        self.history_pending_delete = None;
        if panel == AgentPanel::History {
            self.load_history(cx);
        }
        window.focus(&self.agent_panel_focus, cx);
        cx.notify();
    }

    /// Give the composer its caret back after a drawer or a history replay
    /// owned the window. Without this the input is visible but cannot be typed
    /// into until the user clicks it first.
    pub(crate) fn focus_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = if self.agent.open {
            self.followup.read(cx).focus_handle(cx)
        } else {
            self.goal.read(cx).focus_handle(cx)
        };
        window.focus(&handle, cx);
    }

    pub(crate) fn close_agent_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.agent_panel.take().is_some() {
            self.focus_composer(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn toggle_daily_review(&mut self, cx: &mut Context<Self>) {
        self.agent_settings.daily_review = !self.agent_settings.daily_review;
        self.persist_daily_setting(cx);
    }

    /// 开机自启动开关：默认关闭。真正生效的是系统登录项，所以先写系统、
    /// 成功后才落盘设置；任一步失败都回滚，避免界面与系统状态不一致。
    pub(crate) fn toggle_launch_at_login(&mut self, cx: &mut Context<Self>) {
        let next = !self.agent_settings.launch_at_login;
        if let Err(error) = autostart::set_enabled(next) {
            self.show_toast(format!("开机自启动设置失败：{error}"), ToastTone::Error, cx);
            return;
        }
        self.agent_settings.launch_at_login = next;
        if let Err(error) = save_settings(&self.agent_settings) {
            let _ = autostart::set_enabled(!next);
            self.agent_settings.launch_at_login = !next;
            self.show_toast(error, ToastTone::Error, cx);
        } else {
            self.show_toast(
                if next {
                    "已开启开机自启动"
                } else {
                    "已关闭开机自启动"
                }
                .to_owned(),
                ToastTone::Success,
                cx,
            );
        }
        cx.notify();
    }

    /// 行为记录开关。只影响本机埋点，不影响记账与报告。
    pub(crate) fn toggle_behavior_logging(&mut self, cx: &mut Context<Self>) {
        let next = !self.behavior_enabled;
        let settings = crate::butler::events::BehaviorSettings { enabled: next };
        if let Err(error) = settings.save() {
            self.show_toast(format!("行为记录设置失败：{error:#}"), ToastTone::Error, cx);
            return;
        }
        self.behavior_enabled = next;
        self.show_toast(
            if next {
                "已开启本地行为记录"
            } else {
                "已关闭本地行为记录"
            }
            .to_owned(),
            ToastTone::Success,
            cx,
        );
        cx.notify();
    }

    /// 管家主动开口开关。关掉后唤醒与自整理照常，只是不再产生通知。
    pub(crate) fn toggle_butler_proactive(&mut self, cx: &mut Context<Self>) {
        let next = !self.butler_proactive;
        // 守护进程在跑时 `state.json` 归它写：GUI 只发意图，不落盘——两个
        // 进程同时写一份 JSON 会互相覆盖。
        if cx.global::<GuiState>().butler_daemon {
            let signal = crate::ipc::Signal::SetProactive { enabled: next };
            if crate::ipc::poke("mns-butlerd", &signal).is_ok() {
                self.butler_proactive = next;
                self.show_toast(
                    if next {
                        "管家已开启主动开口"
                    } else {
                        "管家已停止主动开口"
                    }
                    .to_owned(),
                    ToastTone::Success,
                    cx,
                );
                cx.notify();
                return;
            }
            // 连接刚断、`Disconnected` 事件还没到：当作没有守护进程处理。
            let state = cx.global_mut::<GuiState>();
            state.butler_daemon = false;
            state.butler_daemon_conn = None;
        }

        let _guard = crate::butler::writer_lock();
        let mut butler = match crate::butler::Butler::load() {
            Ok(butler) => butler,
            Err(error) => {
                self.show_toast(
                    format!("管家状态读取失败：{error:#}"),
                    ToastTone::Error,
                    cx,
                );
                return;
            }
        };
        butler.state.policy.enabled = next;
        // 只写状态文件：这里只动了开关，顺手重写记忆与任务会把另一个进程
        // 刚写进去的条目覆盖回旧值。
        if let Err(error) = butler.save_state() {
            self.show_toast(format!("管家设置保存失败：{error:#}"), ToastTone::Error, cx);
            return;
        }
        self.butler_proactive = next;
        self.show_toast(
            if next {
                "管家已开启主动开口"
            } else {
                "管家已停止主动开口"
            }
            .to_owned(),
            ToastTone::Success,
            cx,
        );
        cx.notify();
    }

    /// 清除行为记录。只删 `agent/events.jsonl`，不碰账本、报告与助手存档。
    pub(crate) fn clear_behavior_log(&mut self, cx: &mut Context<Self>) {
        match crate::butler::events::clear() {
            Ok(()) => self.show_toast("已清除本地行为记录".to_owned(), ToastTone::Success, cx),
            Err(error) => self.show_toast(format!("清除失败：{error:#}"), ToastTone::Error, cx),
        }
        cx.notify();
    }

    /// 启动补更新。定时任务只在进程存活期间触发；如果当天已过设定时间、
    /// 且今天还没有任何更新，就趁启动补一次，避免开机较晚时一整天都停在
    /// 昨天的价格。
    fn startup_catch_up(&mut self, cx: &mut Context<Self>) {
        let now = chrono::Local::now();
        let target = self.agent_settings.daily_review_time.trim().to_owned();
        if target.is_empty() || now.format("%H:%M").to_string().as_str() < target.as_str() {
            return;
        }
        let today = now.format("%Y-%m-%d").to_string();
        // 今天的巡检已经跑过：它内部执行 `mns today`，价格已是最新。
        if self.last_daily.as_deref() == Some(today.as_str()) {
            return;
        }
        if self.agent_settings.daily_review {
            // 巡检链路会先更新价格再生成报告，这里直接补跑，不必等下一次
            // 分钟轮询。
            self.maybe_start_daily(cx);
            return;
        }
        // 没有开启巡检时，只补价格与情绪数据（不生成报告、不调用 Agent）。
        if let Err(error) = self.jobs.send(Request::CatchUpRefresh { target }) {
            self.show_toast(error, ToastTone::Error, cx);
        }
    }

    pub(crate) fn save_daily_time(&mut self, cx: &mut Context<Self>) {
        let raw = self.daily_time.read(cx).value().trim().to_owned();
        if !valid_clock_time(&raw) {
            self.show_toast(
                "时间格式应为 HH:MM，例如 08:30".to_owned(),
                ToastTone::Error,
                cx,
            );
            cx.notify();
            return;
        }
        self.agent_settings.daily_review_time = raw;
        self.persist_daily_setting(cx);
    }

    fn persist_daily_setting(&mut self, cx: &mut Context<Self>) {
        match save_settings(&self.agent_settings) {
            Ok(()) => {
                self.show_toast(
                    format!(
                        "每日巡检：{}，时间 {}",
                        if self.agent_settings.daily_review {
                            "已开启"
                        } else {
                            "已关闭"
                        },
                        self.agent_settings.daily_review_time
                    ),
                    ToastTone::Success,
                    cx,
                );
            }
            Err(error) => {
                self.show_toast(error, ToastTone::Error, cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn set_policy(&mut self, policy: PermissionPolicy, cx: &mut Context<Self>) {
        self.agent_settings.permission_policy = policy;
        if let Err(error) = save_settings(&self.agent_settings) {
            self.show_toast(error, ToastTone::Error, cx);
        }
        self.agent.push(
            EntryKind::System,
            format!("权限策略：{} — {}", policy.label(), policy.hint()),
        );
        cx.notify();
    }

    pub(crate) fn save_agent_settings(&mut self, cx: &mut Context<Self>) {
        self.persist_agent_settings(cx);
        match save_settings(&self.agent_settings) {
            Ok(()) => {
                // A model id opencode cannot resolve fails at the provider with
                // the same opaque quota/HTTP error shape, so say the shape out
                // loud here rather than letting the run discover it.
                let model = self.agent_settings.model.clone();
                let text = if !model.is_empty() && !model.contains('/') {
                    format!("已保存；模型名通常写作 provider/model，当前是「{model}」")
                } else {
                    "已保存".to_owned()
                };
                self.show_toast(text, ToastTone::Success, cx);
            }
            Err(error) => {
                self.show_toast(error, ToastTone::Error, cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn grant(&mut self, id: u64, option_id: String, cx: &mut Context<Self>) {
        let _ = self.agent_send(AgentCommand::Grant { id, option_id });
        self.agent.push(EntryKind::Permission, "已批准该操作");
        cx.notify();
    }

    pub(crate) fn deny(&mut self, id: u64, cx: &mut Context<Self>) {
        let _ = self.agent_send(AgentCommand::Deny { id });
        self.agent.push(EntryKind::Permission, "已拒绝该操作");
        cx.notify();
    }

    pub(crate) fn shutdown(&mut self, cx: &mut Context<Self>) {
        // Best-effort final flush: the worker is still alive at this point, so
        // the last (possibly streamed) turn survives an ordinary quit. Put the
        // live session back first — a replay must never be what gets saved.
        if self.viewing_history.take().is_some() {
            if let Some(live) = self.live_agent.take() {
                self.agent = *live;
            }
        }
        self.archive_current(cx);
        let _ = self.agent_send(AgentCommand::Shutdown);
    }

    fn apply_job_event(&mut self, event: JobEvent, cx: &mut Context<Self>) {
        match event {
            JobEvent::Loaded(Ok(snapshot)) => {
                self.data = Some(snapshot);
                self.load_error = None;
                self.loading = false;
                // 每次加载都刷新管家事实：这是唯一一次「账户画面变了」，而
                // 守护进程只读这份快照。没有账本事件时走定时巡检（`None`），
                // 它会被 `WakeScheduler` 按间隔拦下，不会多说话。
                let ledger = self.pending_ledger_wake.take();
                cx.defer(move |cx| run_butler_wake_from(cx, ledger));
            }
            JobEvent::Loaded(Err(error)) => {
                self.load_error = Some(error);
                self.loading = false;
            }
            JobEvent::Refreshed(Ok(outcome)) => {
                self.refreshing = false;
                let mut text = outcome.summary();
                if let Some(error) = &outcome.price_error {
                    text.push_str(&format!("；{error}"));
                }
                if let Some(error) = &outcome.fgi_error {
                    text.push_str(&format!("；情绪数据：{error}"));
                }
                self.show_toast(text, ToastTone::Success, cx);
            }
            JobEvent::Refreshed(Err(error)) => {
                self.refreshing = false;
                self.show_toast(error, ToastTone::Error, cx);
            }
            JobEvent::Saved(message) => {
                self.show_toast(message, ToastTone::Success, cx);
                self.dialog = None;
            }
            JobEvent::LedgerRecorded(at) => {
                // 记账成功后由随后的 `Loaded` 带着新快照触发一次判定：
                // 在这里立即跑会读到上一笔的余额。
                self.pending_ledger_wake = Some(at);
            }
            JobEvent::Initialized => {
                self.show_toast(
                    "已创建默认配置，可以直接开始记录".to_owned(),
                    ToastTone::Success,
                    cx,
                );
            }
            JobEvent::Failed(error) => {
                if let Some(dialog) = self.dialog.as_mut() {
                    let slot = match dialog {
                        Dialog::Trade(form) => &mut form.error,
                        Dialog::Guardrail(form) => &mut form.error,
                        Dialog::Cash(form) => &mut form.error,
                        Dialog::AddAsset(form) => &mut form.error,
                    };
                    *slot = Some(error);
                } else {
                    self.show_toast(error, ToastTone::Error, cx);
                }
            }
            JobEvent::Brief(Ok(report)) => {
                let now = chrono::Local::now();
                let date = now.format("%Y-%m-%d").to_string();
                // `weekday_label` uses 1..=7 (Monday..Sunday) to match the
                // `decision_weekday` config, while chrono counts from 0.
                let weekday = crate::app::weekday_label(now.weekday().num_days_from_monday() + 1);
                let cli = crate::agent::cli_path();
                let prompt = routine::daily_review_prompt(&date, weekday, &report, cli.as_deref());
                self.begin_session(
                    format!("每日巡检 · {date}"),
                    prompt,
                    GoalSource::DailyReview,
                    cx,
                );
            }
            JobEvent::Brief(Err(error)) => {
                self.agent.state = RunState::Failed;
                self.agent.open = false;
                self.agent.error = Some(error.clone());
                self.agent
                    .push(EntryKind::Error, format!("无法生成今日报告：{error}"));
            }
            JobEvent::FundSearch { keyword, result } => {
                // 只认当前这轮搜索：用户改词再搜后，上一轮慢响应直接丢弃。
                if let Some(Dialog::AddAsset(form)) = self.dialog.as_mut()
                    && form.searched.as_deref() == Some(keyword.as_str())
                {
                    form.searching = false;
                    match result {
                        Ok(rows) => {
                            form.results = rows;
                            form.error = None;
                        }
                        Err(error) => {
                            form.results.clear();
                            form.error = Some(error);
                        }
                    }
                }
            }
            JobEvent::Conversations(Ok(rows)) => {
                self.history = rows;
                self.history_loaded = true;
                self.history_error = None;
            }
            JobEvent::Conversations(Err(error)) => {
                self.history_loaded = true;
                self.history_error = Some(error.clone());
                self.show_toast(error, ToastTone::Error, cx);
            }
            JobEvent::ConversationOpened(Ok(record)) => {
                self.show_history_record(*record, cx);
            }
            JobEvent::ConversationOpened(Err(error)) => {
                self.show_toast(error, ToastTone::Error, cx);
            }
            JobEvent::ConversationNotice(Ok(message)) => {
                self.show_toast(message, ToastTone::Success, cx);
            }
            JobEvent::ConversationNotice(Err(error)) => {
                self.show_toast(error, ToastTone::Error, cx);
            }
        }
        cx.notify();
    }

    fn apply_agent_event(&mut self, event: AgentEvent, cx: &mut Context<Self>) {
        // Persist before folding into state: the card is rendered from the same
        // JSON, and an archive failure must not block the conversation.
        if let AgentEvent::Decision(report) = &event {
            let _ = self.jobs.send(Request::SaveDecision {
                body: report.raw.clone(),
            });
        }
        // Only events that add something to the visible stream should move the
        // scroll position; a pure state change must not disturb what is on screen.
        let grows = matches!(
            event,
            AgentEvent::Entry(_)
                | AgentEvent::Delta { .. }
                | AgentEvent::Decision(_)
                | AgentEvent::Permission(_)
                | AgentEvent::Failed { .. }
        );
        // A finished turn, a conclusion, a permission or a failure is the
        // natural archive boundary. Streaming deltas are deliberately left
        // out: one JSON write per token would drown the worker thread.
        let archive = matches!(
            event,
            AgentEvent::TurnFinished { .. }
                | AgentEvent::Finished { .. }
                | AgentEvent::Failed { .. }
                | AgentEvent::Decision(_)
                | AgentEvent::Permission(_)
        );
        if let Some(live) = self.live_agent.as_mut() {
            // The replay owns the visible chat surface. Runtime events keep
            // updating the parked live session instead of corrupting the
            // archived transcript the user is reading.
            live.apply(event);
            self.conversation_dirty = self.conversation_dirty || grows || archive;
            cx.notify();
            return;
        }
        self.agent.apply(event);
        if grows || archive {
            self.conversation_dirty = true;
        }
        if grows {
            self.follow_conversation(false);
        }
        if archive {
            self.archive_current(cx);
        }
        cx.notify();
    }

    pub(crate) fn open_dialog(&mut self, dialog: Dialog, cx: &mut Context<Self>) {
        self.dialog = Some(dialog);
        cx.notify();
    }

    pub(crate) fn close_dialog(&mut self, cx: &mut Context<Self>) {
        self.dialog = None;
        cx.notify();
    }

    pub(crate) fn submit_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialog.as_ref() else {
            return;
        };
        // F2：买入先取出表单参数做冷静期判定（触发则转入护栏层而不是直接
        // 落库）；其余表单解析成 Request。先取 owned 值再动 self，避免借用
        // 冲突。
        let buy_params: Option<Result<(String, String, String, f64), String>> = match dialog {
            Dialog::Trade(form) if form.side == TradeSide::Buy => {
                let code = form.code.read(cx).value().trim().to_owned();
                let shares_text = form.shares.read(cx).value().trim().to_owned();
                let price_text = form.price.read(cx).value().trim().to_owned();
                let shares = parse_number(&shares_text);
                let price = parse_number(&price_text);
                Some(match (code.is_empty(), shares, price) {
                    (true, _, _) => Err("请填写资产代码".to_owned()),
                    (_, None, _) => Err("份额必须是数字".to_owned()),
                    (_, _, None) => Err("价格必须是数字".to_owned()),
                    _ => Ok((
                        code,
                        shares_text,
                        price_text,
                        shares.unwrap_or(0.0) * price.unwrap_or(0.0),
                    )),
                })
            }
            _ => None,
        };

        if let Some(parsed) = buy_params {
            match parsed {
                Ok((code, shares_text, price_text, amount)) => {
                    self.open_cooldown_if_triggered(
                        code,
                        shares_text,
                        price_text,
                        amount,
                        window,
                        cx,
                    );
                }
                Err(message) => self.form_error(message, cx),
            }
            cx.notify();
            return;
        }

        let parsed: Result<Request, String> = match dialog {
            // 走到这里只剩卖出：买入在上面已经拦截。
            Dialog::Trade(form) => {
                let code = form.code.read(cx).value().trim().to_owned();
                let shares = parse_number(&form.shares.read(cx).value());
                let price = parse_number(&form.price.read(cx).value());
                match (code.is_empty(), shares, price) {
                    (true, _, _) => Err("请填写资产代码".to_owned()),
                    (_, None, _) => Err("份额必须是数字".to_owned()),
                    (_, _, None) => Err("价格必须是数字".to_owned()),
                    _ => Ok(Request::Sell {
                        code,
                        shares: shares.unwrap_or(0.0),
                        price: price.unwrap_or(0.0),
                    }),
                }
            }
            Dialog::Guardrail(form) => {
                let reason = form.reason.read(cx).value().trim().to_owned();
                if form.verdict.reason_required() && reason.chars().count() < 6 {
                    Err("计划外买入需要写下理由（至少 6 个字），供日后纪律复盘".to_owned())
                } else {
                    Ok(Request::Buy {
                        code: form.code.clone(),
                        shares: form.shares,
                        price: form.price,
                        note: form.verdict.build_note(&reason),
                    })
                }
            }
            Dialog::Cash(form) => {
                let amount = parse_number(&form.amount.read(cx).value());
                let note = form.note.read(cx).value().trim().to_owned();
                match amount {
                    Some(amount) if amount > 0.0 => Ok(match form.kind {
                        CashKind::Deposit => Request::Deposit {
                            amount,
                            note: (!note.is_empty()).then_some(note),
                        },
                        CashKind::Withdraw => Request::Withdraw {
                            amount,
                            note: (!note.is_empty()).then_some(note),
                        },
                    }),
                    _ => Err("金额必须是大于 0 的数字".to_owned()),
                }
            }
            Dialog::AddAsset(form) => {
                let (code, name, exchange_traded) = match &form.selected {
                    Some(candidate) => (
                        candidate.code.clone(),
                        candidate.name.clone(),
                        candidate.exchange_traded,
                    ),
                    None => (
                        form.code.read(cx).value().trim().to_owned(),
                        form.name.read(cx).value().trim().to_owned(),
                        false,
                    ),
                };
                // 类别不静默猜：建议只高亮 chip，提交必须由用户亲自选。
                if code.is_empty() {
                    Err("请搜索并选择一只基金，或直接填写代码".to_owned())
                } else if exchange_traded {
                    // 账本只记场外基金：场内 ETF/LOF 走证券账户，成本、份额、
                    // 分红都不在 MNS 的口径里，硬塞进来只会让偏离算错。
                    Err(
                        "MNS 只记场外基金。场内 ETF/LOF 请在证券账户单独管理，\
                         这里可以添加它对应的场外联接基金"
                            .to_owned(),
                    )
                } else if name.is_empty()
                    && !(code.len() == 6 && code.chars().all(|c| c.is_ascii_digit()))
                {
                    // 6 位基金代码的名称允许留空，由 worker 自动带出；
                    // 其它代码（如美股 ETF）没有可解析的名称来源。
                    Err("非 6 位基金代码需要填写名称".to_owned())
                } else {
                    match form.category {
                        Some(category) => Ok(Request::AddAsset {
                            code,
                            name,
                            category: category.to_owned(),
                        }),
                        None => Err("请选择归属类别：美股 / A 股 / 逆周期".to_owned()),
                    }
                }
            }
        };

        match parsed {
            Ok(request) => {
                if let Err(error) = self.jobs.send(request) {
                    self.form_error(error, cx);
                }
                // The dialog stays open until `Event::Saved` closes it, so a
                // rejected write never discards what the user typed.
            }
            Err(message) => self.form_error(message, cx),
        }
        cx.notify();
    }

    fn form_error(&mut self, message: String, cx: &mut Context<Self>) {
        if let Some(dialog) = self.dialog.as_mut() {
            let slot = match dialog {
                Dialog::Trade(form) => &mut form.error,
                Dialog::Guardrail(form) => &mut form.error,
                Dialog::Cash(form) => &mut form.error,
                Dialog::AddAsset(form) => &mut form.error,
            };
            *slot = Some(message.clone());
        }
        self.show_toast(message, ToastTone::Error, cx);
    }

    /// F2：买入提交后的冷静期判定。
    ///
    /// 触发护栏 → 转入冷静期层（保留已填参数）；未触发或快照还没加载好
    /// （无计划/无情绪数据）→ 不判定、不拦人，直接落库，note 为 None，
    /// 行为与护栏上线前一致。
    fn open_cooldown_if_triggered(
        &mut self,
        code: String,
        shares_text: String,
        price_text: String,
        amount: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let shares = parse_number(&shares_text).unwrap_or(0.0);
        let price = parse_number(&price_text).unwrap_or(0.0);
        let Some(data) = self.data.as_ref() else {
            self.send_buy(&code, shares, price, None, cx);
            return;
        };
        let verdict = crate::app::evaluate_buy_guardrail(
            &code,
            amount,
            data.plan.as_ref(),
            data.rebalance.as_ref(),
            data.config.min_trade_amount,
            data.guardrail_hot.clone(),
        );
        if !verdict.triggered() {
            self.send_buy(&code, shares, price, None, cx);
            return;
        }
        // 护栏命中本身就是「计划外行为」信号：管家不阻断，但要记得发生过。
        let _ = crate::butler::events::append(&crate::butler::events::BehaviorEvent::new(
            crate::butler::events::kind::GUARD_HIT,
            serde_json::json!({ "code": code.clone(), "amount": amount }),
        ));
        let reason = cx.new(|cx| InputState::new(window, cx).placeholder("这笔买入和计划的关系？"));
        self.open_dialog(
            Dialog::Guardrail(GuardrailForm {
                code,
                shares,
                price,
                shares_text,
                price_text,
                amount,
                verdict,
                reason,
                error: None,
            }),
            cx,
        );
    }

    /// 冷静期层判定通过后的落库入口。
    fn send_buy(
        &mut self,
        code: &str,
        shares: f64,
        price: f64,
        note: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self.jobs.send(Request::Buy {
            code: code.to_owned(),
            shares,
            price,
            note,
        }) {
            self.form_error(error, cx);
        }
        // 对话框保持打开，直到 Event::Saved 再关——落库被拒时不丢用户输入。
    }

    /// F2：冷静期层「返回修改」——用已填参数回填买入表单，不让用户重打。
    pub(crate) fn cooldown_back_to_trade(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Dialog::Guardrail(form)) = self.dialog.as_ref() else {
            return;
        };
        let (code, shares_text, price_text) = (
            form.code.clone(),
            form.shares_text.clone(),
            form.price_text.clone(),
        );
        let code_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("资产代码，例如 007605")
                .default_value(code)
        });
        let shares = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("例如 1669.62")
                .default_value(shares_text)
        });
        let price = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("例如 1.2018")
                .default_value(price_text)
        });
        self.open_dialog(
            Dialog::Trade(TradeForm {
                side: TradeSide::Buy,
                code: code_input,
                shares,
                price,
                error: None,
            }),
            cx,
        );
    }
}

fn parse_number(raw: &str) -> Option<f64> {
    let cleaned = raw.trim().replace(',', "");
    if cleaned.is_empty() {
        return None;
    }
    cleaned.parse::<f64>().ok()
}

thread_local! {
    /// The shell is the only consumer of the agent handle, but GPUI views are
    /// created after the runtime. A thread-local keeps the handle reachable
    /// from page callbacks without threading it through every signature.
    static AGENT: std::cell::RefCell<Option<AgentHandle>> = const { std::cell::RefCell::new(None) };
}

pub fn run() {
    // 登录自启动带 `--tray`：只常驻托盘，不打开窗口。手动启动保持原行为。
    let background = autostart::launched_in_background();
    // 点通知拉起 / 守护进程 `launch_gui`：用户要的是管家会话，不是主界面。
    let open_butler_at_start = std::env::args().any(|arg| arg == "--butler");

    // 单实例：常驻托盘的程序被重复双击时，用户要的是「把已有窗口提到前面」，
    // 不是第二个进程。后启动的进程通知 Primary 后直接退出；`--butler` 的
    // 通知会落到「打开管家窗口」那条事件上，而不是只把主界面提到前面。
    let intent = if open_butler_at_start {
        crate::single_instance::Intent::OpenButler
    } else {
        crate::single_instance::Intent::Open
    };
    let instance = match crate::single_instance::acquire_with_intent("mns-gui", intent) {
        crate::single_instance::Acquired::Primary(guard) => guard,
        crate::single_instance::Acquired::AlreadyRunning => {
            // 命名事件是主路径（Primary 还没建好事件或跑的是旧版本时可能落空），
            // 本地通道再补一枪：新版本对重复的 OpenButler 只会把已有管家窗口
            // 置前，不会开出第二个。普通启动保持只走事件，行为与之前一致。
            if open_butler_at_start {
                let _ = crate::ipc::poke("mns-gui", &crate::ipc::Signal::OpenButler);
            }
            return;
        }
    };

    // Finder 启动时进程 cwd 是 `/`（macOS 系统卷只读）。所有自有路径都已
    // 改为绝对解析，这里再把 cwd 固定到应用数据目录，让外部 ACP agent 及
    // 其他子进程继承到的一律是可写位置。
    let data_dir = crate::config::app_data_dir();
    let _ = std::fs::create_dir_all(&data_dir);
    let _ = std::env::set_current_dir(&data_dir);

    // 安装位置可能在升级后变化，启动时按当前 exe 路径刷新一次登录项；
    // 关闭状态则不写入任何系统项。
    if crate::agent::load_settings().launch_at_login
        && let Err(error) = autostart::set_enabled(true)
    {
        eprintln!("[mns-gui] 无法同步开机自启动：{error}");
    }

    let (agent, agent_events) = start_runtime();
    let (jobs, job_events) = Jobs::spawn();
    AGENT.with(|cell| *cell.borrow_mut() = Some(agent.clone()));

    // 本地唤醒通道：管家守护进程与「二次启动」都从这条流进来。监听失败
    // 只影响这两件事，不应挡住主流程（例如同一台机器上另一个用户的实例
    // 已经占住了通道）。
    let (ipc_listener, ipc_events) = match crate::ipc::listen("mns-gui") {
        Ok((listener, events)) => (Some(listener), Some(events)),
        Err(error) => {
            eprintln!("[mns-gui] 本地唤醒通道不可用：{error}");
            (None, None)
        }
    };
    let _ipc_listener = ipc_listener;

    // 管家守护进程：把「判定」放到独立进程里，主窗口关掉之后它继续守着
    // （账本事件、到点任务、定时巡检）。已经在跑时不重复启动。没有守护
    // 进程时（单文件开发构建、通道被占）才退回 GUI 内巡检。
    let daemon_present = ensure_butler_daemon();

    // GUI 重启后要立刻重新握手：守护进程上次连接已经断了，只有它主动
    // `Hello` 才知道「现在有 GUI 可以投递」。这条 poke 触发它重连。
    if daemon_present {
        let _ = crate::ipc::poke(
            "mns-butlerd",
            &crate::ipc::Signal::Wake { ledger: None },
        );
    }

    // `--butler` 冷启动同样把主窗口建成隐藏载体：用户要的是管家会话，
    // 不该先弹一次主界面。
    let hidden = background || open_butler_at_start;

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx| {
            gpui_kit::init(cx);
            theme::apply(cx);
            cx.set_global(GuiState {
                window: None,
                window_id: None,
                window_hidden: hidden,
                view: None,
                butler_window: None,
                butler_view: None,
                butler_notice: None,
                butler_daemon: daemon_present,
                butler_daemon_conn: None,
                tray: None,
                jobs: Some(jobs.clone()),
                snapshot: None,
                last_tray_brief: None,
            });

            // Outside macOS the traffic lights and ctrl-w really destroy the
            // window; clear the handle so `ensure_window` can rebuild it from
            // the tray. macOS hides instead, so its handle stays valid.
            cx.on_window_closed(|cx, closed| {
                let state = cx.global_mut::<GuiState>();
                // Replacing the hidden startup window must not clobber the new
                // visible one when the old window's close lands later.
                if state.window_id == Some(closed) {
                    state.window = None;
                    state.window_id = None;
                }
                if state
                    .butler_window
                    .as_ref()
                    .is_some_and(|handle| handle.window_id() == closed)
                {
                    state.butler_window = None;
                    state.butler_view = None;
                }
            })
            .detach();

            register_actions(cx);

            if let Err(error) = setup_tray(cx) {
                eprintln!("[mns-gui] tray unavailable: {error}");
            }
            let open_requests = notify::init();
            // 后台启动也创建窗口，只是不显示：定时巡检与托盘简报都依赖
            // 窗口里的界面状态，隐藏窗口让它们保持可用，又不会弹到用户面前。
            if let Err(error) = open_main_window(cx, None, !hidden) {
                eprintln!("[mns-gui] failed to open window: {error}");
                cx.quit();
                return;
            }
            refresh_tray(cx);

            if open_butler_at_start {
                open_butler_window(cx);
            }

            spawn_job_bridge(cx, job_events);
            spawn_agent_bridge(cx, agent_events);
            spawn_daily_scheduler(cx);
            spawn_butler_loop(cx);
            spawn_notification_clicks(cx, open_requests);
            if let Some(events) = ipc_events {
                spawn_ipc_bridge(cx, events);
            }
            // 单实例的「有人又启动了」信号：Windows 用命名事件（互斥体 + 事件
            // 在进程启动早期就判定完了，早于本地唤醒通道建好），macOS/Linux 由
            // 系统保证不重复启动。两者都不再靠 400ms 轮询。
            #[cfg(windows)]
            spawn_instance_watcher(cx, instance);
            #[cfg(not(windows))]
            drop(instance);
        });
}

/// 等待「又有人启动了 mns-gui」的信号，并按那次启动的意图唤起窗口。
///
/// `Intent::Open` 与托盘菜单「打开 MNS」走同一条路径：窗口被关到托盘时
/// `ensure_window` 会按快照重建一个可见窗口；窗口只是被最小化或被压在后面
/// 时 `Window::activate_window` 负责还原并置前（Windows 上 `App::activate`
/// 是空实现）。`Intent::OpenButler`（`mns-gui --butler`）直接打开管家窗口，
/// 与托盘里的「管家」入口共用 `open_butler_window`。
///
/// 等待发生在专用线程上，阻塞在内核事件里：命中才醒，不做周期轮询。
#[cfg(windows)]
fn spawn_instance_watcher(cx: &mut App, guard: crate::single_instance::Guard) {
    let (tx, mut requests) = tokio::sync::mpsc::unbounded_channel();
    let _ = std::thread::Builder::new()
        .name("mns-instance-watch".to_owned())
        .spawn(move || {
            while let Some(intent) = guard.wait_wakeup() {
                if tx.send(intent).is_err() {
                    return;
                }
            }
        });
    cx.spawn(async move |cx| {
        while let Some(intent) = requests.recv().await {
            let _ = cx.update(|cx| match intent {
                crate::single_instance::Intent::Open => {
                    ensure_window(cx);
                    if let Some(handle) = cx.global::<GuiState>().window.clone() {
                        let _ = handle.update(cx, |_, window, _| window.activate_window());
                    }
                    cx.activate(true);
                }
                crate::single_instance::Intent::OpenButler => open_butler_window(cx),
            });
        }
    })
    .detach();
}

/// Minute-resolution scheduler for the unattended review.
///
/// A real timer loop beats a launch hook: the app may stay open for days, and
/// the machine may sleep through the target minute. The check is idempotent —
/// `maybe_start_daily` keeps one date stamp per process — so a coarse poll is
/// safer than a precise one-shot that a sleep/wake cycle can swallow.
fn spawn_daily_scheduler(cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(60))
                .await;
            let _ = cx.update(|cx| {
                let view = cx.global::<GuiState>().view.clone();
                if let Some(view) = view {
                    view.update(cx, |shell, cx| {
                        shell.maybe_start_daily(cx);
                        // Coarse safety net for a long streamed answer: an
                        // interrupted turn never had a TurnFinished boundary.
                        if shell.conversation_dirty {
                            shell.archive_current(cx);
                        }
                    });
                }
                // "今日已触发" must change even on a day where the run was
                // skipped, so the minute tick refreshes the tray regardless.
                refresh_tray(cx);
            });
        }
    })
    .detach();
}

/// 管家巡检循环。与每日巡检分开：管家有自己的 ≥4h 间隔与开口频控，状态
/// 持久化在 `~/.mns/agent/butler/state.json`，重启不会重复开口。
fn spawn_butler_loop(cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(60))
                .await;
            let _ = cx.update(|cx| {
                // 到点任务是独立唤醒源：4 小时的巡检间隔不该决定一条「到点
                // 提醒」迟多久。有守护进程时这里会各自被它接管。
                for id in due_task_ids() {
                    run_butler_wake_with(cx, crate::butler::WakeSource::Task { id });
                }
                run_butler_wake(cx);
            });
        }
    })
    .detach();
}

/// 到点且还没被唤醒过的任务 id。去重由 `WakeScheduler` 兜底，这里只负责
/// 「有哪些任务到点了」。
fn due_task_ids() -> Vec<String> {
    crate::butler::tasks::TaskBoard::load()
        .map(|board| board.due(chrono::Local::now()))
        .unwrap_or_default()
        .into_iter()
        .map(|task| task.id)
        .collect()
}

/// 通知点击回调只置位，窗口由主线程打开——回调线程不能直接碰 GPUI。
fn spawn_notification_clicks(
    cx: &mut App,
    mut requests: tokio::sync::mpsc::UnboundedReceiver<()>,
) {
    cx.spawn(async move |cx| {
        // 挂起等事件，不做定时轮询：一次点击唤醒一次，空闲时零开销。
        while requests.recv().await.is_some() {
            cx.update(|cx| open_butler_window(cx));
        }
    })
    .detach();
}

/// 本地唤醒通道的消费端。
///
/// `Open` 来自二次启动（等价于点托盘里的「打开 MNS」），`OpenButler` 来自
/// 管家守护进程。连接断开会在 `Disconnected` 里体现——守护进程崩溃时这条
/// 事件立刻到达，不需要心跳阈值。
fn spawn_ipc_bridge(
    cx: &mut App,
    mut events: tokio::sync::mpsc::UnboundedReceiver<crate::ipc::Event>,
) {
    cx.spawn(async move |cx| {
        while let Some(event) = events.recv().await {
            match event {
                crate::ipc::Event::Signal(_, crate::ipc::Signal::Open) => {
                    let _ = cx.update(|cx| {
                        ensure_window(cx);
                        cx.activate(true);
                        refresh_tray(cx);
                    });
                }
                crate::ipc::Event::Signal(_, crate::ipc::Signal::OpenButler) => {
                    let _ = cx.update(|cx| open_butler_window(cx));
                }
                // 守护进程上线：它接手巡检与系统通知。GUI 让出通知 delegate，
                // 否则同一 bundle 里会有两个 delegate，点击落到谁家是未定义的。
                crate::ipc::Event::Signal(id, crate::ipc::Signal::Hello { role })
                    if role == "daemon" =>
                {
                    let _ = cx.update(|cx| {
                        eprintln!("[mns-gui] 管家守护进程上线，判定交给它");
                        cx.global_mut::<GuiState>().butler_daemon = true;
                        cx.global_mut::<GuiState>().butler_daemon_conn = Some(id);
                        // GUI 关着的时候守护进程可能已经开口过：补齐会话。
                        consume_pending(cx);
                    });
                }
                // 守护进程刚开口，来读 `pending.jsonl`。
                crate::ipc::Event::Signal(_, crate::ipc::Signal::Pending) => {
                    let _ = cx.update(|cx| consume_pending(cx));
                }
                crate::ipc::Event::Disconnected(id) => {
                    let _ = cx.update(|cx| {
                        let state = cx.global_mut::<GuiState>();
                        if state.butler_daemon_conn == Some(id) {
                            eprintln!("[mns-gui] 管家守护进程离线，判定回到本进程");
                            state.butler_daemon = false;
                            state.butler_daemon_conn = None;
                            run_butler_wake(cx);
                        }
                    });
                }
                crate::ipc::Event::Signal(_, _) | crate::ipc::Event::Connected(_) => {}
            }
        }
    })
    .detach();
}

/// 已消费到的 `pending.jsonl` 偏移。丢了就当 0，重读会去重。
fn read_pending_offset() -> u64 {
    std::fs::read_to_string(crate::butler::orchestrator::pending_offset_path())
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

fn write_pending_offset(offset: u64) {
    let _ = crate::butler::store::write_json(
        &crate::butler::orchestrator::pending_offset_path(),
        &offset,
    );
}

/// 把守护进程判定过的开口并进会话。
///
/// `conversation.json` 始终只有 GUI 一个写者：守护进程只往 `pending.jsonl`
/// 追加，投递与去重都在这里完成。
fn consume_pending(cx: &mut App) {
    let offset = read_pending_offset();
    let Ok((new_offset, notices)) = crate::butler::orchestrator::take_pending(offset) else {
        return;
    };
    if notices.is_empty() {
        return;
    }

    let now = chrono::Local::now();
    let open = cx.global::<GuiState>().butler_view.clone();
    let window_open = open.is_some();

    // 先落盘、再推进偏移。反过来写的话，会话保存失败就等于消息永久丢失：
    // 偏移已经越过它，下次也不会重投。
    let persisted = match open {
        Some(view) => {
            // 窗口开着：用户看得见，直接进会话并算已读。
            let saved = view.update(cx, |view, cx| {
                let mut saved = true;
                for notice in &notices {
                    saved &= view.receive_notice(notice);
                }
                cx.notify();
                saved
            });
            cx.global_mut::<GuiState>().butler_notice = None;
            saved
        }
        None => match crate::butler::Session::load() {
            Ok(mut session) => {
                for notice in &notices {
                    session.push(crate::butler::Message {
                        at: notice.at,
                        role: crate::butler::Role::Butler,
                        text: notice.draft.clone(),
                        reason: Some(notice.reason),
                        trigger: Some(notice.trigger.clone()),
                    });
                }
                match session.save() {
                    Ok(()) => true,
                    Err(error) => {
                        eprintln!("[mns-gui] butler session save failed: {error:#}");
                        false
                    }
                }
            }
            Err(error) => {
                // 存档读不出来：重投也读不出来，推进偏移避免每次启动都卡在
                // 同一条上；原文件不覆盖，用户仍可自行修复。
                eprintln!("[mns-gui] butler session unreadable: {error:#}");
                true
            }
        },
    };

    if !persisted {
        // 不推进偏移：下次启动重投这几条，总比静默丢掉强。
        return;
    }
    write_pending_offset(new_offset);

    if !window_open {
        if let Some(last) = notices.last() {
            cx.global_mut::<GuiState>().butler_notice = Some(last.draft.clone());
        }
        // 系统通知只能由本进程发（守护进程发会被系统拒绝），所以横幅在
        // 这里补。只对刚产生的开口弹：启动时补齐的积压不该再响一遍。
        if let Some(fresh) = notices
            .iter()
            .rev()
            .find(|notice| now.signed_duration_since(notice.at) < chrono::Duration::minutes(5))
        {
            crate::notify::send(&fresh.draft);
        }
    }
    refresh_tray(cx);
}

fn run_butler_wake(cx: &mut App) {
    run_butler_wake_with(cx, crate::butler::WakeSource::Scheduled)
}

/// 账本事件用时间戳（毫秒）定位，构造唤醒源后走同一条判定路径。账本事件
/// 不受定时巡检的 4 小时间隔约束——买完就该被看见，而不是等下一次巡检。
fn run_butler_wake_from(cx: &mut App, ledger: Option<String>) {
    let source = match ledger {
        Some(id) => crate::butler::WakeSource::Ledger { id },
        None => crate::butler::WakeSource::Scheduled,
    };
    run_butler_wake_with(cx, source)
}

fn run_butler_wake_with(cx: &mut App, source: crate::butler::WakeSource) {
    let now = chrono::Local::now();
    let facts = {
        let Some(view) = cx.global::<GuiState>().view.clone() else {
            return;
        };
        let shell = view.read(cx);
        // 首屏还没加载完时不判定：空账户不是「没有偏离」，是「还不知道」。
        if shell.data.is_none() {
            return;
        }
        butler::facts_for(&shell, now)
    };

    // 事实先落盘：守护进程只读这一份，不碰数据库。GUI 自己做判定时也写，
    // 因为「谁在判定」随时可能交接，事实不该有两份来源。
    if let Err(error) = crate::butler::orchestrator::write_facts(now, &facts) {
        eprintln!("[mns-gui] 管家事实落盘失败：{error:#}");
    }

    // 守护进程在跑：巡检归它，GUI 只负责算事实和显示。事实刚变过，叫它
    // 立刻看一眼——它自己 5 分钟一次的 tick 太慢，用户刚记完账不该再等。
    // 巡检间隔由它的 `WakeScheduler` 兜着，叫早了不会多说一句。
    if cx.global::<GuiState>().butler_daemon {
        let ledger = match &source {
            crate::butler::WakeSource::Ledger { id } => Some(id.clone()),
            _ => None,
        };
        if let Err(error) = crate::ipc::poke("mns-butlerd", &crate::ipc::Signal::Wake { ledger }) {
            eprintln!("[mns-gui] 管家守护进程暂不可达：{error}");
        }
        return;
    }

    // 读-改-写全程持锁：守护进程与 CLI 可能同时各跑一份判定，后保存的
    // 会把先保存的去重键覆盖回旧值。送达（通知、会话）在锁外——那些
    // 文件只有 GUI 一个写者。
    let report = {
        let _guard = crate::butler::writer_lock();
        let mut butler = match crate::butler::Butler::load() {
            Ok(butler) => butler,
            Err(error) => {
                eprintln!("[mns-gui] butler state unreadable: {error:#}");
                return;
            }
        };
        let report = butler.wake(&source, now, &facts);
        if report.wake == crate::butler::WakeVerdict::Wake
            && let Err(error) = butler.save()
        {
            eprintln!("[mns-gui] butler state save failed: {error:#}");
        }
        report
    };
    if report.wake != crate::butler::WakeVerdict::Wake {
        return;
    }

    let Some(crate::butler::SpeakDecision::Speak(candidate)) = report.speak else {
        return;
    };

    // 出站队列是「已授权开口」的审计记录：GUI 直接送达，也要留下同一行。
    if let Err(error) = crate::butler::orchestrator::enqueue_notice(now, &candidate) {
        eprintln!("[mns-gui] butler outbox append failed: {error:#}");
    }

    let open = cx.global::<GuiState>().butler_view.clone();
    match open {
        Some(view) => {
            // 窗口开着，用户看得见，直接进会话并算已读。
            view.update(cx, |view, cx| {
                view.receive(&candidate);
                cx.notify();
            });
            cx.global_mut::<GuiState>().butler_notice = None;
        }
        None => {
            // 窗口没开：落进会话存档但不标已读，托盘菜单作为可点击的提示面。
            if let Ok(mut session) = crate::butler::Session::load() {
                session.push(crate::butler::Message {
                    at: now,
                    role: crate::butler::Role::Butler,
                    text: candidate.draft.clone(),
                    reason: Some(candidate.reason),
                    trigger: Some(candidate.trigger.clone()),
                });
                if let Err(error) = session.save() {
                    eprintln!("[mns-gui] butler session save failed: {error:#}");
                }
            }
            cx.global_mut::<GuiState>().butler_notice = Some(candidate.draft.clone());
            // 托盘是第一提示面，系统通知负责把话送到用户眼前；点击通知
            // 回到管家窗口（`spawn_notification_clicks`）。
            notify::send(&candidate.draft);
        }
    }
    refresh_tray(cx);
}

fn open_butler_window(cx: &mut App) {
    // 守护进程可能在 GUI 关着时已经开口：先把待投递的条目并进会话，
    // 窗口打开时才能看到完整对话。
    consume_pending(cx);
    if let Some(handle) = cx.global::<GuiState>().butler_window.clone() {
        let _ = handle.update(cx, |_, window, _| window.activate_window());
    } else {
        let now = chrono::Local::now();
        let facts = cx
            .global::<GuiState>()
            .view
            .as_ref()
            .map(|view| butler::facts_for(&view.read(cx), now))
            .unwrap_or_default();
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("MNS · 管家".into()),
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::centered(size(px(760.), px(640.)), cx)),
            ..Default::default()
        };
        match gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| butler::ButlerView::new(facts, window, cx))
        }) {
            Ok((handle, view)) => {
                // 点击通知会让系统把整个 app 激活，隐藏的主窗口也会被一并还原；
                // 管家窗口必须自己拿到焦点，否则用户点的是「管家」却看到主界面。
                let _ = handle.update(cx, |_, window, _| window.activate_window());
                // 对话走 ACP：事件流要在拿到 Entity 之后才能挂，构造期还
                // 拿不到 weak。窗口关掉时 view 被 drop，runtime 随之 Shutdown。
                if let Some(mut events) = view.update(cx, |view, _| view.take_chat_events()) {
                    let weak = view.downgrade();
                    cx.spawn(async move |cx| {
                        while let Some(event) = events.recv().await {
                            if weak
                                .update(cx, |view, cx| view.on_chat_event(event, cx))
                                .is_err()
                            {
                                break;
                            }
                        }
                    })
                    .detach();
                }
                let state = cx.global_mut::<GuiState>();
                state.butler_window = Some(handle);
                state.butler_view = Some(view);
            }
            Err(error) => {
                eprintln!("[mns-gui] failed to open butler window: {error}");
                return;
            }
        }
    }

    // 打开即已读：托盘提示与未读数都清掉。
    if let Ok(mut session) = crate::butler::Session::load() {
        session.mark_read(chrono::Local::now());
        let _ = session.save();
    }
    cx.global_mut::<GuiState>().butler_notice = None;
    if let Some(view) = cx.global::<GuiState>().butler_view.clone() {
        view.update(cx, |_, cx| cx.notify());
    }
    refresh_tray(cx);
}

fn spawn_job_bridge(cx: &mut App, mut events: tokio::sync::mpsc::UnboundedReceiver<JobEvent>) {    cx.spawn(async move |cx| {
        while let Some(event) = events.recv().await {
            let _ = cx.update(|cx| {
                let view = cx.global::<GuiState>().view.clone();
                if let Some(view) = view {
                    view.update(cx, |shell, cx| shell.apply_job_event(event, cx));
                }
                // Refresh outside the view's update lease: the menu builder
                // reads the same entity on the way back in.
                refresh_tray(cx);
            });
        }
    })
    .detach();
}

fn spawn_agent_bridge(cx: &mut App, mut events: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>) {
    cx.spawn(async move |cx| {
        while let Some(event) = events.recv().await {
            let _ = cx.update(|cx| {
                let view = cx.global::<GuiState>().view.clone();
                if let Some(view) = view {
                    view.update(cx, |shell, cx| shell.apply_agent_event(event, cx));
                }
                // Both the menu and the menu-bar label read the view, so the
                // refresh must wait until the update lease above has ended.
                refresh_tray(cx);
            });
        }
    })
    .detach();
}

fn register_actions(cx: &mut App) {
    // GPUI Kit deliberately leaves cmd-q / cmd-w / alt-f4 to the application
    // (its own integration test asserts those bindings stay empty). Without an
    // explicit binding here the menu bar can render but nothing happens.
    let mut bindings = vec![KeyBinding::new(
        "escape",
        CloseAgentPanel,
        Some("agent-panel"),
    )];
    #[cfg(target_os = "macos")]
    bindings.extend([
        KeyBinding::new("cmd-w", CloseMainWindow, None),
        KeyBinding::new("cmd-q", QuitApplication, None),
        KeyBinding::new("cmd-r", RefreshData, None),
    ]);
    #[cfg(not(target_os = "macos"))]
    bindings.extend([
        KeyBinding::new("ctrl-w", CloseMainWindow, None),
        KeyBinding::new("ctrl-r", RefreshData, None),
        KeyBinding::new("alt-f4", CloseMainWindow, None),
    ]);
    cx.bind_keys(bindings);

    // set_menus reads the keymap to draw the shortcut column, so it must come
    // after the bindings above. On macOS the first menu is shown under the app
    // name regardless of the name passed here.
    #[cfg(target_os = "macos")]
    cx.set_menus(app_menus());

    cx.on_action(|_: &ShowMainWindow, cx| {
        // 从管家通知点进来时，用户要的是会话窗口，不是主界面：跳过还原主窗口，
        // 让 `spawn_notification_clicks` 那只手去开管家。
        if !notify::take_click_pending() {
            ensure_window(cx);
            cx.activate(true);
        }
        refresh_tray(cx);
    });
    cx.on_action(|_: &CloseMainWindow, cx| {
        // macOS convention for a menu-bar app: dismissing the window hides the
        // app, the tray stays alive and "打开 MNS" brings it back. Elsewhere
        // the window really closes; the tray is still the way back in.
        if cfg!(target_os = "macos") {
            cx.hide();
            return;
        }
        if let Some(handle) = cx.global::<GuiState>().window.clone() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    });
    cx.on_action(|_: &ShowAbout, cx| {
        ensure_window(cx);
        cx.activate(true);
        if let Some(view) = cx.global::<GuiState>().view.clone() {
            view.update(cx, |shell, cx| {
                shell.about_open = true;
                cx.notify();
            });
        }
    });
    cx.on_action(|_: &RefreshData, cx| {
        ensure_window(cx);
        cx.activate(true);
        if let Some(view) = cx.global::<GuiState>().view.clone() {
            view.update(cx, |shell, cx| shell.refresh(cx));
        }
    });
    cx.on_action(|_: &OpenPlanPage, cx| open_page(cx, Page::Plan));
    cx.on_action(|_: &OpenAgentPage, cx| open_page(cx, Page::Agent));
    cx.on_action(|_: &OpenButlerWindow, cx| open_butler_window(cx));
    cx.on_action(|_: &RunTodayReview, cx| {
        ensure_window(cx);
        cx.activate(true);
        if let Some(view) = cx.global::<GuiState>().view.clone() {
            view.update(cx, |shell, cx| {
                shell.navigate(Page::Agent, cx);
                shell.start_daily_review(true, cx);
            });
        }
    });
    cx.on_action(|_: &RunDefaultGoal, cx| {
        ensure_window(cx);
        cx.activate(true);
        if let Some(view) = cx.global::<GuiState>().view.clone() {
            view.update(cx, |shell, cx| shell.start_agent(cx));
        }
    });
    cx.on_action(|_: &StopCurrentTask, cx| {
        if let Some(view) = cx.global::<GuiState>().view.clone() {
            view.update(cx, |shell, cx| shell.stop_agent(cx));
        }
    });
    cx.on_action(|_: &OpenHelpCenter, _| {
        if let Err(error) = open_help_center(None) {
            eprintln!("启动 mns-help 失败：{error}");
        }
    });
    cx.on_action(|_: &StopButlerDaemon, _| {
        // 守护进程收到就退出；它的连接断开后 GUI 会收回通知与巡检。
        if let Err(error) = crate::ipc::poke("mns-butlerd", &crate::ipc::Signal::Shutdown) {
            eprintln!("[mns-gui] 没有在运行的管家进程：{error}");
        }
    });
    cx.on_action(|_: &QuitApplication, cx| {
        if let Some(view) = cx.global::<GuiState>().view.clone() {
            view.update(cx, |shell, cx| shell.shutdown(cx));
        }
        if let Some(tray) = cx.global::<GuiState>().tray.clone() {
            let _ = tray.close(cx);
        }
        cx.quit();
    });
}

/// Native macOS menu bar. Every item maps to an action registered above;
/// an item with no listener is exactly the "click does nothing" symptom.
#[cfg(target_os = "macos")]
fn app_menus() -> Vec<Menu> {
    vec![
        Menu::new("MNS").items([
            MenuItem::action("关于 MNS", ShowAbout),
            MenuItem::separator(),
            MenuItem::os_submenu("服务", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("退出 MNS", QuitApplication),
        ]),
        Menu::new("文件").items([
            MenuItem::action("打开主窗口", ShowMainWindow),
            MenuItem::separator(),
            MenuItem::action("关闭窗口", CloseMainWindow),
        ]),
        Menu::new("操作").items([
            MenuItem::action("更新数据", RefreshData),
            MenuItem::action("今日巡检", RunTodayReview),
            MenuItem::action("开始自主任务", RunDefaultGoal),
            MenuItem::separator(),
            MenuItem::action("本月计划", OpenPlanPage),
            MenuItem::action("助手结论", OpenAgentPage),
            MenuItem::action("管家", OpenButlerWindow),
            MenuItem::separator(),
            MenuItem::action("帮助中心", OpenHelpCenter),
        ]),
    ]
}

/// 确认管家守护进程在跑；不在就拉起来。返回「现在有人判定」。
///
/// 用本地通道探活而不是查进程名：同名进程可能属于另一个干净的环境（例如
/// 测试实例），认错了就会把判定权交空。
fn ensure_butler_daemon() -> bool {
    if crate::ipc::connect("mns-butlerd").is_ok() {
        return true;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(dir) = exe.parent() else {
        return false;
    };
    let daemon = dir.join(format!("mns-butlerd{}", std::env::consts::EXE_SUFFIX));
    if !daemon.exists() {
        // 单文件开发构建里可能没有这个二进制：退化成 GUI 内巡检，不报错。
        return false;
    }
    if let Err(error) = command(&daemon).spawn() {
        eprintln!("[mns-gui] 无法启动管家守护进程：{error}");
        return false;
    }
    // 等它开始监听：判定权交早了会在两边各说一次，交晚了会漏一轮。
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        if crate::ipc::connect("mns-butlerd").is_ok() {
            return true;
        }
    }
    false
}

fn open_page(cx: &mut App, page: Page) {
    ensure_window(cx);
    cx.activate(true);
    if let Some(view) = cx.global::<GuiState>().view.clone() {
        view.update(cx, |shell, cx| shell.navigate(page, cx));
    }
}

fn ensure_window(cx: &mut App) {
    if let Some(handle) = cx.global::<GuiState>().window.clone() {
        if !cx.global::<GuiState>().window_hidden {
            return;
        }
        // 登录自启动留下的隐藏窗口没有跨平台的“显示”接口：保留它的状态，
        // 移除后按可见参数重建，用户点击「打开 MNS」即可看到窗口。
        let snapshot = cx
            .global::<GuiState>()
            .view
            .as_ref()
            .map(|view| view.read(cx).agent_snapshot());
        {
            let state = cx.global_mut::<GuiState>();
            state.snapshot = snapshot;
            state.window = None;
            state.window_id = None;
            state.window_hidden = false;
        }
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
    let snapshot = cx.global::<GuiState>().snapshot.clone();
    if let Err(error) = open_main_window(cx, snapshot, true) {
        eprintln!("[mns-gui] failed to reopen window: {error}");
    }
}

// ─────────────────────────── Tray brief ───────────────────────────

/// What the tray can say without opening a window.
///
/// The tray is a read surface, not a second dashboard and not a control panel
/// for the agent runtime. Engine advice (derived from the rebalance band) and
/// the external agent's conclusion are separate fields: a model's opinion must
/// never read as a rule of the strategy, and neither is a timing signal.
#[derive(Clone, Debug, Eq, PartialEq)]
struct TrayBrief {
    /// Short label beside the icon in the menu bar.
    title: String,
    /// Full sentence for the native tooltip.
    tooltip: String,
    /// Calendar day of this brief; makes cross-midnight refresh deterministic
    /// even when every account number happens to be unchanged.
    day_label: String,
    /// Date + freshness, always shown so an old number cannot read as "today".
    as_of: String,
    /// Account facts, already formatted, in display order.
    facts: Vec<String>,
    /// Engine advice. Never a buy/sell timing call.
    advice: String,
    /// Daily review state.
    review: String,
    /// External agent conclusion, explicitly labelled.
    agent_note: Option<String>,
    /// Whether an agent run is active (controls the run/stop menu item).
    running: bool,
}

impl Default for TrayBrief {
    fn default() -> Self {
        Self {
            title: "MNS".to_owned(),
            tooltip: "Money Never Sleeps · 正在读取本地数据".to_owned(),
            day_label: "今日".to_owned(),
            as_of: "正在读取本地数据…".to_owned(),
            facts: Vec::new(),
            advice: "打开 MNS 查看账户状态".to_owned(),
            review: "每日巡检：未设置".to_owned(),
            agent_note: None,
            running: false,
        }
    }
}

/// The four states the tray can be in. A pure classification keeps the copy
/// below from silently disagreeing with the rules that produce it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AdviceKind {
    NeedSignal,
    NeedPlan,
    Rebalance,
    Hold,
}

fn tray_advice_kind(has_signal: bool, has_plan: bool, has_orders: bool) -> AdviceKind {
    if !has_signal {
        AdviceKind::NeedSignal
    } else if !has_plan {
        AdviceKind::NeedPlan
    } else if has_orders {
        AdviceKind::Rebalance
    } else {
        AdviceKind::Hold
    }
}

/// Title suffix + advice line for one state. Staleness only appends a caveat:
/// old data must never be upgraded into a fresh "no action" claim.
fn tray_advice_copy(
    kind: AdviceKind,
    plan: Option<&PlanView>,
    blocked: Option<&str>,
    stale: bool,
) -> (&'static str, String) {
    let caveat = if stale {
        "；数据非当日，建议先更新"
    } else {
        ""
    };
    let (label, mut text) = match kind {
        AdviceKind::NeedSignal => ("待更新", "尚无情绪数据，先更新数据才能生成计划".to_owned()),
        AdviceKind::NeedPlan => (
            "待设置",
            format!(
                "计划未生成：{}",
                blocked.unwrap_or("缺少生成计划所需的数据")
            ),
        ),
        AdviceKind::Rebalance => match plan {
            Some(plan) => (
                "待调仓",
                format!(
                    "再平衡计划：买入 {} / 卖出 {}",
                    money(plan.total_buy),
                    money(plan.total_sell)
                ),
            ),
            None => ("待设置", "再平衡计划缺少数据".to_owned()),
        },
        AdviceKind::Hold => {
            let band = plan.map(|plan| plan.band_pp).unwrap_or(0.0);
            (
                "无需动作",
                format!("偏离未触发 ±{:.1}pp，今天无需调仓", band),
            )
        }
    };
    // NeedSignal / NeedPlan are already instructions; a caveat would repeat them.
    if matches!(kind, AdviceKind::Rebalance | AdviceKind::Hold) {
        text.push_str(caveat);
    }
    (label, text)
}

/// A menu item is a single native line: control characters and long agent
/// prose would otherwise blow the menu's width open.
fn tray_line(text: &str, max: usize) -> String {
    let clean: String = text
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    let clean = clean.trim();
    if clean.chars().count() <= max {
        return clean.to_owned();
    }
    let mut shortened: String = clean.chars().take(max.saturating_sub(1)).collect();
    shortened.push('…');
    shortened
}

fn tray_review_line(
    agent: &AgentState,
    settings: &AgentSettings,
    last_daily: Option<&str>,
    today: &str,
) -> String {
    if agent.is_running() {
        return format!("巡检中：{}", tray_line(&agent.detail, 36));
    }
    if agent.state == RunState::Failed {
        return format!(
            "上次任务失败：{}",
            tray_line(agent.error.as_deref().unwrap_or("未知错误"), 36)
        );
    }
    if !settings.daily_review {
        return "每日巡检：未开启".to_owned();
    }
    let time = settings.daily_review_time.trim();
    if last_daily == Some(today) {
        format!("每日巡检 {} · 今日已触发", time)
    } else {
        format!("每日巡检 {} · 今日待触发", time)
    }
}

fn tray_brief(
    data: Option<&Snapshot>,
    load_error: Option<&str>,
    agent: &AgentState,
    settings: &AgentSettings,
    last_daily: Option<&str>,
    day_label: &str,
    today: &str,
) -> TrayBrief {
    let mut brief = TrayBrief::default();
    brief.day_label = day_label.to_owned();
    brief.running = agent.is_running();
    brief.review = tray_review_line(agent, settings, last_daily, today);
    brief.agent_note = agent
        .decision
        .as_ref()
        .map(|report| format!("助手结论：{}", tray_line(&report.headline(), 42)));

    if let Some(error) = load_error {
        brief.title = "MNS · 读取失败".to_owned();
        brief.as_of = "本地数据不可用".to_owned();
        brief.advice = format!("读取失败：{}", tray_line(error, 48));
        brief.tooltip = format!("Money Never Sleeps · {}", brief.advice);
        return brief;
    }

    let Some(data) = data else {
        return brief;
    };
    brief.as_of = format!(
        "数据截至 {} · {}",
        data.as_of.format("%Y-%m-%d"),
        data.freshness.summary()
    );

    brief.facts.push(format!(
        "总资产 {} · XIRR {}",
        money(data.total_assets),
        data.xirr.map(pct_signed).unwrap_or_else(|| "—".to_owned())
    ));
    if let Some(plan) = &data.plan {
        brief.facts.push(format!(
            "风险仓位 {} / 目标 {} · 偏离 {}",
            pct_points(plan.current_risk_pct),
            pct_points(plan.target_risk_pct),
            pp_signed(plan.current_risk_pct - plan.target_risk_pct),
        ));
    } else {
        brief
            .facts
            .push(format!("风险仓位 {}", pct_points(data.risk_weight_pct())));
    }
    if let Some(signal) = &data.signal {
        brief.facts.push(format!(
            "情绪 {:.0} {} · {} · 决策日 {}",
            signal.effective_score,
            signal.effective_zone,
            if signal.confirmed {
                "区间已确认"
            } else {
                "区间未确认"
            },
            data.config.decision_label,
        ));
    } else {
        brief
            .facts
            .push("尚无情绪数据，无法给出目标仓位".to_owned());
    }

    // Sunday prices are legitimately a day old, so staleness is a caveat on
    // the advice rather than a verdict that replaces it.
    let stale = data.freshness.price_is_stale()
        || data
            .freshness
            .fgi_age_days
            .map(|days| days >= 1)
            .unwrap_or(true);
    let kind = tray_advice_kind(
        data.signal.is_some(),
        data.plan.is_some(),
        data.plan.as_ref().is_some_and(|plan| plan.has_orders()),
    );
    let (label, advice) = tray_advice_copy(
        kind,
        data.plan.as_ref(),
        data.plan_blocked.as_deref(),
        stale,
    );
    brief.title = format!("MNS · {label}");
    brief.advice = advice;
    brief.tooltip = format!("{} · {}", brief.advice, brief.as_of);
    brief
}

fn setup_tray(cx: &mut App) -> gpui_tray::Result<()> {
    let tray = Tray::builder()
        .icon(tray_icon()?)
        .title("MNS")
        .tooltip("Money Never Sleeps · 正在读取本地数据")
        .on_activate(ShowMainWindow)
        .menu(build_tray_menu)
        .build(cx)?;
    cx.global_mut::<GuiState>().tray = Some(tray);
    Ok(())
}

/// The window may be hidden or closed; `GuiState` keeps the view alive, so the
/// tray still reads the same account state the dashboard would show.
fn current_tray_brief(cx: &mut App) -> TrayBrief {
    let now = chrono::Local::now();
    let today = now.format("%Y-%m-%d").to_string();
    const WEEKDAYS: [&str; 7] = ["一", "二", "三", "四", "五", "六", "日"];
    let weekday = WEEKDAYS[now.weekday().num_days_from_monday() as usize];
    let day_label = format!("今日 {} 周{}", now.format("%m-%d"), weekday);
    let Some(view) = cx.global::<GuiState>().view.clone() else {
        return TrayBrief::default();
    };
    let shell = view.read(cx);
    tray_brief(
        shell.data.as_ref(),
        shell.load_error.as_deref(),
        &shell.agent,
        &shell.agent_settings,
        shell.last_daily.as_deref(),
        &day_label,
        &today,
    )
}

/// Rebuild every tray surface together. `refresh_menu` alone would leave the
/// menu bar label and the tooltip showing yesterday's conclusion.
fn refresh_tray(cx: &mut App) {
    let Some(tray) = cx.global::<GuiState>().tray.clone() else {
        return;
    };
    let brief = current_tray_brief(cx);
    let _ = tray.set_title(Some(brief.title.clone()), cx);
    let _ = tray.set_tooltip(Some(brief.tooltip.clone()), cx);

    // The title/tooltip setters are cheap no-ops when unchanged; recompiling
    // the native menu is not, so it only happens on a real state change.
    let changed = cx.global::<GuiState>().last_tray_brief.as_ref() != Some(&brief);
    if changed {
        cx.global_mut::<GuiState>().last_tray_brief = Some(brief);
        let _ = tray.refresh_menu(cx);
    }
}

/// Disabled actions are the only way to put read-only text in a native menu.
fn tray_info(label: impl Into<SharedString>) -> MenuItem {
    MenuItem::action(label, NoAction).disabled(true)
}

fn build_tray_menu(cx: &mut App) -> Vec<MenuItem> {
    let brief = current_tray_brief(cx);

    let mut items = vec![
        tray_info(brief.day_label.clone()),
        tray_info(tray_line(&brief.as_of, 64)),
    ];
    for fact in &brief.facts {
        items.push(tray_info(tray_line(fact, 64)));
    }

    items.push(MenuItem::separator());
    items.push(tray_info("建议"));
    items.push(tray_info(tray_line(&brief.advice, 64)));
    items.push(MenuItem::action("打开本月计划", OpenPlanPage));

    items.push(MenuItem::separator());
    items.push(tray_info(tray_line(&brief.review, 64)));
    if let Some(note) = &brief.agent_note {
        items.push(MenuItem::action(tray_line(note, 64), OpenAgentPage));
    }
    if brief.running {
        items.push(MenuItem::action("停止当前任务", StopCurrentTask));
    } else {
        items.push(MenuItem::action("立即巡检", RunTodayReview));
    }

    items.push(MenuItem::separator());
    let notice = cx.global::<GuiState>().butler_notice.clone();
    if let Some(notice) = notice {
        items.push(MenuItem::action(
            format!("管家：{}", tray_line(&notice, 44)),
            OpenButlerWindow,
        ));
    } else {
        let unread = crate::butler::Session::load()
            .map(|session| session.unread())
            .unwrap_or(0);
        let label = if unread > 0 {
            format!("管家 · {unread} 条未读")
        } else {
            "打开管家".to_owned()
        };
        items.push(MenuItem::action(label, OpenButlerWindow));
    }
    items.push(MenuItem::separator());
    items.push(MenuItem::action("更新数据", RefreshData));
    items.push(MenuItem::action("打开 MNS", ShowMainWindow));
    items.push(MenuItem::action("帮助中心", OpenHelpCenter));
    items.push(MenuItem::action("关于 MNS", ShowAbout));
    // 守护进程在跑时给它一个明确的出口：关掉 GUI 它仍然常驻，用户想彻底
    // 停下来不该只能靠命令行 kill。
    if cx.global::<GuiState>().butler_daemon {
        items.push(MenuItem::action("停止管家进程", StopButlerDaemon));
    }
    items.push(MenuItem::action("退出 MNS", QuitApplication));
    items
}

/// Spawns the standalone `mns-help` viewer bundled next to this executable.
/// In dev builds the sibling lives in `target/<profile>/` alongside `mns-gui`.
///
/// `target` 是要直接翻到的那一页（`None` = 首页，或把已开着的窗口提到前面）。
/// 帮助中心是独立进程，关窗即退出，所以「已经在看」这件事只能靠子进程句柄判断：
/// Windows 上的 `mns-help` 自带单实例锁（第二次启动会唤醒已有窗口后自行退出），
/// macOS 上没有任何锁，没有这一层的话连点两次就会开出两个帮助窗口。
///
/// 窗口开着时没有别的通道能把参数送进去，因此**先写请求文件再判断**：
/// 冷启动那条路同时还会把 slug 放进 argv，热路径靠
/// [`crate::help::deeplink::Watcher`] 在 400ms 内把页面翻过去。
fn open_help_center(target: Option<&str>) -> std::io::Result<()> {
    use std::sync::Mutex;

    static VIEWER: Mutex<Option<std::process::Child>> = Mutex::new(None);

    // 空串表示「只置前」：顶栏的「帮助」按钮不该把用户从正在读的一页拽回首页。
    if let Err(error) = crate::help::deeplink::request(target.unwrap_or("")) {
        eprintln!("写入帮助跳页请求失败（窗口仍会打开）：{error}");
    }

    let mut slot = VIEWER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(child) = slot.as_mut() {
        // `try_wait` 同时负责回收已退出的子进程：返回 `None` 说明还活着。
        if child.try_wait()?.is_none() {
            return Ok(());
        }
    }

    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    *slot = Some(help_command(&dir, target).spawn()?);
    Ok(())
}

/// 冷启动时交给 `mns-help` 的命令行：目标页只能从 argv 这一条路进去
/// （窗口还没建，没人能读请求文件）。
fn help_command(dir: &std::path::Path, target: Option<&str>) -> std::process::Command {
    let name = if cfg!(target_os = "windows") {
        "mns-help.exe"
    } else {
        "mns-help"
    };
    let mut child = command(dir.join(name));
    if let Some(slug) = target {
        child.arg("--page").arg(slug);
    }
    child
}

fn tray_icon() -> gpui_tray::Result<Icon> {
    // Bitmaps are generated from `assets/app_icon.png` by `scripts/make-icons.swift`.
    // The menu bar renders ~22pt tall, so a larger bitmap would be scaled down by
    // the platform and lose the gauge entirely.
    #[cfg(target_os = "macos")]
    const SIZE: u32 = 22;
    #[cfg(not(target_os = "macos"))]
    const SIZE: u32 = 32;

    #[cfg(target_os = "macos")]
    let rgba: &[u8] = include_bytes!("../../assets/tray-22.rgba");
    #[cfg(not(target_os = "macos"))]
    let rgba: &[u8] = include_bytes!("../../assets/tray-32.rgba");

    Icon::from_rgba(rgba.to_vec(), SIZE, SIZE).build()
}

/// The app mark, embedded so the window never depends on the working directory.
pub(crate) fn brand_mark(size: f32) -> impl IntoElement {
    use std::sync::Arc;
    let image = Image::from_bytes(
        ImageFormat::Png,
        include_bytes!("../../assets/icon-128.png").to_vec(),
    );
    img(Arc::new(image)).w(px(size)).h(px(size)).flex_none()
}

fn open_main_window(cx: &mut App, restore: Option<ShellSnapshot>, visible: bool) -> Result<()> {
    let jobs = cx
        .global::<GuiState>()
        .jobs
        .clone()
        .expect("app worker must exist before the window opens");
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("MNS · 投资助手".into()),
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::centered(size(px(1180.), px(720.)), cx)),
        // 登录自启动时窗口只作为界面状态的载体，不显示、不抢焦点。
        focus: visible,
        show: visible,
        ..Default::default()
    };
    let (window_handle, view) = gpui_kit::open_window(options, cx, move |window, cx| {
        window.on_window_should_close(cx, |_window, cx| {
            if cfg!(target_os = "macos") {
                cx.hide();
                return false;
            }
            let snapshot = cx
                .global::<GuiState>()
                .view
                .as_ref()
                .map(|view| view.read(cx).agent_snapshot());
            let state = cx.global_mut::<GuiState>();
            state.snapshot = snapshot;
            state.window = None;
            state.window_id = None;
            true
        });
        let jobs_for_view = jobs.clone();
        cx.new(|cx| {
            let mut shell = AppShell::new(window, cx, jobs_for_view, restore);
            // Load the account immediately; the shell shows its skeleton until
            // the worker answers, never a fabricated zero balance. The archive
            // index is a separate, small directory read on the same worker.
            shell.reload();
            shell.load_history(cx);
            // 当天错过了定时更新且还没更新过时，启动即补一次。
            shell.startup_catch_up(cx);
            shell
        })
    })?;
    let state = cx.global_mut::<GuiState>();
    state.window = Some(window_handle.clone());
    state.window_id = Some(window_handle.window_id());
    state.view = Some(view);
    Ok(())
}

// ───────────────────────────── Rendering ─────────────────────────────

impl AppShell {
    fn header(&self, cx: &mut Context<Self>) -> Div {
        let freshness = self
            .data
            .as_ref()
            .map(|data| data.freshness.summary())
            .unwrap_or_else(|| "正在读取本地数据".to_owned());
        let status_color = if self.loading {
            TEXT_TERTIARY
        } else if self.load_error.is_some() {
            DANGER
        } else {
            SUCCESS
        };

        let mut refresh = Button::new("refresh-data").label(if self.refreshing {
            "正在更新…"
        } else {
            "更新数据"
        });
        if !self.refreshing {
            refresh = refresh.primary().on_click(cx.listener(|shell, _, _, cx| {
                shell.refresh(cx);
            }));
        } else {
            refresh = refresh.disabled(true);
        }

        div()
            .h_flex()
            .items_center()
            .gap(px(12.))
            .h(px(64.))
            .px(px(20.))
            .flex_none()
            .bg(rgb(SURFACE))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(brand_mark(44.))
            // A hairline marks where the mark ends and the page name begins,
            // so the two never read as one label.
            .child(div().w(px(1.)).h(px(30.)).flex_none().bg(rgb(DIVIDER)))
            .child(
                div()
                    .v_flex()
                    .gap(px(3.))
                    // The window title already names the product; here we only
                    // say where you are inside it. The `?` rides along with the
                    // page name: it answers "这一屏怎么用", which is a different
                    // question from the 帮助 button's "打开帮助中心".
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap(px(7.))
                            .child(
                                div()
                                    .text_size(px(17.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(self.page.label()),
                            )
                            .child(help_badge(self.page.help_link())),
                    )
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap(px(6.))
                            .child(dot(status_color))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(TEXT_TERTIARY))
                                    .child(freshness),
                            ),
                    ),
            )
            .child(div().flex_1())
            .child(
                Button::new("open-trade")
                    .label("记一笔")
                    .on_click(cx.listener(|shell, _, window, cx| {
                        shell.open_trade(TradeSide::Buy, String::new(), window, cx);
                    })),
            )
            .child(refresh)
            // 帮助中心紧跟「更新数据」：这里是最常用的两个「我现在该干什么」，
            // 一个是拉数据，一个是查这数据怎么读。
            .child(
                Button::new("open-help")
                    .label("帮助")
                    .icon(IconName::BookText)
                    .on_click(cx.listener(|_, _, _, _| {
                        if let Err(error) = open_help_center(None) {
                            eprintln!("打开帮助中心失败：{error}");
                        }
                    })),
            )
    }

    fn rail(&self, cx: &mut Context<Self>) -> Div {
        let mut items = Vec::new();
        for page in Page::ALL {
            let active = self.page == page;
            let mut item = div()
                .id(SharedString::from(format!("nav-{}", page.label())))
                .h_flex()
                .items_center()
                .relative()
                .gap(px(11.))
                .h(px(38.))
                .px(px(11.))
                .rounded(px(RADIUS_SM))
                .cursor_pointer()
                .child(
                    div()
                        .w(px(18.))
                        .h(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_none()
                        .text_size(px(16.))
                        .text_color(rgb(if active { ACCENT } else { TEXT_TERTIARY }))
                        .child(page.icon()),
                )
                .child(
                    div()
                        .text_size(px(13.5))
                        .font_weight(if active {
                            FontWeight::MEDIUM
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(rgb(if active { ACCENT } else { TEXT }))
                        .child(page.label()),
                )
                .on_click(cx.listener(move |shell, _, _, cx| shell.navigate(page, cx)));
            if active {
                // A tab on the leading edge, not just a tinted rectangle: the
                // eye should be able to find the current page from the margin.
                item = item.bg(rgb(SELECTED)).child(
                    div()
                        .absolute()
                        .left(px(0.))
                        .w(px(3.))
                        .h(px(18.))
                        .rounded(px(1.5))
                        .bg(rgb(ACCENT)),
                );
            } else {
                item = item.hover(|style| style.bg(rgb(HOVER)));
            }
            items.push(item);
        }

        let agent_running = self.agent.is_running();
        let agent_state = self.agent.state;

        div()
            .v_flex()
            .w(px(208.))
            .flex_none()
            .h_full()
            .bg(linear_gradient(
                180.,
                linear_color_stop(rgb(RAIL), 0.),
                linear_color_stop(rgb(0xf4f8fe), 1.),
            ))
            .border_r_1()
            .border_color(rgb(BORDER))
            .px(px(12.))
            .py(px(16.))
            .gap(px(2.))
            .child(
                div()
                    .px(px(11.))
                    .pb(px(10.))
                    .text_size(px(11.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(TEXT_TERTIARY))
                    .child("我的账户"),
            )
            .children(items)
            .child(div().flex_1())
            .child({
                let (pill_fg, pill_bg) = match agent_state {
                    RunState::Failed | RunState::Cancelled => (DANGER, DANGER_SOFT),
                    RunState::Finished => (SUCCESS, SUCCESS_SOFT),
                    RunState::Idle => (TEXT_TERTIARY, NEUTRAL_SOFT),
                    _ => (ACCENT, ACCENT_SOFT),
                };
                div()
                    .v_flex()
                    .relative()
                    .overflow_hidden()
                    .gap(px(9.))
                    .p(px(13.))
                    .rounded(px(RADIUS))
                    .bg(rgb(SURFACE))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .shadow(shadow_card())
                    .child(crate::gui::components::watermark(
                        IconName::Sparkles,
                        ACCENT,
                        66.,
                        0.10,
                    ))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .child("自主 Agent"),
                    )
                    .child(status_pill(
                        if agent_running {
                            "执行中"
                        } else {
                            agent_state.label()
                        },
                        pill_fg,
                        pill_bg,
                    ))
            })
    }

    fn content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match self.page {
            Page::Dashboard => pages::dashboard::render(self, cx),
            Page::Portfolio => pages::portfolio::render(self, cx),
            Page::Plan => pages::plan::render(self, cx),
            Page::Performance => pages::performance::render(self, cx),
            Page::Agent => pages::agent::render(self, cx),
            Page::Settings => pages::settings::render(self, cx),
        }
    }

    fn toast_layer(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let toast = self.toast.as_ref()?;
        let (fg, bg) = match toast.tone {
            ToastTone::Success => (SUCCESS, SUCCESS_SOFT),
            ToastTone::Error => (DANGER, DANGER_SOFT),
        };
        Some(
            div()
                .id(SharedString::from(format!("toast-{id}", id = toast.id)))
                .absolute()
                .bottom(px(20.))
                .right(px(20.))
                .max_w(px(520.))
                .px(px(14.))
                .py(px(10.))
                .rounded(px(RADIUS))
                .bg(rgb(bg))
                .border_1()
                .border_color(rgb(fg).opacity(0.25))
                .shadow(shadow_overlay())
                .text_size(px(12.5))
                .text_color(rgb(fg))
                .cursor_pointer()
                .on_click(cx.listener(|shell, _, _, cx| shell.dismiss_toast(cx)))
                .h_flex()
                .items_center()
                .gap(px(12.))
                .child(div().flex_1().child(toast.text.clone()))
                .child(
                    div()
                        .flex_none()
                        .text_size(px(15.))
                        .text_color(rgb(fg).opacity(0.55))
                        .child("×"),
                ),
        )
    }

    fn dialog_layer(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<Div> {
        let dialog = self.dialog.as_ref()?;
        let card = match dialog {
            Dialog::Trade(form) => trade_dialog(form, window, cx),
            Dialog::Guardrail(form) => guardrail_dialog(form, window, cx),
            Dialog::Cash(form) => cash_dialog(form, window, cx),
            Dialog::AddAsset(form) => asset_dialog(form, window, cx),
        };
        Some(
            div()
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .w_full()
                .h_full()
                .bg(rgb(0x000000).opacity(0.32))
                .flex()
                .items_center()
                .justify_center()
                .child(card),
        )
    }

    /// "关于 MNS" lives in the macOS app menu, which stays reachable while the
    /// window is hidden; `ensure_window` opens it before this panel appears.
    fn about_layer(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.about_open {
            return None;
        }
        let data_dir = crate::config::app_data_dir().display().to_string();
        let card = div()
            .v_flex()
            .w(px(420.))
            .gap(px(14.))
            .p(px(24.))
            .rounded(px(RADIUS_LG))
            .bg(rgb(SURFACE))
            .border_1()
            .border_color(rgb(BORDER))
            .shadow(shadow_overlay())
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap(px(12.))
                    .child(brand_mark(44.))
                    .child(
                        div()
                            .v_flex()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(17.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Money Never Sleeps"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(TEXT_TERTIARY))
                                    .child(format!("版本 {}", env!("CARGO_PKG_VERSION"))),
                            ),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(rgb(TEXT_SECONDARY))
                    .child("只记录已成交的交易，不连接券商、不代为下单。"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(TEXT_TERTIARY))
                    .child(format!("数据目录：{data_dir}")),
            )
            .child(
                div().h_flex().justify_end().pt(px(4.)).child(
                    Button::new("about-close")
                        .primary()
                        .label("好")
                        .on_click(cx.listener(|shell, _, _, cx| {
                            shell.about_open = false;
                            cx.notify();
                        })),
                ),
            );
        Some(
            div()
                // `on_click` lives on StatefulInteractiveElement, so the
                // overlay needs an id to become a stateful Div first.
                .id("about-overlay")
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .w_full()
                .h_full()
                .bg(rgb(0x000000).opacity(0.32))
                .flex()
                .items_center()
                .justify_center()
                .on_click(cx.listener(|shell, _, _, cx| {
                    shell.about_open = false;
                    cx.notify();
                }))
                .child(card),
        )
    }
}

impl AppShell {
    pub(crate) fn open_trade(
        &mut self,
        side: TradeSide,
        code: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let code_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("资产代码，例如 007605")
                .default_value(code.clone())
        });
        let shares = cx.new(|cx| InputState::new(window, cx).placeholder("例如 1669.62"));
        let price = cx.new(|cx| InputState::new(window, cx).placeholder("例如 1.2018"));
        if side == TradeSide::Buy {
            // 只记「打开了买入」，不记填了什么：管家关心的是冲动次数，
            // 不是用户输入的草稿内容。
            let _ = crate::butler::events::append(&crate::butler::events::BehaviorEvent::new(
                crate::butler::events::kind::BUY_DIALOG,
                serde_json::json!({ "code": code }),
            ));
        }
        self.open_dialog(
            Dialog::Trade(TradeForm {
                side,
                code: code_input,
                shares,
                price,
                error: None,
            }),
            cx,
        );
    }

    pub(crate) fn open_cash(
        &mut self,
        kind: CashKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let amount = cx.new(|cx| InputState::new(window, cx).placeholder("金额"));
        let note = cx.new(|cx| InputState::new(window, cx).placeholder("备注（可选）"));
        self.open_dialog(
            Dialog::Cash(CashForm {
                kind,
                amount,
                note,
                error: None,
            }),
            cx,
        );
    }

    pub(crate) fn open_add_asset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = cx.new(|cx| {
            InputState::new(window, cx).placeholder("输入代码或名称，例如 007605 / 沪深300")
        });
        let code = cx.new(|cx| InputState::new(window, cx).placeholder("例如 007605"));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("留空则按代码自动带出"));
        self.open_dialog(
            Dialog::AddAsset(AssetForm {
                query,
                searched: None,
                searching: false,
                results: Vec::new(),
                selected: None,
                code,
                name,
                category: None,
                error: None,
            }),
            cx,
        );
    }

    /// F5 搜索：只在 worker 里发网络请求，UI 线程不碰 IO。
    pub(crate) fn start_fund_search(&mut self, cx: &mut Context<Self>) {
        let keyword = {
            let Some(Dialog::AddAsset(form)) = self.dialog.as_mut() else {
                return;
            };
            let keyword = form.query.read(cx).value().trim().to_owned();
            if keyword.is_empty() {
                form.error = Some("先输入代码或名称".to_owned());
                cx.notify();
                return;
            }
            form.searching = true;
            form.searched = Some(keyword.clone());
            form.results.clear();
            form.selected = None;
            form.error = None;
            keyword
        };
        if let Err(error) = self.jobs.send(Request::SearchFunds { keyword }) {
            if let Some(Dialog::AddAsset(form)) = self.dialog.as_mut() {
                form.searching = false;
                form.error = Some(error);
            }
        }
        cx.notify();
    }

    /// 再次点同一行取消选择——用户需要一个不重开弹层的撤销动作。
    pub(crate) fn select_fund_candidate(
        &mut self,
        candidate: FundCandidate,
        cx: &mut Context<Self>,
    ) {
        let Some(Dialog::AddAsset(form)) = self.dialog.as_mut() else {
            return;
        };
        let same = form
            .selected
            .as_ref()
            .is_some_and(|current| current.code == candidate.code);
        form.selected = (!same).then_some(candidate);
        // 类别跟着标走走：换了候选就作废旧选择，避免「美股」标签留在
        // 一只 A 股指数基金上——这比多让用户点一次危险得多。
        if !same {
            form.category = None;
        }
        form.error = None;
        cx.notify();
    }

    pub(crate) fn clear_fund_candidate(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog::AddAsset(form)) = self.dialog.as_mut() else {
            return;
        };
        form.selected = None;
        form.error = None;
        cx.notify();
    }

    pub(crate) fn select_asset_category(&mut self, key: &'static str, cx: &mut Context<Self>) {
        let Some(Dialog::AddAsset(form)) = self.dialog.as_mut() else {
            return;
        };
        form.category = Some(key);
        form.error = None;
        cx.notify();
    }

    /// F7 空账户清单「收起」：只落一个 bool，不存步骤状态——清单本身永远
    /// 由数据派生（现金 / 持仓 / 报告），所以不存在「清单说我做完了但数据
    /// 说没做」这种第二事实源。
    pub(crate) fn dismiss_start_checklist(&mut self, cx: &mut Context<Self>) {
        self.agent_settings.hide_start_checklist = true;
        if let Err(error) = save_settings(&self.agent_settings) {
            self.show_toast(error, ToastTone::Error, cx);
        }
        cx.notify();
    }
}

fn dialog_shell(
    title: &'static str,
    subtitle: &'static str,
    body: Vec<AnyElement>,
    error: Option<String>,
) -> Div {
    let mut heading = div().v_flex().gap(px(3.)).child(
        div()
            .text_size(px(18.))
            .font_weight(FontWeight::SEMIBOLD)
            .child(title),
    );
    if !subtitle.is_empty() {
        heading = heading.child(
            div()
                .text_size(px(13.))
                .text_color(rgb(TEXT_SECONDARY))
                .child(subtitle),
        );
    }
    let mut card = div()
        .v_flex()
        .w(px(420.))
        .gap(px(14.))
        .p(px(22.))
        .rounded(px(RADIUS_LG))
        .bg(rgb(SURFACE))
        .border_1()
        .border_color(rgb(BORDER))
        .shadow(shadow_overlay())
        .child(heading)
        .children(body);
    if let Some(error) = error {
        card = card.child(
            div()
                .px(px(10.))
                .py(px(8.))
                .rounded(px(RADIUS_SM))
                .bg(rgb(DANGER_SOFT))
                .text_size(px(12.))
                .text_color(rgb(DANGER))
                .child(error),
        );
    }
    card
}

fn trade_dialog(form: &TradeForm, _window: &mut Window, cx: &mut Context<AppShell>) -> Div {
    dialog_shell(
        form.side.label(),
        "现金与持仓将同步更新",
        vec![
            field("代码", Input::new(&form.code)).into_any_element(),
            div()
                .h_flex()
                .gap(px(10.))
                .child(
                    div()
                        .flex_1()
                        .child(field("份额", Input::new(&form.shares))),
                )
                .child(div().flex_1().child(field("价格", Input::new(&form.price))))
                .into_any_element(),
        ],
        form.error.clone(),
    )
    .child(
        div()
            .h_flex()
            .justify_end()
            .gap(px(8.))
            .child(
                Button::new("trade-cancel")
                    .outline()
                    .label("取消")
                    .on_click(cx.listener(|shell, _, _, cx| shell.close_dialog(cx))),
            )
            .child(
                Button::new("trade-submit")
                    .primary()
                    .label(form.side.label())
                    .on_click(cx.listener(|shell, _, window, cx| shell.submit_dialog(window, cx))),
            ),
    )
}

/// F2 买入冷静期层。触发护栏时在这里把理由写下来；不阻断、不代做决策，
/// 理由带结构化前缀落库（计划外冷静：/过热冷静：），供 F3 纪律复盘分桶。
fn guardrail_dialog(form: &GuardrailForm, _window: &mut Window, cx: &mut Context<AppShell>) -> Div {
    let v = &form.verdict;
    let mut reasons: Vec<String> = Vec::new();
    if v.out_of_plan {
        reasons.push(format!("{} 不在本月计划内。", form.code));
    }
    if v.oversized && v.planned_amount > 0.0 {
        reasons.push(format!(
            "本次 ¥{:.2} 超过 {} 计划买入 ¥{:.2} 的 2 倍。",
            form.amount, form.code, v.planned_amount
        ));
    }
    if let Some(hot) = &v.hot {
        reasons.push(format!(
            "决策分数 {:.0} ≥ 过热阈值 {:.0}，建议延迟 {} 个交易日再执行。",
            hot.effective_score, hot.hot_threshold, hot.cooldown_days
        ));
    }

    let reason_label: &'static str = if v.reason_required() {
        "买入理由（必填，至少 6 个字）"
    } else {
        "买入理由（选填，记入流水）"
    };

    dialog_shell(
        "买入冷静期",
        "写下理由后继续记录",
        vec![
            div()
                .v_flex()
                .gap(px(6.))
                .children(reasons.into_iter().map(|line| {
                    div()
                        .text_size(px(12.5))
                        .text_color(rgb(TEXT_SECONDARY))
                        .child(line)
                }))
                .into_any_element(),
            field(reason_label, Input::new(&form.reason)).into_any_element(),
        ],
        form.error.clone(),
    )
    .child(
        div()
            .h_flex()
            .justify_end()
            .gap(px(8.))
            .child(
                Button::new("guardrail-back")
                    .outline()
                    .label("返回修改")
                    .on_click(cx.listener(|shell, _, window, cx| {
                        shell.cooldown_back_to_trade(window, cx);
                    })),
            )
            .child(
                Button::new("guardrail-confirm")
                    .primary()
                    .label("写下理由，继续记录")
                    .on_click(cx.listener(|shell, _, window, cx| shell.submit_dialog(window, cx))),
            ),
    )
}

fn cash_dialog(form: &CashForm, _window: &mut Window, cx: &mut Context<AppShell>) -> Div {
    let (title, subtitle) = match form.kind {
        CashKind::Deposit => ("入金", "不计入收益，只抬高本金"),
        CashKind::Withdraw => ("出金", "按回收计入 XIRR"),
    };
    dialog_shell(
        title,
        subtitle,
        vec![
            field("金额", Input::new(&form.amount)).into_any_element(),
            field("备注", Input::new(&form.note)).into_any_element(),
        ],
        form.error.clone(),
    )
    .child(
        div()
            .h_flex()
            .justify_end()
            .gap(px(8.))
            .child(
                Button::new("cash-cancel")
                    .outline()
                    .label("取消")
                    .on_click(cx.listener(|shell, _, _, cx| shell.close_dialog(cx))),
            )
            .child(
                Button::new("cash-submit")
                    .primary()
                    .label("确认")
                    .on_click(cx.listener(|shell, _, window, cx| shell.submit_dialog(window, cx))),
            ),
    )
}

/// 可点 chip：白底 + 描边。
///
/// 组件库的 compact 按钮在白色卡片上填充色与卡片几乎同色，用户看不出
/// 「这里能点」。类别三选一与行内动作改用这套自绘 chip；选中态换主色
/// 描边 + 主色浅底。
fn bordered_chip(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    fg: u32,
    bg: u32,
    border: u32,
) -> Stateful<Div> {
    div()
        .id(id)
        .h_flex()
        .items_center()
        .justify_center()
        .px(px(12.))
        .py(px(5.))
        .rounded(px(999.))
        .border_1()
        .border_color(rgb(border))
        .bg(rgb(bg))
        .text_size(px(12.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(fg))
        .cursor_pointer()
        .child(label.into())
}

fn asset_dialog(form: &AssetForm, _window: &mut Window, cx: &mut Context<AppShell>) -> Div {
    let mut body: Vec<AnyElement> = Vec::new();

    // ── 搜索 ────────────────────────────────────────────────────────
    body.push(
        div()
            .h_flex()
            .gap(px(8.))
            .items_end()
            .child(
                div()
                    .flex_1()
                    .child(field("搜索基金（代码或名称）", Input::new(&form.query))),
            )
            .child(
                Button::new("asset-search")
                    .compact()
                    .primary()
                    .label(if form.searching { "搜索中…" } else { "搜索" })
                    .disabled(form.searching)
                    .on_click(cx.listener(|shell, _, _, cx| shell.start_fund_search(cx))),
            )
            .into_any_element(),
    );

    if form.searching {
        body.push(caption("正在搜索…").into_any_element());
    } else if form.results.is_empty() {
        if form.searched.is_some() {
            body.push(caption("没有找到基金。换个代码或关键词试试。").into_any_element());
        }
    } else {
        let mut rows: Vec<AnyElement> = Vec::new();
        for (index, candidate) in form.results.iter().enumerate() {
            let active = form
                .selected
                .as_ref()
                .is_some_and(|selected| selected.code == candidate.code);
            let candidate = candidate.clone();
            let picked = candidate.clone();
            let row = div()
                .id(("asset-result", index))
                .h_flex()
                .items_center()
                .gap(px(8.))
                .px(px(8.))
                .py(px(6.))
                .rounded(px(RADIUS_SM))
                .bg(rgb(if active { ACCENT_SOFT } else { LAYER }))
                .border_1()
                .border_color(rgb(if active { ACCENT } else { BORDER }))
                .cursor_pointer()
                .on_click(
                    cx.listener(move |shell, _, _, cx| {
                        shell.select_fund_candidate(picked.clone(), cx);
                    }),
                )
                .child(
                    div()
                        .v_flex()
                        .flex_1()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .child(format!("{} {}", candidate.code, candidate.name)),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(TEXT_TERTIARY))
                                .child(format!(
                                    "{} · {}",
                                    candidate.fund_type, candidate.company
                                )),
                        ),
                )
                .child(
                    div()
                        .v_flex()
                        .items_end()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(11.5))
                                .font_family(mono_family())
                                .text_color(rgb(TEXT_SECONDARY))
                                .child(
                                    candidate
                                        .nav
                                        .map(|nav| format!("{nav:.4}"))
                                        .unwrap_or_else(|| "—".to_owned()),
                                ),
                        )
                        .child(if candidate.exchange_traded {
                            pill("场内·不收", WARNING, WARNING_SOFT)
                        } else {
                            pill("场外", TEXT_SECONDARY, NEUTRAL_SOFT)
                        }),
                );
            rows.push(row.into_any_element());
        }
        body.push(
            div()
                .id("asset-results")
                .v_flex()
                .gap(px(4.))
                .max_h(px(168.))
                .overflow_y_scroll()
                .children(rows)
                .into_any_element(),
        );
    }

    // ── 已选 / 建议 ─────────────────────────────────────────────────
    if let Some(candidate) = &form.selected {
        body.push(
            div()
                .h_flex()
                .items_center()
                .gap(px(8.))
                .px(px(10.))
                .py(px(8.))
                .rounded(px(RADIUS_SM))
                .bg(rgb(ACCENT_TINT))
                .border_1()
                .border_color(rgb(ACCENT_SOFT))
                .child(
                    div()
                        .v_flex()
                        .flex_1()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .child(format!("已选：{} {}", candidate.code, candidate.name)),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(TEXT_SECONDARY))
                                .child(if candidate.exchange_traded {
                                    "场外没有对应联接基金的话，这只就不能在 MNS 里跟踪。"
                                } else {
                                    candidate.hint
                                }),
                        ),
                )
                .child(
                    bordered_chip("asset-clear", "清除", TEXT_SECONDARY, SURFACE, BORDER).on_click(
                        cx.listener(|shell, _, _, cx| shell.clear_fund_candidate(cx)),
                    ),
                )
                .into_any_element(),
        );
    }

    // ── 类别：三选一，默认不选 ──────────────────────────────────────
    let suggested = form.selected.as_ref().and_then(|candidate| candidate.suggested);
    let blocked = form
        .selected
        .as_ref()
        .is_some_and(|candidate| candidate.exchange_traded);
    let mut chips: Vec<AnyElement> = Vec::new();
    for key in crate::app::CATEGORY_KEYS {
        let active = form.category == Some(key);
        let hint = suggested == Some(key);
        let label = if hint {
            format!("{} · 建议", category_label(key))
        } else {
            category_label(key).to_owned()
        };
        let (fg, bg, border) = if active {
            (ACCENT, ACCENT_SOFT, ACCENT)
        } else if hint {
            // 建议只是提示，不等于选中：浅底 + 主色字，但提交仍要用户亲点。
            (ACCENT, ACCENT_TINT, ACCENT_SOFT)
        } else {
            (TEXT, SURFACE, BORDER)
        };
        chips.push(
            bordered_chip(format!("asset-category-{key}"), label, fg, bg, border)
                .on_click(cx.listener(move |shell, _, _, cx| shell.select_asset_category(key, cx)))
                .into_any_element(),
        );
    }
    if !blocked {
        body.push(
            div()
                .v_flex()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(TEXT_SECONDARY))
                        .child("归属类别"),
                )
                .child(div().h_flex().gap(px(6.)).children(chips))
                .into_any_element(),
        );
    }

    // 场内不进账本：这里只解释原因，不给「仍要添加」的出口。
    if blocked {
        body.push(
            div()
                .v_flex()
                .gap(px(4.))
                .px(px(10.))
                .py(px(8.))
                .rounded(px(RADIUS_SM))
                .bg(rgb(WARNING_SOFT))
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(WARNING))
                        .child("这是场内标的（ETF/LOF），MNS 不收。"),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(WARNING))
                        .child("场内要在证券账户按市价交易，成本与份额都不在 MNS 的口径里。请改选它对应的场外联接基金。"),
                )
                .into_any_element(),
        );
    }

    // ── 手填：一等入口（选中候选时隐藏，避免两套输入打架） ──────────
    if form.selected.is_none() {
        body.push(
            div()
                .v_flex()
                .gap(px(8.))
                .child(caption("或直接填写。6 位基金代码可留空名称，保存时自动带出。"))
                .child(
                    div()
                        .h_flex()
                        .gap(px(8.))
                        .child(div().flex_1().child(field("代码", Input::new(&form.code))))
                        .child(div().flex_1().child(field("名称", Input::new(&form.name)))),
                )
                .into_any_element(),
        );
    }

    dialog_shell(
        "添加标的",
        "只记录成本，不代下单",
        body,
        form.error.clone(),
    )
    .child(
        div()
            .h_flex()
            .justify_end()
            .gap(px(8.))
            .child(
                Button::new("asset-cancel")
                    .outline()
                    .label("取消")
                    .on_click(cx.listener(|shell, _, _, cx| shell.close_dialog(cx))),
            )
            .child(
                Button::new("asset-submit")
                    .primary()
                    .label("添加")
                    .on_click(cx.listener(|shell, _, window, cx| shell.submit_dialog(window, cx))),
            ),
    )
}

fn field(label: &'static str, input: impl IntoElement) -> Div {
    div()
        .v_flex()
        .gap(px(5.))
        .child(
            div()
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(TEXT_SECONDARY))
                .child(label),
        )
        .child(input)
}

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = self.header(cx);
        let rail = self.rail(cx);
        let content = self.content(cx);
        let mood = crate::gui::mood::Mood::resolve(self.data.as_ref());

        div()
            .v_flex()
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .child(header)
            .child(
                div().h_flex().flex_1().min_h(px(0.)).child(rail).child(
                    div()
                        .v_flex()
                        .flex_1()
                        .h_full()
                        .relative()
                        .overflow_hidden()
                        .min_w(px(0.))
                        .min_h(px(0.))
                        .bg(linear_gradient(
                            178.,
                            linear_color_stop(rgb(mood.tint), 0.),
                            linear_color_stop(rgb(BG), 0.62),
                        ))
                        // A mood-coloured light leak along the top edge: the
                        // content column inherits the atmosphere from the hero
                        // instead of sitting in a separate white box.
                        .child(
                            div()
                                .absolute()
                                .top(px(0.))
                                .left(px(0.))
                                .w_full()
                                .h(px(2.))
                                .bg(linear_gradient(
                                    90.,
                                    linear_color_stop(rgb(mood.ink).opacity(0.0), 0.),
                                    linear_color_stop(rgb(mood.ink).opacity(0.30), 1.),
                                )),
                        )
                        .child(content),
                ),
            )
            .children(self.toast_layer(cx))
            .children(self.dialog_layer(window, cx))
            .children(self.about_layer(cx))
    }
}

#[cfg(test)]
mod tray_tests {
    // Explicit imports, not `use super::*`: gpui_kit re-exports an attribute
    // macro also named `test`, which would shadow the built-in `#[test]`.
    use super::{
        AdviceKind, AgentSettings, AgentState, PlanView, tray_advice_copy, tray_advice_kind,
        tray_line, tray_review_line,
    };

    fn plan(total_buy: f64, total_sell: f64, band_pp: f64) -> PlanView {
        PlanView {
            monthly: 3000.0,
            score: 50.0,
            target_risk_pct: 60.0,
            current_risk_pct: 58.0,
            band_pp,
            total_buy,
            total_sell,
            estimated_fees: 0.0,
            cash_constrained: false,
            legs: Vec::new(),
        }
    }

    #[test]
    fn advice_state_never_reports_orders_without_a_signal() {
        assert_eq!(tray_advice_kind(false, true, true), AdviceKind::NeedSignal);
        assert_eq!(tray_advice_kind(true, false, true), AdviceKind::NeedPlan);
        assert_eq!(tray_advice_kind(true, true, true), AdviceKind::Rebalance);
        assert_eq!(tray_advice_kind(true, true, false), AdviceKind::Hold);
    }

    #[test]
    fn hold_copy_states_the_band_and_flags_stale_data() {
        let plan = plan(0.0, 0.0, 4.0);
        let (label, fresh) = tray_advice_copy(AdviceKind::Hold, Some(&plan), None, false);
        assert_eq!(label, "无需动作");
        assert!(fresh.contains("±4.0pp"), "{fresh}");
        assert!(!fresh.contains("先更新"), "{fresh}");

        let (_, stale) = tray_advice_copy(AdviceKind::Hold, Some(&plan), None, true);
        assert!(stale.contains("数据非当日"), "{stale}");
    }

    #[test]
    fn rebalance_copy_is_about_amounts_not_timing() {
        let plan = plan(3000.0, 500.0, 4.0);
        let (label, text) = tray_advice_copy(AdviceKind::Rebalance, Some(&plan), None, false);
        assert_eq!(label, "待调仓");
        assert!(text.contains("再平衡计划"), "{text}");
        // The FGI is a position target, never a buy/sell trigger.
        assert!(!text.contains("看多"), "{text}");
        assert!(!text.contains("抄底"), "{text}");
    }

    #[test]
    fn missing_plan_explains_itself() {
        let (label, text) = tray_advice_copy(
            AdviceKind::NeedPlan,
            None,
            Some("未设置每月定投金额"),
            false,
        );
        assert_eq!(label, "待设置");
        assert!(text.contains("未设置每月定投金额"), "{text}");
    }

    #[test]
    fn menu_lines_stay_single_line_and_bounded() {
        assert_eq!(tray_line("  今日\n建议  ", 64), "今日 建议");
        let long = "建".repeat(80);
        let shortened = tray_line(&long, 20);
        assert_eq!(shortened.chars().count(), 20);
        assert!(shortened.ends_with('…'));
    }

    #[test]
    fn review_state_reflects_the_schedule() {
        let agent = AgentState::new(None);
        let mut settings = AgentSettings::default();
        settings.daily_review = true;
        settings.daily_review_time = "08:30".to_owned();
        assert_eq!(
            tray_review_line(&agent, &settings, None, "2026-11-09"),
            "每日巡检 08:30 · 今日待触发"
        );
        assert_eq!(
            tray_review_line(&agent, &settings, Some("2026-11-09"), "2026-11-09"),
            "每日巡检 08:30 · 今日已触发"
        );
        settings.daily_review = false;
        assert_eq!(
            tray_review_line(&agent, &settings, None, "2026-11-09"),
            "每日巡检：未开启"
        );
    }
}

#[cfg(test)]
mod help_link_tests {
    // 同上：显式导入，避免 `gpui_kit::test` 这个属性宏盖住内建的 `#[test]`。
    use super::{Page, help_command, help_links};
    use crate::help::content;

    /// 冷启动那条路得把目标页放进 argv：窗口还不存在，没人读得到请求文件。
    #[test]
    fn cold_start_passes_the_page_on_the_command_line() {
        let dir = std::path::Path::new("/tmp");
        let args = |target| -> Vec<String> {
            help_command(dir, target)
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(args(Some("desktop-plan")), ["--page", "desktop-plan"]);
        assert!(
            args(None).is_empty(),
            "不带目标页时不该塞参数，否则 mns-help 会把标志当成 slug"
        );
    }

    /// 每个 `?` 都必须落到一页真实存在的帮助内容上。
    ///
    /// 这类断链在运行时只表现为「点了没反应或跳首页」，肉眼很难归因；
    /// 而 slug 是纯字符串，编译器也管不着。测试是唯一会响的地方。
    #[test]
    fn every_badge_points_at_an_existing_help_page() {
        for page in Page::ALL {
            let link = page.help_link();
            assert!(
                content::page_id(link.slug).is_some(),
                "「{}」顶栏的问号指向不存在的帮助页 `{}`",
                page.label(),
                link.slug
            );
            assert!(!link.tip.is_empty(), "`{}` 的悬停提示不能为空", link.slug);
        }

        for link in help_links::ALL {
            assert!(
                content::page_id(link.slug).is_some(),
                "帮助链接 `{}` 指向不存在的页",
                link.slug
            );
            assert!(
                link.tip.starts_with("帮助："),
                "`{}` 的提示要说明会打开什么，当前为 `{}`",
                link.slug,
                link.tip
            );
        }
    }

    /// 顶栏问号是「这一屏怎么用」，应该落在桌面端章节；落到理财百科就说明
    /// 映射写错了（复盘页是唯一的例外：它没有同名文档）。
    #[test]
    fn page_badges_prefer_the_desktop_chapter() {
        for page in Page::ALL {
            let slug = page.help_link().slug;
            let in_desktop = content::page_id(slug)
                .is_some_and(|(chapter, _)| content::CHAPTERS[chapter].title == "桌面端使用");
            if page == Page::Performance {
                assert!(!in_desktop, "复盘页本就没有桌面端文档，不该硬指过去");
            } else {
                assert!(
                    in_desktop,
                    "「{}」顶栏的问号落到了 `{slug}`，不在桌面端章节",
                    page.label()
                );
            }
        }
    }
}
