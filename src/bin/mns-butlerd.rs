//! `mns-butlerd`：管家常驻进程。
//!
//! 为什么要有它：GUI 关掉之后管家本该继续守着——到点提醒、承诺到期、定时
//! 巡检都不该因为「窗口关了」而停。把这个循环留在 GUI 进程里做不到这件事。
//!
//! 它是一个**事件驱动的常驻进程**，不是一个定时器：
//!
//! - 唤醒源只有三类——定时巡检、账本事件、自设任务/承诺到期；
//! - 循环里没有 tick：它算出「下一次该在什么时候想事情」，然后一直睡到那个
//!   时刻；账本事件与状态变更由 GUI / CLI 通过本地 socket 直接叫醒它；
//! - 没有系统计划任务（launchd / cron / 计划任务）参与：那些每次都要拉起
//!   新进程，还要跟着用户的时钟和休眠漂；这里要的是「同一个进程里的下一次
//!   思考」。
//!
//! 它只做四件事：
//!
//! 1. 按唤醒源跑一次**确定性**判定；
//! 2. 判定要开口时，往 `outbox.jsonl`（审计）与 `pending.jsonl`（投递）各写
//!    一行；
//! 3. 把开口并进会话存档（P4 起本进程是唯一写者），再把投递推给宿主，
//!    由宿主决定要不要弹系统横幅；
//! 4. 确定性内核没有由头时，按需起一次外部 ACP agent，问它有没有一句值得
//!    主动说的话（`agent-init`）。回答必须过通知口径（≤60 字、无买卖话术），
//!    否则本轮沉默；runtime 每轮 `Shutdown`，不留下空闲进程。
//!
//! **它不发系统通知**。macOS 只认 app 的主可执行文件：同 bundle 的守护进程
//! 调 `requestAuthorization` 会拿到 `UNErrorDomain 1`（notificationsNotAllowed）。
//! 判定可以拆进程，送达不行——宿主收到 `DeliverNotice` 后自己发。
//!
//! 它**只读账本**：唤醒时自己用只读连接从数据库提取账户事实
//! （`orchestrator::ledger_facts`），不再依赖 GUI 写出的 `facts.json`——
//! GUI 多久没开都不影响判定。只读边界钉在连接标志上：本进程对账本没有
//! 写路径。
//!
//! 会话存档（`conversation.json`）P4 起只有本进程一个写者（`butler::chat`）：
//! 管家窗口降级为投影，挂上拿快照、断开即 detach，窗口关掉不丢回合——
//! 回答照常收完落盘，重连拿全量快照。ACP 会话的 runtime 也归本进程，
//! 退出时随本进程一起收掉。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Local};
use money_never_sleep::agent::{AgentCommand, AgentEvent, AgentHandle, EntryKind, GoalSource};
use money_never_sleep::butler::agenda;
use money_never_sleep::butler::chat::{self, ChatEffect};
use money_never_sleep::butler::orchestrator::{
    self, AccountFacts, AgentInitOutcome, AgentInitRecord, Notice, AGENT_INIT_MIN_IDLE_HOURS,
    agent_init_backoff, append_init_record,
};
use money_never_sleep::butler::schedule;
use money_never_sleep::butler::talk;
use money_never_sleep::butler::{
    writer_lock, Butler, Session, SilentBecause, SpeakCandidate, SpeakDecision, SpeakReason,
    WakeSource, WakeVerdict,
};
use money_never_sleep::ipc::{self, Client, Listener, Signal};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// 单次睡眠的上限。
///
/// 定时器基于单调时钟，而系统休眠时它不前进——睡 6 小时被睡眠打断，醒来时
/// 真实时间可能已经过去一整晚。每半小时用真实时间重算一次，比相信一个跨
/// 睡眠的绝对时刻更稳。它不代表「每半小时巡检一次」：巡检间隔仍由
/// `WakeScheduler` 判定。
const MAX_SLEEP: Duration = Duration::from_secs(30 * 60);

/// 一次 agent-init 的硬超时。超时即本轮沉默——主动开口没有模板兜底，
/// 不说比说一半好，更不能卡住主循环。
const AGENT_INIT_TIMEOUT: Duration = Duration::from_secs(120);

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("mns-butlerd: {error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    // 监听在前：已有守护进程在跑时直接退出，不做第二个。
    let (listener, mut events) = match ipc::listen("mns-butlerd") {
        Ok(pair) => pair,
        Err(error) => {
            eprintln!("mns-butlerd: {error}（已有守护进程在运行）");
            return Ok(());
        }
    };

    let mut link = GuiLink::default();
    link.ensure();

    // agent-init 跑在独立任务里：`start_runtime` 自己起线程，等待回答的
    // 120 秒不能占住这条 select 循环，否则托盘停止与账本唤醒都要排队。
    // 任务完成后只回一条「去排空 pending」，连接仍归主线程独有。
    let (init_tx, mut init_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let init_busy = Arc::new(AtomicBool::new(false));

    // P4：管家会话归本进程。窗口只是投影——挂上拿快照、断开即 detach，
    // 广播没人收就丢弃，回合照常收完落盘。
    let mut chat = ChatState::new(listener);
    // 会话用的 ACP runtime 与 agent-init 的各自独立：同一个 `start_runtime`
    // 起的两套线程，互不共用会话。会话这套活到进程退出，由 `finish` 收掉。
    let (agent_handle, mut agent_events) = money_never_sleep::agent::start_runtime();
    let (timeout_tx, mut timeout_rx) = tokio::sync::mpsc::unbounded_channel::<u64>();
    let runtime = ChatRuntime {
        agent: agent_handle,
        timeouts: timeout_tx,
    };

    // 启动排空：宿主可能整晚没开，攒下的开口此刻并进会话并试着投递。
    // 投不出去的进 backlog，等宿主上线或 30 秒一轮重试。
    chat.drain_pending(&mut link, &runtime).await;

    loop {
        let now = Local::now();
        let agenda = match Butler::load() {
            Ok(butler) => agenda::next(now, &butler),
            Err(error) => {
                eprintln!("mns-butlerd: 管家状态读取失败：{error:#}");
                None
            }
        };
        let (wait, due) = match agenda {
            Some(agenda) => (until(now, agenda.at), Some(agenda)),
            // 状态都读不出来时别空转：等下一次状态变更事件，或半小时后再试。
            None => (MAX_SLEEP, None),
        };

        tokio::select! {
            biased;
            // 状态变更优先：GUI 记完账、CLI 加完任务，都不该等下一次排程。
            event = events.recv() => {
                // 事件源全没了：accept 线程与所有读连接线程都已退出，本地
                // 通道从此无人能连上。继续待着只会变成「活着但聋」的进程
                // ——GUI 下次启动探活失败会再拉一个新的，进程就攒起来。主动
                // 退出：连接断开后 GUI 会收回巡检与通知。
                let Some(event) = event else {
                    link.send(&Signal::Bye);
                    finish(&mut chat, &runtime, agent_events).await;
                    return Ok(());
                };
                if handle_event(event, &mut chat, &mut link, &runtime, &init_busy, &init_tx).await {
                    finish(&mut chat, &runtime, agent_events).await;
                    return Ok(());
                }
            }
            // 会话的超时回调：定时器到点回一个回合序号，对不上就是空转。
            Some(turn) = timeout_rx.recv() => {
                let effects = chat.service.on_timeout(turn);
                chat.apply(effects, &runtime);
            }
            // 会话的 ACP 事件：正文增量、回合结束、失败、授权请求。
            event = agent_events.recv() => {
                let effects = match event {
                    Some(event) => chat.service.on_agent_event(event),
                    // runtime 线程意外退出（正常退出只会发生在 finish 之后）：
                    // 把在飞的一轮按失败收尾，别让等待挂到天荒地老。
                    None => chat.service.on_agent_event(AgentEvent::Failed {
                        message: "agent runtime 已退出".to_owned(),
                    }),
                };
                chat.apply(effects, &runtime);
            }
            // agent-init 已经写好 pending：并进会话并投递。
            Some(()) = init_rx.recv() => {
                chat.drain_pending(&mut link, &runtime).await;
            }
            _ = shutdown_requested() => {
                // 主动告别：GUI 立刻知道判定权回到自己手里，不用等连接 EOF。
                link.send(&Signal::Bye);
                finish(&mut chat, &runtime, agent_events).await;
                return Ok(());
            }
            // 投递积压的重试节拍：没有这条分支，宿主一直不在线时 backlog
            // 要等到下一次事件才有机会出去。sleep 在每次循环重建，事件频繁
            // 时会被重置——没关系，那些路径各自都会顺手排一次积压。
            _ = tokio::time::sleep(Duration::from_secs(30)), if !link.backlog.is_empty() => {
                chat.drain_pending(&mut link, &runtime).await;
            }
            _ = tokio::time::sleep(wait) => {
                // 睡眠被 MAX_SLEEP 截断时还没到点：只重算，不判定。否则一条
                // 三小时后的任务会在第一次截断时就被当成「到点」说出去。
                if let Some(due) = due
                    && Local::now() >= due.at
                {
                    wake(&mut chat, &mut link, &runtime, due.source, &init_busy, &init_tx).await;
                }
            }
        }
    }
}

/// 到目标时刻还差多久。已经过点了就是 0——立刻醒，别等下一个整点。
fn until(now: DateTime<Local>, at: DateTime<Local>) -> Duration {
    let wait = at.signed_duration_since(now);
    if wait <= chrono::Duration::zero() {
        return Duration::ZERO;
    }
    // chrono 的毫秒数转 Duration：不经过浮点，避免极端值溢出。
    Duration::from_millis(wait.num_milliseconds().min(MAX_SLEEP.as_millis() as i64) as u64)
}

/// 处理一条事件。返回 `true` 表示该退出（仅 `Shutdown`）。
async fn handle_event(
    event: ipc::Event,
    chat: &mut ChatState,
    link: &mut GuiLink,
    runtime: &ChatRuntime,
    init_busy: &Arc<AtomicBool>,
    init_tx: &UnboundedSender<()>,
) -> bool {
    match event {
        ipc::Event::Signal(_, Signal::Wake { ledger }) => {
            let source = match ledger {
                Some(id) => WakeSource::Ledger { id },
                None => WakeSource::Scheduled,
            };
            wake(chat, link, runtime, source, init_busy, init_tx).await;
        }
        ipc::Event::Signal(_, Signal::RequestBrief) => {
            // 宿主不在本进程里跑，托盘数字只能问这里要。读快照是阻塞调用，
            // 与唤醒同一处理：放阻塞线程池，别占住 select 循环。
            let brief = match tokio::task::spawn_blocking(|| {
                match money_never_sleep::app::load_snapshot_read_only() {
                    Ok(snapshot) => {
                        orchestrator::record_health(true, None);
                        money_never_sleep::brief::BriefData::from_snapshot(&snapshot)
                    }
                    Err(error) => {
                        let message = format!("{error:#}");
                        orchestrator::record_health(false, Some(message.clone()));
                        money_never_sleep::brief::BriefData::from_error(message)
                    }
                }
            })
            .await
            {
                Ok(brief) => brief,
                Err(error) => money_never_sleep::brief::BriefData::from_error(format!(
                    "简报计算任务失败：{error}"
                )),
            };
            if let Ok(json) = serde_json::to_string(&brief) {
                link.send(&Signal::Brief { json });
            }
            // 宿主刚跟我们说过话：顺手排一次投递积压。
            chat.drain_pending(link, runtime).await;
        }
        ipc::Event::Signal(_, Signal::SetProactive { enabled }) => {
            if let Err(error) = set_proactive(enabled) {
                eprintln!("mns-butlerd: 保存主动开口开关失败：{error:#}");
            }
        }
        // 窗口挂载：记下连接，回一帧全量快照。挂载不代表回合清零——窗口
        // 断开期间回答照常累积在会话里，快照一帧带全。
        ipc::Event::Signal(conn, Signal::ChatAttach) => {
            chat.conn = Some(conn);
            let effects = chat.service.attach();
            chat.apply(vec![effects], runtime);
            chat.drain_pending(link, runtime).await;
        }
        // 用户发话：账户事实是整段阻塞调用（只读连接全库读取），放阻塞
        // 线程池算完再进会话服务。
        ipc::Event::Signal(_, Signal::ChatSend { text }) => {
            let facts = match tokio::task::spawn_blocking(move || {
                chat::current_facts(Local::now())
            })
            .await
            {
                Ok(facts) => facts,
                Err(error) => {
                    eprintln!("mns-butlerd: 会话事实计算任务失败：{error}");
                    AccountFacts::default()
                }
            };
            let effects = chat.service.send(text, &facts);
            chat.apply(effects, runtime);
        }
        ipc::Event::Signal(_, Signal::ChatClear) => {
            let effects = chat.service.clear();
            chat.apply(effects, runtime);
        }
        ipc::Event::Signal(_, Signal::ChatSeen) => {
            let effects = chat.service.seen();
            chat.apply(effects, runtime);
        }
        // GUI 重连时也会发 Hello：重新对上连接，别让 GUI 以为没人在。
        ipc::Event::Signal(_, Signal::Hello { .. }) => link.ensure(),
        // 托盘「停止管家进程」。GUI 之后会收到断开并收回巡检与通知。
        ipc::Event::Signal(_, Signal::Shutdown) => return true,
        ipc::Event::Signal(_, Signal::Bye) => {}
        // 窗口断开就是 detach：没有告别帧。等待中的一轮继续跑，广播
        // 从此丢弃，落盘不受影响。
        ipc::Event::Disconnected(id) => {
            if chat.conn == Some(id) {
                chat.conn = None;
            }
        }
        ipc::Event::Signal(_, _) | ipc::Event::Connected(_) => {}
    }
    false
}

async fn wake(
    chat: &mut ChatState,
    link: &mut GuiLink,
    runtime: &ChatRuntime,
    source: WakeSource,
    init_busy: &Arc<AtomicBool>,
    init_tx: &UnboundedSender<()>,
) {
    // GUI 可能在我们上次发送之后重启过：先确认连接，既是重连也是 Hello。
    link.ensure();
    let now = Local::now();

    // 账本快照是整段阻塞调用（只读连接全库读取 + 指标计算）。本进程跑在
    // current_thread runtime 上：直接调用会占住 select 循环，巡检期间到达
    // 的停止与账本唤醒都得排队。放阻塞线程池读，读完回主线判定。
    let (ledger, ledger_error) =
        match tokio::task::spawn_blocking(move || orchestrator::ledger_facts_report(now)).await {
            Ok((facts, error)) => (facts, error),
            Err(error) => {
                eprintln!("mns-butlerd: 账户事实计算任务失败：{error}");
                (AccountFacts::default(), Some(format!("账户事实计算任务失败：{error}")))
            }
        };
    // 成败都落一次：失败让托盘亮红灯，成功把旧错误清掉。
    orchestrator::record_health(ledger_error.is_none(), ledger_error);

    // 读-改-写全程持锁：GUI 巡检与 CLI 可能同时各跑一份判定，后保存的
    // 会把先保存的去重键覆盖回旧值。
    let _guard = writer_lock();
    let mut butler = match Butler::load() {
        Ok(butler) => butler,
        Err(error) => {
            eprintln!("mns-butlerd: 管家状态读取失败：{error:#}");
            return;
        }
    };
    // 承诺来自记忆文件（不是账本），在锁内与判定合并：同一份 butler 状态，
    // 「明天到期的承诺」不需要 GUI 那天恰好打开过。
    let facts = ledger.with_promises(&butler.memory, now.date_naive());

    let report = butler.wake(&source, now, &facts);
    if report.wake != WakeVerdict::Wake {
        return;
    }
    if let Err(error) = butler.save() {
        eprintln!("mns-butlerd: 管家状态保存失败：{error:#}");
    }

    match report.speak {
        Some(SpeakDecision::Speak(candidate)) => {
            // 审计队列与投递队列各一行：前者只追加，后者由本进程按偏移消费。
            if let Err(error) = orchestrator::enqueue_notice(now, &candidate) {
                eprintln!("mns-butlerd: 出站队列写入失败：{error:#}");
            }
            if let Err(error) = orchestrator::enqueue_pending(now, &candidate) {
                eprintln!("mns-butlerd: 待投递队列写入失败：{error:#}");
            }

            // 并进会话（本进程是唯一写者）再投给宿主；宿主不在就进积压。
            chat.drain_pending(link, runtime).await;
        }
        // 确定性内核没有由头时，才让 agent 自己找话头。只认定时巡检与
        // 失败重试：「刚记完一笔账」是账本事件，不该变成一次闲聊。
        Some(SpeakDecision::Silent(silent))
            if silent == SilentBecause::NothingToSay
                && matches!(
                    source,
                    WakeSource::Scheduled | WakeSource::AgentInitRetry { .. }
                )
                && schedule::gate(&butler.state.policy, &butler.state.ledger, now).is_ok() =>
        {
            spawn_agent_init(init_busy, init_tx);
        }
        _ => {}
    }
}

/// 起一次 agent-init。`busy` 保证同一时刻最多一个：主动开口没有排队语义，
/// 上一轮还没答完就再起一轮，只会让两句话互相打架。
fn spawn_agent_init(busy: &Arc<AtomicBool>, done: &UnboundedSender<()>) {
    if busy.swap(true, Ordering::SeqCst) {
        return;
    }
    let busy = busy.clone();
    let done = done.clone();
    tokio::spawn(async move {
        let spoke = match run_agent_init().await {
            Ok(spoke) => spoke,
            Err(error) => {
                eprintln!("mns-butlerd: agent 主动开口失败：{error:#}");
                false
            }
        };
        busy.store(false, Ordering::SeqCst);
        if spoke {
            let _ = done.send(());
        }
    });
}

/// 一轮完整的主动开口。
///
/// 成本与风险都写在这里：起一个独立 ACP runtime，120 秒内拿不到回答就放弃；
/// 拿到回答后必须过 `validate_talk` 与通知口径（≤60 字、无买卖话术），
/// 任何一步不过都沉默——主动开口没有模板兜底，不说比说错好。
///
/// 失败不再沉默 12 小时：结果（含失败原因）写进 `initlog.jsonl`，
/// 下一次尝试按 `agent_init_backoff` 退避。模型说「无」是合法回答，
/// 不算失败、不缩短节奏。
///
/// 锁只包「读-改-写」，绝不跨 await：等 agent 回答的 120 秒里不挡
/// 别的写者。
async fn run_agent_init() -> Result<bool> {
    let now = Local::now();
    let (butler, session) = {
        let _guard = writer_lock();
        let mut butler = Butler::load()?;
        if schedule::gate(&butler.state.policy, &butler.state.ledger, now).is_err() {
            return Ok(false);
        }
        // 尝试本身也有节奏：模型可能每次都说「无」，不能每轮巡检都拉起一次
        // 外部会话。成功开口另由 ledger 频控约束。
        let session = Session::load().unwrap_or_default();
        if !agent_init_due(&butler, &session, now) {
            return Ok(false);
        }
        butler.state.last_agent_init = Some(now);
        butler.save_state()?;
        (butler, session)
    };

    // 账户事实与判定同源：只读连接算出来（阻塞调用放阻塞线程池，理由同
    // `wake`），承诺在锁外合并——`butler` 是刚从锁内读出的那份。
    let ledger = match tokio::task::spawn_blocking(move || orchestrator::ledger_facts_report(now)).await
    {
        Ok((facts, error)) => {
            orchestrator::record_health(error.is_none(), error);
            facts
        }
        Err(error) => {
            eprintln!("mns-butlerd: 账户事实计算任务失败：{error}");
            orchestrator::record_health(false, Some(format!("账户事实计算任务失败：{error}")));
            AccountFacts::default()
        }
    };
    let facts = ledger.with_promises(&butler.memory, now.date_naive());
    let memory = talk::memory_lines(&butler.memory, now.date_naive());
    let recent = talk::recent_lines(&session);
    let settings = chat::chat_settings();
    let goal = init_goal(&settings, &facts, &memory, &recent);

    let (handle, mut events) = money_never_sleep::agent::start_runtime();
    if handle
        .send(AgentCommand::Start {
            goal,
            settings,
            source: GoalSource::Conversation,
        })
        .is_err()
    {
        let _ = handle.send(AgentCommand::Shutdown);
        record_init(AgentInitOutcome::Failed, Some("agent 会话没能启动"));
        return Ok(false);
    }
    let raw = collect_reply(&handle, &mut events).await;
    // 每轮都收掉 runtime：butlerd 是长寿进程，留着一个空闲的 ACP 线程
    // 只会在用户看不见的地方攒出孤儿进程。
    let _ = handle.send(AgentCommand::Shutdown);

    let Some(raw) = raw else {
        record_init(
            AgentInitOutcome::Failed,
            Some(&format!(
                "agent {} 秒内没答完或会话中断",
                AGENT_INIT_TIMEOUT.as_secs()
            )),
        );
        return Ok(false);
    };
    let cleaned = match talk::validate_talk(&raw) {
        Ok(cleaned) => cleaned,
        Err(error) => {
            record_init(
                AgentInitOutcome::Failed,
                Some(&format!("回答没过校验：{error}")),
            );
            return Ok(false);
        }
    };
    // 这一条会进系统通知横幅：banner 是纯文本通道，星号会原样显示。会话
    // 气泡也复用同一条文本，单行提醒不需要 markdown。
    let text = talk::notification_text(&cleaned);
    if text.is_empty() || text == "无" || text == "无。" {
        record_init(AgentInitOutcome::Silent, Some("模型说无"));
        return Ok(false);
    }
    // banner 是被动打扰通道：长度与话术口径与会话不同，这里不降级、只沉默。
    if let Err(violation) = schedule::validate_notification(&text) {
        record_init(
            AgentInitOutcome::Failed,
            Some(&format!("回答没过通知口径：{violation}")),
        );
        return Ok(false);
    }

    // 等待回答期间别的路径可能已经开过口：重新加载后再查一次额度，并把
    // 这一条记进同一本 ledger——频控与审计不分 agent 还是确定性内核。
    {
        let _guard = writer_lock();
        let mut butler = Butler::load()?;
        let now = Local::now();
        if schedule::gate(&butler.state.policy, &butler.state.ledger, now).is_err() {
            record_init(AgentInitOutcome::Silent, Some("回答到达时额度已满"));
            return Ok(false);
        }
        let candidate = SpeakCandidate {
            reason: SpeakReason::AgentInit,
            trigger: agent_init_trigger(&butler.state.ledger, now),
            draft: text,
        };
        butler.state.ledger.record(now, &candidate);
        butler.state.ledger.trim(now, 30);
        butler.state.agent_init_failures = 0;
        butler.save_state()?;
        orchestrator::enqueue_notice(now, &candidate)?;
        orchestrator::enqueue_pending(now, &candidate)?;
    }
    record_init(AgentInitOutcome::Spoke, None);
    Ok(true)
}

/// 落一笔 agent-init 尝试记录，并按结果更新退避计数。
///
/// 决策日志先行（append-only，`mns-butler status` 直接读）；状态更新
/// 拿写锁、失败不重试——计数丢了最多是退避不准，不能反过来阻断主流程。
/// Spoke 路径在 `run_agent_init` 里已把计数清零并保存，这里只补日志。
fn record_init(outcome: AgentInitOutcome, detail: Option<&str>) {
    let record = AgentInitRecord {
        at: Local::now(),
        outcome,
        detail: detail.map(str::to_owned),
    };
    if let Err(error) = append_init_record(&record) {
        eprintln!("mns-butlerd: agent-init 决策日志写入失败：{error:#}");
    }
    if outcome != AgentInitOutcome::Failed {
        return; // 开口与正常沉默：不动退避计数。
    }
    let _guard = writer_lock();
    let Ok(mut butler) = Butler::load() else {
        return;
    };
    butler.state.agent_init_failures = butler.state.agent_init_failures.saturating_add(1);
    if let Err(error) = butler.save_state() {
        eprintln!("mns-butlerd: agent-init 退避计数保存失败：{error:#}");
    }
}

/// 等到这一轮正文结束。超时、进程报错、会话提前关闭都返回 `None`。
async fn collect_reply(
    handle: &AgentHandle,
    events: &mut tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
) -> Option<String> {
    let deadline = tokio::time::Instant::now() + AGENT_INIT_TIMEOUT;
    let mut buffer = String::new();
    loop {
        let event = match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(event)) => event,
            Ok(None) | Err(_) => return None,
        };
        match event {
            AgentEvent::Delta {
                kind: EntryKind::Message,
                text,
            } => buffer.push_str(&text),
            AgentEvent::Permission(prompt) => {
                // 守护进程没有授权界面，也无人在旁边点确认：拒绝，别卡住。
                let _ = handle.send(AgentCommand::Deny { id: prompt.id });
            }
            AgentEvent::TurnFinished { .. } => return Some(buffer),
            AgentEvent::Finished { .. } => {
                return if buffer.is_empty() { None } else { Some(buffer) };
            }
            AgentEvent::Failed { message } => {
                eprintln!("mns-butlerd: agent-init 报错：{message}");
                return None;
            }
            _ => {}
        }
    }
}

/// 最近 12 小时有没有任何往来（会话消息或开口记录）。
///
/// 用「任何往来」而不是只看用户发言：用户刚聊完、或管家刚提醒过，都不该再
/// 自己起话头。开口记录也算，因为 GUI 没开时会话文件不会更新。
fn recent_activity(
    butler: &Butler,
    session: &Session,
    now: DateTime<Local>,
) -> bool {
    let cutoff = now - chrono::Duration::hours(AGENT_INIT_MIN_IDLE_HOURS);
    session.messages.iter().any(|message| message.at >= cutoff)
        || butler
            .state
            .ledger
            .records
            .iter()
            .any(|record| record.at >= cutoff)
}

/// 此刻该不该尝试 agent-init：距上次尝试 ≥ 退避时长（无失败 = 常规 12
/// 小时；失败 1/2/≥3 次 = 15 分钟 / 1 小时 / 6 小时），且 12 小时内没有
/// 别的往来。两个条件分开看：管家不该在用户刚说完话时自己起话头，
/// 也不该在模型连续失败时反复花一次会话成本，更不该一次失败就沉默半天。
fn agent_init_due(butler: &Butler, session: &Session, now: DateTime<Local>) -> bool {
    let cooldown = agent_init_backoff(butler.state.agent_init_failures);
    let attempted_recently = butler
        .state
        .last_agent_init
        .is_some_and(|last| now - last < cooldown);
    !attempted_recently && !recent_activity(butler, session, now)
}

/// agent-init 的触发键：`agent-init:<日期>:<当天序号>`。
///
/// 序号让同一天多条记录彼此不同，去重窗口不会误伤；额度由 ledger 的
/// 每日/每周上限约束。
fn agent_init_trigger(ledger: &schedule::SpeakLedger, now: DateTime<Local>) -> String {
    let prefix = format!("agent-init:{}:", now.format("%Y-%m-%d"));
    let seq = ledger
        .records
        .iter()
        .filter(|record| record.trigger.starts_with(&prefix))
        .count()
        + 1;
    format!("{prefix}{seq}")
}

/// 主动开口的首轮 goal。
///
/// `settings.agent_cli` 是运行这次会话的 ACP 程序（opencode），不是数据入口。
/// 曾经把它直接传给 `agent_init_goal`，导致 prompt 里的命令变成
/// `<opencode 路径> portfolio`——数据入口整段失效。管家的命令入口来自
/// `mns` / `mns-butler` 的安装路径（写在 AGENTS.md 与技能里），不是这里。
/// `settings` 保留在签名里，是为了让回归测试证明它被忽略。
fn init_goal(
    settings: &money_never_sleep::agent::AgentSettings,
    facts: &AccountFacts,
    memory: &[String],
    recent: &[String],
) -> String {
    let _ = settings;
    talk::agent_init_goal(facts, memory, recent)
}

fn set_proactive(enabled: bool) -> Result<()> {
    let _guard = writer_lock();
    let mut butler = Butler::load()?;
    if butler.state.policy.enabled == enabled {
        return Ok(());
    }
    butler.state.policy.enabled = enabled;
    // 只写状态文件：这里只动了开关，顺手重写记忆与任务会把另一个进程
    // 刚写进去的条目覆盖回旧值。
    butler.save_state()
}

/// Ctrl-C / SIGTERM。托盘「停止管家进程」走的是 socket 上的 `Shutdown`，
/// 这两条是终端与系统关机时的兜底。
async fn shutdown_requested() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(signal) => signal,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// 会话 ACP runtime 的把手：命令通道 + 超时回调。
///
/// 拆成结构体只是让 `ChatState::apply` 的签名放得下；两个把手同生同灭——
/// daemon 启动时起线程，退出时 `finish` 发 `Shutdown`。
struct ChatRuntime {
    agent: AgentHandle,
    timeouts: UnboundedSender<u64>,
}

/// P4 的管家会话通道：会话服务 + 窗口挂载点。
///
/// 窗口来去只改 `conn`；服务与 runtime 不动——detach 期间回合照常推进、
/// 照常落盘，窗口重连拿全量快照就能追上。
struct ChatState {
    service: chat::ChatService,
    /// 已挂载窗口的连接 id；`None` = 没人看着，广播丢弃，会话照走。
    conn: Option<u64>,
    /// 会话通道的监听端：`send_to` 定向推给挂载的窗口。
    listener: Listener,
}

impl ChatState {
    fn new(listener: Listener) -> Self {
        Self {
            service: chat::ChatService::new(),
            conn: None,
            listener,
        }
    }

    /// 落地一串会话效果：推给挂着的窗口、起超时定时器、命令 runtime。
    /// 没有窗口时 `Broadcast` 丢弃——订阅者数量不参与回合推进。
    fn apply(&mut self, effects: Vec<ChatEffect>, runtime: &ChatRuntime) {
        for effect in effects {
            match effect {
                ChatEffect::Broadcast(signal) => {
                    if let Some(conn) = self.conn {
                        // 出站队列满（慢读者）即拆线：窗口自己重连拿快照，
                        // 不能让一个卡住的窗口拖住会话服务。
                        self.listener.send_to(conn, &signal);
                    }
                }
                ChatEffect::Timeout { turn } => {
                    let timeouts = runtime.timeouts.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(chat::CHAT_TIMEOUT).await;
                        let _ = timeouts.send(turn);
                    });
                }
                ChatEffect::Runtime(command) => {
                    if runtime.agent.send(command).is_err() {
                        // runtime 线程没了：按失败收尾这一轮，别把「正在想」
                        // 挂到天荒地老。收尾只会再产生广播，不会递归。
                        let effects = self.service.on_agent_event(AgentEvent::Failed {
                            message: "agent runtime 已退出".to_owned(),
                        });
                        self.apply(effects, runtime);
                    }
                }
            }
        }
    }

    /// 排空待投递队列：并进会话（按 `notice_id` 幂等），逐条投给宿主，
    /// **全部成功才推进偏移**；投不出去的留在 backlog 等下一轮。
    ///
    /// 顺序不能反：先并进会话再投递的话，宿主崩溃重启也不会丢消息——
    /// 偏移没动，重启后重读重投，会话层有幂等，宿主侧有 delivered 去重。
    async fn drain_pending(&mut self, link: &mut GuiLink, runtime: &ChatRuntime) {
        if link.backlog.is_empty() {
            let offset = orchestrator::read_pending_offset();
            let Ok((new_offset, notices)) = orchestrator::take_pending(offset) else {
                return;
            };
            if notices.is_empty() {
                return;
            }
            link.backlog = notices;
            link.backlog_offset = Some(new_offset);
        }
        let notices = std::mem::take(&mut link.backlog);
        let effects = self.service.merge_notices(&notices, self.conn.is_some());
        self.apply(effects, runtime);

        // 逐条投给宿主：宿主按 notices.jsonl 的 delivered 记录去重，所以
        // 重投是安全的；这里按发送成败决定要不要留着重试。
        let mut undelivered = Vec::new();
        for notice in notices {
            // 序列化失败（实际上不可能：全是字符串与时间戳）只能跳过，
            // 不能让一条坏数据堵住后面的。
            let Ok(json) = serde_json::to_string(&notice) else {
                continue;
            };
            if link.send(&Signal::DeliverNotice { json }) {
                continue;
            }
            undelivered.push(notice);
        }
        link.backlog = undelivered;
        if link.backlog.is_empty() {
            if let Some(offset) = link.backlog_offset.take() {
                orchestrator::write_pending_offset(offset);
            }
        }
    }
}

/// 退出收尾：放弃在飞的一轮（不落半截回答），请会话 runtime 自行结束，
/// 限时等它收摊。
///
/// `Shutdown` → runner 命令循环 break → 事件通道关闭（`recv` 得 `None`）→
/// ACP 连接 Drop → 子进程组被 SIGKILL。限时 3 秒：收不掉也不能陪葬，
/// 宿主会在连接断开后收回巡检与通知。
async fn finish(
    chat: &mut ChatState,
    runtime: &ChatRuntime,
    mut agent_events: UnboundedReceiver<AgentEvent>,
) {
    let effects = chat.service.shutdown();
    for effect in effects {
        if let ChatEffect::Runtime(command) = effect {
            let _ = runtime.agent.send(command);
        }
        // Broadcast：都要退了，没有要照顾的订阅者。
    }
    let _ = tokio::time::timeout(Duration::from_secs(3), agent_events.recv()).await;
}

/// 到 GUI 的长连接。
///
/// 「连接活着」本身就是心跳：GUI 关掉时写失败，下一次 `ensure` 重连；守护
/// 进程崩掉时 GUI 侧收到 `Disconnected`。不需要定时 ping，也不需要心跳文件。
#[derive(Default)]
struct GuiLink {
    client: Option<Client>,
    /// 没投出去的 pending 条目：宿主不在线时攒着，30 秒一轮或宿主/窗口
    /// 下次说话时重试。空 = 没有积压。
    backlog: Vec<Notice>,
    /// backlog 对应的 `pending.jsonl` 目标偏移：全部投出去才推进。
    backlog_offset: Option<u64>,
}

impl GuiLink {
    /// 还没有连接时连一次并自我介绍。已有连接则不动——空闲探测留到真正
    /// 需要发送的时候。
    fn ensure(&mut self) {
        if self.client.is_some() {
            return;
        }
        if let Ok(mut client) = ipc::connect("mns-gui")
            && client
                .send(&Signal::Hello {
                    role: "daemon".to_owned(),
                })
                .is_ok()
        {
            self.client = Some(client);
        }
    }

    /// 发一条信号。连接断了就重连一次再发；仍然失败返回 false。
    fn send(&mut self, signal: &Signal) -> bool {
        if let Some(client) = self.client.as_mut()
            && client.send(signal).is_ok()
        {
            return true;
        }
        self.client = None;
        let Ok(mut client) = ipc::connect("mns-gui") else {
            return false;
        };
        let hello = client.send(&Signal::Hello {
            role: "daemon".to_owned(),
        });
        if hello.is_err() || client.send(signal).is_err() {
            return false;
        }
        self.client = Some(client);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use money_never_sleep::butler::Memory;

    fn at(day: u32, hour: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 11, day, hour, 0, 0)
            .single()
            .unwrap()
    }

    fn butler_with(records: Vec<(DateTime<Local>, &str)>) -> Butler {
        let mut butler = Butler {
            state: money_never_sleep::butler::ButlerState::default(),
            memory: Memory::default(),
            tasks: money_never_sleep::butler::TaskBoard::default(),
        };
        for (when, trigger) in records {
            butler
                .state
                .ledger
                .record(when, &SpeakCandidate {
                    reason: SpeakReason::AgentInit,
                    trigger: trigger.to_owned(),
                    draft: "x".to_owned(),
                });
        }
        butler
    }

    #[test]
    fn 最近十二小时有往来就不主动开口() {
        let now = at(16, 10);
        let empty = Butler {
            state: money_never_sleep::butler::ButlerState::default(),
            memory: Memory::default(),
            tasks: money_never_sleep::butler::TaskBoard::default(),
        };
        assert!(!recent_activity(&empty, &Session::default(), now));

        // 管家刚说过话：12 小时内不再自己起话头，哪怕会话文件还没更新。
        let spoke = butler_with(vec![(at(16, 9), "agent-init:2026-11-16:1")]);
        assert!(recent_activity(&spoke, &Session::default(), now));

        // 用户 13 小时前聊过：可以开口。
        let mut session = Session::default();
        session.messages.push(money_never_sleep::butler::Message::user(
            at(15, 21),
            "晚安",
        ));
        assert!(!recent_activity(&empty, &session, now));
    }

    #[test]
    fn 尝试主动开口也要隔十二小时() {
        let now = at(16, 10);
        let mut butler = butler_with(vec![]);
        assert!(agent_init_due(&butler, &Session::default(), now));
        // 上一轮试过、模型说「无」：成功率不影响节奏，12 小时内不再试。
        butler.state.last_agent_init = Some(at(16, 9));
        assert!(!agent_init_due(&butler, &Session::default(), now));
        // 13 小时后可以再试。
        butler.state.last_agent_init = Some(at(15, 20));
        assert!(agent_init_due(&butler, &Session::default(), now));
    }

    #[test]
    fn 失败后退避而不是沉默十二小时() {
        let now = at(16, 10);
        let mut butler = butler_with(vec![]);
        // 1 小时前失败 1 次：15 分钟退避已过，应当再试。
        butler.state.last_agent_init = Some(now - chrono::Duration::hours(1));
        butler.state.agent_init_failures = 1;
        assert!(agent_init_due(&butler, &Session::default(), now));
        // 10 分钟前刚失败：还在 15 分钟退避里，不试。
        butler.state.last_agent_init = Some(now - chrono::Duration::minutes(10));
        butler.state.agent_init_failures = 1;
        assert!(!agent_init_due(&butler, &Session::default(), now));
        // 失败 2 次、20 分钟前：1 小时退避未到。
        butler.state.last_agent_init = Some(now - chrono::Duration::minutes(20));
        butler.state.agent_init_failures = 2;
        assert!(!agent_init_due(&butler, &Session::default(), now));
        // 失败 2 次、70 分钟前：可以再试。
        butler.state.last_agent_init = Some(now - chrono::Duration::minutes(70));
        butler.state.agent_init_failures = 2;
        assert!(agent_init_due(&butler, &Session::default(), now));
        // 退避再短，用户 12 小时内聊过天就不主动开口。
        butler.state.last_agent_init = Some(now - chrono::Duration::hours(3));
        butler.state.agent_init_failures = 3;
        let mut session = Session::default();
        session.messages.push(money_never_sleep::butler::Message::user(
            now - chrono::Duration::minutes(1),
            "刚聊完",
        ));
        assert!(!agent_init_due(&butler, &session, now));
    }

    #[test]
    fn 主动开口不泄露_acp_agent_路径但保留真实工具入口() {
        let settings = money_never_sleep::agent::AgentSettings {
            agent_cli: "/opt/agent/opencode".to_owned(),
            ..Default::default()
        };
        let goal = init_goal(&settings, &AccountFacts::default(), &[], &[]);
        assert!(
            !goal.contains("/opt/agent/opencode"),
            "agent_cli 是 ACP 程序，不能当成数据入口写进 prompt"
        );
        assert!(
            goal.contains("mns-butler task add"),
            "提醒必须走真实的到点任务"
        );
        assert!(!goal.contains("不要执行命令"), "能力优先，不再全禁工具");
    }

    #[test]
    fn 触发键带上当天序号() {
        let now = at(16, 10);
        let empty = butler_with(vec![]);
        assert_eq!(
            agent_init_trigger(&empty.state.ledger, now),
            "agent-init:2026-11-16:1"
        );
        let spoken = butler_with(vec![(at(16, 9), "agent-init:2026-11-16:1")]);
        assert_eq!(
            agent_init_trigger(&spoken.state.ledger, now),
            "agent-init:2026-11-16:2"
        );
    }
}
