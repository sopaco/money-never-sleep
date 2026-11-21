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
use crate::app::{BuyGuardrail, Snapshot};
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
        StartButlerDaemon,
        RestartButlerDaemon,
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

    /// 窗口子进程用 slug 传页：`mns-gui --window --page plan`。
    /// 中文标签也接受，方便 `MNS_GUI_PAGE` 这类旧入口继续工作。
    pub(crate) fn from_slug(slug: &str) -> Option<Self> {
        let slug = slug.trim();
        Self::ALL
            .into_iter()
            .find(|page| page.key() == slug || page.label() == slug)
    }

    pub(crate) fn slug(self) -> &'static str {
        self.key()
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
    fn new() -> Self {
        Self {
            state: RunState::default(),
            detail: "尚未提交目标".to_owned(),
            timeline: Vec::new(),
            streaming: None,
            pending: None,
            error: None,
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
    view: Option<Entity<AppShell>>,
    /// 管家独立窗口与视图。与主窗口各存一份句柄，关闭其中一个不影响另一个。
    butler_window: Option<AnyWindowHandle>,
    butler_view: Option<Entity<butler::ButlerView>>,
    /// 最近一条已授权但用户还没在窗口里看到的管家结论，用于托盘菜单。
    butler_notice: Option<String>,
    /// 管家守护进程是否在跑。在跑时巡检归它，宿主只负责送达与显示。
    butler_daemon: bool,
    /// 守护进程的监督状态：子进程句柄、自动重启计数与用户意图。
    butler_proc: ButlerSupervisor,
    /// 守护进程那条连接的 id；断开时要能辨认出是它，而不是别的临时连接。
    butler_daemon_conn: Option<u64>,
    /// 管家窗口进程是否连着宿主（它启动时握手，退出即断开）。
    butler_window_conn: Option<u64>,
    tray: Option<Tray>,
    jobs: Option<Jobs>,
    /// 托盘简报：数字由守护进程算好送来，宿主只排版（P2：宿主没有 AppShell）。
    brief: Option<crate::brief::BriefData>,
    /// Last brief actually pushed into the native menu. `refresh_menu` always
    /// bumps its generation, so an unchanged brief must not be pushed again or
    /// macOS rebuilds the NSMenu (and closes an open one) once per minute.
    last_tray_brief: Option<TrayBrief>,
    /// 当前托盘图标是否是红色告警态。只在状态翻转时换图，避免每分钟重设。
    tray_alert: bool,
    /// 本进程的本地通道监听器。它同时是单实例锁，必须活到进程退出——
    /// 放进启动闭包的局部变量会在闭包返回时被 drop，socket 随之被删掉，
    /// 之后所有 `poke` 都会失败。只作守卫持有，从不读取。
    _listener: Option<crate::ipc::Listener>,
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
        page: Option<Page>,
        about: bool,
    ) -> Self {
        let agent_settings = load_settings();
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
            page: page
                .or_else(|| {
                    std::env::var("MNS_GUI_PAGE")
                        .ok()
                        .and_then(|key| Page::from_slug(key.trim()))
                })
                .unwrap_or(Page::Dashboard),
            data: None,
            load_error: None,
            loading: true,
            refreshing: false,
            toast: None,
            toast_seq: 0,
            dialog: None,
            agent: AgentState::new(),
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
            about_open: about,
            history: Vec::new(),
            history_loaded: false,
            history_error: None,
            history_pending_delete: None,
            current_conversation: None,
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
            let live = std::mem::replace(&mut self.agent, AgentState::new());
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
                self.show_toast(format!("管家状态读取失败：{error:#}"), ToastTone::Error, cx);
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
                // 账本变了：叫守护进程立刻看一眼。判定与数字都在它那边，
                // 窗口只负责把事件送过去；它不在时没有任何本地兜底路径。
                let ledger = self.pending_ledger_wake.take();
                cx.defer(move |_cx| {
                    let _ = crate::ipc::poke("mns-butlerd", &crate::ipc::Signal::Wake { ledger });
                });
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
                let cli = crate::agent::mns_bin();
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
                    Err("MNS 只记场外基金。场内 ETF/LOF 请在证券账户单独管理，\
                         这里可以添加它对应的场外联接基金"
                        .to_owned())
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

/// `mns-gui` 的进程角色。
///
/// P2 起一个二进制承担三种进程：
/// - `Host`：托盘宿主，无窗口，负责通知投递与管家守护进程生命周期；
/// - `MainWindow`：主窗口短命子进程，关窗即退；
/// - `ButlerWindow`：管家窗口短命子进程，关窗即退。
///
/// 窗口进程由宿主用 `--window` 拉起，自己不再常驻。宿主持有唯一的 Dock
/// 图标，窗口进程是 `Accessory`，所以开关窗口不会让 Dock 里出现第二个 MNS。
///
/// `show_main` 是「这次启动要不要顺手开一扇主窗」：无参数双击 = 是（首启
/// 必须看得见东西）；`--tray` / `--background`（登录项）= 否，只驻留；已有
/// 实例时的二次启动根本不进这里，由本地通道把意图交给宿主置前。
#[derive(Clone, Debug, PartialEq, Eq)]
enum Role {
    Host {
        open_butler: bool,
        show_main: bool,
    },
    MainWindow {
        page: Option<Page>,
        about: bool,
        refresh: bool,
    },
    ButlerWindow {
        notice: Option<String>,
    },
}

fn parse_role() -> Role {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|arg| arg == flag);
    let value_after = |flag: &str| {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|index| args.get(index + 1))
            .cloned()
    };

    if !has("--window") {
        let quiet = has("--tray") || has("--background") || has("--butler");
        return Role::Host {
            open_butler: has("--butler"),
            show_main: !quiet,
        };
    }

    let kind = value_after("--window");
    if has("--butler") || kind.as_deref() == Some("butler") {
        // `--notice <id>`：用户点系统通知进来，窗口要落到那一条开口上。
        return Role::ButlerWindow {
            notice: value_after("--notice"),
        };
    }
    Role::MainWindow {
        page: value_after("--page").as_deref().and_then(Page::from_slug),
        about: has("--about"),
        refresh: has("--refresh"),
    }
}

pub fn run() {
    // Finder 启动时进程 cwd 是 `/`（macOS 系统卷只读）。所有自有路径都已
    // 改为绝对解析，这里再把 cwd 固定到应用数据目录，让外部 ACP agent 及
    // 其他子进程继承到的一律是可写位置。
    let data_dir = crate::config::app_data_dir();
    let _ = std::fs::create_dir_all(&data_dir);
    let _ = std::env::set_current_dir(&data_dir);

    // Windows Toast 点击由系统按 `mns:` 协议拉起本程序：交给已在运行的宿主
    // 定位那条通知，然后把这次短命进程结束掉；宿主不在时按正常启动处理。
    #[cfg(target_os = "windows")]
    if let Some(notice) = crate::notify::toast_notice_arg() {
        if crate::ipc::poke(
            "mns-gui",
            &crate::ipc::Signal::NoticeFocus {
                notice_id: notice.clone(),
            },
        )
        .is_ok()
        {
            return;
        }
        run_host(true, false, Some(notice));
        return;
    }

    match parse_role() {
        Role::Host {
            open_butler,
            show_main,
        } => run_host(open_butler, show_main, None),
        Role::MainWindow {
            page,
            about,
            refresh,
        } => run_main_window(page, about, refresh),
        Role::ButlerWindow { notice } => run_butler_window(notice),
    }
}

/// 托盘宿主：没有 AppShell，也没有窗口。
///
/// 它只做四件事：Dock 入口与托盘界面、系统通知投递、管家守护进程启停、
/// 拉起短命窗口子进程。托盘上的数字向 `mns-butlerd` 要（`RequestBrief`），
/// 不再自己读库。宿主是进程组里唯一常驻的进程，也是唯一持有 Dock 图标的
/// 进程；窗口子进程用 `Accessory`，不会在 Dock 里多出一个 MNS。
fn run_host(open_butler_at_start: bool, show_main: bool, notice: Option<String>) {
    // 本地唤醒通道本身就是跨平台单实例锁：能监听 = 本进程是唯一宿主。
    // 绑定必须先于所有启动副作用，否则「还没 bind 就被判成 Primary」的
    // 竞态会让两个宿主同时活下来（Windows 上以前靠额外的一套互斥体兜底）。
    let (listener, events) = match crate::ipc::listen("mns-gui") {
        Ok(pair) => pair,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            // 已有宿主：把这次启动的意图交给它，自己退出。Toast 点击要带上
            // notice id，才能落在对应那条消息上而不是只开一个空窗。
            let signal = match &notice {
                Some(notice_id) => crate::ipc::Signal::NoticeFocus {
                    notice_id: notice_id.clone(),
                },
                None if open_butler_at_start => crate::ipc::Signal::OpenButler,
                None => crate::ipc::Signal::Open,
            };
            let _ = crate::ipc::poke("mns-gui", &signal);
            return;
        }
        Err(error) => {
            eprintln!("[mns-gui] 本地唤醒通道不可用：{error}");
            // 没有实例锁就不该再起第二个宿主，避免通知投递出现两个写者。
            return;
        }
    };

    // 安装位置可能在升级后变化，启动时按当前 exe 路径刷新一次登录项；
    // 关闭状态则不写入任何系统项。放在 bind 之后：抢到锁的才配改系统项。
    if crate::agent::load_settings().launch_at_login
        && let Err(error) = autostart::set_enabled(true)
    {
        eprintln!("[mns-gui] 无法同步开机自启动：{error}");
    }

    // 管家守护进程：把「判定」放到独立进程里，宿主退出后它继续守着。
    let (daemon_present, butler_child) = ensure_butler_daemon();

    let app = gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .with_quit_mode(QuitMode::Explicit);

    // macOS：应用已在运行时再次双击图标 / 从 Dock 点开会走 reopen，不会走
    // 单实例事件。点通知进来的那一次仍然优先去管家窗口。
    app.on_reopen(|cx| {
        if notify::take_click_pending() {
            match notify::recent_click_notice().flatten() {
                Some(notice_id) => open_butler_notice_from_host(cx, &notice_id),
                None => open_butler_from_host(cx),
            }
        } else {
            open_main_from_host(cx, None, false, false);
        }
    });

    app.run(move |cx| {
        gpui_kit::init(cx);
        theme::apply(cx);
        // 宿主是唯一持有 Dock 图标的进程（Regular）：点 Dock / 双击应用
        // 都会走 on_reopen 打开或置前主窗，Dock 右键退出也会完整收尾。
        // 窗口子进程在 run_main_window / run_butler_window 里显式设为
        // Accessory，否则会有两个 MNS Dock 图标。
        #[cfg(target_os = "macos")]
        cx.set_activation_policy(gpui_kit::ActivationPolicy::Regular);
        cx.set_global(GuiState {
            window: None,
            view: None,
            butler_window: None,
            butler_view: None,
            butler_notice: None,
            butler_daemon: daemon_present,
            butler_proc: ButlerSupervisor {
                child: butler_child,
                ..ButlerSupervisor::default()
            },
            butler_daemon_conn: None,
            butler_window_conn: None,
            tray: None,
            jobs: None,
            brief: None,
            last_tray_brief: None,
            tray_alert: false,
            _listener: Some(listener),
        });

        register_host_actions(cx);
        // 宿主是唯一常驻的 Regular 进程：窗口活着时 macOS 保留的正是
        // 宿主这份菜单栏，所以它要和窗口里的入口保持一致。
        #[cfg(target_os = "macos")]
        {
            cx.bind_keys([
                KeyBinding::new("cmd-q", QuitApplication, None),
                KeyBinding::new("cmd-w", CloseMainWindow, None),
                KeyBinding::new("cmd-r", RefreshData, None),
            ]);
            cx.set_menus(host_menus());
        }
        if let Err(error) = setup_tray(cx) {
            eprintln!("[mns-gui] tray unavailable: {error}");
            cx.quit();
            return;
        }

        let opens = notify::init();
        spawn_notification_clicks(cx, opens);
        spawn_ipc_bridge(cx, events);
        spawn_host_ticker(cx);
        // 系统发来的 terminate（macOS 的 Apple Event / Dock 右键退出）不
        // 经过我们的菜单：同样要收子进程并撤掉实例锁，否则窗口、守护
        // 进程和 socket 文件都会留在原地等人踩。
        cx.on_app_quit(|cx| {
            shutdown_children(cx);
            release_listener(cx);
            async {}
        })
        .detach();

        if let Some(notice_id) = notice {
            open_butler_notice_from_host(cx, &notice_id);
        } else if open_butler_at_start {
            open_butler_from_host(cx);
        } else if show_main {
            // 首启可见：双击应用（无参数）直接开一次主窗。登录项带
            // `--tray`，只驻留 Dock/托盘，不打扰。
            open_main_from_host(cx, None, false, false);
        }
        // 启动即要一份简报；守护进程不在时先显示「正在读取」，由下一次
        // tick 或用户点「启动管家进程」补齐。
        request_brief(cx);
        refresh_tray(cx);
    });
}

/// 主窗口短命子进程：`mns-gui --window [--page <slug>] [--about] [--refresh]`。
///
/// `QuitMode::LastWindowClosed` + 关窗即退：进程跟着窗口结束，操作系统回收
/// 全部内存，不存在「窗口关了但进程还在」的累积。
fn run_main_window(page: Option<Page>, about: bool, refresh: bool) {
    let (listener, events) = match crate::ipc::listen("mns-gui-window") {
        Ok(pair) => pair,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            // 窗口已经开着：把这次请求交给它，自己退出。
            let signal = match (page, about, refresh) {
                (Some(page), _, _) => crate::ipc::Signal::OpenPage {
                    page: page.slug().to_owned(),
                },
                (_, true, _) => crate::ipc::Signal::OpenAbout,
                (_, _, true) => crate::ipc::Signal::RefreshData,
                _ => crate::ipc::Signal::Open,
            };
            let _ = crate::ipc::poke("mns-gui-window", &signal);
            return;
        }
        Err(error) => {
            eprintln!("[mns-gui] 窗口实例锁不可用：{error}");
            return;
        }
    };

    let (agent, agent_events) = start_runtime();
    let (jobs, job_events) = Jobs::spawn();
    AGENT.with(|cell| *cell.borrow_mut() = Some(agent.clone()));
    announce_to_host("window");

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run(move |cx| {
            gpui_kit::init(cx);
            theme::apply(cx);
            // 窗口进程不持有 Dock 图标：宿主是唯一的 MNS Dock 入口，窗口只是
            // 它拉起的短命子进程（Accessory 仍可显示窗口、拿焦点、出菜单栏）。
            #[cfg(target_os = "macos")]
            cx.set_activation_policy(gpui_kit::ActivationPolicy::Accessory);
            cx.set_global(GuiState {
                window: None,
                view: None,
                butler_window: None,
                butler_view: None,
                butler_notice: None,
                butler_daemon: false,
                butler_proc: ButlerSupervisor::default(),
                butler_daemon_conn: None,
                butler_window_conn: None,
                tray: None,
                jobs: Some(jobs.clone()),
                brief: None,
                last_tray_brief: None,
                tray_alert: false,
                _listener: Some(listener),
            });

            // 红灯关闭按钮 / 系统关窗不会走 `close_window`：在这里撤掉 socket，
            // 否则文件会留到下次开窗才被 `listen` 的探活清理。
            cx.on_window_closed(|cx, _| release_listener(cx)).detach();

            register_window_actions(cx, true);
            if let Err(error) = open_main_window(cx, page, about) {
                eprintln!("[mns-gui] failed to open window: {error}");
                cx.quit();
                return;
            }
            // 窗口进程是 Accessory，建窗不会自动变成前台应用；宿主刚在
            // Dock 上被点过，这里显式激活，窗口才会真的到用户眼前。
            activate_any_window(cx);
            if refresh && let Some(view) = cx.global::<GuiState>().view.clone() {
                view.update(cx, |shell, cx| shell.refresh(cx));
            }

            spawn_job_bridge(cx, job_events);
            spawn_agent_bridge(cx, agent_events);
            spawn_daily_scheduler(cx);
            spawn_window_bridge(cx, events);
        });
}

/// 管家窗口短命子进程：`mns-gui --window --butler [--notice <id>]`。
fn run_butler_window(notice: Option<String>) {
    let (listener, events) = match crate::ipc::listen("mns-gui-butler") {
        Ok(pair) => pair,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            // 窗口已经开着：把「定位到哪一条」交给它，自己退出。
            let signal = match notice {
                Some(notice_id) => crate::ipc::Signal::NoticeFocus { notice_id },
                None => crate::ipc::Signal::Open,
            };
            let _ = crate::ipc::poke("mns-gui-butler", &signal);
            return;
        }
        Err(error) => {
            eprintln!("[mns-gui] 管家窗口实例锁不可用：{error}");
            return;
        }
    };

    // 打开即告诉宿主「用户在看管家」：mark_read 与会话合并都由宿主负责。
    // 长连接 Hello 让宿主知道窗口进程还在，关窗断开即感知。
    announce_to_host("window-butler");

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run(move |cx| {
            gpui_kit::init(cx);
            theme::apply(cx);
            // 窗口进程不持有 Dock 图标：宿主是唯一的 MNS Dock 入口，窗口只是
            // 它拉起的短命子进程（Accessory 仍可显示窗口、拿焦点、出菜单栏）。
            #[cfg(target_os = "macos")]
            cx.set_activation_policy(gpui_kit::ActivationPolicy::Accessory);
            cx.set_global(GuiState {
                window: None,
                view: None,
                butler_window: None,
                butler_view: None,
                butler_notice: None,
                butler_daemon: false,
                butler_proc: ButlerSupervisor::default(),
                butler_daemon_conn: None,
                butler_window_conn: None,
                tray: None,
                jobs: None,
                brief: None,
                last_tray_brief: None,
                tray_alert: false,
                _listener: Some(listener),
            });

            // 红灯关闭按钮 / 系统关窗不会走 `close_window`：在这里撤掉 socket。
            cx.on_window_closed(|cx, _| release_listener(cx)).detach();

            register_window_actions(cx, false);
            if let Err(error) = open_butler_window_view(cx, notice.clone()) {
                eprintln!("[mns-gui] failed to open butler window: {error}");
                cx.quit();
                return;
            }
            activate_any_window(cx);
            spawn_window_bridge(cx, events);
        });
}

/// 向宿主声明「我是哪个窗口」，并在断线后重新握手。
///
/// 宿主据此判断窗口是否还开着（断开事件即关窗），不需要轮询进程名。
/// 宿主崩溃后会被系统重新拉起：新宿主没有旧连接的记忆，所以这里必须
/// 自己发现断线并重新 `Hello`，否则「窗口开着」会被判成「窗口关了」，
/// 通知横幅就会在用户眼前重复弹。
fn announce_to_host(role: &'static str) {
    let _ = std::thread::Builder::new()
        .name("mns-host-hello".to_owned())
        .spawn(move || {
            loop {
                let Ok((mut client, mut incoming)) = crate::ipc::connect_duplex("mns-gui") else {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    continue;
                };
                if client
                    .send(&crate::ipc::Signal::Hello {
                        role: role.to_owned(),
                    })
                    .is_err()
                {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    continue;
                }
                // 宿主收到了这句问候。管家窗口的离线横幅靠宿主拉起守护进程
                // （窗口自己不 spawn），从这一刻起「重试」按钮才有资格亮。
                butler::mark_host_alive();
                // 连接活着就阻塞在这里；通道关闭 = 宿主退出，回到顶部重连。
                while incoming.blocking_recv().is_some() {}
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        });
}

/// 拉起一个短命窗口子进程，并在线程里 `wait` 收割——Unix 上不 wait 会留下
/// 僵尸，正好破坏「开关 20 次无进程累积」。
fn spawn_window(args: &[&str]) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut cmd = command(&exe);
    cmd.arg("--window");
    for arg in args {
        cmd.arg(arg);
    }
    match cmd.spawn() {
        Ok(mut child) => {
            std::thread::Builder::new()
                .name("mns-window-reaper".to_owned())
                .spawn(move || {
                    let _ = child.wait();
                })
                .ok();
        }
        Err(error) => eprintln!("[mns-gui] 无法启动窗口进程：{error}"),
    }
}

/// Minute-resolution scheduler for the unattended review, inside the main
/// window process only. With no window there is no place to show a review, so
/// the host does not run it; butler notifications are still delivered by the
/// daemon.
///
/// A real timer loop beats a launch hook: the window may stay open for days, and
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
            });
        }
    })
    .detach();
}

/// Host ticker: refresh the tray once a minute and ask butlerd for a fresh
/// brief. `poke` doubles as liveness detection — success means the daemon is
/// listening, failure flips the tray to red without a second probe connection.
fn spawn_host_ticker(cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(60))
                .await;
            let _ = cx.update(|cx| {
                request_brief(cx);
                // 断开事件可能漏掉（守护进程还没连上就死了）：托盘探活是
                // 最后一道兜底，最多重启三次，用户主动停止后不再复活。
                maybe_restart_butler(cx);
                refresh_tray(cx);
            });
        }
    })
    .detach();
}

/// 通知点击回调只置位，窗口由主线程拉起——回调线程不能直接碰 GPUI。
fn spawn_notification_clicks(
    cx: &mut App,
    mut requests: tokio::sync::mpsc::UnboundedReceiver<Option<String>>,
) {
    cx.spawn(async move |cx| {
        // 挂起等事件，不做定时轮询：一次点击唤醒一次，空闲时零开销。
        while let Some(notice_id) = requests.recv().await {
            cx.update(|cx| match notice_id {
                // 新格式：带着通知 id 进来，窗口要定位到那一条。
                Some(notice_id) => open_butler_notice_from_host(cx, &notice_id),
                // 旧版通知没有 userInfo：退回只开窗，至少不丢这次点击。
                None => open_butler_from_host(cx),
            });
        }
    })
    .detach();
}

/// 宿主侧本地通道的消费端。
///
/// 它同时是「谁还活着」的传感器：守护进程与窗口进程各自握手/断开，宿主
/// 不需要查进程名，也不会把另一个干净环境里的同名进程认成自己的。
fn spawn_ipc_bridge(
    cx: &mut App,
    mut events: tokio::sync::mpsc::UnboundedReceiver<crate::ipc::Event>,
) {
    cx.spawn(async move |cx| {
        while let Some(event) = events.recv().await {
            match event {
                crate::ipc::Event::Signal(_, crate::ipc::Signal::Open) => {
                    let _ = cx.update(|cx| open_main_from_host(cx, None, false, false));
                }
                crate::ipc::Event::Signal(_, crate::ipc::Signal::OpenButler) => {
                    let _ = cx.update(|cx| open_butler_from_host(cx));
                }
                // Windows Toast 点击：系统拉起的新进程把 notice id 交回来，
                // 和 macOS 的通知回调走同一条定位路径。
                crate::ipc::Event::Signal(_, crate::ipc::Signal::NoticeFocus { notice_id }) => {
                    let _ = cx.update(|cx| open_butler_notice_from_host(cx, &notice_id));
                }
                // 窗口进程里的「退出 MNS」：托盘宿主才是整个程序的拥有者。
                crate::ipc::Event::Signal(_, crate::ipc::Signal::Shutdown) => {
                    eprintln!("[mns-gui] 收到退出请求");
                    let _ = cx.update(|cx| quit_host(cx));
                }
                crate::ipc::Event::Signal(id, crate::ipc::Signal::Hello { role })
                    if role == "daemon" =>
                {
                    let _ = cx.update(|cx| {
                        eprintln!("[mns-gui] 管家守护进程上线");
                        {
                            let state = cx.global_mut::<GuiState>();
                            state.butler_daemon = true;
                            state.butler_daemon_conn = Some(id);
                            state.butler_proc.starting = false;
                            state.butler_proc.restarts = 0;
                        }
                        // P4 起开口由 butlerd 自己并进会话（单写者），宿主
                        // 只管要简报与托盘。
                        request_brief(cx);
                        refresh_tray(cx);
                    });
                }
                crate::ipc::Event::Signal(id, crate::ipc::Signal::Hello { role })
                    if role == "window-butler" =>
                {
                    let _ = cx.update(|cx| {
                        cx.global_mut::<GuiState>().butler_window_conn = Some(id);
                        // 用户刚打开窗口：会话已读由 butlerd 在挂载帧的
                        // `ChatSeen` 里写，宿主只清托盘提示，不再碰会话。
                        cx.global_mut::<GuiState>().butler_notice = None;
                        refresh_tray(cx);
                    });
                }
                crate::ipc::Event::Signal(_, crate::ipc::Signal::Brief { json }) => {
                    let _ = cx.update(|cx| {
                        if let Ok(brief) = serde_json::from_str::<crate::brief::BriefData>(&json) {
                            {
                                let state = cx.global_mut::<GuiState>();
                                state.brief = Some(brief);
                                state.butler_daemon = true;
                                // 能答话就是活的：把监督计数清掉，下一次掉线
                                // 才有资格重新拉起。用户主动停止的意图不在这里
                                // 清除——停止前最后一条简报不能把它抵消掉。
                                state.butler_proc.starting = false;
                                state.butler_proc.restarts = 0;
                            }
                            refresh_tray(cx);
                        }
                    });
                }
                crate::ipc::Event::Signal(_, crate::ipc::Signal::ButlerSeen) => {
                    let _ = cx.update(|cx| {
                        // 会话已读标记由 butlerd 在 `ChatSeen` 里写，宿主只
                        // 撤自己的托盘提示。
                        cx.global_mut::<GuiState>().butler_notice = None;
                        refresh_tray(cx);
                    });
                }
                crate::ipc::Event::Signal(_, crate::ipc::Signal::DeliverNotice { json }) => {
                    let _ = cx.update(|cx| deliver_notice(cx, &json));
                }
                // 管家窗口离线横幅的「重试」：守护进程的生命周期归宿主，
                // 窗口只请、不自己拉起。
                crate::ipc::Event::Signal(_, crate::ipc::Signal::StartButler) => {
                    let _ = cx.update(|cx| start_butler_daemon(cx));
                }
                crate::ipc::Event::Disconnected(id) => {
                    let _ = cx.update(|cx| {
                        let mut restart_after = None;
                        let mut daemon_lost = false;
                        {
                            let state = cx.global_mut::<GuiState>();
                            if state.butler_window_conn == Some(id) {
                                state.butler_window_conn = None;
                            }
                            if state.butler_daemon_conn == Some(id) {
                                eprintln!("[mns-gui] 管家守护进程离线");
                                state.butler_daemon = false;
                                state.butler_daemon_conn = None;
                                state.brief = None;
                                state.butler_proc.starting = false;
                                if state
                                    .butler_proc
                                    .child
                                    .as_ref()
                                    .is_some_and(SupervisedChild::has_exited)
                                {
                                    // 已经退出：清掉句柄，避免拿死进程当“正在跑”。
                                    state.butler_proc.child = None;
                                }
                                // 用户主动停止就不复活；否则 1s → 2s → 4s 退避。
                                if !state.butler_proc.stopped && state.butler_proc.restarts < 3 {
                                    state.butler_proc.restarts += 1;
                                    restart_after = Some(1u64 << (state.butler_proc.restarts - 1));
                                }
                                daemon_lost = true;
                            }
                        }
                        if daemon_lost {
                            refresh_tray(cx);
                        }
                        if let Some(secs) = restart_after {
                            cx.spawn(async move |cx| {
                                cx.background_executor()
                                    .timer(std::time::Duration::from_secs(secs))
                                    .await;
                                let _ = cx.update(|cx| start_butler_daemon(cx));
                            })
                            .detach();
                        }
                    });
                }
                crate::ipc::Event::Signal(_, _) | crate::ipc::Event::Connected(_) => {}
            }
        }
    })
    .detach();
}

/// 窗口子进程的本地通道消费端：置前、翻页、刷新、被宿主关闭。
fn spawn_window_bridge(
    cx: &mut App,
    mut events: tokio::sync::mpsc::UnboundedReceiver<crate::ipc::Event>,
) {
    cx.spawn(async move |cx| {
        while let Some(event) = events.recv().await {
            let _ = cx.update(|cx| match event {
                crate::ipc::Event::Signal(_, crate::ipc::Signal::Open) => {
                    activate_any_window(cx);
                }
                crate::ipc::Event::Signal(_, crate::ipc::Signal::OpenPage { page }) => {
                    if let Some(page) = Page::from_slug(&page) {
                        navigate_window(cx, page);
                    }
                    activate_any_window(cx);
                }
                crate::ipc::Event::Signal(_, crate::ipc::Signal::OpenAbout) => {
                    if let Some(view) = cx.global::<GuiState>().view.clone() {
                        view.update(cx, |shell, cx| {
                            shell.about_open = true;
                            cx.notify();
                        });
                    }
                    activate_any_window(cx);
                }
                crate::ipc::Event::Signal(_, crate::ipc::Signal::RefreshData) => {
                    if let Some(view) = cx.global::<GuiState>().view.clone() {
                        view.update(cx, |shell, cx| shell.refresh(cx));
                    }
                    activate_any_window(cx);
                }
                crate::ipc::Event::Signal(_, crate::ipc::Signal::NoticeFocus { notice_id }) => {
                    if let Some(view) = cx.global::<GuiState>().butler_view.clone() {
                        view.update(cx, |view, cx| view.focus_notice(notice_id, cx));
                    }
                    activate_any_window(cx);
                    // 窗口已在看这条：告诉宿主标记已读、清托盘提示。
                    let _ = crate::ipc::poke("mns-gui", &crate::ipc::Signal::ButlerSeen);
                }
                crate::ipc::Event::Signal(_, crate::ipc::Signal::Shutdown) => {
                    close_window(cx);
                }
                _ => {}
            });
        }
    })
    .detach();
}

/// 把当前进程里的任意窗口置前（主窗或管家窗）。
fn activate_any_window(cx: &mut App) {
    let state = cx.global::<GuiState>();
    let handle = state.window.clone().or_else(|| state.butler_window.clone());
    if let Some(handle) = handle {
        let _ = handle.update(cx, |_, window, _| window.activate_window());
    }
    cx.activate(true);
}

/// 在窗口进程里翻页；窗口不在时什么都不做。
fn navigate_window(cx: &mut App, page: Page) {
    if let Some(view) = cx.global::<GuiState>().view.clone() {
        view.update(cx, |shell, cx| shell.navigate(page, cx));
    }
}

/// 释放本进程的实例锁并删掉 socket 文件。退出路径上的最后一步。
fn release_listener(cx: &mut App) {
    if let Some(mut listener) = cx.global_mut::<GuiState>()._listener.take() {
        listener.release();
    }
}

/// 关掉本进程的窗口。主窗先把 ACP 会话收尾并落盘，再移除窗口；
/// `QuitMode::LastWindowClosed` 随后结束整个窗口进程。
fn close_window(cx: &mut App) {
    let state = cx.global::<GuiState>();
    let handle = state.window.clone().or_else(|| state.butler_window.clone());
    if let Some(view) = state.view.clone() {
        view.update(cx, |shell, cx| shell.shutdown(cx));
    }
    // 窗口关掉之后没人再会连它：先撤掉 socket，免得残留文件让宿主误判
    // 「窗口还开着」（`poke` 连不上，但目录里看得见）。
    release_listener(cx);
    match handle {
        // 这个函数可能正在该窗口自己的 update 里跑（cmd-w 走的是窗口内的
        // 事件回调），此时窗口已被借出 `cx.windows`，再 `handle.update`
        // 只会拿到 None。推到本轮 effect 末尾再移除，窗口已经还回来了。
        Some(handle) => cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }),
        None => cx.quit(),
    }
}

/// butlerd 递来一条开口：决定要不要弹系统通知。
///
/// P4 起会话存档与开口的并档全由 butlerd 单写（`pending.offset` 也由它推进），
/// 宿主只剩两件事：管家窗口开着时话已经在会话里可见，撤托盘提示、不弹
/// 横幅；窗口关着且这条开口还新鲜（≤5 分钟）、没投递过，才弹一次系统
/// 通知并落审计。
fn deliver_notice(cx: &mut App, json: &str) {
    let Ok(notice) = serde_json::from_str::<crate::butler::orchestrator::Notice>(json) else {
        return;
    };
    if cx.global::<GuiState>().butler_window_conn.is_some() {
        // 窗口开着：消息经 butlerd 的快照直达会话，系统横幅只会重复打搅。
        cx.global_mut::<GuiState>().butler_notice = None;
        ack_notice(cx, &notice.notice_id);
        refresh_tray(cx);
        return;
    }
    cx.global_mut::<GuiState>().butler_notice = Some(notice.draft.clone());
    // 只对刚产生的开口弹：宿主重启后补递的旧积压不该再响一遍。
    let now = chrono::Local::now();
    let fresh = now.signed_duration_since(notice.at) < chrono::Duration::minutes(5);
    // 同一 id 已经记过就不再弹，避免宿主重启后重复响一遍。
    let already = crate::butler::orchestrator::delivered_notice(&notice.notice_id).is_some();
    if fresh && !already && crate::notify::send(&notice.draft, &notice.notice_id) {
        // 交给系统投递之后再落审计：开发二进制未打包时 `send` 返回
        // false，不能把「没弹」记成「已送达」。
        let _ = crate::butler::orchestrator::record_notice_delivered(&notice);
    }
    // 不管弹没弹，宿主都已经「消费」了这条：回执让守护进程推进 pending。
    ack_notice(cx, &notice.notice_id);
    refresh_tray(cx);
}

/// 把「这条开口已经处理过」告诉守护进程。投递语义是至少一次 + 幂等：
/// 回执丢了就重投，宿主靠 `notices.jsonl` 去重，不会重复打扰用户。
fn ack_notice(cx: &mut App, notice_id: &str) {
    let state = cx.global::<GuiState>();
    if !state.butler_daemon {
        return;
    }
    let _ = crate::ipc::poke(
        "mns-butlerd",
        &crate::ipc::Signal::NoticeDelivered {
            notice_id: notice_id.to_owned(),
        },
    );
}

/// 拉起或置前主窗口。窗口已经在跑时不重复开进程：先 poke 窗口自己的
/// 通道，连不上才 spawn。窗口内部的单实例锁会兜住并发重复 spawn。
fn open_main_from_host(cx: &mut App, page: Option<Page>, about: bool, refresh: bool) {
    // 宿主先成为前台（最后一个 Regular 应用）：窗口进程是 Accessory，
    // 它随后激活时 macOS 保留的菜单栏仍是宿主这一份 MNS 菜单。
    cx.activate(true);
    let signal = match (page, about, refresh) {
        (Some(page), _, _) => crate::ipc::Signal::OpenPage {
            page: page.slug().to_owned(),
        },
        (_, true, _) => crate::ipc::Signal::OpenAbout,
        (_, _, true) => crate::ipc::Signal::RefreshData,
        _ => crate::ipc::Signal::Open,
    };
    if crate::ipc::poke("mns-gui-window", &signal).is_ok() {
        return;
    }

    let mut args = Vec::new();
    match page {
        Some(page) => {
            args.push("--page");
            args.push(page.slug());
        }
        None if about => args.push("--about"),
        None if refresh => args.push("--refresh"),
        None => {}
    }
    spawn_window(&args);
}

/// 拉起或置前管家窗口。
fn open_butler_from_host(cx: &mut App) {
    cx.activate(true);
    if crate::ipc::poke("mns-gui-butler", &crate::ipc::Signal::Open).is_ok() {
        return;
    }
    spawn_window(&["butler"]);
}

/// 点系统通知进来：拉起或置前管家窗口，并定位到那条开口。
///
/// 窗口已经开着时只 poke 定位信号，不再重开进程；点击审计按 id 去重，
/// 同一条通知点两次只留第一次记录。
fn open_butler_notice_from_host(cx: &mut App, notice_id: &str) {
    cx.activate(true);
    if let Err(error) = crate::butler::orchestrator::record_notice_clicked(notice_id) {
        eprintln!("[mns-gui] 记录通知点击失败：{error:#}");
    }
    let signal = crate::ipc::Signal::NoticeFocus {
        notice_id: notice_id.to_owned(),
    };
    if crate::ipc::poke("mns-gui-butler", &signal).is_ok() {
        return;
    }
    spawn_window(&["butler", "--notice", notice_id]);
}

/// 向守护进程要一份托盘简报。`poke` 同时是存活探测：连不上就把托盘拉红。
fn request_brief(cx: &mut App) {
    let alive = crate::ipc::poke("mns-butlerd", &crate::ipc::Signal::RequestBrief).is_ok();
    let state = cx.global_mut::<GuiState>();
    if state.butler_daemon != alive {
        state.butler_daemon = alive;
        if !alive {
            state.butler_daemon_conn = None;
        }
    }
    if !alive {
        // 没有守护进程就没有可信数字：宁可显示「未运行」，也不拿旧值冒充。
        state.brief = None;
    }
}

/// 打开管家窗口实体。只跑在管家窗口子进程里：会话真相在 butlerd，
/// 视图自己连守护进程拿快照（`ButlerView::new`），宿主不掺和。
fn open_butler_window_view(cx: &mut App, notice: Option<String>) -> Result<()> {
    let facts = crate::butler::chat::current_facts(chrono::Local::now());
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("MNS · 管家".into()),
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::centered(size(px(760.), px(640.)), cx)),
        focus: true,
        show: true,
        ..Default::default()
    };
    let (handle, view) = gpui_kit::open_window(options, cx, move |window, cx| {
        cx.new(|cx| butler::ButlerView::new(facts, window, cx))
    })?;
    let _ = handle.update(cx, |_, window, _| window.activate_window());
    let state = cx.global_mut::<GuiState>();
    state.butler_window = Some(handle);
    state.butler_view = Some(view.clone());
    // 带 `--notice` 启动：窗口第一次渲染前就落到那条消息上，避免先空窗
    // 再跳一下。会话真相要等快照，定位由视图自己等（`pending_notice`）。
    if let Some(notice_id) = notice {
        view.update(cx, |view, cx| view.focus_notice(notice_id, cx));
        // 从系统通知点进来：撤掉宿主的托盘提示（会话已读由 butlerd 在
        // 挂载帧的 `ChatSeen` 里写）。
        let _ = crate::ipc::poke("mns-gui", &crate::ipc::Signal::ButlerSeen);
    }
    Ok(())
}

fn spawn_job_bridge(cx: &mut App, mut events: tokio::sync::mpsc::UnboundedReceiver<JobEvent>) {
    cx.spawn(async move |cx| {
        while let Some(event) = events.recv().await {
            let _ = cx.update(|cx| {
                let view = cx.global::<GuiState>().view.clone();
                if let Some(view) = view {
                    view.update(cx, |shell, cx| shell.apply_job_event(event, cx));
                }
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
            });
        }
    })
    .detach();
}

/// 宿主进程的动作：只拉起/置前窗口，不直接操作 AppShell 或 agent runtime。
fn register_host_actions(cx: &mut App) {
    cx.on_action(|_: &ShowMainWindow, cx| {
        // 从管家通知点进来时，用户要的是会话窗口，不是主界面。
        if !notify::take_click_pending() {
            open_main_from_host(cx, None, false, false);
        }
        refresh_tray(cx);
    });
    cx.on_action(|_: &ShowAbout, cx| open_main_from_host(cx, None, true, false));
    cx.on_action(|_: &RefreshData, cx| open_main_from_host(cx, None, false, true));
    cx.on_action(|_: &OpenPlanPage, cx| open_main_from_host(cx, Some(Page::Plan), false, false));
    cx.on_action(|_: &OpenAgentPage, cx| open_main_from_host(cx, Some(Page::Agent), false, false));
    cx.on_action(|_: &OpenButlerWindow, cx| open_butler_from_host(cx));
    cx.on_action(|_: &OpenHelpCenter, _| {
        if let Err(error) = open_help_center(None) {
            eprintln!("启动 mns-help 失败：{error}");
        }
    });
    cx.on_action(|_: &StopButlerDaemon, cx| stop_butler_daemon(cx));
    cx.on_action(|_: &StartButlerDaemon, cx| start_butler_daemon(cx));
    cx.on_action(|_: &RestartButlerDaemon, cx| restart_butler_daemon(cx));
    cx.on_action(|_: &QuitApplication, cx| quit_host(cx));

    // 这些动作只会在主窗口进程里真正执行；宿主这一份是给「菜单栏停在宿主」
    // 时用的——关闭窗口只需请窗口自己收尾，巡检/任务仍走窗口内的按钮。
    cx.on_action(|_: &CloseMainWindow, _| {
        let _ = crate::ipc::poke("mns-gui-window", &crate::ipc::Signal::Shutdown);
    });
    cx.on_action(|_: &CloseAgentPanel, _| {});
    cx.on_action(|_: &RunTodayReview, _| {});
    cx.on_action(|_: &RunDefaultGoal, _| {});
    cx.on_action(|_: &StopCurrentTask, _| {});
}

/// 窗口进程的动作：操作自己进程里的唯一窗口与 AppShell。
#[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
fn register_window_actions(cx: &mut App, with_menu: bool) {
    // GPUI Kit deliberately leaves cmd-q / cmd-w / alt-f4 to the application.
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
    // after the bindings above. Only the main window draws a menu bar; the
    // butler window is a small chat surface.
    #[cfg(target_os = "macos")]
    if with_menu {
        cx.set_menus(app_menus());
    }

    cx.on_action(|_: &ShowMainWindow, cx| activate_any_window(cx));
    cx.on_action(|_: &CloseMainWindow, cx| close_window(cx));
    cx.on_action(|_: &ShowAbout, cx| {
        if let Some(view) = cx.global::<GuiState>().view.clone() {
            view.update(cx, |shell, cx| {
                shell.about_open = true;
                cx.notify();
            });
        }
        activate_any_window(cx);
    });
    cx.on_action(|_: &RefreshData, cx| {
        if let Some(view) = cx.global::<GuiState>().view.clone() {
            view.update(cx, |shell, cx| shell.refresh(cx));
        }
    });
    cx.on_action(|_: &OpenPlanPage, cx| navigate_window(cx, Page::Plan));
    cx.on_action(|_: &OpenAgentPage, cx| navigate_window(cx, Page::Agent));
    cx.on_action(|_: &OpenButlerWindow, _| {
        // 管家窗口由宿主拉起；窗口进程只发请求，不自己开第二个。
        let _ = crate::ipc::poke("mns-gui", &crate::ipc::Signal::OpenButler);
    });
    cx.on_action(|_: &RunTodayReview, cx| {
        if let Some(view) = cx.global::<GuiState>().view.clone() {
            view.update(cx, |shell, cx| {
                shell.navigate(Page::Agent, cx);
                shell.start_daily_review(true, cx);
            });
        }
    });
    cx.on_action(|_: &RunDefaultGoal, cx| {
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
    cx.on_action(|_: &QuitApplication, cx| {
        // 窗口进程里的 cmd-q / 菜单「退出」：整个程序由托盘宿主拥有，
        // 先让宿主退出，自己再收尾（`close_window` 负责会话落盘）。
        let _ = crate::ipc::poke("mns-gui", &crate::ipc::Signal::Shutdown);
        close_window(cx);
    });
}

/// 托盘探活之外的兜底：确认没人判定时按退避重启守护进程。
fn maybe_restart_butler(cx: &mut App) {
    let should = {
        let state = cx.global::<GuiState>();
        !state.butler_daemon
            && !state.butler_proc.stopped
            && !state.butler_proc.starting
            && state.butler_proc.restarts < 3
    };
    if !should {
        return;
    }
    cx.global_mut::<GuiState>().butler_proc.restarts += 1;
    start_butler_daemon(cx);
}

/// 停止管家守护进程。先手动把状态拉红：断开事件可能晚一拍到达。
fn stop_butler_daemon(cx: &mut App) {
    let state = cx.global_mut::<GuiState>();
    state.butler_proc.stopped = true;
    state.butler_proc.starting = false;
    state.butler_proc.restarts = 0;
    let previous = state.butler_proc.child.take();
    if let Err(error) = crate::ipc::poke("mns-butlerd", &crate::ipc::Signal::Shutdown) {
        eprintln!("[mns-gui] 没有在运行的管家进程：{error}");
    }
    // 强杀兜底放在后台：菜单动作不能卡主线程；reaper 线程负责最终 wait。
    if let Some(child) = previous {
        let _ = std::thread::Builder::new()
            .name("mns-butlerd-stop".to_owned())
            .spawn(move || child.terminate(std::time::Duration::from_secs(3)));
    }
    let state = cx.global_mut::<GuiState>();
    state.butler_daemon = false;
    state.butler_daemon_conn = None;
    state.brief = None;
    refresh_tray(cx);
}

fn start_butler_daemon(cx: &mut App) {
    if cx.global::<GuiState>().butler_proc.starting {
        return;
    }
    if crate::ipc::connect("mns-butlerd").is_ok() {
        {
            let state = cx.global_mut::<GuiState>();
            state.butler_daemon = true;
            state.butler_proc.stopped = false;
            state.butler_proc.starting = false;
            state.butler_proc.restarts = 0;
        }
        request_brief(cx);
        refresh_tray(cx);
        return;
    }
    let Some(child) = spawn_butler_daemon_process() else {
        refresh_tray(cx);
        return;
    };
    {
        let state = cx.global_mut::<GuiState>();
        state.butler_proc.child = Some(child);
        state.butler_proc.starting = true;
        state.butler_proc.stopped = false;
    }
    // 拉起要等它开始监听，但托盘菜单不能再卡主线程：用后台定时器轮询。
    cx.spawn(async move |cx| {
        for _ in 0..20 {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(100))
                .await;
            if crate::ipc::connect("mns-butlerd").is_ok() {
                let _ = cx.update(|cx| {
                    {
                        let state = cx.global_mut::<GuiState>();
                        state.butler_daemon = true;
                        state.butler_proc.starting = false;
                        state.butler_proc.restarts = 0;
                    }
                    request_brief(cx);
                    refresh_tray(cx);
                });
                return;
            }
        }
        let _ = cx.update(|cx| {
            cx.global_mut::<GuiState>().butler_proc.starting = false;
            refresh_tray(cx);
        });
    })
    .detach();
}

fn restart_butler_daemon(cx: &mut App) {
    let previous = {
        let state = cx.global_mut::<GuiState>();
        state.butler_proc.stopped = false;
        state.butler_proc.starting = false;
        state.butler_proc.restarts = 0;
        state.butler_proc.child.take()
    };
    let _ = crate::ipc::poke("mns-butlerd", &crate::ipc::Signal::Shutdown);
    if let Some(child) = previous {
        // 先等旧进程自己退（最多 3s），不走后台的话新人会和旧监听撞上。
        let _ = std::thread::Builder::new()
            .name("mns-butlerd-restart".to_owned())
            .spawn(move || child.terminate(std::time::Duration::from_secs(3)));
    }
    {
        let state = cx.global_mut::<GuiState>();
        state.butler_daemon = false;
        state.butler_daemon_conn = None;
        state.brief = None;
    }
    refresh_tray(cx);
    cx.spawn(async move |cx| {
        // 等旧进程退出（连接失败）再拉起，避免新人撞上旧监听。
        for _ in 0..20 {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(100))
                .await;
            if crate::ipc::connect("mns-butlerd").is_err() {
                break;
            }
        }
        let Some(child) = spawn_butler_daemon_process() else {
            let _ = cx.update(|cx| refresh_tray(cx));
            return;
        };
        let _ = cx.update(move |cx| {
            let state = cx.global_mut::<GuiState>();
            state.butler_proc.child = Some(child);
            state.butler_proc.starting = true;
        });
        for _ in 0..20 {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(100))
                .await;
            if crate::ipc::connect("mns-butlerd").is_ok() {
                let _ = cx.update(|cx| {
                    {
                        let state = cx.global_mut::<GuiState>();
                        state.butler_daemon = true;
                        state.butler_proc.starting = false;
                        state.butler_proc.restarts = 0;
                    }
                    request_brief(cx);
                    refresh_tray(cx);
                });
                return;
            }
        }
        let _ = cx.update(|cx| {
            cx.global_mut::<GuiState>().butler_proc.starting = false;
            refresh_tray(cx);
        });
    })
    .detach();
}

/// 退出整个程序前收拾子进程：窗口先收尾落盘，守护进程再停。
///
/// 每条退出路径都一样——菜单「退出 MNS」、Cmd+Q（窗口进程 poke 宿主）、
/// Dock 右键退出与系统 terminate（宿主 `on_app_quit`）。重复 poke 无害：
/// 对端已经不在时连接直接失败。
fn shutdown_children(cx: &mut App) {
    // 「退出 MNS」= 退出整个程序：守护进程由宿主拥有，一起收掉（只想停它的
    // 时候用托盘里的「停止管家进程」）。
    let _ = crate::ipc::poke("mns-gui-window", &crate::ipc::Signal::Shutdown);
    let _ = crate::ipc::poke("mns-gui-butler", &crate::ipc::Signal::Shutdown);
    let _ = crate::ipc::poke("mns-butlerd", &crate::ipc::Signal::Shutdown);
    // 有界等待一次，确保 Unix 上不留僵尸，也不在退出瞬间截断落盘。
    if let Some(child) = cx.global_mut::<GuiState>().butler_proc.child.take() {
        child.terminate(std::time::Duration::from_secs(2));
    }
}

/// 宿主退出：先让短命窗口自己结束，避免留下没有宿主的孤儿窗口。
fn quit_host(cx: &mut App) {
    shutdown_children(cx);
    if let Some(tray) = cx.global::<GuiState>().tray.clone() {
        let _ = tray.close(cx);
    }
    release_listener(cx);
    cx.quit();
}

/// 宿主自己的菜单栏。
///
/// 窗口子进程是 `Accessory`，macOS 不显示它的菜单栏；宿主是 `Regular`，
/// 窗口活着时菜单栏仍停在宿主这一份上（系统保留最后一个 Regular 应用的
/// 菜单），所以这里放的是「不依赖 AppShell 也能做」的动作：打开/关闭窗口、
/// 更新数据、翻页、管家、帮助、退出。窗口内的今日巡检/自主任务仍有按钮。
#[cfg(target_os = "macos")]
fn host_menus() -> Vec<Menu> {
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
            MenuItem::separator(),
            MenuItem::action("本月计划", OpenPlanPage),
            MenuItem::action("助手结论", OpenAgentPage),
            MenuItem::action("管家", OpenButlerWindow),
            MenuItem::separator(),
            MenuItem::action("帮助中心", OpenHelpCenter),
        ]),
    ]
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

/// 守护进程的监督状态。宿主是唯一有权拉起/停止它的人，窗口只发请求。
#[derive(Default)]
struct ButlerSupervisor {
    /// 用户显式「停止管家进程」后为 true：不再自动拉起，直到用户再次启动。
    stopped: bool,
    /// 正在等待新进程开始监听；防止自动与手动重启各起一个。
    starting: bool,
    /// 连续自动重启次数；连上后归零。
    restarts: u8,
    /// 宿主自己拉起的子进程；连上的是别人起的旧进程时为 `None`。
    child: Option<SupervisedChild>,
}

/// 宿主拉起的常驻子进程。
///
/// `std::process::Child` 被 drop 后不会回收：Unix 上每次「停止/重启管家」
/// 都会留下一个僵尸，直到宿主退出。这里保留句柄，并在专用线程里回收退出
/// 状态；宿主只在需要时决定要不要 `kill`。
struct SupervisedChild {
    child: std::sync::Arc<std::sync::Mutex<std::process::Child>>,
    status: std::sync::Arc<std::sync::Mutex<Option<std::process::ExitStatus>>>,
}

impl SupervisedChild {
    fn spawn(mut command: std::process::Command) -> std::io::Result<Self> {
        let child = command.spawn()?;
        let child = std::sync::Arc::new(std::sync::Mutex::new(child));
        let status = std::sync::Arc::new(std::sync::Mutex::new(None));
        let wait_child = std::sync::Arc::clone(&child);
        let wait_status = std::sync::Arc::clone(&status);
        let _ = std::thread::Builder::new()
            .name("mns-butlerd-reaper".to_owned())
            .spawn(move || {
                loop {
                    let observed = wait_child
                        .lock()
                        .ok()
                        .and_then(|mut child| child.try_wait().ok().flatten());
                    if let Some(observed) = observed {
                        if let Ok(mut slot) = wait_status.lock() {
                            *slot = Some(observed);
                        }
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            });
        Ok(Self { child, status })
    }

    fn has_exited(&self) -> bool {
        self.status
            .lock()
            .map(|slot| slot.is_some())
            .unwrap_or(true)
    }

    /// 先给 `grace` 时间自行退出，仍未退出就强杀；回收由 reaper 线程负责。
    fn terminate(&self, grace: std::time::Duration) {
        let deadline = std::time::Instant::now() + grace;
        while std::time::Instant::now() < deadline {
            if self.has_exited() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }
}

/// 确认管家守护进程在跑；不在就拉起来。返回「现在有人判定」与刚拉起的子进程。
///
/// 用本地通道探活而不是查进程名：同名进程可能属于另一个干净的环境（例如
/// 测试实例），认错了就会把判定权交空。
fn ensure_butler_daemon() -> (bool, Option<SupervisedChild>) {
    if crate::ipc::connect("mns-butlerd").is_ok() {
        return (true, None);
    }
    let Some(child) = spawn_butler_daemon_process() else {
        return (false, None);
    };
    // 等它开始监听：判定权交早了会在两边各说一次，交晚了会漏一轮。
    // 只在宿主启动阶段调用（此时还没有窗口），阻塞主线程可以接受。
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        if crate::ipc::connect("mns-butlerd").is_ok() {
            return (true, Some(child));
        }
    }
    (false, Some(child))
}

/// 只负责把守护进程拉起来，不等它监听。菜单动作走这条路，避免卡 UI。
fn spawn_butler_daemon_process() -> Option<SupervisedChild> {
    let Ok(exe) = std::env::current_exe() else {
        return None;
    };
    let Some(dir) = exe.parent() else {
        return None;
    };
    let daemon = dir.join(format!("mns-butlerd{}", std::env::consts::EXE_SUFFIX));
    if !daemon.exists() {
        // 单文件开发构建里可能没有这个二进制：退化成无守护进程，不报错。
        eprintln!("[mns-gui] 找不到管家守护进程：{}", daemon.display());
        return None;
    }
    match SupervisedChild::spawn(command(&daemon)) {
        Ok(child) => Some(child),
        Err(error) => {
            eprintln!("[mns-gui] 无法启动管家守护进程：{error}");
            None
        }
    }
}

// ─────────────────────────── Tray brief ───────────────────────────

/// What the tray can say without opening a window.
///
/// P2 起宿主没有 AppShell：数字由 `mns-butlerd` 算好送回（`brief::BriefData`），
/// 这里只做排版与状态标记。引擎建议（再平衡带）与守护进程健康是独立字段：
/// 模型意见不能读成策略规则，账本读取失败也不能读成「账户空了」。
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
    /// 管家进程 + 账本健康的一行状态；`butler_ok == false` 时托盘亮红灯。
    butler: String,
    butler_ok: bool,
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
            butler: "管家进程：检查中".to_owned(),
            butler_ok: true,
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
    plan: Option<&crate::brief::PlanBrief>,
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
    agent: Option<&AgentState>,
    settings: &AgentSettings,
    last_daily: Option<&str>,
    today: &str,
) -> String {
    if let Some(agent) = agent {
        if agent.is_running() {
            return format!("巡检中：{}", tray_line(&agent.detail, 36));
        }
        if agent.state == RunState::Failed {
            return format!(
                "上次任务失败：{}",
                tray_line(agent.error.as_deref().unwrap_or("未知错误"), 36)
            );
        }
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

fn tray_brief_from(data: Option<&crate::brief::BriefData>, day_label: &str) -> TrayBrief {
    let mut brief = TrayBrief::default();
    brief.day_label = day_label.to_owned();

    let Some(data) = data else {
        return brief;
    };
    if let Some(error) = &data.load_error {
        brief.title = "MNS · 读取失败".to_owned();
        brief.as_of = "本地数据不可用".to_owned();
        brief.advice = format!("读取失败：{}", tray_line(error, 48));
        brief.tooltip = format!("Money Never Sleeps · {}", brief.advice);
        return brief;
    }
    if data.as_of.is_empty() {
        return brief;
    }

    brief.as_of = format!("数据截至 {} · {}", data.as_of, data.freshness);
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
            .push(format!("风险仓位 {}", pct_points(data.risk_weight_pct)));
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
            data.decision_label,
        ));
    } else {
        brief
            .facts
            .push("尚无情绪数据，无法给出目标仓位".to_owned());
    }

    // Sunday prices are legitimately a day old, so staleness is a caveat on
    // the advice rather than a verdict that replaces it.
    let stale = data.price_stale || data.fgi_age_days.map(|days| days >= 1).unwrap_or(true);
    let kind = tray_advice_kind(
        data.signal.is_some(),
        data.plan.is_some(),
        data.plan.as_ref().is_some_and(|plan| plan.has_orders),
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
    // 菜单栏里只放图标，文字放 tooltip：macOS 26 会把放不下的状态项整项藏进
    // 溢出区（本机菜单栏够挤，加一个「MNS · 待调仓」就直接看不见图标了）。
    // 22pt 的图标在任何菜单栏里都放得下，结论文字进 tooltip 与菜单首行。
    let tray = Tray::builder()
        .icon(tray_icon()?)
        .tooltip("Money Never Sleeps · 正在读取本地数据")
        .on_activate(ShowMainWindow)
        .menu(build_tray_menu)
        .build(cx)?;
    cx.global_mut::<GuiState>().tray = Some(tray);
    Ok(())
}

/// Host tray brief: numbers come from butlerd, status from GuiState.
fn current_tray_brief(cx: &mut App) -> TrayBrief {
    let now = chrono::Local::now();
    let today = now.format("%Y-%m-%d").to_string();
    const WEEKDAYS: [&str; 7] = ["一", "二", "三", "四", "五", "六", "日"];
    let weekday = WEEKDAYS[now.weekday().num_days_from_monday() as usize];
    let day_label = format!("今日 {} 周{}", now.format("%m-%d"), weekday);

    let state = cx.global::<GuiState>();
    let daemon = state.butler_daemon;
    // 健康状态随简报一起从守护进程下发（不再读 health.json）。超过 10 分钟
    // 没更新说明它要么刚起来还没答话，要么卡住了——两种都别拿旧结论当现状。
    let health = state
        .brief
        .as_ref()
        .and_then(|brief| brief.health.clone())
        .filter(|health| {
            chrono::Local::now()
                .signed_duration_since(health.at)
                .num_minutes()
                < 10
        });
    let (butler, butler_ok) = match (daemon, health) {
        (false, _) => ("管家进程：未运行".to_owned(), false),
        (true, Some(health)) if !health.ledger_ok => (
            format!(
                "管家进程：运行中 · 账本读取失败：{}",
                tray_line(health.error.as_deref().unwrap_or("未知原因"), 48)
            ),
            false,
        ),
        (true, Some(_)) => ("管家进程：运行中 · 账本正常".to_owned(), true),
        (true, None) => ("管家进程：运行中".to_owned(), true),
    };

    let review = tray_review_line(None, &load_settings(), read_last_daily().as_deref(), &today);
    let mut brief = tray_brief_from(state.brief.as_ref(), &day_label);
    brief.review = review;
    brief.butler = butler;
    brief.butler_ok = butler_ok;
    brief
}

/// Rebuild every tray surface together. `refresh_menu` alone would leave the
/// old tooltip behind after a state change.
fn refresh_tray(cx: &mut App) {
    let Some(tray) = cx.global::<GuiState>().tray.clone() else {
        return;
    };
    let brief = current_tray_brief(cx);
    // 图标不带文字（见 `setup_tray`）：结论压在 tooltip 里，鼠标停一下就看见。
    let _ = tray.set_tooltip(Some(format!("{} · {}", brief.title, brief.tooltip)), cx);

    // 红灯：管家进程不在或账本读不出来。图标只在翻转时更新。
    let alert = !brief.butler_ok;
    if cx.global::<GuiState>().tray_alert != alert {
        cx.global_mut::<GuiState>().tray_alert = alert;
        if let Ok(icon) = tray_icon_alert(alert) {
            let _ = tray.set_icon(icon, cx);
        }
        // Dock 图标与托盘红灯同一判断，只是换一个不会被菜单栏溢出的落点。
        set_dock_alert(alert);
    }

    // The tooltip setter is a cheap no-op when unchanged; recompiling the
    // native menu is not, so it only happens on a real state change.
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
        tray_info(tray_line(&brief.butler, 64)),
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

    // 管家入口：有未读就先显示最后一条，点进去看全部。
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

    // 守护进程状态与启停：托盘是唯一还能操作它的界面。
    items.push(MenuItem::separator());
    let daemon = cx.global::<GuiState>().butler_daemon;
    if daemon {
        items.push(MenuItem::action("停止管家进程", StopButlerDaemon));
        items.push(MenuItem::action("重启管家进程", RestartButlerDaemon));
    } else {
        items.push(MenuItem::action("启动管家进程", StartButlerDaemon));
    }

    items.push(MenuItem::separator());
    items.push(MenuItem::action("更新数据", RefreshData));
    items.push(MenuItem::action("打开 MNS", ShowMainWindow));
    items.push(MenuItem::action("帮助中心", OpenHelpCenter));
    items.push(MenuItem::action("关于 MNS", ShowAbout));
    items.push(MenuItem::action("退出 MNS", QuitApplication));
    items
}

/// Spawns the standalone `mns-help` viewer bundled next to this executable.
/// In dev builds the sibling lives in `target/<profile>/` alongside `mns-gui`.
///
/// `target` 是要直接翻到的那一页（`None` = 首页，或把已开着的窗口提到前面）。
/// 帮助中心是独立进程，关窗即退出：「已经在看」既靠 `mns-help` 自己的监听
/// 通道判断，也靠这里的子进程句柄兜住冷启动那一小段空窗期。
///
/// 窗口开着时，翻页意图走 IPC（`OpenPage`），不再轮询请求文件；冷启动那条路
/// 仍然把 slug 放进 argv。
fn open_help_center(target: Option<&str>) -> std::io::Result<()> {
    use std::sync::Mutex;

    static VIEWER: Mutex<Option<std::process::Child>> = Mutex::new(None);

    // 空串表示「只置前」：顶栏的「帮助」按钮不该把用户从正在读的一页拽回首页。
    let page = target.unwrap_or("").to_owned();
    if crate::ipc::poke(
        "mns-help",
        &crate::ipc::Signal::OpenPage { page: page.clone() },
    )
    .is_ok()
    {
        return Ok(());
    }

    let mut slot = VIEWER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(child) = slot.as_mut() {
        // `try_wait` 同时负责回收已退出的子进程：返回 `None` 说明还活着。
        if child.try_wait()?.is_none() {
            // 窗口还在启动、监听尚未 bind：后台补递，别把这次翻页吞掉。
            std::thread::spawn(move || {
                for _ in 0..20 {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    if crate::ipc::poke(
                        "mns-help",
                        &crate::ipc::Signal::OpenPage { page: page.clone() },
                    )
                    .is_ok()
                    {
                        return;
                    }
                }
            });
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
    tray_icon_alert(false)
}

/// 托盘图标。`alert = true` 时保留原图形，只在右下角叠一个带透明隔离环的
/// 红色告警点——位置与尺寸都不变，用户不必先点开菜单才知道管家挂了。
fn tray_icon_alert(alert: bool) -> gpui_tray::Result<Icon> {
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

    let mut pixels = rgba.to_vec();
    if alert {
        // 不整枚染红：那会把仪表盘压成一个红色剪影，内部细节全丢。保留原
        // 图形，只在右下角叠一个带透明隔离环的告警点——深浅菜单栏都能读。
        let size = SIZE as i32;
        let radius = (SIZE / 6).max(3) as i32;
        let ring = radius + 2;
        let center = size - radius - 1;
        const DOT: [u8; 3] = [0xE5, 0x48, 0x4D]; // #E5484D
        for y in 0..size {
            for x in 0..size {
                let dx = x - center;
                let dy = y - center;
                let distance = dx * dx + dy * dy;
                let index = ((y as u32 * SIZE + x as u32) * 4) as usize;
                if distance <= ring * ring && distance > radius * radius {
                    // 隔离环清成全透明：不让底色和图形糊在一起。
                    pixels[index..index + 4].copy_from_slice(&[0, 0, 0, 0]);
                } else if distance <= radius * radius {
                    // 位图是 premultiplied α：α=255 时 RGB 就是颜色本身。
                    pixels[index..index + 4].copy_from_slice(&[DOT[0], DOT[1], DOT[2], 0xFF]);
                }
            }
        }
    }
    Icon::from_rgba(pixels, SIZE, SIZE).build()
}

/// 把管家异常画到 Dock 图标上（`false` 恢复默认应用图标）。
///
/// 菜单栏状态项可能被系统整个藏进溢出区，Dock 是系统级稳定落点：红点不占
/// 菜单栏宽度，也不依赖托盘是否可见。只有宿主是 `Regular` 时才有 Dock 图标，
/// 所以必须由宿主调用；AppKit 只能在主线程碰，调用点都在 GPUI 的 update 里。
///
/// 这里换整枚图标而不是 `NSDockTile.badgeLabel`：文字角标走的是 per-bundle
/// 的 Dock 通道，在跑过多份副本（DMG 挂载、临时构建）的机器上会一直不刷新；
/// `applicationIconImage` 走图标通道，Dock 每次都会重绘。
#[cfg(target_os = "macos")]
fn set_dock_alert(alert: bool) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::NSData;

    // SAFETY: `sharedApplication` 一定存在（GPUI 已经建好 App）；图标是编译期
    // 常量，NSImage 自己持有解出来的位图；`alloc` 出来的对象最后 release。
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        if app.is_null() {
            return;
        }
        let image: *mut AnyObject = if alert {
            let data = NSData::with_bytes(include_bytes!("../../assets/icon-dock-alert.png"));
            let image: *mut AnyObject = msg_send![class!(NSImage), alloc];
            msg_send![image, initWithData: &*data as *const NSData as *mut AnyObject]
        } else {
            // nil 让 AppKit 回到 bundle 里的默认图标。
            std::ptr::null_mut()
        };
        let _: () = msg_send![app, setApplicationIconImage: image];
        if !image.is_null() {
            let _: () = msg_send![image, release];
        }
        eprintln!(
            "[mns-gui] Dock 图标：{}",
            if alert {
                "管家异常"
            } else {
                "恢复正常"
            }
        );
    }
}

#[cfg(not(target_os = "macos"))]
fn set_dock_alert(_alert: bool) {}

/// The app mark, embedded so the window never depends on the working directory.
pub(crate) fn brand_mark(size: f32) -> impl IntoElement {
    use std::sync::Arc;
    let image = Image::from_bytes(
        ImageFormat::Png,
        include_bytes!("../../assets/icon-128.png").to_vec(),
    );
    img(Arc::new(image)).w(px(size)).h(px(size)).flex_none()
}

fn open_main_window(cx: &mut App, page: Option<Page>, about: bool) -> Result<()> {
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
        focus: true,
        show: true,
        ..Default::default()
    };
    let (window_handle, view) = gpui_kit::open_window(options, cx, move |window, cx| {
        window.on_window_should_close(cx, |_window, cx| {
            // 关窗即退：先把 ACP 会话收尾并落盘，再让 LastWindowClosed
            // 结束整个窗口进程。
            if let Some(view) = cx.global::<GuiState>().view.clone() {
                view.update(cx, |shell, cx| shell.shutdown(cx));
            }
            true
        });
        let jobs_for_view = jobs.clone();
        cx.new(|cx| {
            let mut shell = AppShell::new(window, cx, jobs_for_view, page, about);
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

    /// "关于 MNS" lives in the macOS app menu. The host raises the main
    /// window process with `--about`, which sets `about_open` at construction.
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
                    .label(if form.searching {
                        "搜索中…"
                    } else {
                        "搜索"
                    })
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
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.select_fund_candidate(picked.clone(), cx);
                }))
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
                                .child(format!("{} · {}", candidate.fund_type, candidate.company)),
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
                    bordered_chip("asset-clear", "清除", TEXT_SECONDARY, SURFACE, BORDER)
                        .on_click(cx.listener(|shell, _, _, cx| shell.clear_fund_candidate(cx))),
                )
                .into_any_element(),
        );
    }

    // ── 类别：三选一，默认不选 ──────────────────────────────────────
    let suggested = form
        .selected
        .as_ref()
        .and_then(|candidate| candidate.suggested);
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
                .child(caption(
                    "或直接填写。6 位基金代码可留空名称，保存时自动带出。",
                ))
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

    dialog_shell("添加标的", "只记录成本，不代下单", body, form.error.clone()).child(
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
        AdviceKind, AgentSettings, AgentState, tray_advice_copy, tray_advice_kind, tray_line,
        tray_review_line,
    };
    use crate::brief::PlanBrief;

    fn plan(total_buy: f64, total_sell: f64, band_pp: f64) -> PlanBrief {
        PlanBrief {
            current_risk_pct: 58.0,
            target_risk_pct: 60.0,
            band_pp,
            total_buy,
            total_sell,
            has_orders: total_buy.abs() >= 1.0 || total_sell.abs() >= 1.0,
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
        let agent = AgentState::new();
        let mut settings = AgentSettings::default();
        settings.daily_review = true;
        settings.daily_review_time = "08:30".to_owned();
        assert_eq!(
            tray_review_line(Some(&agent), &settings, None, "2026-11-09"),
            "每日巡检 08:30 · 今日待触发"
        );
        assert_eq!(
            tray_review_line(Some(&agent), &settings, Some("2026-11-09"), "2026-11-09"),
            "每日巡检 08:30 · 今日已触发"
        );
        settings.daily_review = false;
        assert_eq!(
            tray_review_line(Some(&agent), &settings, None, "2026-11-09"),
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
