//! Headless P0 verification for the ACP external-agent path.
//!
//! The GUI drives exactly the same runtime; this example only removes the
//! window so the protocol chain can be verified on a build machine:
//!
//! ```text
//! cargo run --example agent_smoke --features agent -- "只回复 OK，不要调用工具"
//! ```
//!
//! It prints every timeline entry the UI would render and exits non-zero when
//! the run fails, so it is usable as a pre-release gate.

use std::io::Write;
use std::time::Duration;

use money_never_sleep::agent::{
    AgentCommand, AgentEvent, EntryKind, GoalSource, RunState, load_settings, start_runtime,
};

/// Mirrors the GUI rule: streamed chunks extend the open line, discrete rows
/// start a new one.
#[derive(Default)]
struct Printer {
    open: Option<EntryKind>,
}

impl Printer {
    fn entry(&mut self, kind: EntryKind, text: &str) {
        self.flush();
        println!("[{}] {text}", kind.label());
    }

    fn delta(&mut self, kind: EntryKind, text: &str) {
        if self.open != Some(kind) {
            self.flush();
            print!("[{}] ", kind.label());
            self.open = Some(kind);
        }
        print!("{text}");
        let _ = std::io::stdout().flush();
    }

    fn flush(&mut self) {
        if self.open.take().is_some() {
            println!();
        }
    }
}

#[tokio::main]
async fn main() {
    let goal = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "只回复 OK，不要调用任何工具。".to_owned());
    let settings = load_settings();

    println!("agent_cli = {}", settings.agent_cli);
    println!(
        "model     = {}",
        if settings.model.trim().is_empty() {
            "(agent default)"
        } else {
            settings.model.trim()
        }
    );
    println!("policy    = {:?}", settings.permission_policy);
    println!("goal      = {goal}");
    println!("---");

    let (handle, mut events) = start_runtime();
    if let Err(error) = handle.send(AgentCommand::Start {
        goal,
        settings,
        source: GoalSource::Manual,
    }) {
        eprintln!("无法提交目标：{error}");
        std::process::exit(2);
    }

    let mut printer = Printer::default();
    let mut terminal: Option<RunState> = None;

    loop {
        let next = tokio::time::timeout(Duration::from_secs(120), events.recv()).await;
        match next {
            Ok(Some(AgentEvent::State { state, detail })) => {
                printer.flush();
                println!("[状态] {} — {detail}", state.label());
                if matches!(state, RunState::Failed | RunState::Cancelled) {
                    terminal = Some(state);
                }
            }
            Ok(Some(AgentEvent::TurnFinished { stop_reason })) => {
                printer.entry(EntryKind::System, &format!("本轮结束：{stop_reason}"));
                // The GUI keeps the session open for follow-ups. This example
                // verifies a single turn, so it stops here instead of waiting
                // for a question nobody will ask.
                terminal = Some(RunState::Finished);
            }
            Ok(Some(AgentEvent::Decision(report))) => {
                printer.entry(EntryKind::System, &format!("结构化结论：{}", report.headline()));
            }
            Ok(Some(AgentEvent::Entry(entry))) => {
                printer.entry(entry.kind, &entry.text);
                if entry.kind == EntryKind::Error {
                    terminal = Some(RunState::Failed);
                }
            }
            Ok(Some(AgentEvent::Delta { kind, text })) => printer.delta(kind, &text),
            Ok(Some(AgentEvent::Permission(prompt))) => {
                // An autonomous policy should not reach this branch; print it so
                // a regression in auto-approval is visible instead of hanging.
                printer.entry(
                    EntryKind::Permission,
                    &format!("{} — {}", prompt.title, prompt.detail),
                );
                let allow = prompt
                    .options
                    .iter()
                    .find(|option| option.allow)
                    .map(|option| option.id.clone());
                if let Some(option_id) = allow {
                    let _ = handle.send(AgentCommand::Grant {
                        id: prompt.id,
                        option_id,
                    });
                }
            }
            Ok(Some(AgentEvent::PermissionResolved { id })) => {
                printer.entry(EntryKind::Permission, &format!("#{id} 已处理"));
            }
            Ok(Some(AgentEvent::Finished { stop_reason })) => {
                printer.entry(EntryKind::System, &format!("结束原因：{stop_reason}"));
                terminal = Some(RunState::Finished);
            }
            Ok(Some(AgentEvent::Failed { message })) => {
                printer.entry(EntryKind::Error, &message);
                terminal = Some(RunState::Failed);
            }
            Ok(None) => break,
            Err(_) => {
                printer.entry(EntryKind::Error, "120 秒内未结束，发送停止");
                let _ = handle.send(AgentCommand::Stop);
                terminal = Some(RunState::Cancelled);
                break;
            }
        }

        if matches!(terminal, Some(RunState::Finished) | Some(RunState::Failed)) {
            break;
        }
    }
    printer.flush();

    let _ = handle.send(AgentCommand::Shutdown);

    match terminal {
        Some(RunState::Finished) => println!("---\nRESULT=ok"),
        other => {
            println!("---\nRESULT=failed ({other:?})");
            std::process::exit(1);
        }
    }
}
