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
use crate::{discipline, guardrail, metrics, quote, sentiment, signal, strategy};

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

/// 决策日再平衡结论（`mns report` 决策版同源）。与 `PlanView`（月度定投，
/// 只买不卖）是两个语义：这里允许卖出来回到目标带。
#[derive(Clone, Debug)]
pub struct RebalanceView {
    pub score: f64,
    pub target_risk_pct: f64,
    pub current_risk_pct: f64,
    pub band_pp: f64,
    pub total_buy: f64,
    pub total_sell: f64,
    pub cash_constrained: bool,
    pub legs: Vec<LegView>,
}

impl RebalanceView {
    pub fn has_orders(&self) -> bool {
        self.total_buy >= 1.0 || self.total_sell >= 1.0
    }
}

/// `reports/` 里最新一份存档报告，供 GUI 只读回看。`date` 取自文件名。
#[derive(Clone, Debug)]
pub struct LatestReport {
    pub date: String,
    pub path: String,
    pub content: String,
}

/// F2 买入冷静期的过热上下文。`None` = 护栏关闭或未过热（不过热时是
/// `Some` 里的非 hot 判定，见 `load_snapshot` 的过滤）。
#[derive(Clone, Debug)]
pub struct GuardrailHot {
    pub effective_score: f64,
    pub hot_threshold: f64,
    pub cooldown_days: u32,
}

/// F2 买入冷静期判定结果（纯数据，供 GUI 渲染与 note 前缀推导）。
#[derive(Clone, Debug, Default)]
pub struct BuyGuardrail {
    /// (a) 基金不在计划条目里（定投计划 ∪ 再平衡计划都没有它）。
    pub out_of_plan: bool,
    /// (b) 金额 > 2×该基金计划买入额 且 > 最小交易额。
    pub oversized: bool,
    /// 该基金的计划买入额合计（(b) 的文案用；计划外时为 0）。
    pub planned_amount: f64,
    /// (c) 过热上下文；None = 没有过热（或护栏关闭）。
    pub hot: Option<GuardrailHot>,
}

impl BuyGuardrail {
    /// (a)/(b) 触发 → 理由必填；纯 (c) → 选填。
    pub fn reason_required(&self) -> bool {
        self.out_of_plan || self.oversized
    }

    /// 是否需要弹冷静期层（不阻断，只是先让用户写理由）。
    pub fn triggered(&self) -> bool {
        self.reason_required() || self.hot.is_some()
    }

    /// 落库 note 的结构化前缀（F3 按前缀分桶，不猜测）：
    ///   (a)/(b) → 「计划外冷静：{理由}」；
    ///   纯 (c) 写了理由 → 「过热冷静：{理由}」；
    ///   纯 (c) 没写理由 → 整条为 None（F3 归「未标记」——按计划执行
    ///   不该被表述成上头）。
    pub fn build_note(&self, reason: &str) -> Option<String> {
        let reason = reason.trim();
        if self.reason_required() {
            Some(format!("{}{reason}", discipline::OUT_OF_PLAN_PREFIX))
        } else if self.hot.is_some() && !reason.is_empty() {
            Some(format!("{}{reason}", discipline::HOT_CALM_PREFIX))
        } else {
            None
        }
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
    /// 决策日再平衡结论（与 `mns report` 决策版同源）。缺情绪历史时为
    /// None，UI 给空态，而不是用 50 分伪造一个可执行的结论。
    pub rebalance: Option<RebalanceView>,
    /// 下一个决策日；`decision_weekday = 0`（每天决策）时为 None。
    pub next_decision_day: Option<NaiveDate>,
    /// `reports/` 最新一份存档报告，只读回看用。
    pub latest_report: Option<LatestReport>,
    /// F2 买入冷静期的过热上下文。None = 护栏关闭或未过热。
    pub guardrail_hot: Option<GuardrailHot>,
    pub fgi_history: Vec<(NaiveDate, f64)>,
    pub flows: Vec<FlowRow>,
    pub trades: Vec<TradeRow>,
    /// 本年交易纪律统计（按 note 前缀分桶，不推断未标记的意图）。
    pub discipline: discipline::YearReview,
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
                legs: leg_views(&plan.legs),
            }),
            None,
        )
    };

    // 本周结论的数据底座：决策日再平衡计划，与 `mns report` 决策版同源。
    // 只要本地有情绪历史就算得出（含非决策日的偏离上下文），UI 决定怎么
    // 呈现"操作/不操作"；缺数据时给 None + 空态。
    let rebalance = if fgi_history.is_empty() {
        None
    } else {
        let score = signal_view
            .as_ref()
            .map(|s| s.effective_score)
            .unwrap_or(50.0);
        let plan = strategy::calculate_rebalance_plan(&config, score, cash, &positions, today);
        Some(RebalanceView {
            score,
            target_risk_pct: plan.target_risk_weight,
            current_risk_pct: plan.current_risk_weight,
            band_pp: plan.band_pp,
            total_buy: plan.total_buy,
            total_sell: plan.total_sell,
            cash_constrained: plan.cash_constrained,
            legs: leg_views(&plan.legs),
        })
    };

    let next_decision_day = signal::next_decision_day(today, config.rebalance.decision_weekday);
    let latest_report = latest_archived_report(&config);

    // F2 过热上下文：只在真过热时给出（render 的非过热分支是 CLI 的
    // 「按既定计划执行」话术，对 GUI 冷静期层没有意义）。
    let guardrail_hot = signal_view.as_ref().and_then(|s| {
        guardrail::evaluate(&config, s.effective_score)
            .filter(|advice| advice.hot)
            .map(|advice| GuardrailHot {
                effective_score: s.effective_score,
                hot_threshold: config.guardrail.hot_threshold,
                cooldown_days: advice.cooldown_days,
            })
    });

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
        report_dir: config.report_dir().display().to_string(),
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

    // 纪律复盘按自然年统计「今年操作几次」，不能用上面那 12 条交易
    // （limit 是行数，不是时间窗）。日期解析失败的行跳过：审计链写入的
    // 都是 `YYYY-MM-DD`，无法归年的脏数据不该被算进任何一年。
    let year_txs: Vec<(NaiveDate, Option<String>)> = db
        .list_transactions_since(&format!("{}-01-01", today.year()))?
        .into_iter()
        .filter_map(|tx| {
            NaiveDate::parse_from_str(&tx.tx_date, "%Y-%m-%d")
                .ok()
                .map(|date| (date, tx.note))
        })
        .collect();
    let discipline = discipline::review(
        today.year(),
        config.backtest.annual_trade_budget_min,
        config.backtest.annual_trade_budget_max,
        year_txs,
    );

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
        rebalance,
        next_decision_day,
        latest_report,
        guardrail_hot,
        fgi_history,
        flows,
        trades,
        discipline,
        freshness,
        config: config_view,
        counts,
    })
}

/// `strategy::LegPlan` → `LegView`，定投计划与再平衡结论共用同一映射，
/// 两个视图的字段永远不漂移。
fn leg_views(legs: &[strategy::LegPlan]) -> Vec<LegView> {
    legs.iter()
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
        .collect()
}

/// `reports/` 目录里最新一份存档报告。文件名是 ISO 日期，字典序即时间序。
/// 目录不存在（还没跑过 report）与空目录同等对待，返回 None。
fn latest_archived_report(config: &AppConfig) -> Option<LatestReport> {
    latest_report_in_dir(&config.report_dir())
}

/// 独立出目录参数以便测试；不读 `~/.mns` 也能覆盖选取逻辑。
fn latest_report_in_dir(dir: &std::path::Path) -> Option<LatestReport> {
    let mut newest: Option<(String, std::path::PathBuf, String)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("txt") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // 半写坏的文件读不出来就跳过，不让一份坏文件挡住回看。
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if newest
            .as_ref()
            .map(|(name, _, _)| stem > name.as_str())
            .unwrap_or(true)
        {
            newest = Some((stem.to_owned(), path, content));
        }
    }
    let (date, path, content) = newest?;
    Some(LatestReport {
        date,
        path: path.display().to_string(),
        content,
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

pub fn buy(code: &str, shares: f64, price: f64, note: Option<&str>) -> Result<()> {
    let db = Database::open()?;
    db.buy_position(code.trim(), shares, price, note)
}

/// F2 买入冷静期判定（纯函数，不碰 IO，方便回归测试）。
///
/// 触发条件（满足其一即弹冷静期层；**不阻断交易**，理由写完照常落库）：
///   (a) 基金不在计划条目里——计划 = 定投计划 ∪ 再平衡计划的 items；
///   (b) 金额 > 2×该基金计划买入额 且 > 最小交易额（产品拍板的下限，
///       避免小额偏差也被表述成「计划外」）；
///   (c) 情绪过热（`guardrail::evaluate` 判 hot，阈值与开关同 CLI 单源）。
///
/// 降级规则：定投或再平衡计划**任一**缺失 → 跳过 (a)/(b)——腿计划金额
/// 来自两条链，缺任何一条时 2× 比较与「计划内」判定都不完整，宁可不判，
/// 绝不因数据缺失拦人（产品拍板）；过热上下文缺失 → 跳过 (c)。(c) 独立
/// 于计划数据，不受计划降级影响。
pub fn evaluate_buy_guardrail(
    code: &str,
    amount: f64,
    plan: Option<&PlanView>,
    rebalance: Option<&RebalanceView>,
    min_trade_amount: f64,
    hot: Option<GuardrailHot>,
) -> BuyGuardrail {
    let legs = plan
        .into_iter()
        .flat_map(|view| view.legs.iter())
        .chain(rebalance.into_iter().flat_map(|view| view.legs.iter()));
    let mut planned_amount = 0.0;
    let mut in_plan = false;
    for leg in legs {
        for item in &leg.items {
            if item.code == code {
                in_plan = true;
                planned_amount += item.amount.max(0.0);
            }
        }
    }
    let plans_complete = plan.is_some() && rebalance.is_some();
    BuyGuardrail {
        out_of_plan: plans_complete && !in_plan,
        oversized: plans_complete && amount > 2.0 * planned_amount && amount > min_trade_amount,
        planned_amount,
        hot,
    }
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
    let path = config
        .report_dir()
        .join(format!("{}.txt", Local::now().format("%Y-%m-%d")));
    path.exists().then_some(path)
}

pub fn decision_weekday_today() -> u32 {
    Local::now().weekday().num_days_from_monday() + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 独立临时目录，避免测试碰 `~/.mns`。
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("mns-app-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn 最新报告按文件名取最大日期() {
        let dir = temp_dir("latest-report");
        std::fs::write(dir.join("2026-11-06.txt"), "older").unwrap();
        std::fs::write(dir.join("2026-11-13.txt"), "newer").unwrap();
        std::fs::write(dir.join("2026-11-09.txt"), "middle").unwrap();
        std::fs::write(dir.join("notes.md"), "not a report").unwrap();
        let got = latest_report_in_dir(&dir);
        assert_eq!(got.as_ref().map(|r| r.date.as_str()), Some("2026-11-13"));
        assert_eq!(got.as_ref().map(|r| r.content.as_str()), Some("newer"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn 没有报告或目录不存在时返回空() {
        let dir = temp_dir("empty-report");
        assert!(latest_report_in_dir(&dir).is_none());
        assert!(
            latest_report_in_dir(&dir.join("does-not-exist")).is_none(),
            "目录不存在按无报告处理"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn 坏文件不挡住好文件() {
        let dir = temp_dir("bad-report");
        std::fs::write(dir.join("2026-11-13.txt"), "good").unwrap();
        // 目录伪装成 txt：读不出来必须被跳过
        std::fs::create_dir_all(dir.join("2026-12-01.txt")).unwrap();
        let got = latest_report_in_dir(&dir);
        assert_eq!(got.as_ref().map(|r| r.date.as_str()), Some("2026-11-13"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // ── F2 买入冷静期 ──

    /// 单腿计划：items 直接给 (代码, 计划买入额)。
    fn plan_view(items: Vec<(&str, f64)>) -> PlanView {
        let total: f64 = items.iter().map(|(_, a)| *a).sum();
        PlanView {
            monthly: 3000.0,
            score: 50.0,
            target_risk_pct: 84.0,
            current_risk_pct: 70.0,
            band_pp: 4.0,
            total_buy: total,
            total_sell: 0.0,
            estimated_fees: 0.0,
            cash_constrained: false,
            legs: vec![leg_view("A股", total, items)],
        }
    }

    fn rebalance_view(items: Vec<(&str, f64)>) -> RebalanceView {
        let total: f64 = items.iter().map(|(_, a)| *a).sum();
        RebalanceView {
            score: 50.0,
            target_risk_pct: 84.0,
            current_risk_pct: 70.0,
            band_pp: 4.0,
            total_buy: total,
            total_sell: 0.0,
            cash_constrained: false,
            legs: vec![leg_view("A股", total, items)],
        }
    }

    fn leg_view(category_cn: &str, amount: f64, items: Vec<(&str, f64)>) -> LegView {
        LegView {
            category_cn: category_cn.to_owned(),
            target_pct: 30.0,
            current_pct: 25.0,
            drift_pp: -5.0,
            amount,
            hold_reason: None,
            items: items
                .into_iter()
                .map(|(code, item_amount)| LegItemView {
                    code: code.to_owned(),
                    name: "基金".to_owned(),
                    amount: item_amount,
                    note: None,
                })
                .collect(),
        }
    }

    fn hot() -> Option<GuardrailHot> {
        Some(GuardrailHot {
            effective_score: 88.0,
            hot_threshold: 75.0,
            cooldown_days: 1,
        })
    }

    #[test]
    fn 计划外买入触发且理由必填() {
        let plan = plan_view(vec![("007605", 1000.0)]);
        let rebal = rebalance_view(vec![("007605", 500.0)]);
        let v = evaluate_buy_guardrail("005827", 800.0, Some(&plan), Some(&rebal), 100.0, None);
        assert!(v.out_of_plan, "基金不在任一计划条目里");
        // 计划外基金的计划买入额是 0，任何 > 最小交易额的买入同时满足 (b)——
        // (a)/(b) 重叠是预期行为，都归「计划外冷静」前缀，理由必填。
        assert!(v.oversized);
        assert_eq!(v.planned_amount, 0.0);
        assert!(v.reason_required());
        assert!(v.triggered());
        assert_eq!(
            v.build_note("临时看到估值低想加一笔").as_deref(),
            Some("计划外冷静：临时看到估值低想加一笔")
        );
    }

    #[test]
    fn 超过计划两倍且大于最小交易额才触发() {
        let plan = plan_view(vec![("007605", 1000.0)]);
        let rebal = rebalance_view(vec![("007605", 1000.0)]);
        // 计划买入额 = 1000 + 1000 = 2000，4100 > 2×2000 且 > 100：触发
        let v = evaluate_buy_guardrail("007605", 4100.0, Some(&plan), Some(&rebal), 100.0, None);
        assert!(!v.out_of_plan, "在计划条目里");
        assert!(v.oversized);
        assert!(v.reason_required());
        // 恰好两倍不算超额
        let v2 = evaluate_buy_guardrail("007605", 4000.0, Some(&plan), Some(&rebal), 100.0, None);
        assert!(!v2.oversized);
        assert!(!v2.triggered(), "按计划执行不该被表述成上头");
        // 超过两倍但低于最小交易额：不触发（产品拍板的下限）
        let small_plan = plan_view(vec![("007605", 60.0)]);
        let small_rebal = rebalance_view(vec![("007605", 60.0)]);
        let v3 = evaluate_buy_guardrail(
            "007605",
            300.0,
            Some(&small_plan),
            Some(&small_rebal),
            500.0,
            None,
        );
        assert!(!v3.oversized);
    }

    #[test]
    fn 纯过热理由选填且空理由不落前缀() {
        // (c) 独立于计划数据：即使计划链不齐，过热提示仍然生效。
        let plan = plan_view(vec![("007605", 1000.0)]);
        let v = evaluate_buy_guardrail("007605", 1000.0, Some(&plan), None, 100.0, hot());
        assert!(!v.reason_required(), "纯过热不该强制写理由");
        assert!(v.triggered());
        assert_eq!(v.build_note(""), None, "纯(c)空理由整条为空，F3 归未标记");
        assert_eq!(
            v.build_note("  按计划执行但分两批  ").as_deref(),
            Some("过热冷静：按计划执行但分两批")
        );
    }

    #[test]
    fn 计划外优先于过热前缀() {
        let plan = plan_view(vec![("007605", 1000.0)]);
        let rebal = rebalance_view(vec![("007605", 500.0)]);
        let v = evaluate_buy_guardrail("005827", 800.0, Some(&plan), Some(&rebal), 100.0, hot());
        assert!(v.out_of_plan && v.hot.is_some());
        assert_eq!(
            v.build_note("看到回调想买一笔").as_deref(),
            Some("计划外冷静：看到回调想买一笔")
        );
    }

    #[test]
    fn 计划任一链缺失时降级为不拦() {
        // 两份计划都缺：跳过 (a)/(b)，绝不在数据缺失时拦人
        let v = evaluate_buy_guardrail("005827", 900.0, None, None, 100.0, None);
        assert!(!v.triggered());
        // 缺任何一条也跳过：腿计划金额来自定投与再平衡两条链，缺一时
        // 2× 比较与「计划内」判定都不完整（产品拍板）
        let plan = plan_view(vec![("007605", 1000.0)]);
        let v2 = evaluate_buy_guardrail("005827", 900.0, Some(&plan), None, 100.0, None);
        assert!(!v2.triggered());
        let rebal = rebalance_view(vec![("007605", 1000.0)]);
        let v3 = evaluate_buy_guardrail("005827", 900.0, None, Some(&rebal), 100.0, None);
        assert!(!v3.triggered());
        // 计划齐备但不过热：(a)/(b) 正常判，计划内合规买入直接放行
        let rebal2 = rebalance_view(vec![("007605", 1000.0)]);
        let v4 = evaluate_buy_guardrail("007605", 1000.0, Some(&plan), Some(&rebal2), 100.0, None);
        assert!(!v4.triggered());
    }

    #[test]
    fn 计划判定用定投与再平衡的并集() {
        let plan = plan_view(vec![("007605", 1000.0)]);
        // 只出现在再平衡条目里也算计划内
        let rebal = rebalance_view(vec![("005827", 600.0)]);
        let v = evaluate_buy_guardrail("005827", 600.0, Some(&plan), Some(&rebal), 100.0, None);
        assert!(!v.out_of_plan);
        assert!(!v.triggered());
        // (b) 的基数 = 两份计划里该基金计划买入额之和
        let rebal2 = rebalance_view(vec![("007605", 800.0)]);
        let v2 = evaluate_buy_guardrail("007605", 4000.0, Some(&plan), Some(&rebal2), 100.0, None);
        assert!(v2.oversized, "4000 > 2×(1000+800)=3600");
        assert_eq!(v2.planned_amount, 1800.0);
    }
}
