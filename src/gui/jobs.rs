//! Background worker for application services.
//!
//! SQLite reads, network refreshes and file writes must never run on the GPUI
//! main thread: a 30-second price refresh would freeze the window. Requests go
//! to one worker thread that owns a Tokio runtime; results come back as typed
//! events the shell folds into its state.

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::app::{self, RefreshOutcome, Snapshot};
use crate::gui::conversations::{self, ConversationRecord, ConversationSummary};

pub enum Request {
    Load,
    Refresh,
    SetProfile(String),
    SetMonthly(f64),
    SetWeekday(u32),
    Deposit {
        amount: f64,
        note: Option<String>,
    },
    Withdraw {
        amount: f64,
        note: Option<String>,
    },
    AddAsset {
        code: String,
        name: String,
        category: String,
    },
    Buy {
        code: String,
        shares: f64,
        price: f64,
        /// 冷静期理由（含结构化前缀，见 `BuyGuardrail::build_note`）。
        /// 未触发护栏时为 None。
        note: Option<String>,
    },
    Sell {
        code: String,
        shares: f64,
        price: f64,
    },
    /// Produce the brief that opens a daily review. `refresh` maps to
    /// `mns today`, otherwise `mns report` reuses the latest snapshot.
    AgentBrief {
        refresh: bool,
    },
    /// Persist a structured conclusion. The agent never writes files; the shell
    /// owns storage, so an unwritable working directory cannot lose a run.
    SaveDecision {
        body: String,
    },
    /// Assistant conversation archive. One file per conversation; a running
    /// session overwrites its own file as turns finish.
    LoadConversations,
    SaveConversation(Box<ConversationRecord>),
    LoadConversation {
        id: String,
    },
    DeleteConversation {
        id: String,
    },
}

pub enum Event {
    Loaded(Result<Snapshot, String>),
    Refreshed(Result<RefreshOutcome, String>),
    /// A write succeeded; the string is user-facing confirmation copy.
    Saved(String),
    Failed(String),
    /// Report text for the agent prompt, produced by the CLI itself.
    Brief(Result<String, String>),
    /// Refreshed archive index, sent after load / save / delete.
    Conversations(Result<Vec<ConversationSummary>, String>),
    /// Full transcript for a read-only replay.
    ConversationOpened(Result<Box<ConversationRecord>, String>),
    /// A write outcome that is not a failure of the conversation itself:
    /// archive loss must never interrupt a live agent run.
    ConversationNotice(Result<String, String>),
}

#[derive(Clone)]
pub struct Jobs {
    tx: UnboundedSender<Request>,
}

impl Jobs {
    pub fn spawn() -> (Self, UnboundedReceiver<Event>) {
        let (request_tx, mut request_rx) = unbounded_channel::<Request>();
        let (event_tx, event_rx) = unbounded_channel::<Event>();

        std::thread::Builder::new()
            .name("mns-app-worker".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = event_tx.send(Event::Failed(format!("后台线程启动失败：{error}")));
                        return;
                    }
                };

                // `blocking_recv` keeps the thread outside async context, so a
                // refresh can still call `block_on` further down.
                while let Some(request) = request_rx.blocking_recv() {
                    match request {
                        Request::Load => {
                            let _ = event_tx.send(Event::Loaded(load()));
                        }
                        Request::Refresh => {
                            let result = runtime
                                .block_on(app::refresh_all())
                                .map_err(|error| format!("{error:#}"));
                            let _ = event_tx.send(Event::Refreshed(result));
                            let _ = event_tx.send(Event::Loaded(load()));
                        }
                        Request::SetProfile(id) => finish(
                            &event_tx,
                            app::set_profile(&id).map(|()| "风险档位已更新".to_owned()),
                        ),
                        Request::SetMonthly(amount) => finish(
                            &event_tx,
                            app::set_monthly(amount).map(|()| "每月定投金额已保存".to_owned()),
                        ),
                        Request::SetWeekday(weekday) => finish(
                            &event_tx,
                            app::set_decision_weekday(weekday).map(|()| "决策日已更新".to_owned()),
                        ),
                        Request::Deposit { amount, note } => finish(
                            &event_tx,
                            app::deposit(amount, note.as_deref())
                                .map(|balance| format!("已入金，现金余额 ¥{balance:.2}")),
                        ),
                        Request::Withdraw { amount, note } => finish(
                            &event_tx,
                            app::withdraw(amount, note.as_deref())
                                .map(|balance| format!("已出金，现金余额 ¥{balance:.2}")),
                        ),
                        Request::AddAsset {
                            code,
                            name,
                            category,
                        } => finish(
                            &event_tx,
                            app::add_asset(&code, &name, &category)
                                .map(|()| format!("已添加标的 {code}")),
                        ),
                        Request::Buy {
                            code,
                            shares,
                            price,
                            note,
                        } => finish(
                            &event_tx,
                            app::buy(&code, shares, price, note.as_deref())
                                .map(|()| format!("已记录买入 {code} {shares:.2} 份")),
                        ),
                        Request::Sell {
                            code,
                            shares,
                            price,
                        } => finish(
                            &event_tx,
                            app::sell(&code, shares, price)
                                .map(|()| format!("已记录卖出 {code} {shares:.2} 份")),
                        ),
                        Request::AgentBrief { refresh } => {
                            let _ = event_tx.send(Event::Brief(agent_brief(refresh)));
                            let _ = event_tx.send(Event::Loaded(load()));
                        }
                        Request::SaveDecision { body } => match save_decision(&body) {
                            Ok(path) => {
                                let _ = event_tx
                                    .send(Event::Saved(format!("决策已存档：{}", path.display())));
                            }
                            Err(error) => {
                                let _ = event_tx.send(Event::Failed(error));
                            }
                        },
                        Request::LoadConversations => {
                            let _ = event_tx.send(Event::Conversations(conversations::list()));
                        }
                        Request::SaveConversation(record) => match conversations::save(&record) {
                            Ok(()) => {
                                let _ = event_tx.send(Event::Conversations(conversations::list()));
                            }
                            Err(error) => {
                                let _ = event_tx.send(Event::ConversationNotice(Err(error)));
                            }
                        },
                        Request::LoadConversation { id } => {
                            let result = conversations::load(&id).map(Box::new);
                            let _ = event_tx.send(Event::ConversationOpened(result));
                        }
                        Request::DeleteConversation { id } => match conversations::delete(&id) {
                            Ok(()) => {
                                let _ = event_tx.send(Event::ConversationNotice(Ok(
                                    "已删除这条会话记录".to_owned(),
                                )));
                                let _ = event_tx.send(Event::Conversations(conversations::list()));
                            }
                            Err(error) => {
                                let _ = event_tx.send(Event::ConversationNotice(Err(error)));
                            }
                        },
                    }
                }
            })
            .expect("spawn app worker");

        (Self { tx: request_tx }, event_rx)
    }

    pub fn send(&self, request: Request) -> Result<(), String> {
        self.tx
            .send(request)
            .map_err(|_| "后台线程已退出，请重启应用".to_owned())
    }
}

/// Every successful write reloads the snapshot so no page can keep showing a
/// cash balance that an operation already changed.
fn finish(events: &UnboundedSender<Event>, result: anyhow::Result<String>) {
    match result {
        Ok(message) => {
            let _ = events.send(Event::Saved(message));
            let _ = events.send(Event::Loaded(load()));
        }
        Err(error) => {
            let _ = events.send(Event::Failed(format!("{error:#}")));
        }
    }
}

fn load() -> Result<Snapshot, String> {
    app::load_snapshot().map_err(|error| format!("{error:#}"))
}

/// Append-only archive of agent conclusions, one file per run.
///
/// Keeping the raw JSON (not a re-serialised struct) means a newer agent can add
/// fields without this build silently dropping them.
fn save_decision(body: &str) -> Result<std::path::PathBuf, String> {
    let dir = crate::agent::settings_dir().join("agent").join("runs");
    std::fs::create_dir_all(&dir).map_err(|error| format!("无法创建存档目录：{error}"))?;
    let name = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let path = dir.join(format!("{name}.json"));
    std::fs::write(&path, body).map_err(|error| format!("无法写入决策存档：{error}"))?;
    Ok(path)
}

/// Run the CLI for the report text that opens a review.
///
/// The shell deliberately does not re-implement report generation: the text the
/// agent judges must be byte-identical to what the user sees in a terminal.
fn agent_brief(refresh: bool) -> Result<String, String> {
    let cli = crate::agent::cli_path().ok_or_else(|| {
        "未找到 mns CLI：请安装 MNS.app，或设置 MNS_BIN 指向可执行文件".to_owned()
    })?;

    let args: &[&str] = if refresh {
        &["today", "--quiet"]
    } else {
        &["report"]
    };
    let data_dir = crate::config::app_data_dir();
    let _ = std::fs::create_dir_all(&data_dir);
    let output = std::process::Command::new(&cli)
        .args(args)
        // 显式指定，不依赖父进程 cwd：GUI 可能从 Finder 启动。
        .current_dir(&data_dir)
        .output()
        .map_err(|error| format!("无法运行 mns：{error}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        return Err(if stderr.is_empty() {
            format!("mns {} 失败（退出码 {:?}）", args[0], output.status.code())
        } else {
            format!("mns {} 失败：{stderr}", args[0])
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
