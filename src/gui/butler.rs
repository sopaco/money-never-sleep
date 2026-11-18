//! 管家独立窗口：人设化文本会话，与助手页互不共享存档。
//!
//! 这一层只做三件事：把管家已授权的结论摆出来，把用户的话接住，以及把
//! agent 的回答**边生成边流出来**——正文逐段上屏，收尾才落成正式消息。它不
//! 判断「该不该说」——那是 `butler::orchestrator` 的职责；也不改写数字——
//! 模板文案来自同源账本。外接 ACP 表达层接入后，只替换这里的回复来源。

use std::sync::{Arc, OnceLock};

use chrono::{Local, Timelike};
use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonRounded, ButtonVariants};
use gpui_kit::component::input::{Enter, Textarea, TextareaState};
use gpui_kit::component::text::TextView;
use gpui_kit::*;

use crate::agent::{AgentCommand, AgentEvent, AgentHandle, EntryKind, GoalSource};
use crate::butler::events::{self, page_label};
use crate::butler::expression;
use crate::butler::memory::{self, Fact, Forgot, Memory, Remembered};
use crate::butler::talk;
use crate::butler::{AccountFacts, Message, Role, Session, SpeakCandidate};

use super::components::{dot, pill, watermark};
use super::theme::*;
use super::{AppShell, GuiState};

/// 一轮问答的等待上限。
///
/// 实测一次完整查账（agent 自己去跑几条 `mns` 只读命令）约 60–70 秒，
/// 所以留到 120 秒。到点必须收尾：界面停在「正在想」、而用户又不能再提问，
/// 比给一个半截答案更糟。
const CHAT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// 账本数字的重算节奏。管家窗口常开，用户不该为了「几点了、偏离多少」去
/// 点一个按钮：每 60 秒跟主界面重算一次事实，没有变化就不重绘。
const FACTS_REFRESH: std::time::Duration = std::time::Duration::from_secs(60);

/// 从主窗口已有的快照提取管家事实。不读数据库：窗口打开时主界面已经加载
/// 过一遍，管家不该再走第二条读路径。
pub(crate) fn facts_for(shell: &AppShell, now: chrono::DateTime<Local>) -> AccountFacts {
    let Some(snapshot) = shell.data.as_ref() else {
        return AccountFacts::default();
    };
    let mut facts = AccountFacts::from_snapshot(snapshot, now);
    if let Ok(memory) = Memory::load() {
        facts = facts.with_promises(&memory, now.date_naive());
    }
    let since = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| naive.and_local_timezone(Local).single())
        .unwrap_or(now);
    let window = events::summarize(&events::read_all().unwrap_or_default(), since);
    if let Some((page, views)) = window.page_views.iter().max_by_key(|(_, views)| **views)
        && window.is_heavy_checking(page)
    {
        facts.heavy_page = Some((page_label(page).to_owned(), *views));
    }
    facts
}

/// 一轮问答的等待状态。抽出来是因为它能脱离真实进程测：`begin` 挡住并发
/// 提问，`take` 只让还在等的那一轮收下正文，`abandon` 让清空会话后的迟到
/// 正文落不进来。
#[derive(Default)]
struct TurnState {
    /// 本轮提问的序号。用户换话题就加一，迟到的回答按序号丢弃——
    /// 旧答案插进新话题里比不回答更让人困惑。
    turn: u64,
    /// 等待 agent 回答中。
    waiting: bool,
    /// 当前这一轮累积的正文。只收 Message，不收思考、计划与工具调用。
    /// 边收边进流式气泡；`take` 收尾时整体过一遍校验才落成正式消息。
    buffer: String,
    /// 这一问的本地模板：agent 失败、超时、越界，都回到它。
    template: String,
}

impl TurnState {
    /// 开始新的一轮。返回 false 说明上一条还没回完。
    fn begin(&mut self, template: String) -> bool {
        if self.waiting {
            return false;
        }
        self.turn = self.turn.wrapping_add(1);
        self.template = template;
        self.buffer.clear();
        self.waiting = true;
        true
    }

    fn push(&mut self, text: &str) {
        if self.waiting {
            self.buffer.push_str(text);
        }
    }

    /// 流式上屏的正文：等待中且已有一段，才值得画生长中的气泡。半截正文
    /// 不算「回答」，`take` 收尾后才落成正式消息。
    fn streaming_text(&self) -> Option<&str> {
        (self.waiting && !self.buffer.is_empty()).then_some(self.buffer.as_str())
    }

    /// 收下这一轮的正文与模板。返回 `None` 表示这一轮已经作废。
    fn take(&mut self) -> Option<(String, String)> {
        if !self.waiting {
            return None;
        }
        self.waiting = false;
        Some((
            std::mem::take(&mut self.buffer),
            std::mem::take(&mut self.template),
        ))
    }

    /// 丢弃这一轮：会话被清空时用。
    fn abandon(&mut self) {
        self.turn = self.turn.wrapping_add(1);
        self.waiting = false;
        self.buffer.clear();
        self.template.clear();
    }
}

/// 管家对话的 ACP 会话。
///
/// 每个管家窗口持有**独立**的 runtime：GUI 里已经有一个给每日复盘用的
/// runtime，而 `AgentCommand::Start` 在当前任务未结束时直接拒绝，复用会让
/// 管家和复盘互相抢会话。窗口关闭时发 `Shutdown`，线程随之结束。
///
/// MNS 不持有推理端点与密钥：provider、模型、凭据都在外部 agent 自己的
/// 配置里，这里只递一段账本上下文和一句提问。
struct Chat {
    handle: AgentHandle,
    /// 是否已经开过会话。第一问要带人设与账本事实，之后复用同一 session。
    started: bool,
}

impl Chat {
    fn spawn() -> (Self, tokio::sync::mpsc::UnboundedReceiver<AgentEvent>) {
        let (handle, events) = crate::agent::start_runtime();
        (
            Self {
                handle,
                started: false,
            },
            events,
        )
    }
}

/// 管家会话的 agent 设置。
///
/// 工作目录固定到管家自己的数据目录：opencode 会读取工作目录下的
/// `AGENTS.md`，落在项目仓库里时它会把「每日复盘」那套项目约定带进来
/// （实测会多吐一段 `mns-decision` 块，和正文重复）。管家不处理文件，
/// 也不该继承那套契约。
fn chat_settings() -> crate::agent::AgentSettings {
    let mut settings = crate::agent::load_settings();
    let dir = crate::config::app_data_dir().join("agent").join("butler");
    let _ = std::fs::create_dir_all(&dir);
    settings.working_dir = dir.to_string_lossy().into_owned();
    settings
}

pub(crate) struct ButlerView {
    session: Session,
    memory: Memory,
    /// 记忆文件读不出来时只读不写，避免用空记忆覆盖用户原有条目。
    memory_readable: bool,
    facts: AccountFacts,
    input: Entity<TextareaState>,
    scroll: ScrollHandle,
    error: Option<String>,
    clear_armed: bool,
    /// 关掉后所有写入只留在内存里。窗口正常打开时是 `true`；测试注入
    /// 会话时是 `false`，避免为了验证一段文案去改真实账户目录。
    persist: bool,
    /// ACP 对话会话。`None` 只出现在测试注入的窗口里：验证不该真去起一个
    /// 外部 agent 进程。
    chat: Option<Chat>,
    /// 当前这一轮的等待状态。放在视图层而不是 `Chat` 里：流式上屏后渲染
    /// 每帧都要读它，测试也要能不启进程就驱动它。
    turn: TurnState,
    /// 事件接收端。由打开窗口的一方取走并挂到 GPUI 的异步任务上——
    /// 构造期拿不到 `Entity`，所以先存在这里。
    chat_events: Option<tokio::sync::mpsc::UnboundedReceiver<AgentEvent>>,
    /// 回答来源的一行说明。只在退回模板时出现，避免每句都挂个来源标签。
    hint: Option<String>,
}

impl ButlerView {
    pub(crate) fn new(facts: AccountFacts, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let now = Local::now();
        // 存档损坏时**不**覆盖原文件：那可能是用户唯一的一份对话记录。
        // 本次只留在内存里，并把原因摆到界面上。
        let view = match Session::load() {
            Ok(mut session) => {
                // 打开即视为已读：未读数只用于托盘提示，不是红点游戏。
                session.mark_read(now);
                let _ = session.save();
                let mut view = Self::with_session(session, facts, window, cx).with_agent();
                view.persist = true;
                match Memory::load() {
                    Ok(memory) => view.memory = memory,
                    Err(error) => {
                        view.memory_readable = false;
                        view.error = Some(format!("记忆文件读取失败，本次只读不写：{error:#}"));
                    }
                }
                view
            }
            Err(error) => {
                let mut session = Session::default();
                session.mark_read(now);
                let mut view = Self::with_session(session, facts, window, cx).with_agent();
                view.error = Some(format!(
                    "会话存档读取失败，本次对话不会覆盖原文件：{error:#}"
                ));
                view
            }
        };
        // 数字自动跟随主界面：没有手动刷新按钮。
        view.start_facts_refresh(cx);
        view
    }

    /// 挂上 ACP 会话。只有真实窗口走这条：测试注入的会话不启进程。
    fn with_agent(mut self) -> Self {
        let (chat, events) = Chat::spawn();
        self.chat = Some(chat);
        self.chat_events = Some(events);
        self
    }

    /// 打开窗口的一方取走事件流，挂到 GPUI 的异步任务上。
    pub(crate) fn take_chat_events(
        &mut self,
    ) -> Option<tokio::sync::mpsc::UnboundedReceiver<AgentEvent>> {
        self.chat_events.take()
    }

    /// 会话来源与磁盘解耦：测试要的是「发送后会话与输入框的状态」，
    /// 不该顺带读写真实账户目录。
    fn with_session(
        session: Session,
        facts: AccountFacts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            // 聊天框的直觉是回车即发送：多行模式默认把回车当换行，
            // 打开 submit_on_enter 后普通 Enter 会冒泡出去（由 composer 接住），
            // Shift+Enter 仍然是换行。
            TextareaState::new(window, cx)
                .placeholder("问管家点什么…")
                .submit_on_enter(true)
        });
        Self {
            session,
            memory: Memory::default(),
            memory_readable: true,
            facts,
            input,
            scroll: ScrollHandle::new(),
            error: None,
            clear_armed: false,
            persist: false,
            chat: None,
            turn: TurnState::default(),
            chat_events: None,
            hint: None,
        }
    }

    /// 托盘/定时巡检判定开口后，把结论送进已经打开的窗口。
    ///
    /// `seen` 只在窗口真的是当前焦点时为 true：窗口存在但不活跃（在后面、
    /// 最小化、⌘W 隐藏）时，消息要保留未读并继续走横幅，不能悄悄算已读。
    pub(crate) fn receive(&mut self, candidate: &SpeakCandidate, seen: bool) {
        let now = Local::now();
        self.session.push(Message {
            at: now,
            role: Role::Butler,
            text: candidate.draft.clone(),
            reason: Some(candidate.reason),
            trigger: Some(candidate.trigger.clone()),
        });
        if seen {
            self.session.mark_read(now);
        }
        let _ = self.persist();
        self.scroll.scroll_to_bottom();
    }

    /// 接收守护进程判定过的开口。
    ///
    /// 守护进程只往 `pending.jsonl` 追加，会话始终由 GUI 写——两个进程同时
    /// 写一份 JSON 迟早会互相覆盖。
    ///
    /// 返回是否落盘成功：调用方要据此决定能不能推进 pending 偏移，否则
    /// 保存失败时消息已丢、偏移已过，不会重投。
    pub(crate) fn receive_notice(
        &mut self,
        notice: &crate::butler::orchestrator::Notice,
        seen: bool,
    ) -> bool {
        self.session.push(Message {
            at: notice.at,
            role: Role::Butler,
            text: notice.draft.clone(),
            reason: Some(notice.reason),
            trigger: Some(notice.trigger.clone()),
        });
        if seen {
            self.session.mark_read(Local::now());
        }
        let saved = self.persist();
        self.scroll.scroll_to_bottom();
        saved
    }

    fn persist(&mut self) -> bool {
        if !self.persist {
            return true;
        }
        match self.session.save() {
            Ok(()) => true,
            Err(error) => {
                self.error = Some(format!("会话保存失败：{error:#}"));
                false
            }
        }
    }

    fn refresh_facts(&mut self, cx: &mut Context<Self>) {
        let now = Local::now();
        let Some(view) = cx.global::<GuiState>().view.clone() else {
            return;
        };
        let next = facts_for(&view.read(cx), now);
        self.apply_facts(next, cx);
    }

    /// 应用一份新事实。数字没变就不通知——60 秒一次的轮询多数时候什么都
    /// 没变，重绘整窗只会让打字光标闪一下。
    fn apply_facts(&mut self, next: AccountFacts, cx: &mut Context<Self>) -> bool {
        if next == self.facts && self.error.is_none() {
            return false;
        }
        self.facts = next;
        self.error = None;
        cx.notify();
        true
    }

    /// 真实窗口专用：定时重算事实。测试注入的视图（`with_session`）不挂
    /// 这条循环，测试因此不会真的去读主窗口。
    fn start_facts_refresh(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |weak, async_cx| {
            loop {
                async_cx.background_executor().timer(FACTS_REFRESH).await;
                // 窗口关掉后 update 会失败：这时退出，别留一个空转任务。
                if weak
                    .update(async_cx, |view, cx| view.refresh_facts(cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// 「记住… / 忘掉…」走确定性记忆，不进普通问答。
    ///
    /// 返回 `Some(回复)` 表示这句话已被记忆层接住。识别不了就返回 `None`，
    /// 交给模板问答——不能让「周五要不要补仓」被当成一条承诺记下来。
    fn handle_memory(&mut self, text: &str, now: chrono::DateTime<Local>) -> Option<String> {
        let intent = memory::parse_intent(text, now.date_naive())?;
        if !self.memory_readable {
            return Some("记忆文件这次读不出来，我没记，也没动原有记录。".to_owned());
        }
        match intent {
            memory::Intent::Remember { kind, text, due } => {
                let mut fact = Fact::new(kind, text.clone(), now);
                if let Some(day) = due {
                    fact = fact.expires_on(day);
                }
                match self.update_memory(|memory| memory.remember(fact)) {
                    Ok(verdict) => {
                        self.facts = self
                            .facts
                            .clone()
                            .with_promises(&self.memory, now.date_naive());
                        let due_note = match due {
                            Some(day) => format!("（到期 {}）", day.format("%Y-%m-%d")),
                            None => String::new(),
                        };
                        let reply = match verdict {
                            Remembered::Merged => {
                                format!(
                                    "这条我记过了，又确认了一次：{}{}。",
                                    clip(&text, 40),
                                    due_note
                                )
                            }
                            _ => format!(
                                "记下了：{}{}。要改口径就说「忘掉…」。",
                                clip(&text, 40),
                                due_note
                            ),
                        };
                        Some(valid_or(reply, "记下了。"))
                    }
                    Err(error) => {
                        self.error = Some(error);
                        Some("记忆没写进磁盘，这次只在内存里生效。".to_owned())
                    }
                }
            }
            memory::Intent::Forget { needle, due } => {
                match self.update_memory(|memory| memory.forget(&needle, due)) {
                    Ok(Forgot::NotFound) => {
                        let reply = format!("没找到和「{}」对得上的记忆。", clip(&needle, 20));
                        Some(valid_or(reply, "没找到对得上的记忆，我没动其他记录。"))
                    }
                    Ok(Forgot::Removed(hits)) => {
                        self.facts = self
                            .facts
                            .clone()
                            .with_promises(&self.memory, now.date_naive());
                        let names: Vec<String> = hits.iter().take(3).map(fact_label).collect();
                        let reply = format!(
                            "从常驻记忆里拿掉了：{}。归档里还留一份，随时能查。",
                            names.join("；")
                        );
                        Some(valid_or(
                            reply,
                            "从常驻记忆里拿掉了。归档里还留一份，随时能查。",
                        ))
                    }
                    Ok(Forgot::Ambiguous(candidates)) => {
                        // 宁可不删也不误删：候选带日期念出来，等用户指明那条。
                        let names: Vec<String> =
                            candidates.iter().take(3).map(fact_label).collect();
                        let reply = format!(
                            "「{}」对上了几条：{}。带上日期说要忘哪条，比如「忘掉下周五补美股」。",
                            clip(&needle, 20),
                            names.join("；")
                        );
                        Some(valid_or(
                            reply,
                            "这句对上了不止一条，我没动。带上日期说要忘哪条，比如「忘掉下周五补美股」。",
                        ))
                    }
                    Err(error) => {
                        self.error = Some(error);
                        Some("记忆没写进磁盘，这次只在内存里生效。".to_owned())
                    }
                }
            }
        }
    }

    /// 在写锁内「重读记忆 → 应用这一笔变更 → 落盘」，返回应用的返回值。
    ///
    /// 窗口一开几个小时，期间 CLI（`mns-butler remember`）和守护进程都可能
    /// 写过 `memory.json`：拿窗口打开时的快照整体覆盖会吞掉它们的条目，
    /// 所以每次变更都从磁盘重读。测试注入的窗口（`persist = false`）只在
    /// 内存里生效，不碰真实账户目录。
    ///
    /// 失败时这笔变更哪儿都没落（磁盘没动、内存副本保持旧值）：错误会摆到
    /// 界面上，用户至少知道「这条没记上」，而不是以为记住了。
    fn update_memory<R>(&mut self, apply: impl FnOnce(&mut Memory) -> R) -> Result<R, String> {
        if !self.persist {
            return Ok(apply(&mut self.memory));
        }
        let (memory, result) =
            Memory::update(apply).map_err(|error| format!("记忆更新失败：{error:#}"))?;
        self.memory = memory;
        // 承诺变了：到期的「那一天」可能提前或推后，守护进程要重新排程。
        crate::ipc::poke_daemon();
        Ok(result)
    }

    /// 「你记得什么」：把常驻记忆念出来。这是对话的一部分，不该落到兜底句。
    ///
    /// 只念条目原文，不做总结——总结会把用户的措辞磨平成另一套说法，下一轮
    /// 用户再问「我说的是哪句」就对不上了。
    fn recall_memory(&self, text: &str) -> Option<String> {
        let asks = ["记得", "记了", "记忆", "承诺"]
            .iter()
            .any(|word| text.contains(word));
        if !asks {
            return None;
        }
        if !self.memory_readable {
            return Some("记忆文件这次读不出来，我念不了，也不会拿空记忆糊弄你。".to_owned());
        }
        let today = Local::now().date_naive();
        let active = self.memory.active_facts(today);
        if active.is_empty() {
            return Some("常驻记忆里还是空的。说「记住：…」我就记一条。".to_owned());
        }
        let rows: Vec<String> = active
            .iter()
            .take(5)
            .map(|fact| match fact.expires_at {
                Some(day) => format!("{} · 到期 {}", clip(&fact.text, 24), day.format("%Y-%m-%d")),
                None => clip(&fact.text, 24),
            })
            .collect();
        Some(valid_or(
            format!(
                "我记着 {} 条：{}。要删就说「忘掉…」。",
                active.len(),
                rows.join("；")
            ),
            "记忆里有内容，但这次念不出来。",
        ))
    }

    /// 上一句用户提问。当前这句已经进了会话，所以取倒数第二条。
    fn previous_user_text(&self) -> Option<String> {
        self.session
            .messages
            .iter()
            .rev()
            .filter(|message| message.role == Role::User)
            .nth(1)
            .map(|message| message.text.clone())
    }

    /// 把一个现成问题放进输入框并立刻发送。空态的 suggestion chip 用它：
    /// 点击即开口，不必先点一下输入框再按 Enter。走的是和手动发送同一条
    /// `send` 路径，所以记忆、模板、agent 分支都不会被绕过。
    fn ask(&mut self, question: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_value(question, window, cx));
        self.send(window, cx);
    }

    /// 空态：问候 + 三个一键提问。chips 直接用 `ask` 发送，不经过「先填进
    /// 输入框」这一步——空窗口里用户要的是最短路径，不是先学会怎么用。
    fn empty_state(&self, cx: &mut Context<Self>) -> Div {
        let mut chips = div()
            .h_flex()
            .flex_wrap()
            .items_center()
            .justify_center()
            .gap(px(8.));
        for (id, question) in SUGGESTIONS {
            chips = chips.child(
                div()
                    .id(id)
                    // 只在 debug/test 构建里记录边界，供点击路径的测试定位；
                    // 生产构建是空操作。
                    .debug_selector(move || id.to_owned())
                    .child(
                        Button::new(id)
                            .custom(
                                ButtonCustomVariant::new(cx)
                                    .color(rgb(ACCENT_TINT).into())
                                    .foreground(rgb(ACCENT).into())
                                    .hover(rgb(ACCENT_SOFT).into()),
                            )
                            .rounded(ButtonRounded::Size(px(999.)))
                            .label(question)
                            .on_click(cx.listener(move |view, _, window, cx| {
                                view.ask(question, window, cx);
                            })),
                    ),
            );
        }

        div()
            .relative()
            .v_flex()
            .flex_1()
            .min_h(px(220.))
            .w_full()
            .items_center()
            // 空态不再垂直居中：窗口只有 640 高时居中会在问候上方留下
            // 一大片空白，看起来像内容没加载出来。距顶部 36px 起步。
            .justify_start()
            .pt(px(36.))
            .gap(px(10.))
            .child(watermark(IconName::Sparkles, ACCENT, 180., 0.05))
            .child(empty_badge())
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(TEXT))
                    .child(format!("{}，我是管家。", greeting_for(Local::now().hour()))),
            )
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(rgb(TEXT_SECONDARY))
                    .child("账本、计划、市场，你想先聊哪个？"),
            )
            .child(chips)
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().trim().to_owned();
        if text.is_empty() {
            return;
        }
        let now = Local::now();
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));

        // 记忆类命令在本地就能答：不必为一句「记住了」启一个外部 agent。
        if let Some(reply) = self
            .handle_memory(&text, now)
            .or_else(|| self.recall_memory(&text))
        {
            self.session.push(Message::user(now, text));
            self.session.push(Message::butler(now, reply));
            self.session.mark_read(now);
            self.persist();
            self.scroll.scroll_to_bottom();
            cx.notify();
            return;
        }

        // 普通对话记一句「聊过什么」，供下周自整理。只记用户的问题，
        // 不记管家自己的措辞——复述自己的话会让记忆越走越偏。写之前重读
        // 磁盘：窗口开着的时候 CLI / 守护进程可能也写过记忆。
        let _ = self.update_memory(|memory| memory.summarize(now, vec![clip(&text, 60)]));
        // 首轮注入的取数口径：记忆取**刚重读过的**磁盘快照——CLI / 守护
        // 进程写进来的条目也要进上下文；最近对话取本句入档之前的——正在
        // 问的这句已经在【用户的问题】里，不该在【最近的对话】里再念一遍。
        let memory_for_goal = talk::memory_lines(&self.memory, now.date_naive());
        let recent_for_goal = talk::recent_lines(&self.session);
        self.session.push(Message::user(now, text.clone()));

        let previous = self.previous_user_text();
        let intent = expression::intent_for(&text, previous.as_deref());
        // 模板是这一问的基线：agent 答不出来、超时、答得不像话，都回到它。
        // 预测与观点类问题不再本地拦截——管家人格可以聊倾向，交给 agent；
        // 模板兜底自己会如实说「说不准，只答有把握的部分」。
        let template = expression::reply_with_intent(intent, &text, &self.facts);

        if self.turn.waiting {
            // 上一条还没回完。不发新请求，也不吞掉这一句：先用模板答，
            // 并说明为什么——把用户的话丢掉比答得糙更糟。
            self.finish_local(
                now,
                template,
                Some("上一条还在回答，这一句先按本地模板答".to_owned()),
                cx,
            );
            return;
        }

        let Some(chat) = self.chat.as_mut() else {
            // 测试注入的窗口没有 agent：直接走模板。
            self.finish_local(now, template, None, cx);
            return;
        };

        // 上面已确认不在等待，`begin` 必定接住这一轮。
        self.turn.begin(template);

        let command = if chat.started {
            AgentCommand::Ask { text }
        } else {
            chat.started = true;
            AgentCommand::Start {
                goal: talk::opening_goal(&text, &self.facts, &memory_for_goal, &recent_for_goal),
                settings: chat_settings(),
                source: GoalSource::Conversation,
            }
        };
        if let Err(error) = chat.handle.send(command) {
            // 通道断了就当场降级，别让界面一直停在「正在想」。
            self.settle(Some(format!("agent 没接上（{error}）")), cx);
            return;
        }
        // 超时兜底：agent 卡住时 `waiting` 会一直是 true，用户连下一句都
        // 发不出去。定时器按这一轮的序号收尾，答完了序号就对不上，空转一次。
        let turn = self.turn.turn;
        cx.spawn(async move |weak, async_cx| {
            async_cx.background_executor().timer(CHAT_TIMEOUT).await;
            let _ = weak.update(async_cx, |view, cx| view.on_chat_timeout(turn, cx));
        })
        .detach();
        self.session.mark_read(now);
        self.persist();
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// 一轮超时：停掉这次生成，收下已经吐出的半截，没有就退回模板。
    ///
    /// 序号对不上说明已经答完、或者用户换了话题，直接返回。
    fn on_chat_timeout(&mut self, turn: u64, cx: &mut Context<Self>) {
        if !self.turn.waiting || self.turn.turn != turn {
            return;
        }
        // 被 cancel 的会话不再复用：下一问重新开一个，免得接在半截上下文上。
        if let Some(chat) = self.chat.as_mut() {
            let _ = chat.handle.send(AgentCommand::Stop);
            chat.started = false;
        }
        self.settle(
            Some(format!("agent 超过 {} 秒没答完", CHAT_TIMEOUT.as_secs())),
            cx,
        );
    }

    /// 不经 agent 直接落一句回答（记忆命令、预测拒答、测试窗口）。
    fn finish_local(
        &mut self,
        now: chrono::DateTime<Local>,
        text: String,
        hint: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.session.push(Message::butler(now, text));
        self.session.mark_read(now);
        self.hint = hint;
        self.persist();
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// 流式期间的跟随滚动：正文在长，贴底就跟着走；用户往上翻了就不打扰。
    /// 与主界面 `follow_conversation` 同一条规则。
    fn follow_stream(&self) {
        let near_bottom = self.scroll.max_offset().y + self.scroll.offset().y <= px(96.);
        if near_bottom {
            self.scroll.scroll_to_bottom();
        }
    }

    /// runtime 事件。只关心正文、结束与失败：思考、计划、工具调用不进会话，
    /// 用户要的是答案，不是执行轨迹。
    pub(crate) fn on_chat_event(&mut self, event: AgentEvent, cx: &mut Context<Self>) {
        match event {
            // 正文只在 `Delta` 上出现（runner 对 Message 走流式），不重复收
            // `Entry`，否则同一段话会被拼两遍。边收边上屏：气泡逐段生长，
            // `settle` 收尾后才落成正式消息。
            AgentEvent::Delta {
                kind: EntryKind::Message,
                text,
            } => {
                if self.turn.waiting {
                    self.turn.push(&text);
                    // 红线实时切断：谎称能替你下单的话不能在屏上挂到本轮自然
                    // 结束。判定的对象与 `validate_talk` 同一份清洗文本，所以
                    // 收尾校验会放行的回答（「没法帮你下单」、决策块内部出现
                    // 的字样）在这里同样不会命中；命中即等于收尾必被拦，只是
                    // 提前到几十秒之前。
                    if crate::butler::schedule::claims_execution(&talk::display_text(
                        &self.turn.buffer,
                    ))
                    .is_some()
                    {
                        if let Some(chat) = self.chat.as_mut() {
                            // 被 cut 的会话不再复用：下一问重新开一个。
                            let _ = chat.handle.send(AgentCommand::Stop);
                            chat.started = false;
                        }
                        self.settle(Some("已实时切断".to_owned()), cx);
                        return;
                    }
                    self.follow_stream();
                    cx.notify();
                }
            }
            AgentEvent::TurnFinished { .. } => self.settle(None, cx),
            AgentEvent::Finished { .. } => {
                // 会话本身结束了：下一问要重新开一个，不能对着已关闭的
                // session 发 Ask。
                if let Some(chat) = self.chat.as_mut() {
                    chat.started = false;
                }
                // 这一轮还没收尾就按失败处理，不能让界面一直停在等待态。
                if self.turn.waiting {
                    self.settle(Some("agent 会话提前结束".to_owned()), cx);
                }
            }
            AgentEvent::Failed { message } => {
                if let Some(chat) = self.chat.as_mut() {
                    chat.started = false;
                }
                self.settle(Some(format!("agent 报错：{message}")), cx);
            }
            AgentEvent::Permission(prompt) => {
                // 管家窗口没有授权界面。拒绝并说清楚去哪儿处理，避免
                // agent 卡在等待授权上不给回答。
                if let Some(chat) = self.chat.as_ref() {
                    let _ = chat.handle.send(AgentCommand::Deny { id: prompt.id });
                }
                self.hint = Some(format!(
                    "agent 想调用「{}」，管家窗口不受理授权，已拒绝",
                    prompt.title
                ));
                cx.notify();
            }
            _ => {}
        }
    }

    /// 把这一轮收尾：通过校验就用 agent 的文本，否则退回本地模板。
    fn settle(&mut self, note: Option<String>, cx: &mut Context<Self>) {
        let Some((buffer, template)) = self.turn.take() else {
            return;
        };
        let now = Local::now();
        let (text, hint) = settle_text(&buffer, &template, note.as_deref());
        self.session.push(Message::butler(now, text));
        self.session.mark_read(now);
        // 错误链可能很长，提示只留一行：用户只需要知道这条为什么不是
        // agent 说的。
        self.hint = hint.map(clip_note);
        self.persist();
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn clear_session(&mut self, cx: &mut Context<Self>) {
        if !self.clear_armed {
            self.clear_armed = true;
            cx.notify();
            return;
        }
        if let Err(error) = self.wipe_session() {
            self.error = Some(format!("清除失败：{error:#}"));
        } else {
            self.session = Session::default();
            self.session.mark_read(Local::now());
            // 清空会话也要把 ACP 那头的 session 一并作废：只删本地存档
            // 的话，agent 仍持有清空前的上下文，下一句会以续问接在旧对话
            // 上——用户以为清了，其实没清。`started` 归位后下一问重新走
            // Start（带人设与记忆的首轮），而不是 Ask（续问）。
            if let Some(chat) = self.chat.as_mut() {
                let _ = chat.handle.send(AgentCommand::Stop);
                chat.started = false;
            }
            // 清空会话也要清掉等待中的回答（连同屏幕上的半截气泡）：它接
            // 的是已经被删掉的上下文。序号加一就够了，迟到的正文落不进
            // 新会话。
            self.turn.abandon();
            self.hint = None;
            if self.persist {
                let _ = self.session.save();
            }
            self.clear_armed = false;
        }
        cx.notify();
    }

    fn wipe_session(&mut self) -> anyhow::Result<()> {
        if self.persist {
            Session::clear()?;
        }
        Ok(())
    }
}

/// 窗口关掉就把 ACP 会话收掉。`start_runtime` 起的是真实 OS 线程 + Tokio
/// runtime，不显式 `Shutdown` 会随窗口开关泄漏下去。
impl Drop for ButlerView {
    fn drop(&mut self) {
        if let Some(chat) = &self.chat {
            let _ = chat.handle.send(AgentCommand::Shutdown);
        }
    }
}

/// 收尾判定：通过校验就用 agent 的正文，否则退回模板并说明原因。
///
/// `note` 表示「这一轮为什么不算正常回答」（超时、报错）。内容即便合规也
/// 要把它附上，否则用户看到一句正常的话，却不知道 agent 其实没答完。
///
/// 抽成纯函数是为了能测——真实路径需要起一个外部 agent 进程。
fn settle_text(buffer: &str, template: &str, note: Option<&str>) -> (String, Option<String>) {
    match talk::validate_talk(buffer) {
        Ok(text) => (text, note.map(str::to_owned)),
        Err(error) => (
            template.to_owned(),
            Some(match note {
                Some(note) => format!("{note}：{error}"),
                None => format!("agent 这条没通过校验（{error}），已改回本地模板"),
            }),
        ),
    }
}

impl Render for ButlerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let headline = expression::headline(&self.facts);
        let decision = if self.facts.decision_day {
            "今天是决策日"
        } else {
            "今天不是决策日"
        };

        let mut stream = div()
            .id("butler-conversation")
            .v_flex()
            .flex_1()
            .min_h(px(0.))
            .gap(px(18.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px(px(22.))
            .pt(px(14.))
            .pb(px(20.));

        if self.session.messages.is_empty() {
            stream = stream.child(self.empty_state(cx));
        } else {
            stream = stream.children(
                self.session
                    .messages
                    .iter()
                    .enumerate()
                    .map(|(index, message)| message_row(index, message))
                    .collect::<Vec<_>>(),
            );
        }
        // 流式正文：第一段 Delta 到达后，气泡开始逐段生长，收尾（settle）
        // 通过校验才落成正式消息。
        let streaming = self.turn.streaming_text();
        if let Some(text) = streaming {
            stream = stream.child(streaming_row(text));
        }
        // 等 agent 时给一行状态，否则用户分不清「正在想」和「没反应」。
        // 只在第一段正文到达前显示：正文开始流动后，气泡本身就是进度
        // （正文停住的空档属于「工具活动」提案，不在 V1）。
        if self.turn.waiting && streaming.is_none() {
            stream = stream.child(
                div()
                    .debug_selector(|| "butler-thinking".to_owned())
                    .h_flex()
                    .items_center()
                    .gap(px(8.))
                    .child(dot(ACCENT))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(rgb(TEXT_SECONDARY))
                            .child("管家正在想…"),
                    ),
            );
        }
        if let Some(hint) = &self.hint {
            stream = stream.child(
                div()
                    .text_size(px(12.5))
                    .text_color(rgb(TEXT_TERTIARY))
                    .child(hint.clone()),
            );
        }
        if let Some(error) = &self.error {
            stream = stream.child(
                div()
                    .rounded(px(RADIUS))
                    .bg(rgb(DANGER_SOFT))
                    .px(px(12.))
                    .py(px(8.))
                    .text_size(px(12.5))
                    .text_color(rgb(DANGER))
                    .child(error.clone()),
            );
        }

        div()
            .id("butler-root")
            .v_flex()
            .h_full()
            .w_full()
            // 晨光书房的底：斜向渐变打底，纸纹只做材质。绝对定位的纹理
            // 不参与布局，后面两个子节点自然盖在它上面；根节点裁边。
            .relative()
            .overflow_hidden()
            .bg(linear_gradient(
                160.,
                linear_color_stop(rgb(0xfffbf4), 0.),
                linear_color_stop(rgb(0xeef3fb), 1.),
            ))
            .child(
                img(butler_texture())
                    .absolute()
                    .top(px(0.))
                    .left(px(0.))
                    .w_full()
                    .h_full()
                    .object_fit(ObjectFit::Cover),
            )
            .child(
                div()
                    .id("butler-header")
                    .debug_selector(|| "butler-header".to_owned())
                    .v_flex()
                    .flex_none()
                    .px(px(22.))
                    .pt(px(14.))
                    .pb(px(5.))
                    .child(
                        div()
                            .debug_selector(|| "butler-header-row".to_owned())
                            .h_flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.))
                            .child(
                                div()
                                    .h_flex()
                                    .items_center()
                                    .gap(px(12.))
                                    .min_w(px(0.))
                                    .flex_1()
                                    .child(header_badge())
                                    .child(
                                        div()
                                            .v_flex()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .gap(px(1.))
                                            .child(
                                                div()
                                                    .text_size(px(15.5))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(TEXT))
                                                    .child("管家"),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.5))
                                                    .text_color(rgb(TEXT_SECONDARY))
                                                    .overflow_hidden()
                                                    .text_ellipsis()
                                                    .child(format!("{decision} · {headline}")),
                                            ),
                                    )
                                    // 「只读账本」全场只留这一处，且是说明不是状态：
                                    // 用中性色，不再抢品牌蓝。
                                    .child(
                                        pill("本地 · 只读账本", TEXT_SECONDARY, NEUTRAL_SOFT)
                                            .flex_none(),
                                    ),
                            )
                            .child(
                                div().h_flex().flex_none().items_center().gap(px(4.)).child(
                                    Button::new("butler-clear")
                                        .compact()
                                        .ghost()
                                        .label(if self.clear_armed {
                                            "确认清除"
                                        } else {
                                            "清除会话"
                                        })
                                        .on_click(cx.listener(|view, _, _, cx| {
                                            view.clear_session(cx);
                                        })),
                                ),
                            ),
                    )
                    .child(
                        // 只画一半的 1px 分隔线：从透明渐到 DIVIDER。
                        div().h(px(1.)).w_full().mt(px(6.)).bg(linear_gradient(
                            90.,
                            linear_color_stop(rgb(DIVIDER).opacity(0.), 0.),
                            linear_color_stop(rgb(DIVIDER), 1.),
                        )),
                    ),
            )
            .child(
                div()
                    .id("butler-card")
                    .debug_selector(|| "butler-card".to_owned())
                    .v_flex()
                    .flex_1()
                    .min_h(px(0.))
                    .mt(px(6.))
                    .mx(px(16.))
                    .mb(px(16.))
                    .rounded(px(RADIUS_LG))
                    // 半透明白卡：纹理能从卡片底下透上来，正文底色仍接近
                    // 纯白，对比度不动。实心白会把纸纹挡得只剩窗口四条边。
                    .bg(rgb(SURFACE).opacity(0.74))
                    .border_1()
                    .border_color(rgb(0xffffff).opacity(0.85))
                    .shadow(shadow_card())
                    .overflow_hidden()
                    .child(stream)
                    .child(
                        div()
                            .v_flex()
                            .flex_none()
                            .px(px(14.))
                            .pb(px(14.))
                            .pt(px(8.))
                            // 输入框把普通 Enter 冒泡出来，这里接住再转成发送。
                            // Shift+Enter 在输入框里就被消费成换行，不会走到这。
                            .on_action(cx.listener(|view, action: &Enter, window, cx| {
                                if !action.shift {
                                    view.send(window, cx);
                                }
                            }))
                            .child(
                                div()
                                    .h_flex()
                                    .items_end()
                                    .gap(px(8.))
                                    .rounded(px(12.))
                                    .bg(rgb(LAYER))
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .px(px(12.))
                                    .py(px(8.))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            // 外层已经有胶囊底色和边框，输入框本身
                                            // 不再画一层，避免双框。高度跟随内容：
                                            // 短输入 36px，长输入长高，112px 封顶
                                            // 后在框内滚动——写两三行不该让人自己滚。
                                            .child(
                                                Textarea::new(&self.input)
                                                    .appearance(false)
                                                    .min_h(px(36.))
                                                    .max_h(px(112.)),
                                            ),
                                    )
                                    .child(
                                        Button::new("butler-send")
                                            .primary()
                                            .icon(IconName::ArrowUp)
                                            .w(px(34.))
                                            .h(px(34.))
                                            .rounded(ButtonRounded::Size(px(17.)))
                                            .accessibility_label("发送")
                                            .tooltip("发送")
                                            .on_click(cx.listener(|view, _, window, cx| {
                                                view.send(window, cx);
                                            })),
                                    ),
                            ),
                    ),
            )
    }
}

/// 用户自己的原话可能带越界话术或过长：确认句过不了校验就退到不带引用的版本。
fn valid_or(reply: String, fallback: &str) -> String {
    if crate::butler::schedule::validate_dialogue(&reply).is_ok() {
        reply
    } else {
        fallback.to_owned()
    }
}

/// 降级提示只占一行。错误链可能很长，整段铺在界面上没人读得下去——
/// 用户需要的只是「这条为什么不是模型说的」。
fn clip_note(text: String) -> String {
    let flat = text.replace('\n', " ");
    if flat.chars().count() > 120 {
        format!("{}…", flat.chars().take(120).collect::<String>())
    } else {
        flat
    }
}

fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut clipped: String = text.chars().take(limit).collect();
    clipped.push('…');
    clipped
}

/// 一条事实在对话里的展示：到期日必须带上——同文不同日期的条目，日期是
/// 唯一区分符，不带日期用户没法指认要忘哪条。
fn fact_label(fact: &Fact) -> String {
    match fact.expires_at {
        Some(day) => format!("{} · 到期 {}", clip(&fact.text, 24), day.format("%Y-%m-%d")),
        None => clip(&fact.text, 24),
    }
}

/// 空态的三个一键提问。文案与 `expression::intent_for` 的主干意图对齐：
/// 偏离、计划、加仓，都是账本能直接接住的问题。
const SUGGESTIONS: [(&str, &str); 3] = [
    ("butler-chip-deviation", "现在偏离多少？"),
    ("butler-chip-add", "这周要不要加仓？"),
    ("butler-chip-plan", "我的计划还合适吗？"),
];

/// 按时段问候。边界取左闭右开：5–11 早上、11–14 中午、14–18 下午。
fn greeting_for(hour: u32) -> &'static str {
    match hour {
        5..=10 => "早上好",
        11..=13 => "中午好",
        14..=17 => "下午好",
        _ => "晚上好",
    }
}

/// 管家头像图：只解码一次。`include_bytes!` 把图编进二进制，`OnceLock` 让
/// 每帧渲染复用同一个 `Arc<Image>`，不在绘制阶段反复 PNG 解码。
fn butler_portrait() -> Arc<Image> {
    static PORTRAIT: OnceLock<Arc<Image>> = OnceLock::new();
    PORTRAIT
        .get_or_init(|| {
            Arc::new(Image::from_bytes(
                ImageFormat::Png,
                include_bytes!("../../assets/icon_butler.png").to_vec(),
            ))
        })
        .clone()
}

/// 头部徽章：头像图 + 右下角在线点。图片自带圆角蓝底，容器按圆角裁切即可；
/// 状态点带 2px 白描边，压在圆角边缘上时才不会糊成一团。
fn header_badge() -> Div {
    div()
        .relative()
        .w(px(36.))
        .h(px(36.))
        .flex_none()
        // 图片在内层容器裁切；状态点是外层的绝对定位子节点，必须留在
        // overflow 之外，否则压在圆角边缘的那一半会被裁掉。
        .child(
            div()
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .w(px(36.))
                .h(px(36.))
                .rounded(px(12.))
                .overflow_hidden()
                .child(
                    img(butler_portrait())
                        .w_full()
                        .h_full()
                        .object_fit(ObjectFit::Cover),
                ),
        )
        .child(
            div()
                .absolute()
                .right(px(1.))
                .bottom(px(1.))
                .w(px(8.))
                .h(px(8.))
                .rounded(px(4.))
                .bg(rgb(SUCCESS))
                .border_2()
                .border_color(rgb(SURFACE)),
        )
}

/// 空态徽章比头部大一号：空窗口里它是视觉落点，不是状态灯。
fn empty_badge() -> Div {
    div()
        .w(px(44.))
        .h(px(44.))
        .flex_none()
        .rounded(px(14.))
        .overflow_hidden()
        .child(
            img(butler_portrait())
                .w_full()
                .h_full()
                .object_fit(ObjectFit::Cover),
        )
}

/// 纸纹只解码一次：`include_bytes!` 把图编进二进制，`OnceLock` 让每帧渲染
/// 复用同一个 `Arc<Image>`，不在绘制阶段反复 PNG 解码。
fn butler_texture() -> Arc<Image> {
    static TEXTURE: OnceLock<Arc<Image>> = OnceLock::new();
    TEXTURE
        .get_or_init(|| {
            Arc::new(Image::from_bytes(
                ImageFormat::Png,
                include_bytes!("../../assets/butler-texture.png").to_vec(),
            ))
        })
        .clone()
}

/// 气泡最终显示的文本。
///
/// 管家一侧先过 `talk::display_text`：升级前落盘的旧回答里可能还留着
/// `action:` / `effective:` 四行与 mns-decision 围栏，存储不回洗，但渲染
/// 必须统一清掉（幂等，新消息不受影响）。用户一侧原样返回——用户自己打的
/// 字不能改。
fn bubble_text(role: Role, text: &str) -> String {
    match role {
        Role::Butler => talk::display_text(text),
        Role::User => text.to_owned(),
    }
}

/// 流式中的管家气泡：头像与排版与最终气泡一致，只少时间戳一行——这条
/// 还不是最终消息，`settle` 通过校验后由正式气泡顶上。正文每帧过一遍
/// `bubble_text`：流式半截里冒出的机器字段（`action:` 之类）与未闭合的
/// 决策围栏不能进气泡，宁可显示得保守。
fn streaming_row(text: &str) -> Div {
    div()
        .debug_selector(|| "butler-streaming".to_owned())
        .w_full()
        .flex_none()
        .h_flex()
        .items_start()
        .gap(px(11.))
        .child(butler_avatar())
        .child(
            div()
                .v_flex()
                .flex_1()
                .min_w(px(0.))
                .gap(px(5.))
                .pt(px(2.))
                .child(
                    div()
                        .text_size(px(13.5))
                        .line_height(px(21.))
                        .text_color(rgb(TEXT))
                        .child(TextView::markdown(
                            "butler-md-streaming",
                            bubble_text(Role::Butler, text),
                        )),
                ),
        )
}

fn message_row(index: usize, message: &Message) -> Stateful<Div> {
    match message.role {
        Role::User => div().w_full().flex_none().h_flex().justify_end().child(
            div()
                .v_flex()
                .max_w(px(520.))
                .rounded(px(14.))
                .bg(rgb(ACCENT_SOFT))
                .px(px(14.))
                .py(px(10.))
                .text_size(px(13.5))
                .line_height(px(21.))
                .text_color(rgb(TEXT))
                .children(
                    message
                        .text
                        .lines()
                        .map(|line| div().child(line.to_owned()))
                        .collect::<Vec<_>>(),
                ),
        ),
        Role::Butler => div()
            .w_full()
            .flex_none()
            .h_flex()
            .items_start()
            .gap(px(11.))
            .child(butler_avatar())
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(5.))
                    .pt(px(2.))
                    // The butler speaks markdown (bold numbers, short lists).
                    // Same renderer as the assistant page, so both surfaces
                    // agree on what `**bold**` means; the user bubble stays
                    // plain text because people type prose, not syntax.
                    //
                    // `bubble_text` also strips machine leftovers for old
                    // messages that were persisted before this cleanup existed.
                    .child(
                        div()
                            .text_size(px(13.5))
                            .line_height(px(21.))
                            .text_color(rgb(TEXT))
                            .child(TextView::markdown(
                                ElementId::named_usize("butler-md", index),
                                bubble_text(message.role, &message.text),
                            )),
                    )
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap(px(6.))
                            .child(dot(TEXT_TERTIARY))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgb(TEXT_TERTIARY))
                                    .child(format!(
                                        "{} · {}",
                                        message.at.format("%m-%d %H:%M"),
                                        message
                                            .reason
                                            .map(|reason| reason.label())
                                            .unwrap_or("对话")
                                    )),
                            ),
                    ),
            ),
    }
    .id(ElementId::named_usize("butler-row", index))
}

fn butler_avatar() -> Div {
    div()
        .flex_none()
        .w(px(26.))
        .h(px(26.))
        .rounded(px(13.))
        .overflow_hidden()
        .child(
            img(butler_portrait())
                .w_full()
                .h_full()
                .object_fit(ObjectFit::Cover),
        )
}

#[cfg(all(test, feature = "gui-test"))]
mod tests {
    //! 管家窗口的渲染与发送链路。会话注入走 `with_session`，全程不碰
    //! 真实 `~/.mns/agent/butler/`——测试不该改用户的对话记录。
    use super::{ButlerView, TurnState, bubble_text, greeting_for, settle_text};
    use crate::agent::{AgentEvent, EntryKind};
    use crate::butler::orchestrator::{AccountFacts, DriftRow};
    use crate::butler::session::{Message, Role, Session};
    use chrono::Local;
    use gpui_kit::{Entity, TestAppContext};

    fn facts() -> AccountFacts {
        AccountFacts {
            decision_day: true,
            band_pp: 4.0,
            drift: vec![DriftRow {
                key: "us_stocks".to_owned(),
                name: "美股".to_owned(),
                current_pct: 41.2,
                target_pct: 35.0,
                drift_pp: 6.2,
                market_value: 0.0,
            }],
            ..AccountFacts::default()
        }
    }

    fn open(
        cx: &mut TestAppContext,
        facts: AccountFacts,
    ) -> (Entity<ButlerView>, &mut gpui_kit::VisualTestContext) {
        cx.update(gpui_kit::init);
        cx.add_window_view(|window, cx| {
            // 测试注入的窗口不挂 ACP 会话：验证的是本地模板与状态机，
            // 不该因为开发机上恰好装了 opencode 就真去起一个进程。
            ButlerView::with_session(Session::default(), facts, window, cx)
        })
    }

    #[gpui_kit::test]
    fn 数字变化才通知而相同事实不重绘(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, facts());
        let (changed, facts_now) = cx.update(|window, cx| {
            let _ = window;
            view.update(cx, |view, cx| {
                let mut next = facts();
                next.band_pp = 5.0;
                let changed = view.apply_facts(next.clone(), cx);
                (changed, view.facts.clone())
            })
        });
        assert!(changed, "数字变了应该应用并重绘");
        assert_eq!(facts_now.band_pp, 5.0);

        let changed_again =
            cx.update(|_, cx| view.update(cx, |view, cx| view.apply_facts(facts_now.clone(), cx)));
        assert!(!changed_again, "同一份事实不该再通知一次");
    }

    #[gpui_kit::test]
    fn 空会话与有消息两种状态都能画出来(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, facts());
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });

        view.update(cx, |view, _| {
            view.session.push(Message::butler(
                Local::now(),
                "美股偏离 6.2pp（带宽 ±4.0pp）",
            ));
            view.session
                .push(Message::user(Local::now(), "现在偏离多少"));
        });
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
    }

    #[gpui_kit::test]
    fn 空态chip点击直接发送一轮问答(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, facts());
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });

        // 空态 chip 必须真的可点：坐标取自渲染后的边界，而不是直接调方法。
        let center = cx
            .debug_bounds("butler-chip-deviation")
            .expect("chip 应参与布局并记录边界")
            .center();
        cx.simulate_click(center, gpui_kit::Modifiers::none());

        let messages = cx.update(|_, cx| view.update(cx, |view, _| view.session.messages.clone()));
        assert_eq!(messages.len(), 2, "chip 点击应立刻开始一轮问答");
        assert_eq!(messages[0].role, Role::User);
        assert_eq!(messages[0].text, "现在偏离多少？");
        assert_eq!(messages[1].role, Role::Butler);
    }

    #[test]
    fn 问候按小时分段() {
        assert_eq!(greeting_for(4), "晚上好");
        assert_eq!(greeting_for(5), "早上好");
        assert_eq!(greeting_for(10), "早上好");
        assert_eq!(greeting_for(11), "中午好");
        assert_eq!(greeting_for(13), "中午好");
        assert_eq!(greeting_for(14), "下午好");
        assert_eq!(greeting_for(17), "下午好");
        assert_eq!(greeting_for(18), "晚上好");
        assert_eq!(greeting_for(23), "晚上好");
    }

    #[test]
    fn 窗口不再包含被删掉的两句文案() {
        // 这两行是 human 明确要求删掉的，且「不要换个说法写回来」。
        // 用拼接避免测试自己的字面量触发断言。
        let source = include_str!("butler.rs");
        assert!(!source.contains(concat!("不接", "券商")));
        assert!(!source.contains(concat!("Enter ", "发送")));
        // 数字自动刷新：窗口里不该再出现需要人点的刷新按钮。
        assert!(!source.contains(concat!("刷新", "数字")));
    }

    #[gpui_kit::test]
    fn 空态靠上而不是垂直居中(cx: &mut TestAppContext) {
        let (_view, cx) = open(cx, facts());
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        let header = cx.debug_bounds("butler-header").expect("头部应参与布局");
        let card = cx.debug_bounds("butler-card").expect("卡片应参与布局");
        let chip = cx
            .debug_bounds("butler-chip-deviation")
            .expect("chip 应参与布局");

        let gap = card.origin.y - (header.origin.y + header.size.height);
        assert!(
            gap <= gpui_kit::px(12.),
            "头部到卡片的间距应收紧，实际 {gap:?}"
        );
        assert!(
            chip.origin.y < card.center().y,
            "空态应靠上：垂直居中会在问候上方留一大片空白"
        );
    }

    #[gpui_kit::test]
    fn 头像行到卡片的纹理空白不过宽(cx: &mut TestAppContext) {
        let (_view, cx) = open(cx, facts());
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        let row = cx
            .debug_bounds("butler-header-row")
            .expect("头像行应参与布局");
        let card = cx.debug_bounds("butler-card").expect("卡片应参与布局");
        // 这段空白 = 分隔线 mt + 线高 1px + header pb + 卡片 mt。
        // 只量 card.mt 会漏掉 header 内的两层，人眼看到的却是整段。
        let gap = card.origin.y - (row.origin.y + row.size.height);
        assert!(
            gap <= gpui_kit::px(20.),
            "头像行到卡片的纹理空白应收紧，实际 {gap:?}"
        );
        assert!(
            gap >= gpui_kit::px(8.),
            "仍要留住分隔线与呼吸感，不能贴死，实际 {gap:?}"
        );
    }

    #[gpui_kit::test]
    fn 发送后清空输入框且回复不越界(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, facts());
        let mut input_after = String::new();
        let mut messages = Vec::new();

        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value("今天要不要加仓", window, cx)
                });
                view.send(window, cx);
                input_after = view.input.read(cx).value().to_string();
                messages = view.session.messages.clone();
            });
        });

        assert_eq!(input_after, "", "发送后输入框应清空");
        assert_eq!(messages.len(), 2, "一轮问答应是用户 + 管家两条");
        assert_eq!(messages[0].role, Role::User);
        assert_eq!(messages[1].role, Role::Butler);
        assert!(
            messages[1].text.contains("下单我帮不了"),
            "回复应如实说明不代下单，实际 {}",
            messages[1].text
        );
        assert!(
            crate::butler::schedule::validate_dialogue(&messages[1].text).is_ok(),
            "回复越过表达边界：{}",
            messages[1].text
        );
        view.update(cx, |view, _| {
            assert_eq!(view.memory.summaries.len(), 1, "普通对话应留一条摘要");
            assert!(
                view.memory.summaries[0].points.len() <= crate::butler::memory::MAX_POINTS,
                "单次摘要不得超过三条要点"
            );
        });
    }

    #[gpui_kit::test]
    fn 没有agent时立刻用本地模板回答(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, facts());
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input
                    .update(cx, |input, cx| input.set_value("现在偏离多少", window, cx));
                view.send(window, cx);
                assert!(view.chat.is_none(), "测试窗口不该挂 ACP 会话");
                assert_eq!(view.session.messages.len(), 2, "一轮问答应立即完整");
                assert!(view.hint.is_none());
            });
        });
    }

    /// 等待槽的并发语义：一次只挂一轮，迟到的正文进不了新话题。
    #[test]
    fn 等待槽只收当前这一轮的正文() {
        let mut turn = TurnState::default();
        assert!(turn.begin("模板一".to_owned()));
        assert!(!turn.begin("模板二".to_owned()), "上一轮没回完不接新提问");

        turn.push("美股偏离 6.2pp。");
        assert_eq!(
            turn.take(),
            Some(("美股偏离 6.2pp。".to_owned(), "模板一".to_owned()))
        );
        assert_eq!(turn.take(), None, "一轮只能收一次");

        // 清空会话后，迟到的正文没有等待槽可落。
        assert!(turn.begin("模板三".to_owned()));
        turn.abandon();
        turn.push("迟到的回答");
        assert_eq!(turn.take(), None);
    }

    /// 流式上屏的取数口径：等待中且已有正文才画气泡，收尾或作废后立即撤下。
    #[test]
    fn 等待中的正文才进流式气泡() {
        let mut turn = TurnState::default();
        assert!(turn.begin("模板".to_owned()));
        assert_eq!(turn.streaming_text(), None, "第一段正文没到，只有等待行");

        turn.push("美股偏离 6.2pp。");
        assert_eq!(turn.streaming_text(), Some("美股偏离 6.2pp。"));

        let (buffer, _) = turn.take().expect("等待中的一轮可以收尾");
        assert_eq!(buffer, "美股偏离 6.2pp。");
        assert_eq!(turn.streaming_text(), None, "收尾后不再画流式气泡");

        // 清空会话：半截正文随等待状态一起作废。
        assert!(turn.begin("模板二".to_owned()));
        turn.push("半截");
        turn.abandon();
        assert_eq!(turn.streaming_text(), None);
    }

    /// 端到端的渲染生命周期：正文逐段到达时气泡在场生长，收尾换成正式消息。
    #[gpui_kit::test]
    fn 正文边生成边上屏收尾后换成正式气泡(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, facts());
        view.update(cx, |view, _| {
            view.session
                .push(Message::user(Local::now(), "现在偏离多少"));
            assert!(view.turn.begin("美股偏离 6.2pp（带宽 ±4.0pp）".to_owned()));
        });
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        // 第一段正文没到：等待行在场，还没有流式气泡。
        assert!(cx.debug_bounds("butler-thinking").is_some(), "等待行应在场");
        assert!(
            cx.debug_bounds("butler-streaming").is_none(),
            "还没正文不该有流式气泡"
        );

        view.update(cx, |view, _| {
            view.turn.push("美股偏离 6.2pp。");
        });
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        assert!(
            cx.debug_bounds("butler-streaming").is_some(),
            "正文到达后应画出流式气泡"
        );
        assert!(
            cx.debug_bounds("butler-thinking").is_none(),
            "正文开始流动后等待行应隐藏，气泡本身就是进度"
        );

        // 收尾：流式气泡撤下，正式消息落进会话。
        view.update(cx, |view, cx| {
            view.settle(None, cx);
        });
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        assert!(
            cx.debug_bounds("butler-streaming").is_none(),
            "收尾后流式气泡应撤下"
        );
        view.update(cx, |view, _| {
            let last = view.session.messages.last().expect("应有正式回答");
            assert_eq!(last.role, Role::Butler);
            assert_eq!(last.text, "美股偏离 6.2pp。");
            assert!(!view.turn.waiting, "收尾后不再等待");
        });
    }

    /// 红线实时切断：流式累积中谎称能替用户下单，话音未落就收尾这一轮并
    /// 退回模板；诚实的否定（「没法帮你下单」）不触发切断。
    #[gpui_kit::test]
    fn 流式中谎称下单立即切断而诚实否定不切(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, facts());
        view.update(cx, |view, cx| {
            view.session.push(Message::user(Local::now(), "帮我买了"));
            assert!(view.turn.begin("模板句".to_owned()));

            view.on_chat_event(
                AgentEvent::Delta {
                    kind: EntryKind::Message,
                    text: "这个我没法".to_owned(),
                },
                cx,
            );
            assert!(view.turn.waiting, "诚实否定不是红线，正文继续长");

            view.on_chat_event(
                AgentEvent::Delta {
                    kind: EntryKind::Message,
                    text: "帮你下单，得你自己来。".to_owned(),
                },
                cx,
            );
            assert!(view.turn.waiting, "「没法帮你下单」不该触发切断");
            assert_eq!(
                view.turn.streaming_text(),
                Some("这个我没法帮你下单，得你自己来。")
            );
        });

        // 换一轮真正谎称执行的：命中即收尾、退模板、说清原因。
        view.update(cx, |view, cx| {
            let _ = view.turn.take();
            assert!(view.turn.begin("模板句二".to_owned()));
            view.on_chat_event(
                AgentEvent::Delta {
                    kind: EntryKind::Message,
                    text: "好的，已经".to_owned(),
                },
                cx,
            );
            assert!(view.turn.waiting, "半句话还没到红线");
            view.on_chat_event(
                AgentEvent::Delta {
                    kind: EntryKind::Message,
                    text: "帮你下单了，明天到账。".to_owned(),
                },
                cx,
            );
            assert!(!view.turn.waiting, "命中禁语应立即收尾这一轮");
            let last = view.session.messages.last().expect("应有兜底回复");
            assert_eq!(last.role, Role::Butler);
            assert_eq!(last.text, "模板句二", "谎称执行必须退回模板");
            let hint = view.hint.clone().expect("要说明切断原因");
            assert!(hint.contains("实时切断"), "实际 {hint}");
            assert!(hint.contains("谎称能替用户执行下单"), "实际 {hint}");
        });
    }

    #[test]
    fn 观点类回答被采纳而超长退回模板() {
        // 管家人格可以给倾向性观点：不再因「建议/目标价」这类词整条丢弃，
        // 校验只守诚实性底线（非空、限长、不谎称能下单）。
        let (text, hint) = settle_text(
            "我的看法是可以加仓美股，目标价到 45% 也不奇怪。",
            "美股偏离 6.2pp（带宽 ±4.0pp）",
            None,
        );
        assert_eq!(text, "我的看法是可以加仓美股，目标价到 45% 也不奇怪。");
        assert!(hint.is_none(), "通过校验的回答不该带降级提示");

        let long = "话".repeat(crate::butler::talk::MAX_TALK_CHARS + 1);
        let (text, hint) = settle_text(&long, "美股偏离 6.2pp（带宽 ±4.0pp）", None);
        assert_eq!(
            text, "美股偏离 6.2pp（带宽 ±4.0pp）",
            "超长回答必须退回模板"
        );
        let hint = hint.expect("要说明退回原因");
        assert!(hint.contains("超过"), "提示应说明越界类型：{hint}");
    }

    #[test]
    fn markdown在会话里原样保留给渲染层() {
        // 气泡用 TextView::markdown 渲染，校验层不能把结构洗掉，也不能把
        // 「- 结论：…」这类列表行当成机器字段删掉。
        let (text, _) = settle_text(
            "**美股** 低配 6.2pp。\n\n- 结论：先别动\n- 等决策日再看",
            "模板",
            None,
        );
        assert!(text.contains("**美股**"), "实际 {text}");
        assert!(text.contains("- 结论：先别动"), "实际 {text}");
    }

    #[test]
    fn 渲染旧的机器字段前会先清掉() {
        // 升级前的会话没经过第 10 节的清理：存储不回洗，渲染层必须统一。
        let legacy = "结论：两腿出带宽。\n\naction: adjust_monthly_plan\n\
                      effective: 2026-11-20\nmonthly_amount: 2000\nallocation: 美股 45%";
        let shown = bubble_text(Role::Butler, legacy);
        assert_eq!(shown, "结论：两腿出带宽。");
        assert!(!shown.contains("action"));
        assert!(!shown.contains("monthly_amount"));

        // 新消息幂等，markdown 结构保留。
        let fresh = "**美股** 偏离 6.2pp。\n\n- 等决策日再看";
        assert_eq!(bubble_text(Role::Butler, fresh), fresh);
        // 用户自己的话不洗。
        assert_eq!(bubble_text(Role::User, legacy), legacy);
    }

    #[test]
    fn 谎称已经下单会退回模板() {
        // 唯一保留的会话硬底线：没有券商通道却说能下单是撒谎。
        let (text, hint) = settle_text("已经帮你下单了，明天到账。", "模板句", None);
        assert_eq!(text, "模板句", "谎称执行必须退回模板");
        let hint = hint.expect("要说明退回原因");
        assert!(hint.contains("执行下单"), "提示应说明退回原因：{hint}");
    }

    #[test]
    fn agent失败时说明原因且保留模板() {
        let (text, hint) = settle_text("", "美股偏离 6.2pp（带宽 ±4.0pp）", Some("agent 超时"));
        assert_eq!(text, "美股偏离 6.2pp（带宽 ±4.0pp）");
        let hint = hint.expect("失败要说清原因");
        assert!(hint.starts_with("agent 超时"), "实际 {hint}");
    }

    #[test]
    fn 超时但已有半截时保留内容并标注() {
        let (text, hint) = settle_text(
            "美股偏离 6.2pp，A股低配",
            "模板句",
            Some("agent 超过 120 秒没答完"),
        );
        assert_eq!(text, "美股偏离 6.2pp，A股低配", "已经生成的别扔掉");
        assert!(
            hint.expect("要说明这条没答完").contains("没答完"),
            "用户得知道内容是被截断的"
        );
    }

    #[gpui_kit::test]
    fn 回车发送而shift回车只换行(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, facts());

        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value("回车能发吗", window, cx);
                    input.focus(window, cx);
                });
            });
        });
        cx.simulate_keystrokes("enter");

        let (sent, value) = cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                (
                    view.session.messages.len(),
                    view.input.read(cx).value().to_string(),
                )
            })
        });
        assert_eq!(sent, 2, "Enter 应直接发送一轮问答");
        assert_eq!(value, "", "发送后输入框应清空");

        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value("第一行", window, cx);
                    input.focus(window, cx);
                });
            });
        });
        cx.simulate_keystrokes("shift-enter");

        let (sent, value) = cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                (
                    view.session.messages.len(),
                    view.input.read(cx).value().to_string(),
                )
            })
        });
        assert_eq!(sent, 2, "Shift+Enter 不应发送");
        assert!(value.contains('\n'), "Shift+Enter 应换行，实际 {value:?}");
    }

    #[gpui_kit::test]
    fn 记住与忘掉走记忆层且不写真实目录(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, facts());
        let mut remember_reply = String::new();
        let mut forget_reply = String::new();

        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value("记住：周五补美股", window, cx)
                });
                view.send(window, cx);
                remember_reply = view.session.messages.last().expect("应有回复").text.clone();
                assert_eq!(view.memory.facts.len(), 1, "意图应落进记忆");
                assert!(!view.persist, "测试不得写真实账户目录");

                view.input
                    .update(cx, |input, cx| input.set_value("忘掉补美股", window, cx));
                view.send(window, cx);
                forget_reply = view.session.messages.last().expect("应有回复").text.clone();
                assert!(view.memory.facts.is_empty(), "忘掉后不再参与表达");
                assert_eq!(view.memory.archived_facts.len(), 1, "归档可查");
            });
        });

        assert!(remember_reply.contains("记下了"), "实际 {remember_reply}");
        assert!(
            remember_reply.contains("（到期"),
            "回执应回显到期日，误解析才看得见，实际 {remember_reply}"
        );
        assert!(forget_reply.contains("拿掉了"), "实际 {forget_reply}");
        assert!(
            forget_reply.contains("· 到期"),
            "删除回执要带日期，实际 {forget_reply}"
        );
        assert!(
            crate::butler::schedule::validate_dialogue(&remember_reply).is_ok(),
            "回复越过表达边界：{remember_reply}"
        );
        assert!(
            crate::butler::schedule::validate_dialogue(&forget_reply).is_ok(),
            "回复越过表达边界：{forget_reply}"
        );
    }

    #[gpui_kit::test]
    fn 同文不同到期日忘掉必须带日期消歧(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, facts());
        let mut ambiguous = String::new();
        let mut removed = String::new();

        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                // 用绝对日期，避免测试依赖运行当天的日历。
                view.input.update(cx, |input, cx| {
                    input.set_value("记住 2026-12-27 截止 补美股", window, cx)
                });
                view.send(window, cx);
                view.input.update(cx, |input, cx| {
                    input.set_value("记住 2026-12-30 截止 补美股", window, cx)
                });
                view.send(window, cx);
                assert_eq!(view.memory.facts.len(), 2, "不同到期日应各存一条");

                // 不带日期：同文两条 → 歧义，一条不动，候选带日期。
                view.input
                    .update(cx, |input, cx| input.set_value("忘掉补美股", window, cx));
                view.send(window, cx);
                ambiguous = view.session.messages.last().expect("应有回复").text.clone();
                assert_eq!(view.memory.facts.len(), 2, "歧义时一条都不能动");

                // 带日期：只删那一天的那条。
                view.input.update(cx, |input, cx| {
                    input.set_value("忘掉 2026-12-27 截止 补美股", window, cx)
                });
                view.send(window, cx);
                removed = view.session.messages.last().expect("应有回复").text.clone();
                assert_eq!(view.memory.facts.len(), 1, "带日期只删那一条");
            });
        });

        assert!(
            ambiguous.contains("2026-12-27") && ambiguous.contains("2026-12-30"),
            "候选要带日期，实际 {ambiguous}"
        );
        assert!(
            ambiguous.contains("带上日期"),
            "要引导用户带日期，实际 {ambiguous}"
        );
        assert!(
            removed.contains("2026-12-27"),
            "回执要带删掉那条的日期，实际 {removed}"
        );
        assert!(
            crate::butler::schedule::validate_dialogue(&ambiguous).is_ok(),
            "回复越过表达边界：{ambiguous}"
        );
        assert!(
            crate::butler::schedule::validate_dialogue(&removed).is_ok(),
            "回复越过表达边界：{removed}"
        );
    }

    #[gpui_kit::test]
    fn 问记得什么时念出常驻记忆(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, facts());
        let mut recall = String::new();

        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value("记住：周五补美股", window, cx)
                });
                view.send(window, cx);
                view.input
                    .update(cx, |input, cx| input.set_value("你记得什么", window, cx));
                view.send(window, cx);
                recall = view.session.messages.last().expect("应有回复").text.clone();
            });
        });

        // 「周五」被识别成到期日摘出了正文：条目原文是「补美股」，
        // recall 必须把到期日念出来——用户要能核对管家记的日子对不对。
        assert!(recall.contains("补美股"), "应念出条目原文，实际 {recall}");
        assert!(
            recall.contains("· 到期"),
            "recall 应回显到期日，误解析才看得见，实际 {recall}"
        );
        assert!(
            crate::butler::schedule::validate_dialogue(&recall).is_ok(),
            "回复越过表达边界：{recall}"
        );
    }

    #[gpui_kit::test]
    fn 清除会话需要二次确认(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, facts());
        view.update(cx, |view, _| {
            view.session.push(Message::user(Local::now(), "你好"));
        });

        view.update(cx, |view, cx| view.clear_session(cx));
        view.update(cx, |view, _| {
            assert!(view.clear_armed, "第一次点击只进入待确认");
            assert_eq!(view.session.messages.len(), 1, "第一次点击不删消息");
        });

        view.update(cx, |view, cx| view.clear_session(cx));
        view.update(cx, |view, _| {
            assert!(!view.clear_armed);
            assert!(view.session.messages.is_empty(), "第二次点击才清空");
        });
    }

    /// 清空会话必须把 ACP session 一并作废（`AgentCommand::Stop` +
    /// `started` 归位）：只删本地存档的话，下一句会以续问接在被清掉的
    /// 上下文上——用户以为清了，其实没清。
    ///
    /// 测试窗口照常只起 runtime 的命令环：`Start` 从未发出，外部 agent
    /// 进程不会存在；`Stop` 落在无活动会话上只是记一条系统行。
    #[gpui_kit::test]
    fn 清空会话后ACP会话一并作废下一问重新走start(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            ButlerView::with_session(Session::default(), facts(), window, cx).with_agent()
        });
        view.update(cx, |view, _| {
            let chat = view.chat.as_mut().expect("真实窗口挂 ACP 会话");
            chat.started = true; // 之前聊过：清空前 session 是续问状态
        });

        view.update(cx, |view, cx| view.clear_session(cx)); // 第一次：待确认
        view.update(cx, |view, cx| view.clear_session(cx)); // 第二次：真清

        view.update(cx, |view, _| {
            let chat = view.chat.as_ref().expect("ACP 会话仍在");
            assert!(!chat.started, "清空后下一问必须重新走 Start 而不是续问");
            assert!(!view.turn.waiting, "等待中的回答随会话一起作废");
        });
    }
}
