//! MNS desktop shell: a Fluent-inspired window over the shared engine.
//!
//! The shell owns presentation and interaction only. Every number it shows
//! comes from `crate::app`; every investment rule resolves to the same engine
//! modules the CLI uses. Background work runs in `jobs` and in the ACP runtime.

mod chart;
mod components;
mod jobs;
mod mood;
mod pages;
mod theme;

use chrono::Datelike;
use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState, TextareaState};
use gpui_kit::*;
use gpui_tray::{Icon, Tray};

use crate::agent::{
    AgentCommand, AgentEvent, AgentHandle, AgentSettings, DecisionReport, EntryKind, GoalSource,
    PermissionPolicy, PermissionPrompt, RunState, TimelineEntry, load_settings, routine,
    save_settings, start_runtime,
};
use crate::app::Snapshot;

use components::*;
use jobs::{Event as JobEvent, Jobs, Request};
use theme::*;

actions!(
    mns_gui,
    [
        ShowMainWindow,
        RunDefaultGoal,
        StopCurrentTask,
        QuitApplication,
        CloseAgentPanel
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

    pub(crate) fn subtitle(self) -> &'static str {
        match self {
            Page::Dashboard => "账户全貌与今天该不该动手",
            Page::Portfolio => "每一笔持仓的成本、现值与配置位置",
            Page::Plan => "按情绪与偏离生成的定投分配",
            Page::Performance => "金额加权收益、回撤与资金流水",
            Page::Agent => "每天看一眼账户，有结论再找你",
            Page::Settings => "风险档位、定投金额与外部 Agent",
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

pub(crate) struct AssetForm {
    pub code: Entity<InputState>,
    pub name: Entity<InputState>,
    pub category: Entity<InputState>,
    pub error: Option<String>,
}

pub(crate) enum Dialog {
    Trade(TradeForm),
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
}

struct GuiState {
    window: Option<AnyWindowHandle>,
    view: Option<Entity<AppShell>>,
    tray: Option<Tray>,
    jobs: Option<Jobs>,
    snapshot: Option<ShellSnapshot>,
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
    pub(crate) daily_time: Entity<InputState>,
    pub(crate) monthly: Entity<InputState>,
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
}

impl AppShell {
    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        jobs: Jobs,
        restore: Option<ShellSnapshot>,
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
        let daily_time = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("08:30")
                .default_value(agent_settings.daily_review_time.clone())
        });

        // A config read is a single small TOML file; loading the whole account
        // here would put a database read on the UI thread.
        let monthly_value = crate::config::AppConfig::load()
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

        Self {
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
            daily_time,
            monthly,
            last_daily: read_last_daily(),
            agent_panel: None,
            conversation_scroll: ScrollHandle::new(),
            agent_panel_focus: cx.focus_handle(),
        }
    }

    fn agent_snapshot(&self) -> ShellSnapshot {
        ShellSnapshot {
            agent: self.agent.snapshot(),
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
        self.page = page;
        // A drawer belongs to the page that opened it; leaving must not leave
        // it hanging around when the user comes back.
        self.agent_panel = None;
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
        window.focus(&self.agent_panel_focus, cx);
        cx.notify();
    }

    pub(crate) fn close_agent_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.agent_panel.take().is_some() {
            // Give the composer its caret back; otherwise closing a drawer
            // would leave the window with no focused input.
            let handle = if self.agent.open {
                self.followup.read(cx).focus_handle(cx)
            } else {
                self.goal.read(cx).focus_handle(cx)
            };
            window.focus(&handle, cx);
            cx.notify();
        }
    }

    pub(crate) fn toggle_daily_review(&mut self, cx: &mut Context<Self>) {
        self.agent_settings.daily_review = !self.agent_settings.daily_review;
        self.persist_daily_setting(cx);
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
                self.show_toast("外部 Agent 设置已保存".to_owned(), ToastTone::Success, cx);
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

    pub(crate) fn shutdown(&self) {
        let _ = self.agent_send(AgentCommand::Shutdown);
    }

    fn apply_job_event(&mut self, event: JobEvent, cx: &mut Context<Self>) {
        match event {
            JobEvent::Loaded(Ok(snapshot)) => {
                self.data = Some(snapshot);
                self.load_error = None;
                self.loading = false;
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
            JobEvent::Failed(error) => {
                if let Some(dialog) = self.dialog.as_mut() {
                    let slot = match dialog {
                        Dialog::Trade(form) => &mut form.error,
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
                let weekday = crate::app::weekday_label(now.weekday().num_days_from_monday());
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
        self.agent.apply(event);
        if grows {
            self.follow_conversation(false);
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

    pub(crate) fn submit_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialog.as_ref() else {
            return;
        };
        let parsed: Result<Request, String> = match dialog {
            Dialog::Trade(form) => {
                let code = form.code.read(cx).value().trim().to_owned();
                let shares = parse_number(&form.shares.read(cx).value());
                let price = parse_number(&form.price.read(cx).value());
                match (code.is_empty(), shares, price) {
                    (true, _, _) => Err("请填写资产代码".to_owned()),
                    (_, None, _) => Err("份额必须是数字".to_owned()),
                    (_, _, None) => Err("价格必须是数字".to_owned()),
                    _ => Ok(match form.side {
                        TradeSide::Buy => Request::Buy {
                            code,
                            shares: shares.unwrap_or(0.0),
                            price: price.unwrap_or(0.0),
                        },
                        TradeSide::Sell => Request::Sell {
                            code,
                            shares: shares.unwrap_or(0.0),
                            price: price.unwrap_or(0.0),
                        },
                    }),
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
                let code = form.code.read(cx).value().trim().to_owned();
                let name = form.name.read(cx).value().trim().to_owned();
                let category = form.category.read(cx).value().trim().to_owned();
                if code.is_empty() || name.is_empty() {
                    Err("代码与名称都要填写".to_owned())
                } else if !crate::app::CATEGORY_KEYS.contains(&category.as_str()) {
                    Err("分类填 us_stocks / cn_stocks / counter_cyclical".to_owned())
                } else {
                    Ok(Request::AddAsset {
                        code,
                        name,
                        category,
                    })
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
                Dialog::Cash(form) => &mut form.error,
                Dialog::AddAsset(form) => &mut form.error,
            };
            *slot = Some(message.clone());
        }
        self.show_toast(message, ToastTone::Error, cx);
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
    // Finder 启动时进程 cwd 是 `/`（macOS 系统卷只读）。所有自有路径都已
    // 改为绝对解析，这里再把 cwd 固定到应用数据目录，让外部 ACP agent 及
    // 其他子进程继承到的一律是可写位置。
    let data_dir = crate::config::app_data_dir();
    let _ = std::fs::create_dir_all(&data_dir);
    let _ = std::env::set_current_dir(&data_dir);

    let (agent, agent_events) = start_runtime();
    let (jobs, job_events) = Jobs::spawn();
    AGENT.with(|cell| *cell.borrow_mut() = Some(agent.clone()));

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx| {
            gpui_kit::init(cx);
            theme::apply(cx);
            cx.set_global(GuiState {
                window: None,
                view: None,
                tray: None,
                jobs: Some(jobs.clone()),
                snapshot: None,
            });

            register_actions(cx);

            if let Err(error) = setup_tray(cx) {
                eprintln!("[mns-gui] tray unavailable: {error}");
            }
            if let Err(error) = open_main_window(cx, None) {
                eprintln!("[mns-gui] failed to open window: {error}");
                cx.quit();
                return;
            }

            spawn_job_bridge(cx, job_events);
            spawn_agent_bridge(cx, agent_events);
            spawn_daily_scheduler(cx);
        });
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
                    view.update(cx, |shell, cx| shell.maybe_start_daily(cx));
                }
            });
        }
    })
    .detach();
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
                    // `build_tray_menu` reads the view. Refresh only after the
                    // update lease ends, otherwise it re-enters `AppShell`.
                    if let Some(tray) = cx.global::<GuiState>().tray.clone() {
                        let _ = tray.refresh_menu(cx);
                    }
                }
            });
        }
    })
    .detach();
}

fn register_actions(cx: &mut App) {
    // The drawer owns Esc while it is focused. A context-scoped binding beats
    // the text input's own Escape action, which would otherwise swallow the
    // key whenever the composer behind the drawer had the caret.
    cx.bind_keys([KeyBinding::new(
        "escape",
        CloseAgentPanel,
        Some("agent-panel"),
    )]);
    cx.on_action(|_: &ShowMainWindow, cx| {
        ensure_window(cx);
        cx.activate(true);
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
    cx.on_action(|_: &QuitApplication, cx| {
        if let Some(view) = cx.global::<GuiState>().view.clone() {
            view.update(cx, |shell, _cx| shell.shutdown());
        }
        if let Some(tray) = cx.global::<GuiState>().tray.clone() {
            let _ = tray.close(cx);
        }
        cx.quit();
    });
}

fn ensure_window(cx: &mut App) {
    if cx.global::<GuiState>().window.is_some() {
        return;
    }
    let snapshot = cx.global::<GuiState>().snapshot.clone();
    if let Err(error) = open_main_window(cx, snapshot) {
        eprintln!("[mns-gui] failed to reopen window: {error}");
    }
}

fn setup_tray(cx: &mut App) -> gpui_tray::Result<()> {
    let tray = Tray::builder()
        .icon(tray_icon()?)
        .title("MNS")
        .tooltip("Money Never Sleeps · 投资助手")
        .on_activate(ShowMainWindow)
        .menu(build_tray_menu)
        .build(cx)?;
    cx.global_mut::<GuiState>().tray = Some(tray);
    Ok(())
}

fn build_tray_menu(cx: &mut App) -> Vec<MenuItem> {
    let status = cx
        .global::<GuiState>()
        .view
        .as_ref()
        .map(|view| view.read(cx).agent.state)
        .unwrap_or_default();
    vec![
        MenuItem::action("打开 MNS", ShowMainWindow),
        MenuItem::separator(),
        MenuItem::action(format!("任务状态：{}", status.label()), NoAction).disabled(true),
        MenuItem::action("开始自主任务", RunDefaultGoal),
        MenuItem::action("停止当前任务", StopCurrentTask),
        MenuItem::separator(),
        MenuItem::action("退出 MNS", QuitApplication),
    ]
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

fn open_main_window(cx: &mut App, restore: Option<ShellSnapshot>) -> Result<()> {
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
            true
        });
        let jobs_for_view = jobs.clone();
        cx.new(|cx| {
            let mut shell = AppShell::new(window, cx, jobs_for_view, restore);
            // Load the account immediately; the shell shows its skeleton until
            // the worker answers, never a fabricated zero balance.
            shell.reload();
            shell
        })
    })?;
    let state = cx.global_mut::<GuiState>();
    state.window = Some(window_handle);
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
                    // say where you are inside it.
                    .child(
                        div()
                            .text_size(px(17.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.page.label()),
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
                .default_value(code)
        });
        let shares = cx.new(|cx| InputState::new(window, cx).placeholder("例如 1669.62"));
        let price = cx.new(|cx| InputState::new(window, cx).placeholder("例如 1.2018"));
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
        let code = cx.new(|cx| InputState::new(window, cx).placeholder("例如 007605"));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("例如 沪深300ETF联接"));
        let category = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("us_stocks / cn_stocks / counter_cyclical")
                .default_value("cn_stocks")
        });
        self.open_dialog(
            Dialog::AddAsset(AssetForm {
                code,
                name,
                category,
                error: None,
            }),
            cx,
        );
    }
}

fn dialog_shell(
    title: &'static str,
    subtitle: &'static str,
    body: Vec<Div>,
    error: Option<String>,
) -> Div {
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
        .child(
            div()
                .v_flex()
                .gap(px(3.))
                .child(
                    div()
                        .text_size(px(18.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(TEXT_SECONDARY))
                        .child(subtitle),
                ),
        )
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
        "记录一笔已经成交的交易，现金与持仓会同步更新",
        vec![
            field("代码", Input::new(&form.code)),
            div()
                .h_flex()
                .gap(px(10.))
                .child(
                    div()
                        .flex_1()
                        .child(field("份额", Input::new(&form.shares))),
                )
                .child(div().flex_1().child(field("价格", Input::new(&form.price)))),
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
                    .on_click(cx.listener(|shell, _, _, cx| shell.submit_dialog(cx))),
            ),
    )
}

fn cash_dialog(form: &CashForm, _window: &mut Window, cx: &mut Context<AppShell>) -> Div {
    let (title, subtitle) = match form.kind {
        CashKind::Deposit => ("入金", "外部资金进入账户，不计入收益，只抬高本金"),
        CashKind::Withdraw => ("出金", "资金离开账户，XIRR 会把它当作已实现的回收"),
    };
    dialog_shell(
        title,
        subtitle,
        vec![
            field("金额", Input::new(&form.amount)),
            field("备注", Input::new(&form.note)),
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
                    .on_click(cx.listener(|shell, _, _, cx| shell.submit_dialog(cx))),
            ),
    )
}

fn asset_dialog(form: &AssetForm, _window: &mut Window, cx: &mut Context<AppShell>) -> Div {
    dialog_shell(
        "添加标的",
        "先建标的，再用「记一笔」记录买入",
        vec![
            field("代码", Input::new(&form.code)),
            field("名称", Input::new(&form.name)),
            field("分类", Input::new(&form.category)),
        ],
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
                    .on_click(cx.listener(|shell, _, _, cx| shell.submit_dialog(cx))),
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
    }
}
