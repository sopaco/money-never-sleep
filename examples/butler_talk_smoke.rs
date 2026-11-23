//! 管家对话链路的无窗口验证：用真实的人设与账本上下文起一次 ACP 会话，
//! 按亲和力升级的三条验收问句各问一轮，最后再跑一轮主动开口，确认
//! `[BANNER]` / `[CHAT]` 两段能被确定性拆开。
//!
//! ```text
//! cargo run --example butler_talk_smoke --features agent
//! cargo run --example butler_talk_smoke --features agent -- --init-only
//! ```

use std::time::Duration;

use chrono::Local;
use money_never_sleep::agent::{
    AgentCommand, AgentEvent, EntryKind, GoalSource, load_settings, start_runtime,
};
use money_never_sleep::butler::orchestrator::{AccountFacts, DriftRow};
use money_never_sleep::butler::talk;

const TIMEOUT: Duration = Duration::from_secs(150);

/// 亲和力升级的三条验收问句：情绪接住、术语翻译、引用半截报告。
///
/// 第三条把「引用了一份没跑完的巡检」写进问题里：真正的引用注入在
/// `chat::reference_block`，这里的目的是验提示词口径（有接住句、不评价
/// 产品、无「别当依据看」式命令）。
const QUESTIONS: [&str; 3] = [
    "我是不是搞砸了？",
    "现在偏离多少",
    "你看靠谱吗？（引用了一份还没跑完的巡检，正文只有一句「我先拉取持仓、现金、绩效和买入冷静期状态」）",
];

fn facts() -> AccountFacts {
    AccountFacts {
        band_pp: 4.0,
        drift: vec![
            DriftRow {
                key: "us".to_owned(),
                name: "美股".to_owned(),
                current_pct: 70.3,
                target_pct: 83.0,
                drift_pp: -12.7,
                market_value: 70_300.0,
            },
            DriftRow {
                key: "cn".to_owned(),
                name: "A股".to_owned(),
                current_pct: 29.7,
                target_pct: 17.0,
                drift_pp: 12.7,
                market_value: 29_700.0,
            },
        ],
        total_assets: 100_000.0,
        holdings_value: 100_000.0,
        cash: 0.0,
        positions_count: 2,
        next_decision_day: Some("2026-11-20".to_owned()),
        data_date: Some(chrono::Local::now().format("%Y-%m-%d").to_string()),
        ..AccountFacts::default()
    }
}

/// 跑一轮，返回agent 累积的正文。
async fn run_turn(
    handle: &money_never_sleep::agent::AgentHandle,
    events: &mut tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    command: AgentCommand,
) -> String {
    handle.send(command).expect("发送失败");
    let mut buffer = String::new();
    loop {
        let event = match tokio::time::timeout(TIMEOUT, events.recv()).await {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(_) => {
                println!("\n超时：{} 秒内没有收尾", TIMEOUT.as_secs());
                break;
            }
        };
        match event {
            AgentEvent::Delta { kind, text } => {
                if kind == EntryKind::Message {
                    buffer.push_str(&text);
                    print!("{text}");
                }
            }
            AgentEvent::Entry(entry) => println!("[{}] {}", entry.kind.label(), entry.text),
            AgentEvent::Permission(prompt) => {
                println!("[授权] {}（已拒绝：管家窗口不授权工具）", prompt.title);
                let _ = handle.send(AgentCommand::Deny { id: prompt.id });
            }
            AgentEvent::TurnFinished { stop_reason } => {
                println!("\n--- 结束：{stop_reason} ---");
                match talk::validate_talk(&buffer) {
                    Ok(text) => println!("校验通过：\n{text}"),
                    Err(error) => println!("校验未通过：{error}"),
                }
                break;
            }
            AgentEvent::Failed { message } => {
                println!("\n失败：{message}");
                break;
            }
            _ => {}
        }
    }
    buffer
}

/// 与 GUI 同一份设置：工作目录固定到管家数据目录。
///
/// 不在仓库根上跑，否则 opencode 会读到项目的 AGENTS.md 并套用复盘契约。
fn settings() -> money_never_sleep::agent::AgentSettings {
    let mut settings = load_settings();
    let dir = money_never_sleep::config::app_data_dir().join("agent/butler");
    let _ = std::fs::create_dir_all(&dir);
    settings.working_dir = dir.to_string_lossy().into_owned();
    settings
}

/// 对话链路：首轮 `Start` + 两轮 `Ask`（同一个 session）。
async fn conversation(facts: &AccountFacts, memory: &[String], recent: &[String]) {
    let (handle, mut events) = start_runtime();
    let settings = settings();
    for (index, question) in QUESTIONS.iter().enumerate() {
        println!("\n=== 第 {} 问：{question} ===", index + 1);
        let started = std::time::Instant::now();
        let command = if index == 0 {
            AgentCommand::Start {
                goal: talk::opening_goal(question, facts, memory, recent),
                settings: settings.clone(),
                source: GoalSource::Conversation,
            }
        } else {
            AgentCommand::Ask {
                text: talk::follow_up(question),
            }
        };
        run_turn(&handle, &mut events, command).await;
        println!("耗时 {:?}", started.elapsed());
    }
    let _ = handle.send(AgentCommand::Shutdown);
}

/// 主动开口：单独一个 runtime。
///
/// 复用一个刚跑完的 runtime 会赶上上一轮的收尾，runner 回「已有任务在运行」——
/// 那是 harness 的时序问题，不是产品行为；真实链路里 agent-init 每次都是新起的。
async fn init(facts: &AccountFacts, memory: &[String], recent: &[String]) {
    let (handle, mut events) = start_runtime();
    println!("\n=== 主动开口（双通道）===");
    let raw = run_turn(
        &handle,
        &mut events,
        AgentCommand::Start {
            goal: talk::agent_init_goal(facts, memory, recent),
            settings: settings(),
            source: GoalSource::Conversation,
        },
    )
    .await;
    let _ = handle.send(AgentCommand::Shutdown);

    let reply = talk::split_init_reply(&talk::validate_talk(&raw).unwrap_or(raw));
    let banner = talk::notification_text(&reply.banner);
    println!("BANNER（{} 字）：{banner}", banner.chars().count());
    println!(
        "CHAT（{} 字）：{}",
        reply.chat.as_deref().unwrap_or("").chars().count(),
        reply.chat.as_deref().unwrap_or("（回落为横幅同文）")
    );
    match money_never_sleep::butler::schedule::validate_notification(&banner) {
        Ok(()) => println!("横幅口径：通过"),
        Err(violation) => println!("横幅口径：{violation}"),
    }
    if reply.chat.is_none() {
        println!("两段式：未拆出 CHAT（会走回退，横幅与会话同文）");
    }
}

#[tokio::main]
async fn main() {
    let facts = facts();
    // 与 GUI 同一条注入路径：记忆与最近对话也进首轮（这里从账户目录读
    // 真实记忆，空记忆则注入为空）。
    let today = Local::now().date_naive();
    let memory = money_never_sleep::butler::Memory::load().unwrap_or_default();
    let session = money_never_sleep::butler::Session::load().unwrap_or_default();
    let memory_lines = talk::memory_lines(&memory, today);
    let recent_lines = talk::recent_lines(&session);

    if !std::env::args().any(|arg| arg == "--init-only") {
        conversation(&facts, &memory_lines, &recent_lines).await;
    }
    init(&facts, &memory_lines, &recent_lines).await;
}
