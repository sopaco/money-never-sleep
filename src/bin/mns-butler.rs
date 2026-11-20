//! `mns-butler`：管家确定性内核的命令行入口。
//!
//! 它不是「另一个 mns」：簿记、策略、报告仍由 `mns` / `mns-gui` 负责。这里
//! 只暴露管家的唤醒、开口判定、记忆与任务，用于终端审计——「今天为什么没
//! 说话」「这条通知是谁授权的」都能当场复算。
//!
//! 表达层（ACP 会话）与通知层（系统通知、窗口）是它的下游：判定结果先落到
//! `~/.mns/agent/butler/outbox.jsonl`，送达方只负责发送，不再判断该不该发。

use anyhow::{Context, Result};
use chrono::{Local, NaiveDate, NaiveDateTime};
use clap::{Parser, Subcommand};
use money_never_sleep::butler::events;
use money_never_sleep::butler::orchestrator::{self, agent_init_backoff};
use money_never_sleep::butler::tasks::{
    ButlerTask, MAX_ACTIVE, MAX_NEW_PER_DAY, TaskKind, TaskOrigin, TaskVerdict,
};
use money_never_sleep::butler::{Butler, memory, schedule};

#[derive(Parser)]
#[command(
    name = "mns-butler",
    version,
    about = "MNS 管家：确定性唤醒、开口判定、记忆与任务（本地、可审计）",
    after_help = "常用：
  mns-butler status             查看唤醒状态、记忆与任务
  mns-butler wake               按当前账户做一次定时巡检判定
  mns-butler tick               跑一次每周自整理（记忆合并、任务复盘）
  mns-butler remember \"周五补美股\" --due 2026-11-20   写入一条事实
  mns-butler memory             查看常驻记忆（--archived 看归档）
  mns-butler tasks              查看待执行与已归档任务
  mns-butler task add \"下个决策日核对计划\" --at \"2026-11-20 09:30\"
  mns-butler task add \"提醒我喝水\" --in 5m --user
  mns-butler task add \"到点陪我聊两句\" --in 1h --talk
  mns-butler outbox             查看最近已授权但未送达的通知"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 查看唤醒状态、开口记录、记忆与任务队列
    Status,
    /// 做一次唤醒判定。默认定时巡检；`--ledger ID` 模拟一条账本事件
    Wake {
        /// 账本事件 id（买入/卖出/入金/出金完成时产生）
        #[arg(long)]
        ledger: Option<String>,
    },
    /// 每周自整理：合并记忆、复盘任务、修剪去重集合
    Tick,
    /// 用户说「记住这件事」：写入一条事实
    Remember {
        /// 要记住的内容（带「记住」前缀也可以，会自动去掉）
        text: String,
        /// 到期日，格式 `2026-11-20`；到期后转入归档
        #[arg(long)]
        due: Option<String>,
    },
    /// 列出常驻记忆与归档
    Memory {
        /// 只看归档（「忘掉」或过期/超限移出的条目）
        #[arg(long)]
        archived: bool,
    },
    /// 用户说「忘掉这件事」：命中的事实移入归档（可查）
    Forget {
        /// 关键词或原句
        text: String,
    },
    /// 列出待执行与已归档任务
    Tasks,
    /// 新建一个自设任务
    Task {
        #[command(subcommand)]
        action: TaskAction,
    },
    /// 查看出站通知队列（最近 20 条）
    Outbox,
    /// 关闭主动开口：唤醒与自整理照常，只是不再产生通知
    Mute,
    /// 恢复主动开口
    Unmute,
}

#[derive(Subcommand)]
enum TaskAction {
    /// 提议一个到点任务
    Add {
        /// 到时提醒/检查什么（不能写成预测）
        intent: String,
        /// 到点时间，格式 `2026-11-20 09:30` 或 `2026-11-20 09:30:15`
        #[arg(long, conflicts_with = "within")]
        at: Option<String>,
        /// 从现在起多久，格式 `90s` / `5m` / `2h` / `1d`
        #[arg(long = "in", conflicts_with = "at")]
        within: Option<String>,
        /// 用户亲口要的提醒：跳过「必须是检查」的措辞校验，也不占管家的每日额度
        #[arg(long)]
        user: bool,
        /// 到点主动说一句问候，而不是回显任务原文（隐含 --user）
        #[arg(long)]
        talk: bool,
        /// 去重键，同一事件只允许一个任务；缺省用「意图前 24 字 @ 到点时刻」
        #[arg(long)]
        key: Option<String>,
    },
    /// 标记任务完成
    Done { id: String },
}

fn main() {
    if let Err(error) = run() {
        eprintln!("mns-butler: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Status => status(),
        Command::Wake { ledger } => wake(ledger),
        Command::Tick => tick(),
        Command::Remember { text, due } => remember(&text, due.as_deref()),
        Command::Memory { archived } => list_memory(archived),
        Command::Forget { text } => forget(&text),
        Command::Tasks => list_tasks(),
        Command::Task { action } => task(action),
        Command::Outbox => outbox(),
        Command::Mute => set_proactive(false),
        Command::Unmute => set_proactive(true),
    }
}

/// 告诉在跑的守护进程「状态变了，重算下一次该在什么时候醒」。
///
/// 守护进程不扫盘：它只在自己算出的时刻或有人叫醒时思考。CLI 是第三个写者
/// （GUI、守护进程之外），改了状态却不说，用户就得等到下一次巡检才看见效果。
/// 没有守护进程时（纯 CLI、Windows）静默失败，不影响命令本身。
fn nudge_daemon() {
    money_never_sleep::ipc::poke_daemon();
}

/// 把命令交给在跑的守护进程。成功返回 `true`，调用方就不再本地执行。
///
/// `state.json` 有三个潜在写者（GUI 巡检、守护进程、CLI）。守护进程常驻时
/// 以自己内存里的状态为准，CLI 再写一份文件只会被它下一次保存覆盖掉，中间的
/// 开关变化就丢了。所以能委托就委托，委托不了（没在跑）才本地写。
fn delegate(signal: money_never_sleep::ipc::Signal) -> bool {
    money_never_sleep::ipc::poke("mns-butlerd", &signal).is_ok()
}

fn status() -> Result<()> {
    let butler = Butler::load()?;
    let now = Local::now();
    let state = &butler.state;

    println!("管家状态 · {}", now.format("%Y-%m-%d %H:%M"));
    // 对话不在 MNS 里：provider、模型与凭据都在外部 ACP agent 自己的配置
    // 里，这边没有可读的「模型状态」。写死一句口径，避免让人以为要在这儿
    // 找开关。
    println!("对话：经外部 ACP agent（桌面端设置里指定 CLI；本机不存密钥）");
    match state.scheduler.last_scheduled {
        Some(last) => println!(
            "上次定时巡检：{}（最小间隔 {} 小时）",
            last.format("%Y-%m-%d %H:%M"),
            state.scheduler.min_interval_hours
        ),
        None => println!(
            "上次定时巡检：尚未运行（最小间隔 {} 小时）",
            state.scheduler.min_interval_hours
        ),
    }
    let week = now - chrono::Duration::days(7);
    let recent_speaks = state
        .ledger
        .records
        .iter()
        .filter(|record| record.at >= week)
        .count();
    println!(
        "主动开口：{}（每日上限 {}、每周上限 {}、去重 {} 天、静音 {:02}–{:02} 点）",
        if state.policy.enabled {
            "已开启"
        } else {
            "已关闭"
        },
        state.policy.daily_cap,
        state.policy.weekly_cap,
        state.policy.dedupe_days,
        state.policy.quiet_start_hour,
        state.policy.quiet_end_hour
    );
    println!("近 7 日开口：{recent_speaks} 次");

    // agent 自主开口的尝试记录：回答「昨天 8 点为什么没说话」。
    // 失败原因只在守护进程 stderr 里的话，GUI 拉起的守护进程谁也看不见。
    match state.last_agent_init {
        None => println!("agent 自主开口：从未尝试"),
        Some(last) => {
            let failures = state.agent_init_failures;
            let next = last + agent_init_backoff(failures);
            match failures {
                0 => println!(
                    "agent 自主开口：上次尝试 {}（常规节奏，最早 {}）",
                    last.format("%Y-%m-%d %H:%M"),
                    next.format("%m-%d %H:%M")
                ),
                count => println!(
                    "agent 自主开口：上次尝试 {} · 连续失败 {count} · 按退避最早 {}",
                    last.format("%Y-%m-%d %H:%M"),
                    next.format("%m-%d %H:%M")
                ),
            }
            for record in orchestrator::read_init_records(3).iter().rev() {
                let detail = record
                    .detail
                    .as_deref()
                    .unwrap_or(if record.outcome == orchestrator::AgentInitOutcome::Spoke {
                        "已开口"
                    } else {
                        ""
                    });
                if detail.is_empty() {
                    println!("  {} {}", record.at.format("%m-%d %H:%M"), record.outcome.label());
                } else {
                    println!(
                        "  {} {}：{}",
                        record.at.format("%m-%d %H:%M"),
                        record.outcome.label(),
                        detail
                    );
                }
            }
        }
    }

    println!(
        "记忆：事实 {} 条 / 摘要 {} 条；归档事实 {} 条、摘要 {} 条",
        butler.memory.facts.len(),
        butler.memory.summaries.len(),
        butler.memory.archived_facts.len(),
        butler.memory.archived_summaries.len()
    );
    let pending = butler.tasks.pending().len();
    println!(
        "任务：待执行 {} 条、已归档 {} 条；到点 {} 条",
        pending,
        butler.tasks.archived.len(),
        butler.tasks.due(now).len()
    );

    let behavior = events::read_all().unwrap_or_default();
    let window = events::summarize(&behavior, now - chrono::Duration::hours(24));
    println!(
        "近 24h 行为：打开应用 {} 次、打开买入弹层 {} 次、护栏命中 {} 次",
        window.opens, window.buy_dialogs, window.guard_hits
    );
    for (page, views) in &window.page_views {
        println!("  · {page} {views} 次");
    }
    Ok(())
}

fn wake(ledger: Option<String>) -> Result<()> {
    let now = Local::now();
    let source = match ledger.clone() {
        Some(id) => schedule::WakeSource::Ledger { id },
        None => schedule::WakeSource::Scheduled,
    };

    // 守护进程在跑时由它判定：两个进程各跑一遍会重复消耗频控额度、
    // 各自写一份 state.json，后写的覆盖先写的去重键。
    if delegate(money_never_sleep::ipc::Signal::Wake { ledger }) {
        println!("已交由守护进程执行（同一事件不会重复开口）");
        return Ok(());
    }

    // 与守护进程同一条事实路径（`orchestrator::ledger_facts`）：谁判定谁
    // 计算，审计复算与常驻判定看到的是同一套事实，含价格新鲜度闸门。
    let base = money_never_sleep::butler::orchestrator::ledger_facts(now);

    // 读-改-写全程持锁（守护进程没在跑才会走到这里）。
    let _guard = money_never_sleep::butler::writer_lock();
    let mut butler = Butler::load()?;
    // 承诺来自记忆文件：与判定用的是同一份 butler 状态。
    let facts = base.with_promises(&butler.memory, now.date_naive());
    let report = butler.wake(&source, now, &facts);
    butler.save()?;

    match report.wake {
        schedule::WakeVerdict::Wake => println!("唤醒：是（{source:?}）"),
        schedule::WakeVerdict::TooSoon { next_allowed } => {
            println!("唤醒：否，距上次巡检不足间隔，最早 {}", next_allowed.format("%H:%M"));
            return Ok(());
        }
        schedule::WakeVerdict::Duplicate => {
            println!("唤醒：否，同一事件已处理过");
            return Ok(());
        }
    }

    match report.speak {
        Some(schedule::SpeakDecision::Speak(candidate)) => {
            println!("开口：是（{}）", candidate.reason.label());
            println!("文案：{}", candidate.draft);
            money_never_sleep::butler::orchestrator::enqueue_notice(now, &candidate)?;
            println!("已放入出站队列：{}", money_never_sleep::butler::orchestrator::outbox_path().display());
        }
        Some(schedule::SpeakDecision::Silent(reason)) => {
            println!("开口：否（{}）", reason.label());
        }
        None => println!("开口：未判定"),
    }
    Ok(())
}

fn tick() -> Result<()> {
    let now = Local::now();
    let today = now.date_naive();
    // 读-改-写全程持锁：自整理会同时动三份文件。
    let _guard = money_never_sleep::butler::writer_lock();
    let mut butler = Butler::load()?;

    let memory = butler.memory.compact(today);
    let tasks = butler.tasks.review(now);
    butler.state.scheduler.trim_seen(200);
    butler.state.ledger.trim(now, 30);
    butler.state.last_maintenance = Some(now);
    butler.save()?;

    println!(
        "记忆整理：合并 {} 条、归档过期 {} 条、压掉摘要 {} 条、清出归档 {} 条",
        memory.merged, memory.expired, memory.summaries_archived, memory.archived_trimmed
    );
    println!(
        "任务复盘：合并 {} 条、丢弃失效 {} 条、清出归档 {} 条",
        tasks.merged, tasks.dropped_stale, tasks.archived
    );
    println!(
        "现状：事实 {} 条、摘要 {} 条、待执行任务 {} 条",
        butler.memory.facts.len(),
        butler.memory.summaries.len(),
        butler.tasks.pending().len()
    );
    nudge_daemon();
    Ok(())
}

fn remember(text: &str, due: Option<&str>) -> Result<()> {
    let now = Local::now();
    let (kind, body, inline_due) = match memory::parse_intent(text, now.date_naive()) {
        Some(memory::Intent::Remember { kind, text, due }) => (kind, text, due),
        Some(memory::Intent::Forget { .. }) => {
            anyhow::bail!("这是「忘掉」的句子，请改用 mns-butler forget");
        }
        None => (memory::classify(text), text.trim().to_owned(), None),
    };
    anyhow::ensure!(!body.is_empty(), "要记住的内容不能为空");
    let due = match due {
        Some(day) => Some(
            NaiveDate::parse_from_str(day.trim(), "%Y-%m-%d")
                .with_context(|| format!("无法解析到期日「{day}」，格式 2026-11-20"))?,
        ),
        None => inline_due,
    };

    let mut fact = memory::Fact::new(kind, body.clone(), now);
    if let Some(day) = due {
        fact = fact.expires_on(day);
    }
    // 只改记忆文件：`Memory::update` 在写锁内重读再落盘——顺手重写
    // state/tasks 会覆盖掉守护进程刚写的去重键，拿旧快照覆盖记忆则会
    // 吞掉 GUI 窗口刚记下的条目。
    let (_, verdict) = memory::Memory::update(|memory| memory.remember(fact))?;

    match verdict {
        memory::Remembered::Merged => println!("已有同义条目，引用计数 +1：{body}"),
        _ => println!("已记住（{}）：{body}", kind.label()),
    }
    if let Some(day) = due {
        println!("到期日：{day}（到期后转入归档）");
    }
    nudge_daemon();
    Ok(())
}

fn list_memory(archived: bool) -> Result<()> {
    let butler = Butler::load()?;
    let today = Local::now().date_naive();
    let rows = if archived {
        &butler.memory.archived_facts
    } else {
        &butler.memory.facts
    };
    if rows.is_empty() {
        println!("{}：空", if archived { "归档事实" } else { "常驻事实" });
        return Ok(());
    }
    println!(
        "{}：{} 条",
        if archived { "归档事实" } else { "常驻事实" },
        rows.len()
    );
    for fact in rows {
        let due = fact
            .expires_at
            .map(|day| format!(" · 到期 {day}"))
            .unwrap_or_default();
        let expired = if fact.is_expired(today) { " · 已过期" } else { "" };
        println!(
            "  {} [{}] {}{due}{expired}",
            fact.id,
            fact.kind.label(),
            fact.text
        );
    }
    if !archived {
        println!(
            "归档 {} 条（`mns-butler memory --archived` 查看）",
            butler.memory.archived_facts.len()
        );
    }
    Ok(())
}

fn forget(text: &str) -> Result<()> {
    // 允许直接粘贴会话里的话（「忘掉补美股」），前缀与日期由意图层去掉。
    let (needle, due) = match memory::parse_intent(text, Local::now().date_naive()) {
        Some(memory::Intent::Forget { needle, due }) => (needle, due),
        _ => (text.trim().to_owned(), None),
    };
    // 锁内重读再落盘：GUI 窗口可能正开着，别拿启动快照覆盖它的条目。
    let (_, outcome) = memory::Memory::update(|memory| memory.forget(&needle, due))?;
    match outcome {
        memory::Forgot::NotFound => {
            println!("没有匹配「{needle}」的记忆");
        }
        memory::Forgot::Removed(hits) => {
            for fact in &hits {
                println!("已忘掉（{}）：{}", fact.kind.label(), fact_label(fact));
            }
            println!("已移入归档，仍可通过 memory.json 查阅");
            // 只有真的动了记忆才需要守护进程重排程。
            nudge_daemon();
        }
        memory::Forgot::Ambiguous(candidates) => {
            println!("「{needle}」对上了多条，一条都没动：");
            for fact in &candidates {
                println!("  - （{}）：{}", fact.kind.label(), fact_label(fact));
            }
            println!("带上日期说要忘哪条，比如「忘掉下周五补美股」");
        }
    }
    Ok(())
}

/// 一条事实在终端里的展示：到期日是区分同文条目的唯一信息，必须带上。
fn fact_label(fact: &memory::Fact) -> String {
    match fact.expires_at {
        Some(day) => format!("{} · 到期 {}", fact.text, day.format("%Y-%m-%d")),
        None => fact.text.clone(),
    }
}

fn list_tasks() -> Result<()> {
    let butler = Butler::load()?;
    let now = Local::now();
    if butler.tasks.tasks.is_empty() && butler.tasks.archived.is_empty() {
        println!("没有任务");
        return Ok(());
    }
    // 新版本在 `task done` 当刻就搬进归档；这里再按状态分栏，是为了兼容
    // 旧版本留下的「已结束但还在 tasks 里」的记录，别让 agent 以为它还挂着。
    let (pending, leftover): (Vec<_>, Vec<_>) = butler
        .tasks
        .tasks
        .iter()
        .partition(|task| task.state == money_never_sleep::butler::TaskState::Pending);
    if !pending.is_empty() {
        println!("待执行：");
    }
    for task in pending {
        let status = if task.is_expired(now) { "已过期待清理" } else { "待执行" };
        println!(
            "  {} [{}] {} · 到点 {} · {}",
            task.id,
            status,
            task.intent,
            task.due_at.format("%Y-%m-%d %H:%M"),
            task.key
        );
    }
    if !leftover.is_empty() {
        println!("已结束（待归档，下次巡检迁入归档）：");
        for task in leftover {
            let status = match task.state {
                money_never_sleep::butler::TaskState::Done => "完成",
                money_never_sleep::butler::TaskState::Dropped => "作废",
                money_never_sleep::butler::TaskState::Pending => "待执行",
            };
            println!("  {} [{}] {}", task.id, status, task.intent);
        }
    }
    if !butler.tasks.archived.is_empty() {
        println!("已归档（保留 7 天）：");
        for task in &butler.tasks.archived {
            println!(
                "  {} [{}] {}",
                task.id,
                match task.state {
                    money_never_sleep::butler::TaskState::Done => "完成",
                    money_never_sleep::butler::TaskState::Dropped => "作废",
                    money_never_sleep::butler::TaskState::Pending => "待执行",
                },
                task.intent
            );
        }
    }
    Ok(())
}

fn task(action: TaskAction) -> Result<()> {
    let now = Local::now();
    // 读-改-写全程持锁：GUI 巡检与守护进程可能同时在写 tasks.json。
    let _guard = money_never_sleep::butler::writer_lock();
    let mut butler = Butler::load()?;
    match action {
        TaskAction::Add {
            intent,
            at,
            within,
            user,
            talk,
            key,
        } => {
            let due = match (at, within) {
                (Some(at), None) => {
                    parse_local(&at).with_context(|| format!("无法解析时间「{at}」"))?
                }
                (None, Some(within)) => {
                    let wait = parse_duration(&within)
                        .with_context(|| format!("无法解析时长「{within}」"))?;
                    now + wait
                }
                (None, None) => {
                    anyhow::bail!(
                        "要指定到点时间：相对时间用 --in 5m / --in 90s，绝对时刻用 --at \"YYYY-MM-DD HH:MM\""
                    )
                }
                _ => anyhow::bail!("--at 和 --in 必须二选一"),
            };
            let kind = if talk { TaskKind::Talk } else { TaskKind::Check };
            let origin = if user || talk {
                TaskOrigin::User
            } else {
                TaskOrigin::Butler
            };
            // 同一句话可以在不同时间各提醒一次：默认键带上到点时刻，否则
            // 「提醒我喝水」第二次会被自己的去重挡住。
            let key = key.unwrap_or_else(|| {
                format!(
                    "{}@{}",
                    intent.chars().take(24).collect::<String>(),
                    due.format("%Y-%m-%d %H:%M")
                )
            });
            let verdict = butler.tasks.propose_as(
                ButlerTask::new(key, intent, due, now).with_kind(kind),
                now,
                origin,
            );
            match verdict {
                TaskVerdict::Accepted { id } => {
                    butler.save_tasks()?;
                    println!(
                        "已接受任务 {id}，到点 {}（{}）",
                        due.format("%Y-%m-%d %H:%M:%S"),
                        describe_wait(due - now)
                    );
                    nudge_daemon();
                }
                other => println!("{}", reject_reason(&other)),
            }
        }
        TaskAction::Done { id } => {
            if butler.tasks.complete(&id, now) {
                butler.save_tasks()?;
                println!("已完成任务 {id}");
                nudge_daemon();
            } else {
                println!("没有待执行的任务 {id}");
            }
        }
    }
    Ok(())
}

/// 把 `TaskVerdict` 翻成 agent 能直接转述的中文。
///
/// agent 拿到的 `{:?}` 是 Rust 调试文本（`IntentNotCheck(...)` /
/// `DailyLimit`），它要么照抄给用户，要么自己猜意思。这里把原因写成一句
/// 人话，失败时也保持诚实。
fn reject_reason(verdict: &TaskVerdict) -> String {
    match verdict {
        TaskVerdict::Accepted { id } => format!("已接受任务 {id}"),
        TaskVerdict::Duplicate => {
            "未接受：同一件事已经有任务了，先处理完或换个说法".to_owned()
        }
        TaskVerdict::DailyLimit => {
            format!("未接受：今天已经建了 {MAX_NEW_PER_DAY} 条待办任务，明天再来；用户亲口要的提醒请加 --user")
        }
        TaskVerdict::QueueFull => {
            format!("未接受：待执行任务已达 {MAX_ACTIVE} 条上限，先完成或等旧的作废")
        }
        TaskVerdict::TooSoon { now, due_at } => format!(
            "未接受：到点时间 {} 已经过去（现在 {}）。要现在起算就用 --in 5m / --in 60s，别自己算绝对时刻",
            due_at.format("%Y-%m-%d %H:%M:%S"),
            now.format("%Y-%m-%d %H:%M:%S")
        ),
        TaskVerdict::IntentNotCheck(reason) => format!("未接受：{reason}"),
    }
}

fn outbox() -> Result<()> {
    let path = money_never_sleep::butler::orchestrator::outbox_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!("出站队列为空");
            return Ok(());
        }
        Err(error) => return Err(error).with_context(|| format!("读取 {} 失败", path.display())),
    };
    let mut lines: Vec<&str> = text.lines().filter(|line| !line.trim().is_empty()).collect();
    for line in lines.drain(lines.len().saturating_sub(20)..) {
        println!("{line}");
    }
    Ok(())
}

fn set_proactive(enabled: bool) -> Result<()> {
    let label = if enabled {
        "已开启（到点才会提示，仍然每日/每周限次）"
    } else {
        "已关闭（唤醒与自整理照常，只是不再提示）"
    };
    // 守护进程在跑时由它落盘。委托成功就不再本地写 state——否则这一份
    // 会被它内存里的旧值在下次保存时覆盖，用户看到的开关又跳回去。
    if delegate(money_never_sleep::ipc::Signal::SetProactive { enabled }) {
        println!("主动开口：{label}（已交由守护进程落盘）");
        return Ok(());
    }
    let _guard = money_never_sleep::butler::writer_lock();
    let mut butler = Butler::load()?;
    butler.state.policy.enabled = enabled;
    butler.save_state()?;
    println!("主动开口：{label}");
    Ok(())
}

fn parse_local(text: &str) -> Result<chrono::DateTime<Local>> {
    let text = text.trim();
    // 先试带秒的：`%H:%M` 解析不了 `17:28:00`，反过来会吞掉秒。
    for format in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(text, format) {
            return naive
                .and_local_timezone(Local)
                .single()
                .context("本地时间不唯一");
        }
    }
    anyhow::bail!("时间要写成 `YYYY-MM-DD HH:MM` 或 `YYYY-MM-DD HH:MM:SS`")
}

/// 解析相对时长：`90s` / `5m` / `2h` / `1d`。
///
/// 只认一个数字 + 一个 ASCII 单位。刻意不认自然语言：模型会把「5 分钟」
/// 算成整分边界，那正是这次提前 8 秒触发的根因。
fn parse_duration(text: &str) -> Result<chrono::Duration> {
    let text = text.trim();
    let unit = text.chars().last().context("时长不能为空")?;
    let number = &text[..text.len() - unit.len_utf8()];
    let value: i64 = number
        .trim()
        .parse()
        .context("时长要写成 90s / 5m / 2h / 1d")?;
    if value <= 0 {
        anyhow::bail!("时长必须大于 0");
    }
    let duration = match unit.to_ascii_lowercase() {
        's' => chrono::Duration::seconds(value),
        'm' => chrono::Duration::minutes(value),
        'h' => chrono::Duration::hours(value),
        'd' => chrono::Duration::days(value),
        _ => anyhow::bail!("时长单位只支持 s/m/h/d，例如 90s、5m、2h、1d"),
    };
    Ok(duration)
}

/// 给模型一句能直接转述的「还有多久」，省得它自己算。
fn describe_wait(wait: chrono::Duration) -> String {
    let secs = wait.num_seconds().max(0);
    if secs < 60 {
        format!("还有 {secs} 秒")
    } else if secs < 3600 {
        let (minutes, seconds) = (secs / 60, secs % 60);
        if seconds == 0 {
            format!("还有 {minutes} 分钟")
        } else {
            format!("还有 {minutes} 分 {seconds} 秒")
        }
    } else if secs < 86_400 {
        format!("还有 {} 小时 {} 分钟", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("还有 {} 天", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 相对时长支持秒分时天() {
        assert_eq!(parse_duration("90s").unwrap(), chrono::Duration::seconds(90));
        assert_eq!(parse_duration("5m").unwrap(), chrono::Duration::minutes(5));
        assert_eq!(parse_duration("2h").unwrap(), chrono::Duration::hours(2));
        assert_eq!(parse_duration("1d").unwrap(), chrono::Duration::days(1));
        assert!(parse_duration("5分钟").is_err(), "自然语言不该被猜");
        assert!(parse_duration("0s").is_err());
        assert!(parse_duration("5w").is_err());
    }

    #[test]
    fn 绝对时间支持到秒() {
        let with_secs = parse_local("2026-11-18 17:28:54").unwrap();
        assert_eq!(with_secs.format("%H:%M:%S").to_string(), "17:28:54");
        let minute = parse_local("2026-11-18 17:28").unwrap();
        assert_eq!(minute.format("%H:%M:%S").to_string(), "17:28:00");
    }

    #[test]
    fn 回执把等待时长说成人话() {
        assert_eq!(describe_wait(chrono::Duration::seconds(42)), "还有 42 秒");
        assert_eq!(
            describe_wait(chrono::Duration::seconds(90)),
            "还有 1 分 30 秒"
        );
        assert_eq!(describe_wait(chrono::Duration::minutes(5)), "还有 5 分钟");
        assert_eq!(
            describe_wait(chrono::Duration::minutes(130)),
            "还有 2 小时 10 分钟"
        );
        assert_eq!(describe_wait(chrono::Duration::days(2)), "还有 2 天");
    }
}
