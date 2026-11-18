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
//! 3. 通过本地唤醒通道告诉 GUI「有新话」，让 GUI 把消息并进会话存档；
//! 4. 确定性内核没有由头时，按需起一次外部 ACP agent，问它有没有一句值得
//!    主动说的话（`agent-init`）。回答必须过通知口径（≤60 字、无买卖话术），
//!    否则本轮沉默；runtime 每轮 `Shutdown`，不留下空闲进程。
//!
//! **它不发系统通知**。macOS 只认 app 的主可执行文件：同 bundle 的守护进程
//! 调 `requestAuthorization` 会拿到 `UNErrorDomain 1`（notificationsNotAllowed）。
//! 判定可以拆进程，送达不行——GUI 收到 `Pending` 后自己发。
//!
//! 它**不打开数据库**：账户事实由 GUI / CLI 写进 `facts.json`
//! （`AccountFacts`），这里只读那一份。这是结构保证——本文件的 `use` 里没有
//! `crate::app`、没有 `rusqlite`，想偷偷读账本得先改这段代码。
//!
//! 会话存档（`conversation.json`）始终只有 GUI 一个写者；守护进程崩溃也不会
//! 留下半截对话。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Local};
use money_never_sleep::agent::{AgentCommand, AgentEvent, AgentHandle, EntryKind, GoalSource};
use money_never_sleep::butler::agenda;
use money_never_sleep::butler::orchestrator::{
    self, AccountFacts, AgentInitOutcome, AgentInitRecord, AGENT_INIT_MIN_IDLE_HOURS,
    agent_init_backoff, append_init_record,
};
use money_never_sleep::butler::schedule;
use money_never_sleep::butler::talk;
use money_never_sleep::butler::{
    butler_dir, writer_lock, Butler, Memory, Role, Session, SilentBecause, SpeakCandidate,
    SpeakDecision, SpeakReason, WakeSource, WakeVerdict,
};
use money_never_sleep::ipc::{self, Client, Signal};
use tokio::sync::mpsc::UnboundedSender;

/// 事实超过这个年龄就整份不信：仓位可能早就变了，拿旧数字开口等于编造。
const FACTS_MAX_AGE_HOURS: i64 = 24 * 7;

/// 超过这个年龄只保留「与价格无关」的由头（到点任务、承诺到期）。
/// 一天的旧价格不足以支撑「偏离多少」，但足以支撑「今天该检查什么」。
const FACTS_PRICE_AGE_HOURS: i64 = 24;

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
    let (_listener, mut events) = match ipc::listen("mns-butlerd") {
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
    // 任务完成后只回一条「去读 pending」，连接仍归主线程独有。
    let (init_tx, mut init_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let init_busy = Arc::new(AtomicBool::new(false));

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
                    return Ok(());
                };
                if handle_event(event, &mut link, &init_busy, &init_tx) {
                    return Ok(());
                }
            }
            // agent-init 已经写好 pending：主线程负责通知 GUI。
            Some(()) = init_rx.recv() => {
                link.send(&Signal::Pending);
            }
            _ = shutdown_requested() => {
                // 主动告别：GUI 立刻知道判定权回到自己手里，不用等连接 EOF。
                link.send(&Signal::Bye);
                return Ok(());
            }
            _ = tokio::time::sleep(wait) => {
                // 睡眠被 MAX_SLEEP 截断时还没到点：只重算，不判定。否则一条
                // 三小时后的任务会在第一次截断时就被当成「到点」说出去。
                if let Some(due) = due
                    && Local::now() >= due.at
                {
                    wake(&mut link, due.source, &init_busy, &init_tx);
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
fn handle_event(
    event: ipc::Event,
    link: &mut GuiLink,
    init_busy: &Arc<AtomicBool>,
    init_tx: &UnboundedSender<()>,
) -> bool {
    match event {
        ipc::Event::Signal(_, Signal::Wake { ledger }) => {
            let source = match ledger {
                Some(id) => WakeSource::Ledger { id },
                None => WakeSource::Scheduled,
            };
            wake(link, source, init_busy, init_tx);
        }
        ipc::Event::Signal(_, Signal::SetProactive { enabled }) => {
            if let Err(error) = set_proactive(enabled) {
                eprintln!("mns-butlerd: 保存主动开口开关失败：{error:#}");
            }
        }
        // GUI 重连时也会发 Hello：重新对上连接，别让 GUI 以为没人在。
        ipc::Event::Signal(_, Signal::Hello { .. }) => link.ensure(),
        // 托盘「停止管家进程」。GUI 之后会收到断开并收回巡检与通知。
        ipc::Event::Signal(_, Signal::Shutdown) => return true,
        ipc::Event::Signal(_, Signal::Bye) => {}
        ipc::Event::Signal(_, _) | ipc::Event::Connected(_) | ipc::Event::Disconnected(_) => {}
    }
    false
}

fn wake(
    link: &mut GuiLink,
    source: WakeSource,
    init_busy: &Arc<AtomicBool>,
    init_tx: &UnboundedSender<()>,
) {
    // GUI 可能在我们上次发送之后重启过：先确认连接，既是重连也是 Hello。
    link.ensure();
    let now = Local::now();

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
    // 承诺来自记忆文件（不是账本），所以由守护进程自己算「今天到期的承诺」：
    // GUI 可能整天没打开，它的快照里不会有别人这一天该提的事。
    let facts = account_facts(&butler, now);

    let report = butler.wake(&source, now, &facts);
    if report.wake != WakeVerdict::Wake {
        return;
    }
    if let Err(error) = butler.save() {
        eprintln!("mns-butlerd: 管家状态保存失败：{error:#}");
    }

    match report.speak {
        Some(SpeakDecision::Speak(candidate)) => {
            // 审计队列与投递队列各一行：前者只追加，后者由 GUI 按偏移消费。
            if let Err(error) = orchestrator::enqueue_notice(now, &candidate) {
                eprintln!("mns-butlerd: 出站队列写入失败：{error:#}");
            }
            if let Err(error) = orchestrator::enqueue_pending(now, &candidate) {
                eprintln!("mns-butlerd: 待投递队列写入失败：{error:#}");
            }

            // GUI 没在跑也没关系：它下次连上来时会在 `Hello` 之后补齐积压。
            link.send(&Signal::Pending);
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

    let facts = account_facts(&butler, now);
    let memory = memory_lines(&butler.memory);
    let recent = recent_lines(&session);
    let settings = chat_settings();
    let cli = std::path::PathBuf::from(&settings.agent_cli);
    let goal = talk::agent_init_goal(&facts, Some(&cli), &memory, &recent);

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

/// 记忆压成几行：未过期的事实优先，随后是最近一次自整理的要点。
fn memory_lines(memory: &Memory) -> Vec<String> {
    let today = Local::now().date_naive();
    let mut lines: Vec<String> = memory
        .facts
        .iter()
        .filter(|fact| !fact.is_expired(today))
        .map(|fact| fact.text.clone())
        .take(8)
        .collect();
    if let Some(summary) = memory.summaries.last() {
        lines.extend(summary.points.iter().take(3).cloned());
    }
    lines
}

/// 最近几轮对话压成「谁：说了什么」，给主动开口提供语气与上下文。
fn recent_lines(session: &Session) -> Vec<String> {
    let start = session.messages.len().saturating_sub(6);
    session.messages[start..]
        .iter()
        .map(|message| {
            let who = match message.role {
                Role::User => "用户",
                Role::Butler => "管家",
            };
            format!("{who}：{}", clip_chars(&message.text, 80))
        })
        .collect()
}

fn clip_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
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

/// 主动开口用的 agent 设置。工作目录固定到管家数据目录，理由同
/// `gui::butler`：避免继承项目仓库里的 AGENTS.md 约定。
fn chat_settings() -> money_never_sleep::agent::AgentSettings {
    let mut settings = money_never_sleep::agent::load_settings();
    let dir = butler_dir();
    let _ = std::fs::create_dir_all(&dir);
    settings.working_dir = dir.to_string_lossy().into_owned();
    settings
}

/// 账户事实，按年龄决定能信多少。
///
/// 没有事实不等于「账户是空的」——那会让偏离、决策日这些判断凭空成立。所以
/// 缺事实时退化成 `AccountFacts::default()`：只会产出「到点任务」「承诺到期」
/// 这类不依赖数字的由头，不会编造一条偏离。
fn account_facts(butler: &Butler, now: DateTime<Local>) -> AccountFacts {
    let facts = match orchestrator::read_facts() {
        Ok(Some((at, facts))) => {
            let age = now.signed_duration_since(at);
            if age > chrono::Duration::hours(FACTS_MAX_AGE_HOURS) {
                // 一周没更新过：账户可能早已不是这个样子，宁可不说话。
                AccountFacts::default()
            } else if age > chrono::Duration::hours(FACTS_PRICE_AGE_HOURS)
                || at.date_naive() != now.date_naive()
            {
                // 跨天的事实：偏离、当日入金、决策日都是「那一天」的判断，不能挪用。
                let mut facts = facts;
                facts.drift.clear();
                facts.new_cash_today = 0.0;
                facts.plan_ready = false;
                facts.decision_day = false;
                facts.heavy_page = None;
                facts
            } else {
                facts
            }
        }
        Ok(None) => AccountFacts::default(),
        Err(error) => {
            eprintln!("mns-butlerd: 账户事实读取失败：{error:#}");
            AccountFacts::default()
        }
    };
    // 承诺与数字无关，始终以记忆文件为准：这样「明天到期的承诺」不需要
    // GUI 那天恰好打开过。
    facts.with_promises(&butler.memory, now.date_naive())
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

/// 到 GUI 的长连接。
///
/// 「连接活着」本身就是心跳：GUI 关掉时写失败，下一次 `ensure` 重连；守护
/// 进程崩掉时 GUI 侧收到 `Disconnected`。不需要定时 ping，也不需要心跳文件。
#[derive(Default)]
struct GuiLink {
    client: Option<Client>,
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
