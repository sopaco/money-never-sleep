//! Background worker for application services.
//!
//! SQLite reads, network refreshes and file writes must never run on the GPUI
//! main thread: a 30-second price refresh would freeze the window. Requests go
//! to one worker thread that owns a Tokio runtime; results come back as typed
//! events the shell folds into its state.

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::app::{self, RefreshOutcome, Snapshot};

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
    },
    Sell {
        code: String,
        shares: f64,
        price: f64,
    },
}

pub enum Event {
    Loaded(Result<Snapshot, String>),
    Refreshed(Result<RefreshOutcome, String>),
    /// A write succeeded; the string is user-facing confirmation copy.
    Saved(String),
    Failed(String),
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
                        } => finish(
                            &event_tx,
                            app::buy(&code, shares, price)
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
