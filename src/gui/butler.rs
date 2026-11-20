//! 管家独立窗口：人设化文本会话，与助手页互不共享存档。
//!
//! P4 起这是一层**薄客户端**：会话的真相（存档、回合、事件流）全在
//! `mns-butlerd` 的 `butler::chat` 服务里。窗口只做三件事：把快照摆出来、
//! 把用户的话递过去、把流式增量逐段上屏。它不判断「该不该说」，不写
//! `conversation.json`，也不持有 ACP runtime——关窗即 detach，正在回答的
//! 一轮在守护进程里照常收完落盘，重开窗口拿一帧全量快照就能追上。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use chrono::{Local, Timelike};
use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonRounded, ButtonVariants};
use gpui_kit::component::input::{Enter, Textarea, TextareaState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::Disableable;
use gpui_kit::*;

use crate::butler::chat::{self, ChatSnapshotData};
use crate::butler::expression;
use crate::butler::orchestrator;
use crate::butler::talk;
use crate::butler::{AccountFacts, Message, Role, Session};
use crate::ipc::{self, Signal};

use super::components::{dot, pill, watermark};
use super::theme::*;

/// 宿主是否在线（窗口拉起时 `announce_to_host` 的连接结果）。
///
/// 离线横幅的「重试」按钮依赖宿主拉起守护进程——窗口自己不 spawn 进程，
/// 宿主也不在线时点它没有意义，置灰。
static HOST_ALIVE: AtomicBool = AtomicBool::new(false);

/// 记录宿主在线。窗口进程启动时连上宿主后调用一次。
pub(crate) fn mark_host_alive() {
    HOST_ALIVE.store(true, Ordering::Relaxed);
}

fn host_alive() -> bool {
    HOST_ALIVE.load(Ordering::Relaxed)
}

/// 账本数字的重算节奏。管家窗口常开，用户不该为了「几点了、偏离多少」去
/// 点一个按钮：每 60 秒重算一次事实，没有变化就不重绘。
const FACTS_REFRESH: std::time::Duration = std::time::Duration::from_secs(60);

/// 点系统通知进来时要落到的那一条。
///
/// `index` 是 `session.messages` 里的位置；通知已经不在会话里（被 200 条上限
/// 裁掉、或宿主还没写入）时为 `None`，这时界面上仍保留一条上下文横幅，
/// 但不假装能滚到某一行。
struct NoticeFocus {
    index: Option<usize>,
    at: Option<chrono::DateTime<Local>>,
    text: Option<String>,
}

pub(crate) struct ButlerView {
    /// 会话的**展示副本**：真相在 butlerd，每帧快照整帧替换。
    session: Session,
    facts: AccountFacts,
    input: Entity<TextareaState>,
    scroll: ScrollHandle,
    error: Option<String>,
    clear_armed: bool,
    /// 会话连接的发送端。`None` = 离线（守护进程没在跑或刚断开）。
    outbox: Option<tokio::sync::mpsc::UnboundedSender<Signal>>,
    online: bool,
    /// 「重试」进行中：横幅换成正在连接，按钮置灰防连点。
    retrying: bool,
    /// 快照里的等待态：一轮问答在守护进程里进行中。等待期间输入框禁用——
    /// 并发提问会被守护进程明确拒绝，与其把话递过去再退回来，不如不收。
    waiting: bool,
    /// 流式中的半截正文：`ChatDelta` 逐段累进，快照整帧替换。
    stream_buffer: String,
    /// 被拒/断线时没递出去的话：render 时放回输入框（`set_value` 需要
    /// `&mut Window`，只能在渲染阶段应用）。
    pending_restore: Option<String>,
    /// 点系统通知进来时要定位的那条。等快照解析：会话真相到了才能定位。
    pending_notice: Option<String>,
    /// `pending_notice` 已经等过的快照帧数，防无限等待。
    pending_notice_frames: u32,
    /// 最近一帧快照的服务实例。`None` = 还没收到过任何快照。
    snapshot_instance: Option<u64>,
    /// 回答来源的一行说明。只在退回模板时出现，避免每句都挂个来源标签。
    hint: Option<String>,
    /// 从系统通知点进来的那一条。窗口开着时再点一次会换到这里，不重开。
    notice_focus: Option<NoticeFocus>,
}

impl ButlerView {
    /// 真实窗口。会话真相在守护进程那边：这里从空副本起步，第一帧快照
    /// 到了才有内容可画；守护进程不在就画一条离线横幅。
    pub(crate) fn new(facts: AccountFacts, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let view = Self::with_session(Session::default(), facts, window, cx);
        // 数字自动跟随主界面：没有手动刷新按钮。
        view.start_facts_refresh(cx);
        // 会话连接：挂上拿快照，断开即离线横幅。
        view.start_link(cx);
        view
    }

    /// 会话来源与守护进程解耦：测试要的是「快照到了会话与输入框的状态」，
    /// 不该连本地通道、也不该定时重算事实。
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
            facts,
            input,
            scroll: ScrollHandle::new(),
            error: None,
            clear_armed: false,
            outbox: None,
            online: false,
            retrying: false,
            waiting: false,
            stream_buffer: String::new(),
            pending_restore: None,
            pending_notice: None,
            pending_notice_frames: 0,
            snapshot_instance: None,
            hint: None,
            notice_focus: None,
        }
    }

    /// 落到某条通知上：滚动到对应消息并高亮，窗口顶部留一条上下文横幅。
    ///
    /// 会话真相在 butlerd：这里先记下要定位的 id，用当前副本试着解析；
    /// 副本里还没有（窗口刚开、快照未到）就等下一帧快照，到了再解析。
    pub(crate) fn focus_notice(&mut self, notice_id: String, cx: &mut Context<Self>) {
        self.pending_notice = Some(notice_id);
        self.pending_notice_frames = 0;
        self.resolve_pending_notice(cx);
    }

    /// 解析等定位的通知。定位不到就等下一帧快照——但最多等两帧：第一帧
    /// 可能是挂载帧，待并积压还没并进会话；两帧之后仍没有，送达审计里的
    /// 原文（或「已不在会话里」）就是最终答案，不能无限等下去。
    fn resolve_pending_notice(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.pending_notice.clone() else {
            return;
        };
        let index = notice_index(&self.session.messages, &id);
        if index.is_none() && self.pending_notice_frames < 2 {
            return;
        }
        // 会话里找得到就用会话；找不到时送达审计（notices.jsonl）里可能
        // 还留着原文——被 200 条上限裁掉的旧通知靠它给出横幅文案。
        let delivered = index
            .is_none()
            .then(|| orchestrator::delivered_notice(&id))
            .flatten();
        let at = index
            .and_then(|index| self.session.messages.get(index).map(|message| message.at))
            .or_else(|| delivered.as_ref().and_then(|event| event.notice_at));
        let text = index
            .and_then(|index| {
                self.session
                    .messages
                    .get(index)
                    .map(|message| message.text.clone())
            })
            .or_else(|| delivered.as_ref().and_then(|event| event.draft.clone()));
        if let Some(index) = index {
            self.scroll.scroll_to_top_of_item(index);
        }
        self.pending_notice = None;
        self.notice_focus = Some(NoticeFocus {
            index,
            at,
            text,
        });
        cx.notify();
    }

    fn dismiss_notice(&mut self, cx: &mut Context<Self>) {
        self.notice_focus = None;
        cx.notify();
    }

    /// 窗口顶部的通知上下文条：让用户知道「这条是从哪来的」，并给出关闭入口。
    fn notice_banner(&self, cx: &mut Context<Self>) -> Div {
        let Some(focus) = self.notice_focus.as_ref() else {
            return div();
        };
        let when = focus
            .at
            .map(|at| format!(" · {}", at.format("%m-%d %H:%M")))
            .unwrap_or_default();
        let body = match (focus.text.as_deref(), focus.index) {
            (Some(text), _) => clip(text, 90),
            (None, Some(_)) => "已在会话中定位到这条通知".to_owned(),
            (None, None) => "这条通知的内容已不在会话里".to_owned(),
        };
        div()
            .debug_selector(|| "butler-notice-banner".to_owned())
            .mx(px(16.))
            .mt(px(8.))
            .px(px(14.))
            .py(px(10.))
            .rounded(px(RADIUS_LG))
            .bg(rgb(ACCENT_TINT))
            .border_1()
            .border_color(rgb(ACCENT_SOFT))
            .h_flex()
            .items_start()
            .gap(px(10.))
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(ACCENT))
                            .child(format!("点开的通知{when}")),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .line_height(px(18.))
                            .text_color(rgb(TEXT_SECONDARY))
                            .child(body),
                    ),
            )
            .child(
                Button::new("butler-notice-dismiss")
                    .compact()
                    .ghost()
                    .label("知道了")
                    .on_click(cx.listener(|view, _, _, cx| view.dismiss_notice(cx))),
            )
    }

    /// 离线横幅：守护进程不在（没起过、被停掉、刚断开）。会话的真相在那边，
    /// 这里只说明发生了什么、给一条出路——请宿主拉起守护进程再重连。
    /// 宿主自己也不在线时（窗口不是从宿主拉起的）重连无从发起，按钮置灰，
    /// 横幅文案本身就是答案。
    fn offline_banner(&self, cx: &mut Context<Self>) -> Div {
        let (title, body) = if self.retrying {
            (
                "正在连接守护进程…",
                "最长等 8 秒，起来了会自动回到会话。",
            )
        } else {
            (
                "没连上守护进程",
                "会话都存在那边，重连后照旧。点「重试」把守护进程拉起来。",
            )
        };
        div()
            .debug_selector(|| "butler-offline".to_owned())
            .mx(px(16.))
            .mt(px(8.))
            .px(px(14.))
            .py(px(10.))
            .rounded(px(RADIUS_LG))
            .bg(rgb(WARNING_SOFT))
            .border_1()
            .border_color(rgb(WARNING))
            .h_flex()
            .items_start()
            .gap(px(10.))
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(WARNING))
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .line_height(px(18.))
                            .text_color(rgb(TEXT_SECONDARY))
                            .child(body),
                    ),
            )
            .child(
                Button::new("butler-retry")
                    .compact()
                    .ghost()
                    .label("重试")
                    .disabled(self.retrying || !host_alive())
                    .on_click(cx.listener(|view, _, _, cx| view.retry_connect(cx))),
            )
    }

    // ---- 会话连接（薄客户端的一半） -------------------------------------
    //
    // 窗口对 butlerd 一条双向长连接：上行 Chat* 请求，下行快照与增量。
    // 上行交给一条专职写线程（阻塞写不占界面线程）；下行由一个 GPUI 异步
    // 任务泵回视图。连接断了（守护进程没了、被拆线）就是离线，横幅说明、
    // 重试请宿主拉起——窗口自己不 spawn 进程。

    /// 初次连接：窗口刚开时守护进程通常已在跑（宿主负责拉起）；不在就画
    /// 离线横幅，等用户点「重试」。
    fn start_link(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |weak, async_cx| {
            if let Ok((client, downlink)) = ipc::connect_duplex("mns-butlerd") {
                let _ = weak.update(async_cx, |view, cx| view.link_up(client, downlink, cx));
            }
        })
        .detach();
    }

    /// 连接就绪：起上行写线程、记下发送端、挂上（回全量快照）并标记
    /// 已读，然后泵下行帧。
    fn link_up(
        &mut self,
        mut client: ipc::Client,
        mut downlink: tokio::sync::mpsc::UnboundedReceiver<Signal>,
        cx: &mut Context<Self>,
    ) {
        let (outbox, mut uplink) = tokio::sync::mpsc::unbounded_channel::<Signal>();
        // 上行写线程：独占连接的写半边。视图 drop（关窗）或写失败（守护
        // 进程没了）时线程退出，`Client` 随之 Drop——socket 双向关闭，守护
        // 进程看到 detach，下行通道也随之关闭，视图转离线。
        let writer = std::thread::Builder::new()
            .name("mns-butler-uplink".to_owned())
            .spawn(move || {
                while let Some(signal) = uplink.blocking_recv() {
                    if client.send(&signal).is_err() {
                        break;
                    }
                }
            });
        if writer.is_err() {
            // 写线程起不来就当没连上：丢弃连接保持离线，别造一个假在线。
            return;
        }
        self.outbox = Some(outbox);
        self.online = true;
        self.retrying = false;
        // 挂载（butlerd 回全量快照）+ 打开即已读。
        let _ = self.send_signal(Signal::ChatAttach);
        let _ = self.send_signal(Signal::ChatSeen);
        // 下行泵：快照与增量回视图；通道关了（守护进程没了/被拆线）即
        // 离线。
        cx.spawn(async move |weak, async_cx| {
            while let Some(signal) = downlink.recv().await {
                if weak
                    .update(async_cx, |view, cx| view.handle_signal(signal, cx))
                    .is_err()
                {
                    return;
                }
            }
            let _ = weak.update(async_cx, |view, cx| view.go_offline(cx));
        })
        .detach();
        cx.notify();
    }

    /// 连接没了：撤横幅之外的全部会话状态——在飞的那一轮跟着连接走了，
    /// 重连后快照会带回真相（可能比本地看到的更完整）。
    fn go_offline(&mut self, cx: &mut Context<Self>) {
        if !self.online && self.outbox.is_none() {
            return;
        }
        self.online = false;
        self.outbox = None;
        self.waiting = false;
        self.stream_buffer.clear();
        cx.notify();
    }

    /// 离线横幅的「重试」：先请宿主拉起守护进程（它才管生命周期），
    /// 然后每半秒试一次，最多 8 秒——拉起一个进程用不了更久。
    fn retry_connect(&mut self, cx: &mut Context<Self>) {
        if self.online || self.retrying {
            return;
        }
        let _ = ipc::poke("mns-gui", &Signal::StartButler);
        self.retrying = true;
        cx.notify();
        cx.spawn(async move |weak, async_cx| {
            for _ in 0..16 {
                async_cx
                    .background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                if let Ok((client, downlink)) = ipc::connect_duplex("mns-butlerd") {
                    let _ = weak.update(async_cx, |view, cx| view.link_up(client, downlink, cx));
                    return;
                }
            }
            let _ = weak.update(async_cx, |view, cx| {
                view.retrying = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// 递一句话给守护进程。发送端没了或通道断了返回 false——调用方据此
    /// 把话放回去并转离线。
    fn send_signal(&self, signal: Signal) -> bool {
        self.outbox
            .as_ref()
            .is_some_and(|outbox| outbox.send(signal).is_ok())
    }

    /// 守护进程送下来的帧：快照整帧替换；增量正文只在等待中累进流式气泡。
    fn handle_signal(&mut self, signal: Signal, cx: &mut Context<Self>) {
        match signal {
            Signal::ChatSnapshot { json } => self.apply_snapshot(&json, cx),
            Signal::ChatDelta { text } => {
                if self.waiting {
                    self.stream_buffer.push_str(&text);
                    self.follow_stream();
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    /// 快照整帧替换：会话、等待态、半截正文、提示与错误都以 butlerd 为准。
    /// `restore` 是被拒的那句话，render 时放回输入框。
    fn apply_snapshot(&mut self, json: &str, cx: &mut Context<Self>) {
        let Ok(data) = serde_json::from_str::<ChatSnapshotData>(json) else {
            return;
        };
        let had_snapshot = self.snapshot_instance.is_some();
        let grew = had_snapshot && data.session.messages.len() > self.session.messages.len();
        self.snapshot_instance = Some(data.instance);
        self.session = data.session;
        self.waiting = data.waiting;
        self.stream_buffer = data.buffer;
        self.hint = data.hint;
        self.error = data.error;
        if let Some(text) = data.restore {
            self.pending_restore = Some(text);
        }
        if self.pending_notice.is_some() {
            self.pending_notice_frames += 1;
        }
        // 新消息落地才跟随滚动；首帧不动，保持「按存档打开」的旧行为。
        if grew {
            self.scroll.scroll_to_bottom();
        }
        self.resolve_pending_notice(cx);
        cx.notify();
    }

    fn refresh_facts(&mut self, cx: &mut Context<Self>) {
        let next = chat::current_facts(Local::now());
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

    /// 发送。薄客户端只递话：守护进程决定接不接（回合进行中会拒绝并把话
    /// 原样退回来），会话、记忆、落盘全在那边。本地只守两条——离线时不吞
    /// 字（话留在输入框里，等连上再发），等待中不递（守护进程必拒，与其
    /// 递过去再退回来，不如不收）。
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.online || self.waiting {
            return;
        }
        let text = self.input.read(cx).value().trim().to_owned();
        if text.is_empty() {
            return;
        }
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        if !self.send_signal(Signal::ChatSend { text: text.clone() }) {
            // 递送失败：连接刚断。话不能丢——放回输入框（render 时应用），
            // 并转离线，横幅说明怎么重连。
            self.pending_restore = Some(text);
            self.go_offline(cx);
        }
    }

    /// 流式期间的跟随滚动：正文在长，贴底就跟着走；用户往上翻了就不打扰。
    /// 与主界面 `follow_conversation` 同一条规则。
    fn follow_stream(&self) {
        let near_bottom = self.scroll.max_offset().y + self.scroll.offset().y <= px(96.);
        if near_bottom {
            self.scroll.scroll_to_bottom();
        }
    }

    /// 清空会话：两次确认后把意图递给守护进程。磁盘上的真相（存档、
    /// 在飞的回合、ACP 上下文）全由它清；本地副本等下一帧快照整帧替换，
    /// 不抢先清——清一半的画面比等半秒更让人不安。
    fn clear_session(&mut self, cx: &mut Context<Self>) {
        if !self.clear_armed {
            self.clear_armed = true;
            cx.notify();
            return;
        }
        self.clear_armed = false;
        if !self.send_signal(Signal::ChatClear) {
            self.error =
                Some("没连上守护进程，这次没清。连上后再试一次。".to_owned());
        }
        cx.notify();
    }
}

impl Render for ButlerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 递送失败被退回的话在这里放回输入框：`set_value` 需要 `&mut Window`，
        // 信号回调里拿不到，只能攒到渲染时应用。取走即清零，只放一次。
        if let Some(text) = self.pending_restore.take() {
            self.input
                .update(cx, |input, cx| input.set_value(text.as_str(), window, cx));
        }
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
            let highlight = self.notice_focus.as_ref().and_then(|focus| focus.index);
            stream = stream.children(
                self.session
                    .messages
                    .iter()
                    .enumerate()
                    .map(|(index, message)| message_row(index, message, highlight == Some(index)))
                    .collect::<Vec<_>>(),
            );
        }
        // 流式正文：守护进程逐段推来，气泡逐段生长；快照落地（等待解除）
        // 后由正式气泡顶上。
        let streaming = (self.waiting && !self.stream_buffer.is_empty())
            .then(|| self.stream_buffer.as_str());
        if let Some(text) = streaming {
            stream = stream.child(streaming_row(text));
        }
        // 等 agent 时给一行状态，否则用户分不清「正在想」和「没反应」。
        // 只在第一段正文到达前显示：正文开始流动后，气泡本身就是进度
        // （正文停住的空档属于「工具活动」提案，不在 V1）。
        if self.waiting && streaming.is_none() {
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
            .children(
                self.notice_focus
                    .is_some()
                    .then(|| self.notice_banner(cx)),
            )
            .children((!self.online).then(|| self.offline_banner(cx)))
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
                                                    .max_h(px(112.))
                                                    // 离线递不出去、等待中递了也必被退回：
                                                    // 两种状态都先不收，但字留在框里，
                                                    // 等状态过了再发（见 `send`）。
                                                    .disabled(!self.online || self.waiting),
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
                                            .disabled(!self.online || self.waiting)
                                            .on_click(cx.listener(|view, _, window, cx| {
                                                view.send(window, cx);
                                            })),
                                    ),
                            ),
                    ),
            )
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

fn message_row(index: usize, message: &Message, highlighted: bool) -> Stateful<Div> {
    let row = match message.role {
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
    };
    // 点通知进来的那一条：给整行一层浅蓝底，滚动定位后仍能一眼认出。
    if highlighted {
        return row
            .rounded(px(RADIUS_LG))
            .bg(rgb(ACCENT_TINT))
            .px(px(8.))
            .py(px(6.))
            .id(ElementId::named_usize("butler-row", index));
    }
    row.id(ElementId::named_usize("butler-row", index))
}

/// 通知 id → 会话里的行号。同 id 理论上只出现一次，真重复了取最后一条
/// （宿主重投时后写的那条才是用户刚看到的）。
fn notice_index(messages: &[Message], notice_id: &str) -> Option<usize> {
    messages
        .iter()
        .rposition(|message| message.notice_id.as_deref() == Some(notice_id))
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
    //! 管家窗口的渲染与上行链路。会话注入走 `with_session`，守护进程一侧
    //! 用**假连接**（`fake_link`：往 `outbox` 里塞一个通道）替代——上行
    //! 断言收到的 `Signal`，下行用 `handle_signal` 直接喂帧。全程不碰真实
    //! `~/.mns/agent/butler/`，也不连本地通道——测试不该改用户的对话记录，
    //! 更不该把宿主/守护进程搅进来。
    use super::{ButlerView, bubble_text, greeting_for, notice_index};
    use crate::butler::chat::ChatSnapshotData;
    use crate::butler::orchestrator::{AccountFacts, DriftRow};
    use crate::butler::session::{Message, Role, Session};
    use crate::ipc::Signal;
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
            // 测试注入的窗口不连守护进程：验证的是快照/上行的状态机，
            // 不该真去连本地通道。
            ButlerView::with_session(Session::default(), facts, window, cx)
        })
    }

    /// 假装连上了守护进程：注入 outbox 并标记在线。返回上行信号的接收端。
    fn fake_link(view: &mut ButlerView) -> tokio::sync::mpsc::UnboundedReceiver<Signal> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        view.outbox = Some(tx);
        view.online = true;
        rx
    }

    /// 造一帧快照 JSON。实例号固定：单一测试里不存在「服务换人」。
    fn snapshot_json(
        session: &Session,
        waiting: bool,
        buffer: &str,
        restore: Option<&str>,
    ) -> String {
        serde_json::to_string(&ChatSnapshotData {
            instance: 7,
            session: session.clone(),
            waiting,
            buffer: buffer.to_owned(),
            hint: None,
            error: None,
            restore: restore.map(str::to_owned),
        })
        .expect("快照应能序列化")
    }

    /// 断言上行通道里是一条 `ChatSend`，且文本相符。
    fn expect_chat_send(
        uplink: &mut tokio::sync::mpsc::UnboundedReceiver<Signal>,
        expected: &str,
    ) {
        match uplink.try_recv().expect("应递出 ChatSend") {
            Signal::ChatSend { text } => assert_eq!(text, expected, "递出的话应与输入一致"),
            other => panic!("应递 ChatSend，实际 {other:?}"),
        }
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
    fn 空态chip点击直接把问题递给守护进程(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, facts());
        let mut uplink = view.update(cx, |view, _| fake_link(view));
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

        expect_chat_send(&mut uplink, "现在偏离多少？");
        let messages = cx.update(|_, cx| view.update(cx, |view, _| view.session.messages.clone()));
        assert!(
            messages.is_empty(),
            "薄客户端不先落消息：用户的话由守护进程写进会话、快照带回来"
        );
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
        let (view, cx) = open(cx, facts());
        // 布局断言量的是正常（在线）状态：先连上，撤掉离线横幅再量。
        let _uplink = view.update(cx, |view, _| fake_link(view));
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
        let (view, cx) = open(cx, facts());
        // 同上：量在线状态的布局，先撤掉离线横幅。
        let _uplink = view.update(cx, |view, _| fake_link(view));
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
    fn 发送后输入框清空且话递到守护进程(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, facts());
        let mut uplink = view.update(cx, |view, _| fake_link(view));

        let input_after = cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value("今天要不要加仓", window, cx)
                });
                view.send(window, cx);
                view.input.read(cx).value().to_string()
            })
        });

        assert_eq!(input_after, "", "发送后输入框应清空");
        expect_chat_send(&mut uplink, "今天要不要加仓");
        // 会话与越界校验全在守护进程：本地不先插消息，等快照整帧带回。
        let messages = cx.update(|_, cx| view.update(cx, |view, _| view.session.messages.clone()));
        assert!(messages.is_empty(), "本地不先落消息");
    }

    #[gpui_kit::test]
    fn 等待中发送不吞字也不递话(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, facts());
        let mut uplink = view.update(cx, |view, _| fake_link(view));
        // 快照带回等待态：守护进程那边一轮还在进行。
        view.update(cx, |view, cx| {
            let mut session = Session::default();
            session.push(Message::user(Local::now(), "上一问"));
            view.handle_signal(
                Signal::ChatSnapshot {
                    json: snapshot_json(&session, true, "上一答的半截", None),
                },
                cx,
            );
        });

        let value = cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value("插队的一句", window, cx)
                });
                view.send(window, cx);
                view.input.read(cx).value().to_string()
            })
        });

        assert_eq!(value, "插队的一句", "等待中不吞字：话留在输入框里等下一轮");
        assert!(
            uplink.try_recv().is_err(),
            "等待中不该递 ChatSend：守护进程必拒，递过去只会被原样退回"
        );
    }

    /// 端到端的渲染生命周期：快照带等待态、增量逐段生长气泡、收尾快照
    /// 整帧替换成正式消息——薄客户端只按帧画。
    #[gpui_kit::test]
    fn 正文逐段上屏快照落地后换成正式气泡(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, facts());
        // 等待快照：会话里只有这一问，回答还没来。
        view.update(cx, |view, cx| {
            let mut session = Session::default();
            session.push(Message::user(Local::now(), "现在偏离多少"));
            view.handle_signal(
                Signal::ChatSnapshot {
                    json: snapshot_json(&session, true, "", None),
                },
                cx,
            );
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

        // 增量到达：流式气泡在场生长，等待行让位。
        view.update(cx, |view, cx| {
            view.handle_signal(
                Signal::ChatDelta {
                    text: "美股偏离 6.2pp。".to_owned(),
                },
                cx,
            )
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

        // 收尾快照：等待解除，半截落成正式消息。
        view.update(cx, |view, cx| {
            let mut session = Session::default();
            session.push(Message::user(Local::now(), "现在偏离多少"));
            session.push(Message::butler(Local::now(), "美股偏离 6.2pp。"));
            view.handle_signal(
                Signal::ChatSnapshot {
                    json: snapshot_json(&session, false, "", None),
                },
                cx,
            );
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
            assert!(!view.waiting, "收尾后不再等待");
        });
    }

    /// 快照整帧替换：restore 是被拒的那句话，render 时放回输入框。
    #[gpui_kit::test]
    fn 被拒的话由快照带回并放回输入框(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, facts());
        let _uplink = view.update(cx, |view, _| fake_link(view));

        view.update(cx, |view, cx| {
            view.handle_signal(
                Signal::ChatSnapshot {
                    json: snapshot_json(&Session::default(), false, "", Some("被拒绝的一句")),
                },
                cx,
            )
        });
        // restore 只能在 render 里应用：set_value 需要 &mut Window。
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        let value =
            cx.update(|_, cx| view.update(cx, |view, cx| view.input.read(cx).value().to_string()));
        assert_eq!(value, "被拒绝的一句", "被拒的话应放回输入框");
    }

    /// 离线横幅：断连在场、连上撤下、再断连回来。
    #[gpui_kit::test]
    fn 离线时横幅在场连上后撤下(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, facts());
        // 初始离线：还没连上守护进程。
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        assert!(
            cx.debug_bounds("butler-offline").is_some(),
            "离线应显示横幅"
        );

        // 连上：横幅撤下。
        let _uplink = view.update(cx, |view, _| fake_link(view));
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        assert!(
            cx.debug_bounds("butler-offline").is_none(),
            "连上后横幅应撤下"
        );

        // 断连：横幅回来。
        view.update(cx, |view, cx| view.go_offline(cx));
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        assert!(
            cx.debug_bounds("butler-offline").is_some(),
            "断连应重新显示横幅"
        );
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

    #[gpui_kit::test]
    fn 回车把话递出去而shift回车只换行(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, facts());
        let mut uplink = view.update(cx, |view, _| fake_link(view));

        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value("回车能发吗", window, cx);
                    input.focus(window, cx);
                });
            });
        });
        cx.simulate_keystrokes("enter");

        let value =
            cx.update(|_, cx| view.update(cx, |view, cx| view.input.read(cx).value().to_string()));
        assert_eq!(value, "", "发送后输入框应清空");
        expect_chat_send(&mut uplink, "回车能发吗");

        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value("第一行", window, cx);
                    input.focus(window, cx);
                });
            });
        });
        cx.simulate_keystrokes("shift-enter");

        let value =
            cx.update(|_, cx| view.update(cx, |view, cx| view.input.read(cx).value().to_string()));
        assert!(value.contains('\n'), "Shift+Enter 应换行，实际 {value:?}");
        assert!(
            uplink.try_recv().is_err(),
            "Shift+Enter 不该把话递给守护进程"
        );
    }

    #[gpui_kit::test]
    fn 清除会话二次确认后才递意图且不抢先清本地副本(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, facts());
        let mut uplink = view.update(cx, |view, _| fake_link(view));
        view.update(cx, |view, cx| {
            let mut session = Session::default();
            session.push(Message::user(Local::now(), "你好"));
            view.handle_signal(
                Signal::ChatSnapshot {
                    json: snapshot_json(&session, false, "", None),
                },
                cx,
            );
        });

        // 第一次：只进入待确认，不动会话、不递意图。
        view.update(cx, |view, cx| view.clear_session(cx));
        view.update(cx, |view, _| {
            assert!(view.clear_armed, "第一次点击只进入待确认");
            assert_eq!(view.session.messages.len(), 1, "第一次点击不删消息");
        });
        assert!(
            uplink.try_recv().is_err(),
            "第一次点击不该递 ChatClear"
        );

        // 第二次：意图递给守护进程；本地副本等快照整帧替换，不抢先清。
        view.update(cx, |view, cx| view.clear_session(cx));
        match uplink.try_recv().expect("第二次确认应递 ChatClear") {
            Signal::ChatClear => {}
            other => panic!("应递 ChatClear，实际 {other:?}"),
        }
        view.update(cx, |view, _| {
            assert!(!view.clear_armed, "递出后退出待确认");
            assert_eq!(
                view.session.messages.len(),
                1,
                "本地副本不抢先清：磁盘上的真相由守护进程清，快照带回后再换"
            );
        });
    }

    #[test]
    fn 通知id只匹配对应消息且重复取最后一条() {
        let mut session = Session::default();
        session.push(Message::butler(Local::now(), "第一条"));
        let mut targeted = Message::butler(Local::now(), "要定位的");
        targeted.notice_id = Some("n1-abc".to_owned());
        session.push(targeted);
        session.push(Message::butler(Local::now(), "最后一条"));

        assert_eq!(notice_index(&session.messages, "n1-abc"), Some(1));
        assert_eq!(notice_index(&session.messages, "missing"), None);
    }

    #[gpui_kit::test]
    fn 点通知定位到消息并显示可关闭的上下文横幅(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let mut session = Session::default();
        let mut targeted = Message::butler(Local::now(), "要定位的那条通知");
        targeted.notice_id = Some("n1-abc".to_owned());
        session.push(targeted);
        session.push(Message::butler(Local::now(), "后来的一条"));
        let (view, cx) = cx.add_window_view(|window, cx| {
            ButlerView::with_session(session, facts(), window, cx)
        });

        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        assert!(
            cx.debug_bounds("butler-notice-banner").is_none(),
            "没有点击通知时不该出现上下文横幅"
        );

        view.update(cx, |view, cx| view.focus_notice("n1-abc".to_owned(), cx));
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        assert!(
            cx.debug_bounds("butler-notice-banner").is_some(),
            "定位后应显示通知上下文"
        );
        view.update(cx, |view, _| {
            let focus = view.notice_focus.as_ref().expect("应保留通知上下文");
            assert_eq!(focus.index, Some(0), "应定位到带该 id 的消息");
            assert_eq!(focus.text.as_deref(), Some("要定位的那条通知"));
        });

        view.update(cx, |view, cx| view.dismiss_notice(cx));
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        assert!(
            cx.debug_bounds("butler-notice-banner").is_none(),
            "点「知道了」后横幅应消失"
        );
    }

    /// 通知先到、快照后到：窗口刚开时本地副本还没有那条消息，定位要等
    /// 快照到了再解析——这正是「点通知拉起窗口」路径上的真实时序。
    #[gpui_kit::test]
    fn 通知先到快照后到也能定位(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, facts());
        // 窗口刚开、还没收到任何快照：副本里没有那条消息。
        view.update(cx, |view, cx| view.focus_notice("n1-abc".to_owned(), cx));
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        assert!(
            cx.debug_bounds("butler-notice-banner").is_none(),
            "快照没到不该画横幅：先等会话真相"
        );

        // 快照到了：横幅出现，定位完成。
        view.update(cx, |view, cx| {
            let mut session = Session::default();
            let mut targeted = Message::butler(Local::now(), "要定位的那条");
            targeted.notice_id = Some("n1-abc".to_owned());
            session.push(targeted);
            view.handle_signal(
                Signal::ChatSnapshot {
                    json: snapshot_json(&session, false, "", None),
                },
                cx,
            );
        });
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        assert!(
            cx.debug_bounds("butler-notice-banner").is_some(),
            "快照到了应画出横幅"
        );
        view.update(cx, |view, _| {
            let focus = view.notice_focus.as_ref().expect("应定位到消息");
            assert_eq!(focus.index, Some(0), "应定位到带该 id 的消息");
            assert_eq!(focus.text.as_deref(), Some("要定位的那条"));
        });
    }
}
