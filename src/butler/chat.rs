//! 管家会话服务：`mns-butlerd` 里的唯一会话写者（P4）。
//!
//! P4 之前这段状态机住在管家窗口进程里：窗口一关，正在生成的回答就没了。
//! 现在它搬进守护进程——窗口只是「投影」：
//!
//! - **会话与等待状态归 butlerd**。`conversation.json` 只有本服务一个写者；
//!   窗口断开（detach）不丢回合：回答照常收完、落盘，窗口重连拿全量快照。
//! - **服务是纯同步的**。所有方法（`send` / `on_agent_event` / …）不碰
//!   线程也不碰网络，把「要做的事」作为 [`ChatEffect`] 列表返回，由
//!   守护进程的 select 循环执行——推快照给谁、起不起超时定时器、命令发给
//!   哪个 runtime，都是调用方的布局，测试因此不需要真起一个外部 agent。
//! - **订阅者数量不参与回合推进**：`Broadcast` 效果没有窗口接收时自然
//!   丢弃，会话与落盘照常进行。这是 detach 语义的一半；另一半是「连接
//!   断开就是 detach」，见 `ipc` 模块。
//! - **记忆每命令现读现改**（[`super::memory`]）：`memory.json` 还有 CLI 与
//!   守护进程自整理两个写者，拿启动时的快照整体覆盖会吞掉它们的条目。
//!   写锁只在「重读-修改-落盘」的区间内持有，绝不跨回合。
//!
//! 无 `agent` feature 时本服务仍可用：提问降级为本地模板直答，与旧「窗口
//! 没挂 ACP 会话」同一条路径。`Runtime` / `Timeout` 效果只在 agent 构建里
//! 出现。

use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use super::expression;
use super::memory::{self, Fact, Forgot, Memory, Remembered};
use super::orchestrator::{self, AccountFacts, Notice};
use super::session::{Message, Role, Session};
#[cfg(any(test, feature = "agent"))]
use super::talk;
use crate::ipc::Signal;

/// 一轮问答的等待上限。
///
/// 实测一次完整查账（agent 自己去跑几条 `mns` 只读命令）约 60–70 秒，
/// 所以留到 120 秒。到点必须收尾：界面停在「正在想」、而用户又不能再提问，
/// 比给一个半截答案更糟。
pub const CHAT_TIMEOUT: Duration = Duration::from_secs(120);

/// 管家会话的账户事实：与守护进程判定、CLI 审计同一条路径
/// （`orchestrator::ledger_facts`）。窗口进程与守护进程共用它——两边的
/// 数字永远同源，不存在「界面一套、回答一套」。
///
/// 阻塞调用（只读连接全库读取）：窗口侧放在刷新任务里，守护进程侧放
/// `spawn_blocking`，都不占主线程。
pub fn current_facts(now: DateTime<Local>) -> AccountFacts {
    let mut facts = orchestrator::ledger_facts(now);
    if let Ok(memory) = Memory::load() {
        facts = facts.with_promises(&memory, now.date_naive());
    }
    facts
}

/// 管家会话的 agent 设置。
///
/// 工作目录固定到管家自己的数据目录：opencode 会读取工作目录下的
/// `AGENTS.md`，落在项目仓库里时它会把「每日复盘」那套项目约定带进来
/// （实测会多吐一段 `mns-decision` 块，和正文重复）。管家不处理文件，
/// 也不该继承那套契约。
#[cfg(feature = "agent")]
pub fn chat_settings() -> crate::agent::AgentSettings {
    let mut settings = crate::agent::load_settings();
    let dir = super::butler_dir();
    let _ = std::fs::create_dir_all(&dir);
    settings.working_dir = dir.to_string_lossy().into_owned();
    settings
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
    #[cfg(any(test, feature = "agent"))]
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

    #[cfg(any(test, feature = "agent"))]
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
    #[cfg(any(test, feature = "agent"))]
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

    /// 丢弃这一轮：会话被清空、服务退出时用。
    fn abandon(&mut self) {
        self.turn = self.turn.wrapping_add(1);
        self.waiting = false;
        self.buffer.clear();
        self.template.clear();
    }
}

/// 收尾判定：通过校验就用 agent 的正文，否则退回模板并说明原因。
///
/// `note` 表示「这一轮为什么不算正常回答」（超时、报错）。内容即便合规也
/// 要把它附上，否则用户看到一句正常的话，却不知道 agent 其实没答完。
///
/// 抽成纯函数是为了能测——真实路径需要起一个外部 agent 进程。
#[cfg(any(test, feature = "agent"))]
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

/// 窗口拿到的会话快照。一帧带全：会话、等待态、半截正文、提示与错误——
/// 窗口不维护第二份增量状态，重连即恢复。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSnapshotData {
    /// 服务实例标识。窗口用它识别「对面的服务换了人」（守护进程重启过）：
    /// 实例变了，本地累积的流式半截必须作废，一切以新快照为准。
    pub instance: u64,
    pub session: Session,
    pub waiting: bool,
    /// 流式中的半截正文（等待中才非空）。
    pub buffer: String,
    /// 回答来源的一行说明。只在退回模板时出现。
    pub hint: Option<String>,
    /// 最近一次操作错误（会话保存、记忆写入）。没有为 `None`。
    pub error: Option<String>,
    /// 被拒绝的那句话：回合进行中发来的提问不排队，原样放回输入框。
    pub restore: Option<String>,
}

/// 一次会话操作产生的副作用。守护进程按序执行；测试直接断言。
#[derive(Debug)]
pub enum ChatEffect {
    /// 推给已连接窗口的帧。没有窗口挂着就丢弃——会话与落盘不依赖订阅者。
    Broadcast(Signal),
    /// 这一轮开始了：调用方在 [`CHAT_TIMEOUT`] 后回调 `on_timeout(turn)`。
    /// 序号对不上（已答完/已作废）就是空转一次，无害。
    Timeout { turn: u64 },
    /// 要发给 ACP runtime 的命令。无 `agent` feature 的构建不产生这条。
    #[cfg(feature = "agent")]
    Runtime(crate::agent::AgentCommand),
}

/// 会话服务。一个守护进程持有一个；窗口来来去去，服务不动。
pub struct ChatService {
    session: Session,
    turn: TurnState,
    /// 回答来源的一行说明。只在退回模板时出现，避免每句都挂个来源标签。
    hint: Option<String>,
    /// 最近一次操作错误，随快照带到窗口上。
    error: Option<String>,
    /// 存档读不出来时只读不写：那可能是用户唯一的一份对话记录，本次会话
    /// 只留在内存里，原因摆到快照的 `error` 字段上。
    persist: bool,
    /// 服务实例标识：进程重启必换，同进程内也随构造递增。
    instance: u64,
    /// ACP 会话已开过：`Start` 只发一次（带人设与记忆），之后 `Ask` 续问。
    /// 超时、报错、红线切断、清空会话都归位——被中断的会话不能续问。
    started: bool,
    /// 测试注入的 scratch 目录；`None` 时用真实账户目录。
    #[cfg(test)]
    scratch: Option<PathBuf>,
}

impl ChatService {
    /// 真实服务：从磁盘读会话。存档损坏不覆盖，只把原因放进 `error`。
    pub fn new() -> Self {
        let (session, error, persist) = match Session::load() {
            Ok(session) => (session, None, true),
            Err(error) => (
                Session::default(),
                Some(format!(
                    "会话存档读取失败，本次对话不会覆盖原文件：{error:#}"
                )),
                false,
            ),
        };
        Self {
            session,
            turn: TurnState::default(),
            hint: None,
            error,
            persist,
            instance: next_instance(),
            started: false,
            #[cfg(test)]
            scratch: None,
        }
    }

    /// 测试注入：会话与记忆都落在 scratch 目录，不碰真实账户目录。
    ///
    /// 直接按注入路径构造而不是先 `new()` 再覆盖——后者会先读一遍真实的
    /// `~/.mns/agent/butler/conversation.json`，测试不该碰用户的对话记录。
    #[cfg(test)]
    fn with_scratch(scratch: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&scratch);
        let (session, error, persist) = match Session::load_from(&scratch.join("conversation.json"))
        {
            Ok(session) => (session, None, true),
            Err(error) => (
                Session::default(),
                Some(format!(
                    "会话存档读取失败，本次对话不会覆盖原文件：{error:#}"
                )),
                false,
            ),
        };
        Self {
            session,
            turn: TurnState::default(),
            hint: None,
            error,
            persist,
            instance: next_instance(),
            started: false,
            scratch: Some(scratch),
        }
    }

    /// 窗口挂载：给一帧全量快照。连接断开就是 detach，没有告别帧。
    pub fn attach(&self) -> ChatEffect {
        self.snapshot_effect(None)
    }

    /// 用户发了一句话。`facts` 由调用方现算（守护进程放 `spawn_blocking`），
    /// 服务不缓存数字——缓存会让「刚记完账再提问」用上旧偏离。
    pub fn send(&mut self, text: String, facts: &AccountFacts) -> Vec<ChatEffect> {
        let now = Local::now();
        let text = text.trim().to_owned();
        if text.is_empty() {
            return Vec::new();
        }

        // 记忆类命令在本地就能答：不必为一句「记住了」启一个外部 agent。
        // 等待 agent 回答期间也照常受理——丢一句「记住」比晚一秒回答更糟。
        if let Some(reply) = self
            .handle_memory(&text, now)
            .or_else(|| self.recall_memory(&text))
        {
            self.session.push(Message::user(now, text));
            self.session.push(Message::butler(now, reply));
            self.session.mark_read(now);
            self.hint = None;
            self.persist();
            return vec![self.snapshot_effect(None)];
        }

        // 回合进行中：明确拒绝，不排队。窗口在等待时会禁用输入，走到这里
        // 多半是竞态——把话原样放回输入框（`restore`），不吞用户的字。
        if self.turn.waiting {
            return vec![self.snapshot_effect(Some(text))];
        }

        // 普通对话记一句「聊过什么」，供下周自整理。只记用户的问题，
        // 不记管家自己的措辞——复述自己的话会让记忆越走越偏。写之前重读
        // 磁盘：会话期间 CLI / 守护进程自整理可能也写过记忆。
        let memory = self.update_memory(|memory| memory.summarize(now, vec![clip(&text, 60)]));
        // 首轮注入的取数口径：记忆取**刚重读过的**磁盘快照——CLI 写进来的
        // 条目也要进上下文；最近对话取本句入档以前的——正在问的这句已经在
        // 【用户的问题】里，不该在【最近的对话】里再念一遍。
        #[cfg(feature = "agent")]
        let (memory_for_goal, recent_for_goal) = {
            let memory_lines = match memory {
                Ok((loaded, _)) => talk::memory_lines(&loaded, now.date_naive()),
                Err(_) => self
                    .load_memory()
                    .map(|loaded| talk::memory_lines(&loaded, now.date_naive()))
                    .unwrap_or_default(),
            };
            (memory_lines, talk::recent_lines(&self.session))
        };
        // 无 agent 构建用不上首轮注入，但摘要照写：两个构建的记忆口径
        // 必须一致。
        #[cfg(not(feature = "agent"))]
        let _ = memory;
        self.session.push(Message::user(now, text.clone()));

        let previous = self.previous_user_text();
        let intent = expression::intent_for(&text, previous.as_deref());
        // 模板是这一问的基线：agent 答不出来、超时、答得不像话，都回到它。
        // 预测与观点类问题不在本地拦截——管家人格可以聊倾向，交给 agent；
        // 模板兜底自己会如实说「说不准，只答有把握的部分」。
        let template = expression::reply_with_intent(intent, &text, facts);

        #[cfg(feature = "agent")]
        {
            self.turn.begin(template);
            let command = if self.started {
                crate::agent::AgentCommand::Ask { text }
            } else {
                self.started = true;
                crate::agent::AgentCommand::Start {
                    goal: talk::opening_goal(&text, facts, &memory_for_goal, &recent_for_goal),
                    settings: chat_settings(),
                    source: crate::agent::GoalSource::Conversation,
                }
            };
            self.session.mark_read(now);
            self.persist();
            vec![
                ChatEffect::Runtime(command),
                ChatEffect::Timeout {
                    turn: self.turn.turn,
                },
                self.snapshot_effect(None),
            ]
        }
        #[cfg(not(feature = "agent"))]
        {
            // 无 agent 的降级路径：与旧「窗口没挂 ACP 会话」相同，模板直答。
            self.session.push(Message::butler(now, template));
            self.session.mark_read(now);
            self.hint = None;
            self.persist();
            vec![self.snapshot_effect(None)]
        }
    }

    /// 用户确认清空会话（窗口已做过二次确认）。
    pub fn clear(&mut self) -> Vec<ChatEffect> {
        if let Err(error) = self.wipe_session_file() {
            self.error = Some(format!("清除失败：{error:#}"));
            return vec![self.snapshot_effect(None)];
        }
        self.session = Session::default();
        self.session.mark_read(Local::now());
        // 清空会话也要把 ACP 那头的 session 一并作废：只删存档的话，agent
        // 仍持有清空前的上下文，下一句会以续问接在旧对话上——用户以为清
        // 了，其实没清。`started` 归位后下一问重新走 Start（带人设与记忆
        // 的首轮），而不是 Ask（续问）。
        self.started = false;
        // 清空会话也要清掉等待中的回答（连同屏幕上的半截气泡）：它接的是
        // 已经被删掉的上下文。序号加一就够了，迟到的正文落不进新会话。
        self.turn.abandon();
        self.hint = None;
        // 不再落盘：磁盘上的语义就是「文件没了」。清空后再 persist 会把
        // 空会话写回去，等于没删干净；内存里的已读标记只影响本进程的
        // 未读数，重启后自然归零。
        let mut effects = Vec::new();
        #[cfg(feature = "agent")]
        effects.push(ChatEffect::Runtime(crate::agent::AgentCommand::Stop));
        effects.push(self.snapshot_effect(None));
        effects
    }

    /// 窗口已打开：标记会话已读（宿主的托盘未读数读这份存档）。
    pub fn seen(&mut self) -> Vec<ChatEffect> {
        self.session.mark_read(Local::now());
        self.persist();
        Vec::new()
    }

    /// 守护进程判定的开口并进会话（幂等：同 `notice_id` 只落一条）。
    ///
    /// `seen` 为 true 表示并入时窗口挂着（用户正在看），顺手标记已读——
    /// 对着用户脸说出去的话不该再算未读。
    pub fn merge_notices(&mut self, notices: &[Notice], seen: bool) -> Vec<ChatEffect> {
        let mut merged = false;
        for notice in notices {
            if self
                .session
                .messages
                .iter()
                .any(|message| message.notice_id.as_deref() == Some(notice.notice_id.as_str()))
            {
                continue;
            }
            self.session.push(Message {
                at: notice.at,
                role: Role::Butler,
                text: notice.draft.clone(),
                reason: Some(notice.reason),
                trigger: Some(notice.trigger.clone()),
                notice_id: Some(notice.notice_id.clone()),
            });
            merged = true;
        }
        if !merged {
            return Vec::new();
        }
        if seen {
            self.session.mark_read(Local::now());
        }
        self.persist();
        vec![self.snapshot_effect(None)]
    }

    /// runtime 事件。只关心正文、结束与失败：思考、计划、工具调用不进
    /// 会话——用户要的是答案，不是执行轨迹。
    #[cfg(feature = "agent")]
    pub fn on_agent_event(&mut self, event: crate::agent::AgentEvent) -> Vec<ChatEffect> {
        use crate::agent::{AgentCommand, AgentEvent, EntryKind};
        match event {
            // 正文只在 `Delta` 上出现（runner 对 Message 走流式），不重复收
            // `Entry`，否则同一段话会被拼两遍。边收边推给窗口。
            AgentEvent::Delta {
                kind: EntryKind::Message,
                text,
            } => {
                if !self.turn.waiting {
                    return Vec::new();
                }
                self.turn.push(&text);
                // 红线实时切断：谎称能替你下单的话不能挂到本轮自然结束。
                // 判定的对象与 `validate_talk` 同一份清洗文本，所以收尾
                // 校验会放行的回答（「没法帮你下单」）在这里同样不会命中；
                // 命中即等于收尾必被拦，只是提前到几十秒之前。
                if super::schedule::claims_execution(&talk::display_text(&self.turn.buffer))
                    .is_some()
                {
                    // 被 cut 的会话不再复用：下一问重新开一个。
                    self.started = false;
                    let mut effects = vec![ChatEffect::Runtime(AgentCommand::Stop)];
                    effects.extend(self.settle(Some("已实时切断".to_owned())));
                    return effects;
                }
                vec![ChatEffect::Broadcast(Signal::ChatDelta { text })]
            }
            AgentEvent::TurnFinished { .. } => self.settle(None),
            AgentEvent::Finished { .. } => {
                // 会话本身结束了：下一问要重新开一个，不能对着已关闭的
                // session 发 Ask。
                self.started = false;
                // 这一轮还没收尾就按失败处理，不能让等待态挂到天荒地老。
                if self.turn.waiting {
                    self.settle(Some("agent 会话提前结束".to_owned()))
                } else {
                    Vec::new()
                }
            }
            AgentEvent::Failed { message } => {
                self.started = false;
                self.settle(Some(format!("agent 报错：{message}")))
            }
            AgentEvent::Permission(prompt) => {
                // 守护进程没有授权界面，也无人在旁边点确认：拒绝并说清楚，
                // 避免卡在等待授权上不给回答。
                self.hint = Some(format!(
                    "agent 想调用「{}」，管家窗口不受理授权，已拒绝",
                    prompt.title
                ));
                vec![
                    ChatEffect::Runtime(AgentCommand::Deny { id: prompt.id }),
                    self.snapshot_effect(None),
                ]
            }
            _ => Vec::new(),
        }
    }

    /// 一轮超时：停掉这次生成，收下已经吐出的半截，没有就退回模板。
    ///
    /// 序号对不上说明已经答完、或者用户换了话题，直接返回。
    #[cfg(feature = "agent")]
    pub fn on_timeout(&mut self, turn: u64) -> Vec<ChatEffect> {
        if !self.turn.waiting || self.turn.turn != turn {
            return Vec::new();
        }
        // 被 cancel 的会话不再复用：下一问重新开一个，免得接在半截上下文上。
        self.started = false;
        let mut effects = vec![ChatEffect::Runtime(crate::agent::AgentCommand::Stop)];
        effects.extend(self.settle(Some(format!(
            "agent 超过 {} 秒没答完",
            CHAT_TIMEOUT.as_secs()
        ))));
        effects
    }

    /// 守护进程退出前收摊：放弃在飞的一轮（不落半截回答），请求 runtime
    /// 自行结束（ACP 子进程组随之被收掉）。
    pub fn shutdown(&mut self) -> Vec<ChatEffect> {
        self.turn.abandon();
        #[cfg(feature = "agent")]
        {
            vec![ChatEffect::Runtime(crate::agent::AgentCommand::Shutdown)]
        }
        #[cfg(not(feature = "agent"))]
        {
            Vec::new()
        }
    }

    /// 把这一轮收尾：通过校验就用 agent 的文本，否则退回本地模板。
    #[cfg(feature = "agent")]
    fn settle(&mut self, note: Option<String>) -> Vec<ChatEffect> {
        let Some((buffer, template)) = self.turn.take() else {
            return Vec::new();
        };
        let now = Local::now();
        let (text, hint) = settle_text(&buffer, &template, note.as_deref());
        self.session.push(Message::butler(now, text));
        self.session.mark_read(now);
        // 错误链可能很长，提示只留一行：用户只需要知道这条为什么不是
        // agent 说的。
        self.hint = hint.map(clip_note);
        self.persist();
        vec![self.snapshot_effect(None)]
    }

    fn snapshot_effect(&self, restore: Option<String>) -> ChatEffect {
        let data = ChatSnapshotData {
            instance: self.instance,
            session: self.session.clone(),
            waiting: self.turn.waiting,
            buffer: self.turn.streaming_text().unwrap_or_default().to_owned(),
            hint: self.hint.clone(),
            error: self.error.clone(),
            restore,
        };
        // 全是字符串与已有序列化类型，没有可失败的分支。
        let json = serde_json::to_string(&data).expect("会话快照序列化不会失败");
        ChatEffect::Broadcast(Signal::ChatSnapshot { json })
    }

    /// 「记住… / 忘掉…」走确定性记忆，不进普通问答。
    ///
    /// 返回 `Some(回复)` 表示这句话已被记忆层接住。识别不了就返回 `None`，
    /// 交给模板问答——不能让「周五要不要补仓」被当成一条承诺记下来。
    fn handle_memory(&mut self, text: &str, now: DateTime<Local>) -> Option<String> {
        let intent = memory::parse_intent(text, now.date_naive())?;
        // 先确认读得出来：读不出来就不动——拿空记忆做「记住」等于用空
        // 文件覆盖用户原有条目。
        if self.load_memory().is_err() {
            return Some("记忆文件这次读不出来，我没记，也没动原有记录。".to_owned());
        }
        match intent {
            memory::Intent::Remember { kind, text, due } => {
                let mut fact = Fact::new(kind, text.clone(), now);
                if let Some(day) = due {
                    fact = fact.expires_on(day);
                }
                match self.update_memory(|memory| memory.remember(fact)) {
                    Ok((_, verdict)) => {
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
                    Ok((_, Forgot::NotFound)) => {
                        let reply = format!("没找到和「{}」对得上的记忆。", clip(&needle, 20));
                        Some(valid_or(reply, "没找到对得上的记忆，我没动其他记录。"))
                    }
                    Ok((_, Forgot::Removed(hits))) => {
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
                    Ok((_, Forgot::Ambiguous(candidates))) => {
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
        let memory = match self.load_memory() {
            Ok(memory) => memory,
            Err(_) => {
                return Some("记忆文件这次读不出来，我念不了，也不会拿空记忆糊弄你。".to_owned());
            }
        };
        let today = Local::now().date_naive();
        let active = memory.active_facts(today);
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

    /// 每命令现读磁盘记忆。读不出时给空——记忆坏了不该挡住对话。
    fn load_memory(&self) -> Result<Memory, String> {
        let path = self.memory_path();
        Memory::load_at(&path).map_err(|error| format!("记忆文件读取失败：{error:#}"))
    }

    /// 在写锁内「重读记忆 → 应用这一笔变更 → 落盘」，返回落盘后的记忆。
    ///
    /// `memory.json` 有三个写者（本服务、`mns-butlerd` 自整理、CLI
    /// `remember` / `forget`）。会话可能开几个小时，期间别的写者可能写过：
    /// 拿旧快照整体覆盖会吞掉它们的条目，所以每次变更都从磁盘重读。
    /// 锁只在 load→apply→save 区间持有，绝不跨回合——等 agent 回答的
    /// 120 秒里不挡别的写者。
    ///
    /// 失败时这笔变更哪儿都没落（磁盘没动）：错误随快照带到窗口上，用户
    /// 至少知道「这条没记上」，而不是以为记住了。
    fn update_memory<R>(
        &mut self,
        apply: impl FnOnce(&mut Memory) -> R,
    ) -> Result<(Memory, R), String> {
        let path = self.memory_path();
        let guard = super::writer_lock();
        Memory::update_at(&path, guard.as_ref(), apply)
            .map_err(|error| format!("记忆更新失败：{error:#}"))
    }

    fn memory_path(&self) -> PathBuf {
        #[cfg(test)]
        if let Some(scratch) = &self.scratch {
            return scratch.join("memory.json");
        }
        Memory::path()
    }

    fn session_path(&self) -> PathBuf {
        #[cfg(test)]
        if let Some(scratch) = &self.scratch {
            return scratch.join("conversation.json");
        }
        Session::path()
    }

    fn persist(&mut self) {
        if !self.persist {
            return;
        }
        if let Err(error) = self.session.save_to(&self.session_path()) {
            self.error = Some(format!("会话保存失败：{error:#}"));
        }
    }

    fn wipe_session_file(&mut self) -> anyhow::Result<()> {
        if !self.persist {
            return Ok(());
        }
        Session::clear_at(&self.session_path())
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

    /// 测试辅助：直接把等待槽置为「这一轮在等」。无 agent 构建里
    /// `send` 不产生回合，拒绝路径只有靠它才能覆盖。
    #[cfg(test)]
    fn begin_turn_for_test(&mut self, template: &str) -> u64 {
        assert!(self.turn.begin(template.to_owned()));
        self.turn.turn
    }
}

/// 服务实例标识：进程号占高位、进程内序号占低位。进程重启必换，同进程
/// 内多次构造（测试）也互不相同。
fn next_instance() -> u64 {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    ((std::process::id() as u64) << 32) | COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn valid_or(reply: String, fallback: &str) -> String {
    if super::schedule::validate_dialogue(&reply).is_ok() {
        reply
    } else {
        fallback.to_owned()
    }
}

/// 降级提示只占一行。错误链可能很长，整段铺在界面上没人读得下去——
/// 用户需要的只是「这条为什么不是模型说的」。
#[cfg(feature = "agent")]
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    use super::super::orchestrator::{DriftRow, Notice};
    use super::super::schedule::SpeakReason;

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

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("mns-butler-chat-{name}-{}", std::process::id()))
    }

    /// 全新 scratch 目录上的服务：目录先清空，保证每个测试从空状态开始。
    fn open(name: &str) -> ChatService {
        let dir = scratch(name);
        let _ = std::fs::remove_dir_all(&dir);
        ChatService::with_scratch(dir)
    }

    fn snapshot_of(effects: &[ChatEffect]) -> ChatSnapshotData {
        effects
            .iter()
            .find_map(|effect| match effect {
                ChatEffect::Broadcast(Signal::ChatSnapshot { json }) => {
                    Some(serde_json::from_str(json).expect("快照可解析"))
                }
                _ => None,
            })
            .expect("应带一帧快照")
    }

    fn last_reply(effects: &[ChatEffect]) -> String {
        let data = snapshot_of(effects);
        data.session.messages.last().expect("应有回复").text.clone()
    }

    fn memory_of(name: &str) -> Memory {
        Memory::load_at(&scratch(name).join("memory.json")).unwrap()
    }

    /// 快照整帧带全：会话、等待态、半截正文、提示与错误。
    #[test]
    fn 快照一帧带全会话与等待状态() {
        let mut service = open("snapshot");
        let effects = service.send("现在偏离多少".to_owned(), &facts());
        let data = snapshot_of(&effects);
        assert_eq!(data.instance, service.instance);
        assert!(data.restore.is_none());
        #[cfg(feature = "agent")]
        {
            assert_eq!(
                data.session.messages.len(),
                1,
                "agent 构建首帧只有用户的问题"
            );
            assert!(data.waiting, "agent 构建在等回答");
            assert!(data.buffer.is_empty(), "正文未到，没有半截");
        }
        #[cfg(not(feature = "agent"))]
        {
            assert_eq!(
                data.session.messages.len(),
                2,
                "无 agent 构建一轮问答立即完整（用户 + 模板）"
            );
            assert!(!data.waiting, "模板直答没有等待态");
        }
    }

    /// 服务实例标识：同进程两次构造互不相同，快照原样带出。
    #[test]
    fn 服务实例标识随构造递增() {
        let first = open("instance-a");
        let second = open("instance-b");
        assert_ne!(first.instance, second.instance);
        assert!(
            first.instance >> 32 == second.instance >> 32,
            "同进程高位相同"
        );
    }

    /// 回合进行中的提问被明确拒绝：不进会话、不排队，`restore` 把话
    /// 原样带回输入框。
    #[test]
    fn 回合进行中明确拒绝不排队() {
        let mut service = open("reject");
        let turn = service.begin_turn_for_test("上一轮模板");
        assert!(turn > 0);
        let effects = service.send("还没答完就想再问".to_owned(), &facts());
        let data = snapshot_of(&effects);
        assert_eq!(
            data.restore.as_deref(),
            Some("还没答完就想再问"),
            "被拒的话应放回输入框"
        );
        assert_eq!(data.session.messages.len(), 0, "被拒的话不该进会话");
        assert!(data.waiting, "拒绝不应打断在飞的一轮");
    }

    /// 普通对话走模板（无 agent 降级路径），回复过表达边界校验，
    /// 并给记忆留一条摘要。
    #[test]
    fn 普通对话走本地模板且回复不越界() {
        let mut service = open("template");
        let effects = service.send("今天要不要加仓".to_owned(), &facts());
        let data = snapshot_of(&effects);
        #[cfg(not(feature = "agent"))]
        {
            assert_eq!(
                data.session.messages.len(),
                2,
                "一轮问答应是用户 + 管家两条"
            );
            assert_eq!(data.session.messages[0].role, Role::User);
            assert_eq!(data.session.messages[1].role, Role::Butler);
            assert!(
                data.session.messages[1].text.contains("下单我帮不了"),
                "回复应如实说明不代下单，实际 {}",
                data.session.messages[1].text
            );
            assert!(
                super::super::schedule::validate_dialogue(&data.session.messages[1].text).is_ok(),
                "回复越过表达边界：{}",
                data.session.messages[1].text
            );
        }
        #[cfg(feature = "agent")]
        {
            assert_eq!(
                data.session.messages.len(),
                1,
                "agent 构建回答未回，先只有问题"
            );
        }
        // 摘要落了盘（两个构建同口径）：直接从 scratch 读回。
        let memory = memory_of("template");
        assert_eq!(memory.summaries.len(), 1, "普通对话应留一条摘要");
        assert!(
            memory.summaries[0].points.len() <= memory::MAX_POINTS,
            "单次摘要不得超过三条要点"
        );
    }

    /// 「记住 / 忘掉 / 念记忆」走记忆层，不启 agent，也不写真实目录。
    #[test]
    fn 记住与忘掉走记忆层且不写真实目录() {
        let mut service = open("memory");

        let effects = service.send("记住：周五补美股".to_owned(), &facts());
        let remember_reply = last_reply(&effects);
        assert_eq!(memory_of("memory").facts.len(), 1, "意图应落进记忆");

        let effects = service.send("忘掉补美股".to_owned(), &facts());
        let forget_reply = last_reply(&effects);
        let memory = memory_of("memory");
        assert!(memory.facts.is_empty(), "忘掉后不再参与表达");
        assert_eq!(memory.archived_facts.len(), 1, "归档可查");

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
    }

    /// 同文不同到期日的条目，忘掉必须带日期消歧：歧义时一条不动。
    #[test]
    fn 同文不同到期日忘掉必须带日期消歧() {
        let mut service = open("ambiguity");
        // 用绝对日期，避免测试依赖运行当天的日历。
        service.send("记住 2026-12-27 截止 补美股".to_owned(), &facts());
        service.send("记住 2026-12-30 截止 补美股".to_owned(), &facts());
        assert_eq!(
            memory_of("ambiguity").facts.len(),
            2,
            "不同到期日应各存一条"
        );

        // 不带日期：同文两条 → 歧义，一条不动，候选带日期。
        let effects = service.send("忘掉补美股".to_owned(), &facts());
        let ambiguous = last_reply(&effects);
        assert_eq!(memory_of("ambiguity").facts.len(), 2, "歧义时一条都不能动");

        // 带日期：只删那一天的那条。
        let effects = service.send("忘掉 2026-12-27 截止 补美股".to_owned(), &facts());
        let removed = last_reply(&effects);
        assert_eq!(memory_of("ambiguity").facts.len(), 1, "带日期只删那一条");

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
    }

    /// 「你记得什么」：念出条目原文与到期日，让用户能核对记的日子。
    #[test]
    fn 问记得什么时念出常驻记忆() {
        let mut service = open("recall");
        service.send("记住：周五补美股".to_owned(), &facts());
        let effects = service.send("你记得什么".to_owned(), &facts());
        let recall = last_reply(&effects);
        assert!(recall.contains("补美股"), "应念出条目原文，实际 {recall}");
        assert!(
            recall.contains("· 到期"),
            "recall 应回显到期日，误解析才看得见，实际 {recall}"
        );
        assert!(
            super::super::schedule::validate_dialogue(&recall).is_ok(),
            "回复越过表达边界：{recall}"
        );
    }

    /// 清空会话：存档删除、等待槽作废、下一问从头开始。
    #[test]
    fn 清除会话重置存档与等待槽() {
        let mut service = open("clear");
        service.send("现在偏离多少".to_owned(), &facts());
        let session_path = scratch("clear").join("conversation.json");
        assert!(session_path.exists(), "应答前先落盘");

        let effects = service.clear();
        let data = snapshot_of(&effects);
        assert!(data.session.messages.is_empty(), "清除后会话为空");
        assert!(!session_path.exists(), "存档文件应被删除");
        // 清空后新的一问照常工作。
        let effects = service.send("还在吗".to_owned(), &facts());
        let data = snapshot_of(&effects);
        #[cfg(not(feature = "agent"))]
        assert_eq!(data.session.messages.len(), 2);
        #[cfg(feature = "agent")]
        assert_eq!(
            data.session.messages.len(),
            1,
            "agent 构建新的一问先进等待，回答由事件收尾"
        );
    }

    /// 存档损坏：本次不覆盖原文件，错误随快照带到窗口。
    #[test]
    fn 会话存档损坏时不覆盖原文件() {
        let dir = scratch("corrupt");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let session_path = dir.join("conversation.json");
        std::fs::write(&session_path, "{\"messages\":[{\"role\":\"drift\"}]}").unwrap();

        let mut service = ChatService::with_scratch(dir);
        let effects = service.send("现在偏离多少".to_owned(), &facts());
        let data = snapshot_of(&effects);
        assert!(
            data.error
                .as_deref()
                .is_some_and(|error| error.contains("读取失败")),
            "损坏原因应摆到快照上，实际 {:?}",
            data.error
        );
        assert_eq!(
            std::fs::read_to_string(&session_path).unwrap(),
            "{\"messages\":[{\"role\":\"drift\"}]}",
            "损坏的存档不能被覆盖"
        );
    }

    /// 守护进程的开口并进会话，按 notice_id 幂等：重复投递不重复落条。
    #[test]
    fn merge_notices按id幂等() {
        let mut service = open("merge");
        let notice = Notice {
            at: Local
                .with_ymd_and_hms(2026, 11, 20, 9, 0, 0)
                .single()
                .unwrap(),
            reason: SpeakReason::ActionableStep,
            trigger: "drift:2026-11-20".to_owned(),
            draft: "美股偏离 6.2pp".to_owned(),
            notice_id: "n-merge-1".to_owned(),
        };

        let effects = service.merge_notices(&[notice.clone()], false);
        let data = snapshot_of(&effects);
        assert_eq!(data.session.messages.len(), 1, "第一次并入应落一条");
        assert_eq!(
            data.session.messages[0].notice_id.as_deref(),
            Some("n-merge-1")
        );
        assert!(
            data.session.last_read_at.is_none(),
            "窗口没开着并入，不该顺手标已读"
        );

        // 幂等：同一批再来一遍，不追加、不重置已读。
        service.seen();
        let effects = service.merge_notices(&[notice.clone()], false);
        assert!(
            effects.is_empty(),
            "全部重复时没有新东西可广播，实际 {effects:?}"
        );
        assert_eq!(service.session.messages.len(), 1);

        // 窗口开着并入：顺手标记已读。先把 seen() 留下的标记抹掉，
        // 才能证明这次是 merge 标的，不是上一轮的残留。
        service.session.last_read_at = None;
        service.merge_notices(
            &[Notice {
                notice_id: "n-merge-2".to_owned(),
                at: Local
                    .with_ymd_and_hms(2026, 11, 20, 9, 5, 0)
                    .single()
                    .unwrap(),
                ..notice
            }],
            true,
        );
        assert!(
            service.session.last_read_at.is_some(),
            "对着窗口说的不算未读"
        );
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

    /// 观点类回答被采纳而超长退回模板：管家人格可以给倾向性观点，校验只守
    /// 诚实性底线（非空、限长、不谎称能下单）。
    #[test]
    fn 观点类回答被采纳而超长退回模板() {
        let (text, hint) = settle_text(
            "我的看法是可以加仓美股，目标价到 45% 也不奇怪。",
            "美股偏离 6.2pp（带宽 ±4.0pp）",
            None,
        );
        assert_eq!(text, "我的看法是可以加仓美股，目标价到 45% 也不奇怪。");
        assert!(hint.is_none(), "通过校验的回答不该带降级提示");

        let long = "话".repeat(talk::MAX_TALK_CHARS + 1);
        let (text, hint) = settle_text(&long, "美股偏离 6.2pp（带宽 ±4.0pp）", None);
        assert_eq!(
            text, "美股偏离 6.2pp（带宽 ±4.0pp）",
            "超长回答必须退回模板"
        );
        let hint = hint.expect("要说明退回原因");
        assert!(hint.contains("超过"), "提示应说明越界类型：{hint}");
    }

    /// markdown 结构在会话里原样保留：渲染层用 markdown 画，校验不能把
    /// 「- 结论：…」这类列表行当成机器字段删掉。
    #[test]
    fn markdown在会话里原样保留给渲染层() {
        let (text, _) = settle_text(
            "**美股** 低配 6.2pp。\n\n- 结论：先别动\n- 等决策日再看",
            "模板",
            None,
        );
        assert!(text.contains("**美股**"), "实际 {text}");
        assert!(text.contains("- 结论：先别动"), "实际 {text}");
    }

    /// 唯一保留的会话硬底线：没有券商通道却说能下单是撒谎。
    #[test]
    fn 谎称已经下单会退回模板() {
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

    /// 磁盘上的会话与内存一致：send 落盘后重开服务能读回同一份。
    #[test]
    fn 会话随每次变更落盘() {
        let dir = scratch("persist");
        let _ = std::fs::remove_dir_all(&dir);
        let mut service = ChatService::with_scratch(dir.clone());
        service.send("现在偏离多少".to_owned(), &facts());

        let reopened = ChatService::with_scratch(dir.clone());
        #[cfg(not(feature = "agent"))]
        let expected = 2;
        #[cfg(feature = "agent")]
        let expected = 1;
        assert_eq!(
            reopened.session.messages.len(),
            expected,
            "重开服务应从磁盘读回会话"
        );
        assert_eq!(reopened.session.messages[0].text, "现在偏离多少");
    }

    /// 记忆每命令重读磁盘：CLI 同期写进去的条目不被吞。
    #[test]
    fn 记忆更新前总是重读磁盘() {
        let dir = scratch("reload");
        let _ = std::fs::remove_dir_all(&dir);
        let mut service = ChatService::with_scratch(dir.clone());

        // 另一个写者（CLI 语义）直接落一条。
        let (_, _) = Memory::update_at(&dir.join("memory.json"), None, |memory| {
            memory.remember(Fact::new(
                memory::FactKind::Commitment,
                "外部写入的承诺".to_owned(),
                Local
                    .with_ymd_and_hms(2026, 11, 20, 8, 0, 0)
                    .single()
                    .unwrap(),
            ))
        })
        .unwrap();

        // 本服务再写一条：重读后两条都在。
        service.send("记住：内部条目".to_owned(), &facts());
        let memory = Memory::load_at(&dir.join("memory.json")).unwrap();
        assert_eq!(memory.facts.len(), 2, "外部写入的条目不能被吞");
    }

    // ---- agent 门控的行为面：回合推进与 runtime 命令的判定。纯同步断言，
    // 不起真进程——有没有外部 agent 不影响这些分支的对错。

    #[cfg(feature = "agent")]
    use crate::agent::{AgentCommand, AgentEvent, EntryKind};

    #[cfg(feature = "agent")]
    fn runtime_command(effects: &[ChatEffect]) -> Option<AgentCommand> {
        effects.iter().find_map(|effect| match effect {
            ChatEffect::Runtime(command) => Some(command.clone()),
            _ => None,
        })
    }

    /// 首问 `Start`（带人设与记忆），答完后的追问走 `Ask` 续问。
    #[cfg(feature = "agent")]
    #[test]
    fn 首问start答完后续问问ask() {
        let mut service = open("start-ask");
        let effects = service.send("现在偏离多少".to_owned(), &facts());
        assert!(matches!(
            runtime_command(&effects),
            Some(AgentCommand::Start { .. })
        ));

        // 答完这一轮，下一问才是续问。
        service.on_agent_event(AgentEvent::TurnFinished {
            stop_reason: "end".to_owned(),
        });
        let effects = service.send("A股呢".to_owned(), &facts());
        assert!(matches!(
            runtime_command(&effects),
            Some(AgentCommand::Ask { .. })
        ));
    }

    /// 红线实时切断：流式累积中谎称能替用户下单，话音未落就收尾这一轮并
    /// 退回模板；诚实的否定（「没法帮你下单」）不触发切断。
    #[cfg(feature = "agent")]
    #[test]
    fn 流式中谎称下单立即切断而诚实否定不切() {
        let mut service = open("redline");
        let _effects = service.send("帮我买了".to_owned(), &facts());
        assert!(service.started, "首问应标记 ACP 会话已开");
        let template = service.turn.template.clone();

        // 诚实否定：正文继续长，只有 Delta 广播。
        let effects = service.on_agent_event(AgentEvent::Delta {
            kind: EntryKind::Message,
            text: "这个我没法".to_owned(),
        });
        assert!(
            matches!(
                effects.as_slice(),
                [ChatEffect::Broadcast(Signal::ChatDelta { .. })]
            ),
            "正文增长只广播 Delta，实际 {effects:?}"
        );
        service.on_agent_event(AgentEvent::Delta {
            kind: EntryKind::Message,
            text: "帮你下单，得你自己来。".to_owned(),
        });
        assert!(service.turn.waiting, "「没法帮你下单」不该触发切断");

        // 思考流不进正文缓冲。
        service.on_agent_event(AgentEvent::Delta {
            kind: EntryKind::Thought,
            text: "用户想让我代买".to_owned(),
        });
        assert_eq!(service.turn.buffer, "这个我没法帮你下单，得你自己来。");

        // 换一段真正谎称执行的：命中即收尾、退模板、切断 runtime。
        let effects = service.on_agent_event(AgentEvent::Delta {
            kind: EntryKind::Message,
            text: "好的，已经帮你下单了，明天到账。".to_owned(),
        });
        assert!(matches!(
            runtime_command(&effects),
            Some(AgentCommand::Stop)
        ));
        assert!(!service.turn.waiting, "命中禁语应立即收尾这一轮");
        assert!(!service.started, "被切断的 ACP 会话不能复用");
        let data = snapshot_of(&effects);
        let last = data.session.messages.last().expect("应有兜底回复");
        assert_eq!(last.role, Role::Butler);
        assert_eq!(last.text, template, "谎称执行必须退回模板");
        let hint = data.hint.expect("要说明切断原因");
        assert!(hint.contains("实时切断"), "实际 {hint}");
        assert!(hint.contains("谎称能替用户执行下单"), "实际 {hint}");
    }

    /// 清空会话：停掉 runtime、作废等待槽与 ACP 会话，下一问重新 Start。
    #[cfg(feature = "agent")]
    #[test]
    fn 清空会话作废runtime与等待槽() {
        let mut service = open("clear-agent");
        service.send("现在偏离多少".to_owned(), &facts());
        assert!(service.started && service.turn.waiting);

        let effects = service.clear();
        assert!(matches!(
            runtime_command(&effects),
            Some(AgentCommand::Stop)
        ));
        assert!(!service.started, "清空后 ACP 会话作废，续问要重新开");
        assert!(!service.turn.waiting, "等待中的回答随会话作废");

        let effects = service.send("还在吗".to_owned(), &facts());
        assert!(matches!(
            runtime_command(&effects),
            Some(AgentCommand::Start { .. })
        ));
    }

    /// 超时收尾：序号对不上的超时是空转；对得上的停掉生成、收下半截正文
    /// 并标注「没答完」。
    #[cfg(feature = "agent")]
    #[test]
    fn 超时停掉生成并收下半截() {
        let mut service = open("timeout");
        let effects = service.send("现在偏离多少".to_owned(), &facts());
        let turn = effects
            .iter()
            .find_map(|effect| match effect {
                ChatEffect::Timeout { turn } => Some(*turn),
                _ => None,
            })
            .expect("send 应带超时序号");

        service.on_agent_event(AgentEvent::Delta {
            kind: EntryKind::Message,
            text: "美股偏离 6.2pp，".to_owned(),
        });

        // 别人的序号：这一轮不该被打断。
        let effects = service.on_timeout(turn.wrapping_add(999));
        assert!(effects.is_empty(), "过期超时应是空操作");
        assert!(service.turn.waiting, "过期超时不该打断这一轮");

        // 自己的序号：Stop + 收尾，半截正文通过校验就保留。
        let effects = service.on_timeout(turn);
        assert!(matches!(
            runtime_command(&effects),
            Some(AgentCommand::Stop)
        ));
        assert!(!service.turn.waiting);
        assert!(!service.started, "被超时打断的 ACP 会话不能复用");
        let data = snapshot_of(&effects);
        let last = data.session.messages.last().expect("应有收尾回答");
        assert_eq!(last.text, "美股偏离 6.2pp，", "已经生成的别扔掉");
        let hint = data.hint.expect("要说明这条没答完");
        assert!(hint.contains("没答完"), "实际 {hint}");
    }

    /// agent 报错：退回模板、作废 ACP 会话，提示带原因。
    #[cfg(feature = "agent")]
    #[test]
    fn agent报错退回模板并作废会话() {
        let mut service = open("failed");
        service.send("现在偏离多少".to_owned(), &facts());
        let template = service.turn.template.clone();

        let effects = service.on_agent_event(AgentEvent::Failed {
            message: "连不上模型".to_owned(),
        });
        assert!(!service.turn.waiting);
        assert!(!service.started, "失败的 ACP 会话不能复用");
        let data = snapshot_of(&effects);
        assert_eq!(data.session.messages.last().unwrap().text, template);
        let hint = data.hint.expect("失败要说清原因");
        assert!(hint.contains("agent 报错"), "实际 {hint}");
    }

    /// 授权请求没有界面可点：拒绝并提示，正文缓冲不动。
    #[cfg(feature = "agent")]
    #[test]
    fn 授权请求被拒绝并提示() {
        let mut service = open("permission");
        service.send("现在偏离多少".to_owned(), &facts());

        let effects =
            service.on_agent_event(AgentEvent::Permission(crate::agent::PermissionPrompt {
                id: 7,
                title: "执行 shell 命令".to_owned(),
                detail: String::new(),
                options: Vec::new(),
            }));
        assert!(matches!(
            runtime_command(&effects),
            Some(AgentCommand::Deny { id: 7 })
        ));
        let data = snapshot_of(&effects);
        let hint = data.hint.expect("要说明已拒绝");
        assert!(hint.contains("不受理授权"), "实际 {hint}");
        assert!(service.turn.waiting, "拒绝授权不打断这一轮");
    }

    /// 回合无人订阅照常收尾落盘：窗口可以在生成中途关掉，回答不丢。
    /// 这就是 detach 语义的服务端一半——Broadcast 没人收，会话照走。
    #[cfg(feature = "agent")]
    #[test]
    fn 回合无人订阅照常收尾落盘() {
        let dir = scratch("no-subscriber");
        let _ = std::fs::remove_dir_all(&dir);
        let mut service = ChatService::with_scratch(dir.clone());
        let effects = service.send("现在偏离多少".to_owned(), &facts());
        drop(effects);
        drop(service.on_agent_event(AgentEvent::Delta {
            kind: EntryKind::Message,
            text: "美股偏离 6.2pp。".to_owned(),
        }));
        drop(service.on_agent_event(AgentEvent::TurnFinished {
            stop_reason: "end".to_owned(),
        }));

        // 从磁盘重开：回答已落盘，不依赖任何窗口曾收过它。
        let reopened = ChatService::with_scratch(dir);
        let last = reopened.session.messages.last().expect("回答应已落盘");
        assert_eq!(last.role, Role::Butler);
        assert_eq!(last.text, "美股偏离 6.2pp。");
    }
}
