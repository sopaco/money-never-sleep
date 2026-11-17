//! 管家对话链路的无窗口验证：用真实的人设与账本上下文起一次 ACP 会话，
//! 再追问一句，确认 `Ask` 复用同一个 session 不会被拒。
//!
//! ```text
//! cargo run --example butler_talk_smoke --features agent
//! ```

use std::time::Duration;

use money_never_sleep::agent::{
    AgentCommand, AgentEvent, EntryKind, GoalSource, load_settings, start_runtime,
};
use money_never_sleep::butler::orchestrator::{AccountFacts, DriftRow};
use money_never_sleep::butler::talk;

const TIMEOUT: Duration = Duration::from_secs(150);

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

#[tokio::main]
async fn main() {
    let facts = facts();
    // 与 GUI 走同一条路径：随应用安装的 CLI 绝对路径。example 在 crate 外，
    // `agent::cli_path` 是 pub(crate)，这里按同一规则找一个可用的副本。
    let cli = ["/Applications/MNS.app/Contents/MacOS/mns"]
        .into_iter()
        .map(std::path::PathBuf::from)
        .find(|path| path.is_file());
    let question = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "现在偏离多少".to_owned());
    let goal = talk::opening_goal(&question, &facts, cli.as_deref());
    println!("--- 第一轮 goal 长度 {} 字 ---", goal.chars().count());

    let (handle, mut events) = start_runtime();
    // 与 GUI 保持一致：工作目录固定到管家数据目录，不在仓库根上跑，
    // 否则 opencode 会读到项目的 AGENTS.md 并套用复盘契约。
    let mut settings = load_settings();
    let dir = money_never_sleep::config::app_data_dir().join("agent/butler");
    let _ = std::fs::create_dir_all(&dir);
    settings.working_dir = dir.to_string_lossy().into_owned();

    let started = std::time::Instant::now();
    run_turn(
        &handle,
        &mut events,
        AgentCommand::Start {
            goal,
            settings,
            source: GoalSource::Conversation,
        },
    )
    .await;
    println!("第一轮耗时 {:?}", started.elapsed());

    println!("\n=== 追问 ===");
    let started = std::time::Instant::now();
    run_turn(
        &handle,
        &mut events,
        AgentCommand::Ask {
            text: talk::follow_up("那 A 股呢"),
        },
    )
    .await;
    println!("追问耗时 {:?}", started.elapsed());

    let _ = handle.send(AgentCommand::Shutdown);
}
