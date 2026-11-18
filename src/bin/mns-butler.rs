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
use money_never_sleep::butler::tasks::ButlerTask;
use money_never_sleep::butler::{AccountFacts, Butler, memory, schedule};

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
    /// 提议一个到点检查任务
    Add {
        /// 到时检查什么（不能写成预测）
        intent: String,
        /// 到点时间，格式 `2026-11-20 09:30`
        #[arg(long)]
        at: String,
        /// 去重键，同一事件只允许一个任务；缺省用意图前 24 字
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

    let mut facts = match money_never_sleep::app::load_snapshot() {
        Ok(snapshot) => {
            AccountFacts::from_snapshot(&snapshot, now).with_promises(
                &Butler::load()?.memory,
                now.date_naive(),
            )
        }
        Err(error) => {
            // 账户还没初始化也要能回答「为什么没说话」：只是没有账本事实。
            eprintln!("提示：未读到账户快照（{error:#}），本轮按空账户判定。");
            AccountFacts::default()
        }
    };

    // 行为画像：同一天同一页面看 ≥5 次才算高频。它不再作为开口由头，
    // 只进入事实文件，供管家在会话里回答「今天看了几次」。
    let since = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("合法零点")
        .and_local_timezone(Local)
        .single()
        .unwrap_or(now);
    let window = events::summarize(&events::read_all().unwrap_or_default(), since);
    if let Some((page, views)) = window
        .page_views
        .iter()
        .max_by_key(|(_, views)| **views)
        && window.is_heavy_checking(page)
    {
        facts.heavy_page = Some((page_label(page).to_owned(), *views));
    }

    // 读-改-写全程持锁（守护进程没在跑才会走到这里）。
    let _guard = money_never_sleep::butler::writer_lock();
    let mut butler = Butler::load()?;
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
        "记忆整理：合并 {} 条、归档过期 {} 条、压掉摘要 {} 条",
        memory.merged, memory.expired, memory.summaries_archived
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
    let (kind, body, inline_due) = match memory::parse_intent(text) {
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
    // 允许直接粘贴会话里的话（「忘掉补美股」），前缀由意图层去掉。
    let needle = match memory::parse_intent(text) {
        Some(memory::Intent::Forget { needle }) => needle,
        _ => text.trim().to_owned(),
    };
    // 锁内重读再落盘：GUI 窗口可能正开着，别拿启动快照覆盖它的条目。
    let (_, hits) = memory::Memory::update(|memory| memory.forget(&needle))?;
    if hits.is_empty() {
        println!("没有匹配「{needle}」的记忆");
        return Ok(());
    }
    for fact in &hits {
        println!("已忘掉（{}）：{}", fact.kind.label(), fact.text);
    }
    println!("已移入归档，仍可通过 memory.json 查阅");
    nudge_daemon();
    Ok(())
}

fn list_tasks() -> Result<()> {
    let butler = Butler::load()?;
    let now = Local::now();
    if butler.tasks.tasks.is_empty() && butler.tasks.archived.is_empty() {
        println!("没有任务");
        return Ok(());
    }
    println!("待执行：");
    for task in &butler.tasks.tasks {
        let status = if task.state == money_never_sleep::butler::TaskState::Pending {
            if task.is_expired(now) { "已过期待清理" } else { "待执行" }
        } else {
            "已结束"
        };
        println!(
            "  {} [{}] {} · 到点 {} · {}",
            task.id,
            status,
            task.intent,
            task.due_at.format("%Y-%m-%d %H:%M"),
            task.key
        );
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
        TaskAction::Add { intent, at, key } => {
            let due = parse_local(&at).with_context(|| format!("无法解析时间「{at}」"))?;
            let key = key.unwrap_or_else(|| intent.chars().take(24).collect());
            let verdict = butler
                .tasks
                .propose(ButlerTask::new(key, intent, due, now), now);
            match verdict {
                money_never_sleep::butler::TaskVerdict::Accepted { id } => {
                    butler.save_tasks()?;
                    println!("已接受任务 {id}，到点 {}", due.format("%Y-%m-%d %H:%M"));
                    nudge_daemon();
                }
                other => println!("未接受：{other:?}"),
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

fn page_label(key: &str) -> &str {
    events::page_label(key)
}

fn parse_local(text: &str) -> Result<chrono::DateTime<Local>> {
    let naive = NaiveDateTime::parse_from_str(text.trim(), "%Y-%m-%d %H:%M")?;
    naive
        .and_local_timezone(Local)
        .single()
        .context("本地时间不唯一")
}
