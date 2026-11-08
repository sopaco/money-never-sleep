//! Typed application services shared by the CLI and the desktop shell.
//!
//! Everything here is presentation-free: no terminal strings, no table
//! rendering, no `println!`. The desktop shell calls these functions and draws
//! the result; the CLI keeps its own formatting. Investment rules are not
//! duplicated — allocation, signals, fees and metrics all come from the sibling
//! engine modules.

use anyhow::{Context, Result};
use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, Utc};

use crate::config::{AppConfig, RiskProfile};
use crate::db::Database;
use crate::models::Position;
use crate::{metrics, quote, sentiment, signal, strategy};

/// Category keys in the order the product presents them.
pub const CATEGORY_KEYS: [&str; 3] = ["us_stocks", "cn_stocks", "counter_cyclical"];

pub fn category_cn(key: &str) -> &'static str {
    match key {
        "us_stocks" => "美股",
        "cn_stocks" => "A股",
        "counter_cyclical" => "逆周期",
        _ => "其他",
    }
}

/// 0 = every day, 1..=7 = Monday..Sunday (same convention as `Rebalance`).
pub fn weekday_label(weekday: u32) -> &'static str {
    match weekday {
        0 => "每天",
        1 => "周一",
        2 => "周二",
        3 => "周三",
        4 => "周四",
        5 => "周五",
        6 => "周六",
        _ => "周日",
    }
}

// ───────────────────────────── Read models ─────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct Freshness {
    pub price_age_hours: Option<i64>,
    pub snapshot_date: Option<NaiveDate>,
    pub snapshot_age_days: Option<i64>,
    pub fgi_date: Option<NaiveDate>,
    pub fgi_age_days: Option<i64>,
}

impl Freshness {
    /// One honest sentence for the header, never a green checkmark the data
    /// does not support.
    pub fn summary(&self) -> String {
        let price = match self.price_age_hours {
            Some(h) if h <= 2 => "价格刚刚更新".to_owned(),
            Some(h) if h < 24 => format!("价格 {h} 小时前更新"),
            Some(h) => format!("价格 {} 天前更新", h / 24),
            None => "尚未获取价格".to_owned(),
        };
        let fgi = match self.fgi_age_days {
            Some(0) => "情绪数据为今日".to_owned(),
            Some(d) => format!("情绪数据 {d} 天前"),
            None => "尚无情绪数据".to_owned(),
        };
        format!("{price} · {fgi}")
    }

    pub fn price_is_stale(&self) -> bool {
        self.price_age_hours.map(|h| h >= 24).unwrap_or(true)
    }
}

#[derive(Clone, Debug)]
pub struct PositionRow {
    pub code: String,
    pub name: String,
    pub category_cn: String,
    pub shares: f64,
    pub cost_price: f64,
    pub price: Option<f64>,
    pub market_value: f64,
    pub pnl: Option<f64>,
    pub pnl_pct: Option<f64>,
    pub annualized: Option<f64>,
    pub weight_pct: f64,
    pub holding_days: Option<i64>,
    /// 该标的所在腿的目标权重（占总资产 %），用于和当前权重对照。
    pub target_pct: f64,
}

#[derive(Clone, Debug)]
pub struct CategoryRow {
    pub key: String,
    pub name: String,
    pub current_pct: f64,
    pub target_pct: f64,
    pub drift_pp: f64,
    pub market_value: f64,
}

#[derive(Clone, Debug)]
pub struct DrawdownView {
    pub max_dd: f64,
    pub peak_date: NaiveDate,
    pub peak_value: f64,
    pub trough_date: NaiveDate,
    pub trough_value: f64,
    pub all_time_peak: f64,
    pub all_time_peak_date: NaiveDate,
    pub current_value: Option<f64>,
}

impl DrawdownView {
    pub fn recovery_note(&self) -> String {
        match self.current_value {
            Some(v) if self.all_time_peak > 0.0 && v < self.all_time_peak => format!(
                "当前距峰值 {:.1}%（峰值 ¥{:.0}）",
                (self.all_time_peak - v) / self.all_time_peak * 100.0,
                self.all_time_peak
            ),
            Some(_) => "当前处于全期峰值".to_owned(),
            None => "暂无当日快照".to_owned(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SignalView {
    pub raw_score: f64,
    pub raw_zone: String,
    pub smoothed_score: f64,
    pub effective_score: f64,
    pub effective_zone: String,
    pub confirmed: bool,
    pub decision_day: bool,
    pub report_kind: String,
    pub samples_used: usize,
    pub note: String,
}

#[derive(Clone, Debug)]
pub struct LegView {
    pub category_cn: String,
    pub target_pct: f64,
    pub current_pct: f64,
    pub drift_pp: f64,
    pub amount: f64,
    pub hold_reason: Option<String>,
    pub items: Vec<LegItemView>,
}

#[derive(Clone, Debug)]
pub struct LegItemView {
    pub code: String,
    pub name: String,
    pub amount: f64,
    pub note: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PlanView {
    pub monthly: f64,
    pub score: f64,
    pub target_risk_pct: f64,
    pub current_risk_pct: f64,
    pub band_pp: f64,
    pub total_buy: f64,
    pub total_sell: f64,
    pub estimated_fees: f64,
    pub cash_constrained: bool,
    pub legs: Vec<LegView>,
}

impl PlanView {
    pub fn has_orders(&self) -> bool {
        self.legs.iter().any(|leg| leg.amount.abs() >= 1.0)
    }
}

#[derive(Clone, Debug)]
pub struct FlowRow {
    pub date: NaiveDate,
    pub amount: f64,
    pub kind: String,
    pub note: Option<String>,
}

impl FlowRow {
    pub fn kind_cn(&self) -> &str {
        match self.kind.as_str() {
            "deposit" => "入金",
            "withdraw" => "出金",
            "opening" => "期初",
            "adjust" => "校准",
            other => other,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TradeRow {
    pub date: String,
    pub side: String,
    pub code: String,
    pub shares: f64,
    pub price: f64,
    pub amount: f64,
}

impl TradeRow {
    pub fn side_cn(&self) -> &str {
        if self.side == "buy" {
            "买入"
        } else {
            "卖出"
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProfileOption {
    pub id: String,
    pub label: String,
    pub description: String,
    pub active: bool,
}

#[derive(Clone, Debug)]
pub struct ConfigView {
    pub profile_id: String,
    pub profile_label: String,
    pub profiles: Vec<ProfileOption>,
    pub monthly: f64,
    pub band_pp: f64,
    pub min_trade_amount: f64,
    pub min_holding_days_for_sell: i64,
    pub decision_weekday: u32,
    pub decision_label: String,
    pub neutral_weight: f64,
    pub annualized_target_high: f64,
    pub min_holding_days: i64,
    pub cash_annual_yield: f64,
    pub buy_fee_pct: f64,
    pub report_dir: String,
    pub db_path: String,
    pub config_path: String,
}

#[derive(Clone, Debug, Default)]
pub struct Counts {
    pub positions: usize,
    pub snapshots: usize,
    pub trades: usize,
    pub flows: usize,
}

/// One complete view of the account. The shell loads it off the UI thread and
/// renders every page from it, so two pages can never disagree about cash.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub as_of: NaiveDate,
    pub cash: f64,
    pub holdings_value: f64,
    pub total_assets: f64,
    pub net_inflow: f64,
    pub xirr: Option<f64>,
    pub tracking_days: i64,
    pub positions: Vec<PositionRow>,
    pub categories: Vec<CategoryRow>,
    pub series: Vec<(NaiveDate, f64)>,
    pub drawdown: Option<DrawdownView>,
    pub signal: Option<SignalView>,
    pub plan: Option<PlanView>,
    pub plan_blocked: Option<String>,
    pub fgi_history: Vec<(NaiveDate, f64)>,
    pub flows: Vec<FlowRow>,
    pub trades: Vec<TradeRow>,
    pub freshness: Freshness,
    pub config: ConfigView,
    pub counts: Counts,
}

impl Snapshot {
    pub fn holding_pnl(&self) -> f64 {
        self.positions.iter().filter_map(|p| p.pnl).sum()
    }

    pub fn risk_weight_pct(&self) -> f64 {
        if self.total_assets <= 0.0 {
            0.0
        } else {
            self.holdings_value / self.total_assets * 100.0
        }
    }
}

// ───────────────────────────── Loading ─────────────────────────────

pub fn load_snapshot() -> Result<Snapshot> {
    let config = AppConfig::load()?;
    let db = Database::open()?;
    let today = Local::now().date_naive();

    let positions = db.list_positions()?;
    let cash = db.get_cash_balance()?;
    let holdings_value: f64 = positions.iter().map(Position::market_value_or_cost).sum();
    let total_assets = cash + holdings_value;

    // Cash-flow ledger → XIRR. Both are the same call the CLI uses.
    let flows_raw = db.all_cash_flows()?;
    let ledger: Vec<(NaiveDate, f64)> = flows_raw.iter().map(|f| (f.flow_date, f.amount)).collect();
    let net_inflow: f64 = flows_raw.iter().map(|f| f.amount).sum();
    let xirr = if flows_raw.is_empty() {
        None
    } else {
        metrics::xirr(&metrics::xirr_flows_from_ledger(
            &ledger,
            total_assets,
            today,
        ))
    };
    let tracking_days = flows_raw
        .first()
        .map(|f| (today - f.flow_date).num_days())
        .unwrap_or(0);

    let series = db.portfolio_snapshot_series()?;
    let drawdown = metrics::drawdown_summary(&series).map(|s| DrawdownView {
        max_dd: s.max_dd,
        peak_date: s.peak_date,
        peak_value: s.peak_value,
        trough_date: s.trough_date,
        trough_value: s.trough_value,
        all_time_peak: s.all_time_peak,
        all_time_peak_date: s.all_time_peak_date,
        current_value: series.last().map(|(_, v)| *v),
    });

    // Signal requires a stored fear & greed history. Missing data is a normal
    // first-run state, not an error.
    let fgi_history = db.recent_fear_greed_scores(90)?;
    let signal_view = fgi_history.last().map(|(_, latest)| {
        let decision = signal::compute(&config, *latest, &fgi_history, today);
        SignalView {
            raw_score: decision.raw_score,
            raw_zone: decision.raw_zone,
            smoothed_score: decision.smoothed_score,
            effective_score: decision.effective_score,
            effective_zone: decision.effective_zone,
            confirmed: decision.confirmed,
            decision_day: decision.decision_day,
            report_kind: decision.report_kind.to_owned(),
            samples_used: decision.samples_used,
            note: decision.note,
        }
    });

    // Position weights need a target to compare against; the neutral score is
    // used when no signal exists so the page still explains the intended mix.
    let score_for_targets = signal_view
        .as_ref()
        .map(|s| s.effective_score)
        .unwrap_or(50.0);
    let (tu, tc, tcc) = config.asset_target_weights(score_for_targets);
    let target_by_key = [
        ("us_stocks", tu * 100.0),
        ("cn_stocks", tc * 100.0),
        ("counter_cyclical", tcc * 100.0),
    ];

    let mut categories = Vec::new();
    for (key, target_pct) in target_by_key {
        let market_value: f64 = positions
            .iter()
            .filter(|p| p.category == key)
            .map(Position::market_value_or_cost)
            .sum();
        let current_pct = if total_assets > 0.0 {
            market_value / total_assets * 100.0
        } else {
            0.0
        };
        categories.push(CategoryRow {
            key: key.to_owned(),
            name: category_cn(key).to_owned(),
            current_pct,
            target_pct,
            drift_pp: current_pct - target_pct,
            market_value,
        });
    }

    let position_rows: Vec<PositionRow> = positions
        .iter()
        .map(|p| {
            let target_pct = target_by_key
                .iter()
                .find(|(key, _)| *key == p.category)
                .map(|(_, t)| *t)
                .unwrap_or(0.0);
            let market_value = p.market_value_or_cost();
            PositionRow {
                code: p.asset_code.clone(),
                name: p.asset_name.clone(),
                category_cn: category_cn(&p.category).to_owned(),
                shares: p.shares,
                cost_price: p.cost_price,
                price: p.current_price,
                market_value,
                pnl: p
                    .current_price
                    .map(|price| (price - p.cost_price) * p.shares),
                pnl_pct: p.absolute_return(),
                annualized: p
                    .annualized_return_with_min_days(&today, config.settings.min_holding_days),
                weight_pct: if total_assets > 0.0 {
                    market_value / total_assets * 100.0
                } else {
                    0.0
                },
                holding_days: NaiveDate::parse_from_str(&p.first_buy_date, "%Y-%m-%d")
                    .ok()
                    .map(|d| (today - d).num_days()),
                target_pct,
            }
        })
        .collect();

    // Plan: same function the CLI uses. A missing monthly amount or missing
    // sentiment history blocks it with an actionable message instead of a zero
    // plan that looks like "do nothing".
    let monthly = config.cashflow.monthly;
    let (plan, plan_blocked) = if monthly <= 0.0 {
        (
            None,
            Some("还没有设置每月定投金额，先在「设置」里填一个数字。".to_owned()),
        )
    } else if fgi_history.is_empty() {
        (
            None,
            Some("本地还没有情绪数据，先点「更新数据」抓取一次。".to_owned()),
        )
    } else {
        let score = signal_view
            .as_ref()
            .map(|s| s.effective_score)
            .unwrap_or(50.0);
        let plan = strategy::calculate_monthly_plan(&config, score, cash, &positions, monthly);
        let legs = plan
            .legs
            .iter()
            .map(|leg| LegView {
                category_cn: leg.category_cn.clone(),
                target_pct: leg.target_weight,
                current_pct: leg.current_weight,
                drift_pp: leg.drift_pp,
                amount: leg.amount,
                hold_reason: leg.hold_reason.clone(),
                items: leg
                    .items
                    .iter()
                    .map(|item| LegItemView {
                        code: item.asset_code.clone(),
                        name: item.asset_name.clone(),
                        amount: item.amount,
                        note: item.note.clone(),
                    })
                    .collect(),
            })
            .collect();
        (
            Some(PlanView {
                monthly,
                score,
                target_risk_pct: plan.target_risk_weight,
                current_risk_pct: plan.current_risk_weight,
                band_pp: plan.band_pp,
                total_buy: plan.total_buy,
                total_sell: plan.total_sell,
                estimated_fees: plan.estimated_fees(&config.costs),
                cash_constrained: plan.cash_constrained,
                legs,
            }),
            None,
        )
    };

    let freshness = build_freshness(&db, &positions, &series, today)?;

    let profiles = RiskProfile::ALL
        .iter()
        .map(|profile| ProfileOption {
            id: profile.id().to_owned(),
            label: profile.label().to_owned(),
            description: if profile.core_blend() <= 0.0 {
                "曲线原样执行：择时摆幅最大，常驻仓位最低".to_owned()
            } else {
                format!(
                    "目标仓位向满仓收缩 {:.0}%：常驻仓位更高，回撤也更接近满仓",
                    profile.core_blend() * 100.0
                )
            },
            active: config.profile.name == profile.id(),
        })
        .collect();

    let config_view = ConfigView {
        profile_id: config.profile.name.clone(),
        profile_label: config.profile_label().to_owned(),
        profiles,
        monthly,
        band_pp: config.rebalance.band_pp,
        min_trade_amount: config.rebalance.min_trade_amount,
        min_holding_days_for_sell: config.rebalance.min_holding_days_for_sell,
        decision_weekday: config.rebalance.decision_weekday,
        decision_label: weekday_label(config.rebalance.decision_weekday).to_owned(),
        neutral_weight: config.effective_neutral_weight(),
        annualized_target_high: config.settings.annualized_target_high,
        min_holding_days: config.settings.min_holding_days,
        cash_annual_yield: config.costs.cash_annual_yield,
        buy_fee_pct: config.costs.buy_fee_pct,
        report_dir: config.settings.report_output_dir.clone(),
        db_path: AppConfig::db_path()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        config_path: AppConfig::config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    };

    let flows: Vec<FlowRow> = db
        .list_cash_flows(12)?
        .into_iter()
        .map(|f| FlowRow {
            date: f.flow_date,
            amount: f.amount,
            kind: f.kind,
            note: f.note,
        })
        .collect();

    let trades: Vec<TradeRow> = db
        .list_transactions(12)?
        .into_iter()
        .map(|t| TradeRow {
            date: t.tx_date,
            side: t.tx_type,
            code: t.asset_code,
            shares: t.shares,
            price: t.price,
            amount: t.amount,
        })
        .collect();

    let counts = Counts {
        positions: positions.len(),
        snapshots: series.len(),
        trades: trades.len(),
        flows: flows.len(),
    };

    Ok(Snapshot {
        as_of: today,
        cash,
        holdings_value,
        total_assets,
        net_inflow,
        xirr,
        tracking_days,
        positions: position_rows,
        categories,
        series,
        drawdown,
        signal: signal_view,
        plan,
        plan_blocked,
        fgi_history,
        flows,
        trades,
        freshness,
        config: config_view,
        counts,
    })
}

fn build_freshness(
    db: &Database,
    positions: &[Position],
    series: &[(NaiveDate, f64)],
    today: NaiveDate,
) -> Result<Freshness> {
    let newest = positions
        .iter()
        .filter_map(|p| parse_sqlite_ts(&p.updated_at))
        .max();
    let price_age_hours = newest.map(|ts| (Utc::now().naive_utc() - ts).num_hours().max(0));

    let snapshot_date = series.last().map(|(d, _)| *d);
    let snapshot_age_days = snapshot_date.map(|d| (today - d).num_days());

    let fgi_date = db.latest_fear_greed_snapshot()?.map(|s| s.date);
    let fgi_age_days = fgi_date.map(|d| (today - d).num_days());

    Ok(Freshness {
        price_age_hours,
        snapshot_date,
        snapshot_age_days,
        fgi_date,
        fgi_age_days,
    })
}

fn parse_sqlite_ts(raw: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S").ok()
}

// ───────────────────────────── Refresh ─────────────────────────────

#[derive(Clone, Debug)]
pub struct PriceChange {
    pub code: String,
    pub name: String,
    pub old_price: Option<f64>,
    pub new_price: f64,
    pub source: String,
}

#[derive(Clone, Debug, Default)]
pub struct RefreshOutcome {
    pub prices: Vec<PriceChange>,
    pub price_error: Option<String>,
    pub fgi_score: Option<f64>,
    pub fgi_error: Option<String>,
    pub snapshot_saved: bool,
    pub total_assets: f64,
}

impl RefreshOutcome {
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.prices.is_empty() {
            parts.push(format!("更新 {} 个价格", self.prices.len()));
        }
        if let Some(score) = self.fgi_score {
            parts.push(format!("情绪分数 {score:.0}"));
        }
        if self.snapshot_saved {
            parts.push("已保存今日快照".to_owned());
        }
        if parts.is_empty() {
            "没有可更新的内容".to_owned()
        } else {
            parts.join(" · ")
        }
    }
}

/// Refresh every remote input the decision needs, then persist today's total
/// assets so the performance curve has a point for today.
///
/// Failures are reported per source instead of aborting: a price outage must
/// not throw away a valid sentiment score, and vice versa.
pub async fn refresh_all() -> Result<RefreshOutcome> {
    let config = AppConfig::load()?;
    let db = Database::open()?;
    let mut outcome = RefreshOutcome::default();

    let positions = db.list_positions()?;
    if positions.is_empty() {
        outcome.price_error = Some("还没有持仓，跳过价格更新".to_owned());
    } else {
        match quote::update_all_prices(&positions).await {
            Ok(updates) => {
                if updates.is_empty() {
                    outcome.price_error = Some("没有获取到任何最新价格".to_owned());
                }
                for update in updates {
                    match db.update_price(&update.asset_code, update.new_price) {
                        Ok(()) => outcome.prices.push(PriceChange {
                            code: update.asset_code,
                            name: update.asset_name,
                            old_price: update.old_price,
                            new_price: update.new_price,
                            source: update.source,
                        }),
                        Err(error) => {
                            outcome.price_error =
                                Some(format!("{} 写入失败：{error}", update.asset_code));
                        }
                    }
                }
            }
            Err(error) => outcome.price_error = Some(error.to_string()),
        }
    }

    match sentiment::fetch_fear_greed_data(&config.api.fear_greed_url).await {
        Ok(data) => {
            let score = data.score as f64;
            let rating = config.sentiment_zone(score);
            db.save_fear_greed_snapshot(
                score,
                rating,
                data.previous_close,
                data.previous_1_week,
                data.previous_1_month,
                data.previous_1_year,
            )?;
            outcome.fgi_score = Some(score);
        }
        Err(error) => outcome.fgi_error = Some(error.to_string()),
    }

    let positions = db.list_positions()?;
    let cash = db.get_cash_balance()?;
    let holdings: f64 = positions.iter().map(Position::market_value_or_cost).sum();
    let total = cash + holdings;
    db.upsert_portfolio_snapshot(
        Local::now().date_naive(),
        cash,
        holdings,
        total,
        outcome.fgi_score,
    )
    .context("保存今日资产快照失败")?;
    outcome.snapshot_saved = true;
    outcome.total_assets = total;

    Ok(outcome)
}

// ───────────────────────────── Writes ─────────────────────────────

pub fn set_profile(id: &str) -> Result<()> {
    let profile = RiskProfile::parse(id)
        .with_context(|| format!("未知风险档位：{id}（可选 none/steady/balanced/aggressive）"))?;
    let mut config = AppConfig::load()?;
    config.apply_profile(profile);
    config.save()
}

pub fn set_monthly(amount: f64) -> Result<()> {
    if !amount.is_finite() || amount < 0.0 {
        anyhow::bail!("每月定投金额必须是非负数字");
    }
    let mut config = AppConfig::load()?;
    config.cashflow.monthly = amount;
    config.save()
}

pub fn set_decision_weekday(weekday: u32) -> Result<()> {
    if weekday > 7 {
        anyhow::bail!("决策日必须是 0（每天）或 1..=7（周一..周日）");
    }
    let mut config = AppConfig::load()?;
    config.rebalance.decision_weekday = weekday;
    config.save()
}

pub fn deposit(amount: f64, note: Option<&str>) -> Result<f64> {
    if amount <= 0.0 {
        anyhow::bail!("入金金额必须为正数");
    }
    let db = Database::open()?;
    let balance = db.add_cash(amount)?;
    db.add_cash_flow(Local::now().date_naive(), amount, "deposit", note)?;
    Ok(balance)
}

pub fn withdraw(amount: f64, note: Option<&str>) -> Result<f64> {
    if amount <= 0.0 {
        anyhow::bail!("出金金额必须为正数");
    }
    let db = Database::open()?;
    let balance = db.get_cash_balance()?;
    if amount > balance + 0.005 {
        anyhow::bail!("出金超过现金余额（当前 ¥{balance:.2}）。若资金在持仓里，请先卖出一笔再取。");
    }
    db.set_cash_balance(balance - amount)?;
    db.add_cash_flow(Local::now().date_naive(), -amount, "withdraw", note)?;
    Ok(balance - amount)
}

pub fn add_asset(code: &str, name: &str, category: &str) -> Result<()> {
    let code = code.trim();
    let name = name.trim();
    if code.is_empty() || name.is_empty() {
        anyhow::bail!("资产代码与名称都不能为空");
    }
    if !CATEGORY_KEYS.contains(&category) {
        anyhow::bail!("分类必须是 us_stocks / cn_stocks / counter_cyclical");
    }
    let db = Database::open()?;
    db.add_position(code, name, category)
}

pub fn buy(code: &str, shares: f64, price: f64) -> Result<()> {
    let db = Database::open()?;
    db.buy_position(code.trim(), shares, price)
}

pub fn sell(code: &str, shares: f64, price: f64) -> Result<()> {
    let db = Database::open()?;
    db.sell_position(code.trim(), shares, price)
}

/// Whether the last stored snapshot is today's, used to avoid writing a second
/// point when the user only wants to look.
pub fn has_snapshot_today() -> Result<bool> {
    let db = Database::open()?;
    Ok(db
        .portfolio_snapshot_series()?
        .last()
        .map(|(d, _)| *d == Local::now().date_naive())
        .unwrap_or(false))
}

/// Today's report path if a report already exists, so the shell can offer
/// "打开今天已生成的报告" instead of regenerating it.
pub fn today_report_path() -> Option<std::path::PathBuf> {
    let config = AppConfig::load().ok()?;
    let path = std::path::Path::new(&config.settings.report_output_dir)
        .join(format!("{}.txt", Local::now().format("%Y-%m-%d")));
    path.exists().then_some(path)
}

pub fn decision_weekday_today() -> u32 {
    Local::now().weekday().num_days_from_monday() + 1
}
