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
    /// 启动补更新：当天已过设定时间且今天还没有落过快照时，执行一次
    /// `refresh_all`；否则什么都不做（判断放在 worker，避免 UI 线程读库）。
    CatchUpRefresh {
        target: String,
    },
    SetProfile(String),
    SetMonthly(f64),
    SetWeekday(u32),
    /// 管家主动开口频控（设置页）。
    SetButlerPolicy {
        daily_cap: usize,
        weekly_cap: usize,
        dedupe_days: i64,
        quiet_start_hour: u32,
        quiet_end_hour: u32,
    },
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
    /// F5 标的搜索。关键词送网络的事放在 worker，UI 线程绝不发请求。
    SearchFunds {
        keyword: String,
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
    /// 首次启动补写了默认配置（仅一次）。
    Initialized,
    Refreshed(Result<RefreshOutcome, String>),
    /// A write succeeded; the string is user-facing confirmation copy.
    Saved(String),
    /// 一次记账成功并已写入行为日志。带毫秒时间戳，作为该事件的去重 id。
    LedgerRecorded(String),
    Failed(String),
    /// Report text for the agent prompt, produced by the CLI itself.
    Brief(Result<String, String>),
    /// F5 搜索结果。`keyword` 原样带回：慢响应不能覆盖后一次搜索的结果。
    FundSearch {
        keyword: String,
        result: Result<Vec<crate::quote::FundCandidate>, String>,
    },
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
                            // 新用户没有 `~/.mns/config.toml`：先按 `mns init`
                            // 的默认结果补齐，避免首屏只显示一条“缺少配置”。
                            match app::ensure_initialized() {
                                Ok(initialized) => {
                                    if initialized {
                                        let _ = event_tx.send(Event::Initialized);
                                    }
                                    let _ = event_tx.send(Event::Loaded(load()));
                                }
                                Err(error) => {
                                    let _ = event_tx.send(Event::Loaded(Err(format!(
                                        "无法初始化本地数据：{error:#}"
                                    ))));
                                }
                            }
                        }
                        Request::Refresh => {
                            let result = runtime
                                .block_on(app::refresh_all())
                                .map_err(|error| format!("{error:#}"));
                            let _ = event_tx.send(Event::Refreshed(result));
                            let _ = event_tx.send(Event::Loaded(load()));
                        }
                        Request::CatchUpRefresh { target } => {
                            let passed =
                                chrono::Local::now().format("%H:%M").to_string().as_str()
                                    >= target.as_str();
                            // 读失败（例如还没有数据库）时保持沉默：宁可不补，
                            // 也不在每次启动时重复一次无意义的网络请求。
                            let updated = app::has_snapshot_today().unwrap_or(true);
                            if passed && !updated {
                                let result = runtime
                                    .block_on(app::refresh_all())
                                    .map_err(|error| format!("{error:#}"));
                                let _ = event_tx.send(Event::Refreshed(result));
                                let _ = event_tx.send(Event::Loaded(load()));
                            }
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
                        Request::SetButlerPolicy {
                            daily_cap,
                            weekly_cap,
                            dedupe_days,
                            quiet_start_hour,
                            quiet_end_hour,
                        } => finish(
                            &event_tx,
                            app::set_butler_policy(
                                daily_cap,
                                weekly_cap,
                                dedupe_days,
                                quiet_start_hour,
                                quiet_end_hour,
                            )
                            .map(|()| "管家频控已保存".to_owned()),
                        ),
                        Request::Deposit { amount, note } => {
                            let result = app::deposit(amount, note.as_deref());
                            let ledger = result
                                .is_ok()
                                .then(|| {
                                    record_ledger(
                                        "deposit",
                                        serde_json::json!({ "amount": amount }),
                                    )
                                })
                                .flatten();
                            finish(
                                &event_tx,
                                result.map(|balance| format!("已入金，现金余额 ¥{balance:.2}")),
                            );
                            if let Some(at) = ledger {
                                let _ = event_tx.send(Event::LedgerRecorded(at));
                            }
                        }
                        Request::Withdraw { amount, note } => {
                            let result = app::withdraw(amount, note.as_deref());
                            let ledger = result
                                .is_ok()
                                .then(|| {
                                    record_ledger(
                                        "withdraw",
                                        serde_json::json!({ "amount": amount }),
                                    )
                                })
                                .flatten();
                            finish(
                                &event_tx,
                                result.map(|balance| format!("已出金，现金余额 ¥{balance:.2}")),
                            );
                            if let Some(at) = ledger {
                                let _ = event_tx.send(Event::LedgerRecorded(at));
                            }
                        }
                        Request::AddAsset {
                            code,
                            name,
                            category,
                        } => {
                            // 手填代码是一等入口，但口径和搜索入口一致：只收
                            // 场外基金。6 位基金代码先按代码查一次，既补齐名称
                            // 又确认它不是场内 ETF/LOF；查询失败时不阻断已写好
                            // 名称的录入，网络问题不该演变成「不能记账」。
                            let mut name = name;
                            if code.len() == 6 && code.chars().all(|c| c.is_ascii_digit()) {
                                match runtime.block_on(crate::quote::search_funds(&code)) {
                                    Ok(rows) => {
                                        if let Some(found) =
                                            rows.iter().find(|row| row.code == code)
                                        {
                                            if found.exchange_traded {
                                                let _ = event_tx.send(Event::Failed(format!(
                                                    "{} 是场内标的（ETF/LOF），MNS 只记场外基金；请添加它对应的场外联接基金",
                                                    code
                                                )));
                                                continue;
                                            }
                                            if name.trim().is_empty() {
                                                name = found.name.clone();
                                            }
                                        } else if name.trim().is_empty() {
                                            let _ = event_tx.send(Event::Failed(format!(
                                                "未能获取 {code} 的名称，请手动填写"
                                            )));
                                            continue;
                                        }
                                    }
                                    Err(error) => {
                                        if name.trim().is_empty() {
                                            let _ = event_tx.send(Event::Failed(format!(
                                                "未能获取 {code} 的名称：{error:#}"
                                            )));
                                            continue;
                                        }
                                    }
                                }
                            }
                            finish(
                                &event_tx,
                                app::add_asset(&code, &name, &category)
                                    .map(|()| format!("已添加标的 {code} {name}")),
                            );
                        }
                        Request::SearchFunds { keyword } => {
                            let result = runtime
                                .block_on(crate::quote::search_funds(&keyword))
                                .map_err(|error| format!("{error:#}"));
                            let _ = event_tx.send(Event::FundSearch { keyword, result });
                        }
                        Request::Buy {
                            code,
                            shares,
                            price,
                            note,
                        } => {
                            let result = app::buy(&code, shares, price, note.as_deref());
                            let ledger = result
                                .is_ok()
                                .then(|| {
                                    record_ledger(
                                        "buy",
                                        serde_json::json!({
                                            "code": code,
                                            "shares": shares,
                                            "price": price,
                                        }),
                                    )
                                })
                                .flatten();
                            finish(
                                &event_tx,
                                result.map(|()| format!("已记录买入 {code} {shares:.2} 份")),
                            );
                            if let Some(at) = ledger {
                                let _ = event_tx.send(Event::LedgerRecorded(at));
                            }
                        }
                        Request::Sell {
                            code,
                            shares,
                            price,
                        } => {
                            let result = app::sell(&code, shares, price);
                            let ledger = result
                                .is_ok()
                                .then(|| {
                                    record_ledger(
                                        "sell",
                                        serde_json::json!({
                                            "code": code,
                                            "shares": shares,
                                            "price": price,
                                        }),
                                    )
                                })
                                .flatten();
                            finish(
                                &event_tx,
                                result.map(|()| format!("已记录卖出 {code} {shares:.2} 份")),
                            );
                            if let Some(at) = ledger {
                                let _ = event_tx.send(Event::LedgerRecorded(at));
                            }
                        }
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

/// 账本事件写入行为日志，供管家唤醒。失败忽略：记账已经成功，埋点不该
/// 反过来让用户看到报错。返回唤醒 id，供去重。
///
/// id 一律由 `orchestrator::ledger_wake` 生成，GUI 不自己拼字符串——两处
/// 各拼一遍，改规则时必然漂移，去重就失效了。
fn record_ledger(side: &str, detail: serde_json::Value) -> Option<String> {
    let mut detail = detail;
    if let Some(map) = detail.as_object_mut() {
        map.insert(
            "side".to_owned(),
            serde_json::Value::String(side.to_owned()),
        );
    }
    let event = crate::butler::events::BehaviorEvent::new(
        crate::butler::events::kind::TRADE_RECORDED,
        detail,
    );
    let wake = crate::butler::orchestrator::ledger_wake(&event);
    crate::butler::events::append(&event).ok()?;
    match wake {
        Some(crate::butler::WakeSource::Ledger { id }) => Some(id),
        _ => None,
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
    let output = super::command(&cli)
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
