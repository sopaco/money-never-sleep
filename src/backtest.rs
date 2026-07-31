//! 回测引擎。
//!
//! 设计要点（相对旧版的关键修正）：
//!
//! 1. **统一引擎**：目标仓位框架、旧比例框架、买入持有基准全部走同一份数据、
//!    同一套成本模型。旧版三者实现各自独立，基准还硬编码了与被对比策略不同的
//!    资产配置（70/15/15 vs 55/25/20），导致对比不公平。
//! 2. **真实成本**：买入费、按持有天数的阶梯赎回费（FIFO 分批计费）、闲置现金
//!    货币基金收益。旧版三者皆无，系统性高估了高频调仓策略。
//! 3. **XIRR 口径**：存在分批注资时 `(期末/总投入)^(1/年数)` 会忽略资金到账时点，
//!    改用现金流加权收益率。
//! 4. **数据**：使用 `monthly_total_return.csv`（真实全收益序列，见 build_dataset.py）。
//!    旧 `monthly_real_final.csv` 中两条腿为人工估填且存在拼接断点。

use crate::config::AppConfig;
use crate::metrics::{self, CashFlow, RiskMetrics};
use crate::models::Position;
use crate::strategy::{calculate_buy_suggestions, calculate_sell_suggestions, check_risk_warnings};
use chrono::{Datelike, NaiveDate};
use comfy_table::{Cell, Color, Table, modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL};
use std::collections::HashMap;

/// 真实全收益数据集（人民币计价，含分红，无拼接断点）
const DATASET: &str =
    include_str!("../.agents/skills/mns-backtest/data/monthly_total_return.csv");

/// 样本外 holdout 区块（CNN 近端 FGI，与主序列不连续，单独回测）
const DATASET_HOLDOUT: &str =
    include_str!("../.agents/skills/mns-backtest/data/monthly_total_return_holdout.csv");

pub const LEG_NAMES: [&str; 3] = ["美股(纳指QDII)", "A股(红利)", "黄金"];
pub const LEG_CODES: [&str; 3] = ["US", "CN", "GOLD"];
pub const LEG_CATEGORIES: [&str; 3] = ["us_stocks", "cn_stocks", "counter_cyclical"];

// ───────────────────────── 数据 ─────────────────────────

#[derive(Debug, Clone)]
pub struct MonthRow {
    pub date: NaiveDate,
    pub fgi: f64,
    pub prices: [f64; 3],
}

fn month_end(year: i32, month: u32) -> Option<NaiveDate> {
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    };
    next?.pred_opt()
}

pub fn parse_dataset(data: &str) -> Vec<MonthRow> {
    let mut out = Vec::new();
    for line in data.lines().skip(1) {
        let p: Vec<&str> = line.split(',').collect();
        if p.len() < 5 {
            continue;
        }
        let ym: Vec<&str> = p[0].split('-').collect();
        if ym.len() != 2 {
            continue;
        }
        let (Ok(y), Ok(m)) = (ym[0].parse::<i32>(), ym[1].parse::<u32>()) else {
            continue;
        };
        let Some(date) = month_end(y, m) else { continue };
        let Ok(fgi) = p[1].parse::<f64>() else { continue };
        let mut prices = [0.0; 3];
        let mut ok = true;
        for i in 0..3 {
            match p[2 + i].parse::<f64>() {
                Ok(v) if v > 0.0 => prices[i] = v,
                _ => ok = false,
            }
        }
        if ok {
            out.push(MonthRow { date, fgi, prices });
        }
    }
    out.sort_by_key(|r| r.date);
    out
}

pub fn load_main() -> Vec<MonthRow> {
    parse_dataset(DATASET)
}

pub fn load_holdout() -> Vec<MonthRow> {
    parse_dataset(DATASET_HOLDOUT)
}

// ───────────────────────── 持仓（FIFO 分批） ─────────────────────────

#[derive(Debug, Clone)]
struct Lot {
    shares: f64,
    price: f64,
    date: NaiveDate,
}

#[derive(Debug, Clone, Default)]
struct Leg {
    lots: Vec<Lot>, // 按买入时间升序，FIFO
}

impl Leg {
    fn shares(&self) -> f64 {
        self.lots.iter().map(|l| l.shares).sum()
    }

    fn value(&self, price: f64) -> f64 {
        self.shares() * price
    }

    fn cost_basis(&self) -> f64 {
        self.lots.iter().map(|l| l.shares * l.price).sum()
    }

    fn avg_cost(&self) -> f64 {
        let s = self.shares();
        if s > 0.0 { self.cost_basis() / s } else { 0.0 }
    }

    fn first_buy(&self) -> Option<NaiveDate> {
        self.lots.first().map(|l| l.date)
    }

    fn buy(&mut self, shares: f64, price: f64, date: NaiveDate) {
        if shares <= 0.0 {
            return;
        }
        self.lots.push(Lot { shares, price, date });
    }

    /// FIFO 卖出指定份额，按每个批次各自的持有天数计赎回费。
    /// 返回 (净入账现金, 费用合计, 实际卖出份额)
    fn sell_fifo(
        &mut self,
        want: f64,
        price: f64,
        today: NaiveDate,
        costs: &crate::config::Costs,
        min_days: i64,
    ) -> (f64, f64, f64) {
        let mut remaining = want;
        let mut net = 0.0;
        let mut fees = 0.0;
        let mut sold = 0.0;
        let mut keep: Vec<Lot> = Vec::with_capacity(self.lots.len());

        for lot in std::mem::take(&mut self.lots) {
            let days = (today - lot.date).num_days();
            if remaining <= 1e-12 || days < min_days {
                keep.push(lot); // 未到最短持有期，跳过（规避惩罚性赎回费）
                continue;
            }
            let take = remaining.min(lot.shares);
            let gross = take * price;
            let fee = gross * costs.total_sell_rate(days);
            net += gross - fee;
            fees += fee;
            sold += take;
            remaining -= take;
            let left = lot.shares - take;
            if left > 1e-12 {
                keep.push(Lot { shares: left, price: lot.price, date: lot.date });
            }
        }
        self.lots = keep;
        self.lots.sort_by_key(|l| l.date);
        (net, fees, sold)
    }

    fn to_position(&self, idx: usize, price: f64) -> Option<Position> {
        let shares = self.shares();
        if shares <= 0.0 {
            return None;
        }
        Some(Position {
            id: idx as i64 + 1,
            asset_code: LEG_CODES[idx].to_string(),
            asset_name: LEG_NAMES[idx].to_string(),
            category: LEG_CATEGORIES[idx].to_string(),
            shares,
            cost_price: self.avg_cost(),
            current_price: Some(price),
            first_buy_date: self
                .first_buy()
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
            updated_at: String::new(),
        })
    }
}

// ───────────────────────── 回测配置 ─────────────────────────

#[derive(Debug, Clone)]
pub struct BacktestConfig {
    pub initial_cash: f64,
    pub annual_inflow: f64,
    pub start_date: NaiveDate,
    pub end_date: NaiveDate,
}

impl Default for BacktestConfig {
    fn default() -> Self {
        Self {
            initial_cash: 100_000.0,
            annual_inflow: 50_000.0,
            start_date: NaiveDate::from_ymd_opt(2016, 1, 1).unwrap(),
            end_date: NaiveDate::from_ymd_opt(2025, 4, 30).unwrap(),
        }
    }
}

impl BacktestConfig {
    /// 覆盖数据实际区间
    pub fn spanning(rows: &[MonthRow], initial_cash: f64, annual_inflow: f64) -> Self {
        Self {
            initial_cash,
            annual_inflow,
            start_date: rows.first().map(|r| r.date).unwrap_or_default(),
            end_date: rows.last().map(|r| r.date).unwrap_or_default(),
        }
    }
}

/// 策略引擎类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// 目标仓位 + 偏离带（新框架）
    TargetWeight,
    /// 旧框架：买入=现金的百分比，卖出=份额的百分比 + 逆向加权
    Legacy,
    /// 买入持有基准：按 allocation 固定权重，仅注资时买入，不再平衡
    BuyHold,
    /// 买入持有 + 年度再平衡（用于分离"再平衡贡献"与"择时贡献"）
    BuyHoldRebalanced,
}

impl Engine {
    pub fn label(&self) -> &'static str {
        match self {
            Engine::TargetWeight => "目标仓位+偏离带",
            Engine::Legacy => "旧框架(现金比例)",
            Engine::BuyHold => "买入持有",
            Engine::BuyHoldRebalanced => "买入持有+年度再平衡",
        }
    }
}

/// 主信号锚点。
///
/// FGI 的均值回归周期是数周，而中长线组合的决策周期是数月到数年——直接用它做
/// 主驱动存在时间尺度错配。`TrendTilt` 把月级趋势作为主锚，FGI 降级为有限倾斜。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// 仅用情绪决定目标仓位
    SentimentOnly,
    /// 趋势为主锚（价格 vs N月均线），情绪仅做 ±tilt_pp 倾斜
    TrendTilt,
}

/// 情绪信号处理：把 FGI 平滑，避免用周级噪声驱动年级组合
#[derive(Debug, Clone, Copy)]
pub struct SignalConfig {
    /// 移动平均窗口（月）。1 = 不平滑（旧行为）
    pub smooth_months: usize,
    pub anchor: Anchor,
    /// 趋势均线窗口（月）
    pub trend_months: usize,
    /// 单腿处于上升趋势时的目标权重（占该腿满仓的比例，%）
    pub trend_on_weight: f64,
    /// 单腿处于下降趋势时的目标权重（%）
    pub trend_off_weight: f64,
    /// 情绪倾斜幅度（百分点）：极度恐慌 +tilt，极度贪婪 -tilt
    pub tilt_pp: f64,
}

impl Default for SignalConfig {
    fn default() -> Self {
        Self {
            smooth_months: 3,
            anchor: Anchor::SentimentOnly,
            trend_months: 12,
            trend_on_weight: 95.0,
            trend_off_weight: 55.0,
            tilt_pp: 10.0,
        }
    }
}

impl SignalConfig {
    pub fn trend_tilt() -> Self {
        Self { anchor: Anchor::TrendTilt, ..Self::default() }
    }

    /// 情绪倾斜量（百分点），随情绪升高单调递减
    fn tilt_for(&self, config: &AppConfig, signal: f64) -> f64 {
        let t = &config.thresholds;
        let k = self.tilt_pp;
        if signal < t.extreme_fear {
            k
        } else if signal < t.fear {
            k / 2.0
        } else if signal < t.neutral {
            0.0
        } else if signal < t.greed {
            -k / 2.0
        } else {
            -k
        }
    }
}

/// 每月的风险资产目标总权重（小数）。
///
/// 抽出成独立函数便于单测与对照：`SentimentOnly` 复现旧的情绪→仓位映射，
/// `TrendTilt` 用趋势做主锚。
pub fn compute_target_risk_weights(
    config: &AppConfig,
    rows: &[MonthRow],
    signals: &[f64],
    cfg: &SignalConfig,
) -> Vec<f64> {
    match cfg.anchor {
        Anchor::SentimentOnly => signals
            .iter()
            .map(|s| config.target_weight_for(*s) / 100.0)
            .collect(),
        Anchor::TrendTilt => {
            let (a, b, c) = config.sleeve_split();
            let sleeve = [a, b, c];
            let w = cfg.trend_months.max(1);
            rows.iter()
                .enumerate()
                .map(|(i, row)| {
                    // 各腿趋势状态加权成组合层面的基准仓位
                    let mut base = 0.0;
                    for leg in 0..3 {
                        let lo = i.saturating_sub(w - 1);
                        let ma: f64 = rows[lo..=i].iter().map(|r| r.prices[leg]).sum::<f64>()
                            / (i - lo + 1) as f64;
                        // 历史不足时按上升趋势处理，避免开局系统性空仓
                        let up = i + 1 < w || row.prices[leg] >= ma;
                        base += sleeve[leg]
                            * if up { cfg.trend_on_weight } else { cfg.trend_off_weight };
                    }
                    let tilted = base + cfg.tilt_for(config, signals[i]);
                    (tilted / 100.0).clamp(0.0, 1.0)
                })
                .collect()
        }
    }
}

/// 计算平滑后的情绪序列
pub fn smooth_fgi(rows: &[MonthRow], window: usize) -> Vec<f64> {
    let w = window.max(1);
    rows.iter()
        .enumerate()
        .map(|(i, _)| {
            let lo = i.saturating_sub(w - 1);
            let slice = &rows[lo..=i];
            slice.iter().map(|r| r.fgi).sum::<f64>() / slice.len() as f64
        })
        .collect()
}

// ───────────────────────── 结果 ─────────────────────────

#[derive(Debug, Clone)]
pub struct Trade {
    pub date: NaiveDate,
    pub action: &'static str,
    pub leg: usize,
    pub zone: String,
    pub fgi: f64,
    pub shares: f64,
    pub price: f64,
    pub amount: f64,
    pub fee: f64,
}

#[derive(Debug, Clone)]
pub struct Monthly {
    pub date: NaiveDate,
    pub fgi: f64,
    pub signal: f64,
    pub zone: String,
    pub cash: f64,
    pub leg_values: [f64; 3],
    pub total_value: f64,
    /// 本月外部注资（用于剔除注资后计算真实收益率）
    pub inflow: f64,
    /// 剔除注资影响的当期收益率
    pub period_return: f64,
    /// 风险资产实际权重 vs 目标权重（百分比）
    pub risk_weight: f64,
    pub target_risk_weight: f64,
}

#[derive(Debug, Clone)]
pub struct BacktestResult {
    pub name: String,
    /// 保留以便调用方按引擎类型区分结果
    #[allow(dead_code)]
    pub engine: Engine,
    pub total_inflow: f64,
    pub final_value: f64,
    pub total_return: f64,
    /// 现金流加权年化（XIRR）
    pub xirr: f64,
    /// 简单年化，仅为与旧版口径对照
    pub naive_annualized: f64,
    pub max_drawdown: f64,
    pub risk: RiskMetrics,
    pub total_fees: f64,
    pub cash_interest: f64,
    pub trades: Vec<Trade>,
    pub buy_count: usize,
    pub sell_count: usize,
    pub monthly: Vec<Monthly>,
    pub trades_per_year: f64,
}

impl BacktestResult {
    pub fn returns(&self) -> Vec<f64> {
        self.monthly.iter().skip(1).map(|m| m.period_return).collect()
    }
}

// ───────────────────────── 引擎实现 ─────────────────────────

struct State {
    cash: f64,
    legs: [Leg; 3],
    total_inflow: f64,
    fees: f64,
    interest: f64,
    flows: Vec<CashFlow>,
    trades: Vec<Trade>,
    last_inflow_year: i32,
}

impl State {
    fn new(initial: f64, start: NaiveDate) -> Self {
        Self {
            cash: initial,
            legs: Default::default(),
            total_inflow: initial,
            fees: 0.0,
            interest: 0.0,
            flows: vec![CashFlow { date: start, amount: -initial }],
            trades: Vec::new(),
            last_inflow_year: 0,
        }
    }

    fn leg_values(&self, prices: &[f64; 3]) -> [f64; 3] {
        [
            self.legs[0].value(prices[0]),
            self.legs[1].value(prices[1]),
            self.legs[2].value(prices[2]),
        ]
    }

    fn total(&self, prices: &[f64; 3]) -> f64 {
        self.cash + self.leg_values(prices).iter().sum::<f64>()
    }

    fn positions(&self, prices: &[f64; 3]) -> Vec<Position> {
        (0..3)
            .filter_map(|i| self.legs[i].to_position(i, prices[i]))
            .collect()
    }

    fn do_buy(
        &mut self,
        leg: usize,
        amount: f64,
        price: f64,
        date: NaiveDate,
        costs: &crate::config::Costs,
        zone: &str,
        fgi: f64,
    ) {
        if amount <= 0.0 || price <= 0.0 || amount > self.cash + 1e-9 {
            let amount = amount.min(self.cash);
            if amount <= 0.0 {
                return;
            }
        }
        let amount = amount.min(self.cash);
        if amount <= 0.0 {
            return;
        }
        let fee = amount * costs.buy_rate();
        let shares = (amount - fee) / price;
        if shares <= 0.0 {
            return;
        }
        self.legs[leg].buy(shares, price, date);
        self.cash -= amount;
        self.fees += fee;
        self.trades.push(Trade {
            date,
            action: "买入",
            leg,
            zone: zone.to_string(),
            fgi,
            shares,
            price,
            amount,
            fee,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn do_sell(
        &mut self,
        leg: usize,
        target_value: f64,
        price: f64,
        date: NaiveDate,
        costs: &crate::config::Costs,
        min_days: i64,
        zone: &str,
        fgi: f64,
    ) {
        if target_value <= 0.0 || price <= 0.0 {
            return;
        }
        let want = target_value / price;
        let (net, fee, sold) = self.legs[leg].sell_fifo(want, price, date, costs, min_days);
        if sold <= 0.0 {
            return;
        }
        self.cash += net;
        self.fees += fee;
        self.trades.push(Trade {
            date,
            action: "卖出",
            leg,
            zone: zone.to_string(),
            fgi,
            shares: sold,
            price,
            amount: sold * price,
            fee,
        });
    }
}

pub fn run(
    config: &AppConfig,
    bt: &BacktestConfig,
    signal_cfg: &SignalConfig,
    engine: Engine,
    rows: &[MonthRow],
) -> BacktestResult {
    let data: Vec<&MonthRow> = rows
        .iter()
        .filter(|r| r.date >= bt.start_date && r.date <= bt.end_date)
        .collect();
    if data.is_empty() {
        panic!("回测区间内无数据");
    }
    let owned: Vec<MonthRow> = data.iter().map(|r| (*r).clone()).collect();
    let signals = smooth_fgi(&owned, signal_cfg.smooth_months);
    let target_ws = compute_target_risk_weights(config, &owned, &signals, signal_cfg);

    let costs = &config.costs;
    let band = config.rebalance.band_pp;
    let min_trade = config.rebalance.min_trade_amount;
    let min_hold = config.rebalance.min_holding_days_for_sell;

    let mut st = State::new(bt.initial_cash, owned[0].date);
    let mut monthly: Vec<Monthly> = Vec::new();
    let mut prev_date: Option<NaiveDate> = None;
    let mut prev_total: f64 = bt.initial_cash;

    for (i, row) in owned.iter().enumerate() {
        // 1) 现金按货币基金收益增长
        if let Some(pd) = prev_date {
            let days = (row.date - pd).num_days();
            let grown = st.cash * costs.cash_growth(days);
            st.interest += grown - st.cash;
            st.cash = grown;
        }

        // 2) 年度注资（每年3月末之后的首个月）
        let mut inflow = 0.0;
        if row.date.year() > st.last_inflow_year && row.date.month() >= 3 && i > 0 {
            st.cash += bt.annual_inflow;
            st.total_inflow += bt.annual_inflow;
            st.last_inflow_year = row.date.year();
            inflow = bt.annual_inflow;
            st.flows.push(CashFlow { date: row.date, amount: -bt.annual_inflow });
        } else if i == 0 {
            st.last_inflow_year = row.date.year();
        }

        let signal = signals[i];
        let zone = config.sentiment_zone(signal).to_string();

        // 3) 调仓
        match engine {
            Engine::TargetWeight => rebalance_to_weights(
                &mut st,
                config,
                row,
                config.sleeve_split(),
                target_ws[i],
                &zone,
                band,
                min_trade,
                min_hold,
            ),
            Engine::Legacy => step_legacy(&mut st, config, row, signal, &zone),
            Engine::BuyHold => {
                // 只在有可投现金时按固定权重建仓，之后不动
                if st.cash > min_trade {
                    let (a, b, c) = config.sleeve_split();
                    let cash = st.cash;
                    for (leg, w) in [(0, a), (1, b), (2, c)] {
                        st.do_buy(leg, cash * w, row.prices[leg], row.date, costs, &zone, row.fgi);
                    }
                }
            }
            Engine::BuyHoldRebalanced => {
                // 满仓固定权重 + 每年3月再平衡一次
                let yearly = inflow > 0.0 || i == 0;
                if st.cash > min_trade {
                    let (a, b, c) = config.sleeve_split();
                    let cash = st.cash;
                    for (leg, w) in [(0, a), (1, b), (2, c)] {
                        st.do_buy(leg, cash * w, row.prices[leg], row.date, costs, &zone, row.fgi);
                    }
                }
                if yearly && i > 0 {
                    rebalance_to_weights(
                        &mut st, config, row, config.sleeve_split(), 1.0, &zone, 0.0, min_trade,
                        min_hold,
                    );
                }
            }
        }

        // 4) 记账
        let leg_values = st.leg_values(&row.prices);
        let total = st.cash + leg_values.iter().sum::<f64>();
        let risk_value: f64 = leg_values.iter().sum();
        let period_return = if i == 0 {
            0.0
        } else {
            let base = prev_total + inflow;
            if base > 0.0 { total / base - 1.0 } else { 0.0 }
        };
        monthly.push(Monthly {
            date: row.date,
            fgi: row.fgi,
            signal,
            zone: zone.clone(),
            cash: st.cash,
            leg_values,
            total_value: total,
            inflow,
            period_return,
            risk_weight: if total > 0.0 { risk_value / total * 100.0 } else { 0.0 },
            target_risk_weight: match engine {
                Engine::TargetWeight => target_ws[i] * 100.0,
                _ => 100.0,
            },
        });
        prev_total = total;
        prev_date = Some(row.date);
    }

    finalize(config, bt, engine, st, monthly)
}

/// 把组合调向 `sleeve` 比例 × `risk_w` 总风险权重；偏离在带宽内则不动作。
#[allow(clippy::too_many_arguments)]
fn rebalance_to_weights(
    st: &mut State,
    config: &AppConfig,
    row: &MonthRow,
    sleeve: (f64, f64, f64),
    risk_w: f64,
    zone: &str,
    band: f64,
    min_trade: f64,
    min_hold: i64,
) {
    let costs = &config.costs;
    let total = st.total(&row.prices);
    if total <= 0.0 {
        return;
    }
    let targets = [
        total * risk_w * sleeve.0,
        total * risk_w * sleeve.1,
        total * risk_w * sleeve.2,
    ];
    let current = st.leg_values(&row.prices);

    // 先卖出超配腿（回收现金供买入使用）
    for leg in 0..3 {
        let excess = current[leg] - targets[leg];
        let drift_pp = excess / total * 100.0;
        if drift_pp > band && excess >= min_trade {
            st.do_sell(
                leg, excess, row.prices[leg], row.date, costs, min_hold, zone, row.fgi,
            );
        }
    }

    // 再买入低配腿；现金不足时按缺口比例分配
    let current = st.leg_values(&row.prices);
    let mut needs = [0.0_f64; 3];
    let mut need_sum = 0.0;
    for leg in 0..3 {
        let short = targets[leg] - current[leg];
        let drift_pp = short / total * 100.0;
        if drift_pp > band && short >= min_trade {
            needs[leg] = short;
            need_sum += short;
        }
    }
    if need_sum <= 0.0 {
        return;
    }
    let budget = st.cash.min(need_sum);
    if budget < min_trade {
        return;
    }
    for leg in 0..3 {
        if needs[leg] <= 0.0 {
            continue;
        }
        let amount = budget * (needs[leg] / need_sum);
        if amount >= min_trade.min(budget) {
            st.do_buy(leg, amount, row.prices[leg], row.date, costs, zone, row.fgi);
        }
    }
}

/// 旧框架：买入=可用现金的百分比（逆向加权分配），卖出=份额的百分比。
/// 保留以便在同一成本模型下与新框架做公平对照。
fn step_legacy(
    st: &mut State,
    config: &AppConfig,
    row: &MonthRow,
    signal: f64,
    zone: &str,
) {
    let costs = &config.costs;
    let min_hold = config.rebalance.min_holding_days_for_sell;
    let positions = st.positions(&row.prices);

    // 卖出
    let sells = calculate_sell_suggestions(config, signal, &positions);
    for s in &sells {
        if let Some(leg) = LEG_CODES.iter().position(|c| *c == s.asset_code) {
            st.do_sell(
                leg,
                s.sell_amount,
                row.prices[leg],
                row.date,
                costs,
                min_hold,
                zone,
                row.fgi,
            );
        }
    }

    // 买入
    let positions = st.positions(&row.prices);
    let warns = check_risk_warnings(config, signal, &positions);
    let buy = calculate_buy_suggestions(config, signal, st.cash, &positions, &[], &warns);
    if buy.total_amount <= 0.0 {
        return;
    }
    let (a, b, c) = config.sleeve_split();
    let amounts = [
        buy.total_amount * a,
        buy.total_amount * b,
        buy.total_amount * c,
    ];
    for leg in 0..3 {
        st.do_buy(leg, amounts[leg], row.prices[leg], row.date, costs, zone, row.fgi);
    }
}

fn finalize(
    config: &AppConfig,
    bt: &BacktestConfig,
    engine: Engine,
    mut st: State,
    monthly: Vec<Monthly>,
) -> BacktestResult {
    let final_value = monthly.last().map(|m| m.total_value).unwrap_or(0.0);
    let end = monthly.last().map(|m| m.date).unwrap_or(bt.end_date);
    st.flows.push(CashFlow { date: end, amount: final_value });

    let total_return = if st.total_inflow > 0.0 {
        final_value / st.total_inflow - 1.0
    } else {
        0.0
    };
    let years = (end - monthly.first().map(|m| m.date).unwrap_or(end)).num_days() as f64 / 365.0;
    let naive = if years > 0.0 && st.total_inflow > 0.0 {
        (final_value / st.total_inflow).powf(1.0 / years) - 1.0
    } else {
        0.0
    };
    let xirr = metrics::xirr(&st.flows).unwrap_or(naive);

    let values: Vec<f64> = monthly.iter().map(|m| m.total_value).collect();
    let max_dd = metrics::max_drawdown(&values);
    let returns: Vec<f64> = monthly.iter().skip(1).map(|m| m.period_return).collect();
    let risk = metrics::risk_metrics(
        &returns,
        12.0,
        config.costs.cash_annual_yield / 100.0,
        xirr,
        max_dd,
    );

    let buy_count = st.trades.iter().filter(|t| t.action == "买入").count();
    let sell_count = st.trades.len() - buy_count;
    let trades_per_year = if years > 0.0 {
        st.trades.len() as f64 / years
    } else {
        0.0
    };

    BacktestResult {
        name: engine.label().to_string(),
        engine,
        total_inflow: st.total_inflow,
        final_value,
        total_return,
        xirr,
        naive_annualized: naive,
        max_drawdown: max_dd,
        risk,
        total_fees: st.fees,
        cash_interest: st.interest,
        trades: st.trades,
        buy_count,
        sell_count,
        monthly,
        trades_per_year,
    }
}

// ───────────────────────── 输出 ─────────────────────────

pub fn print_report(r: &BacktestResult) {
    println!();
    println!("=================================================================");
    println!("   {} 回测报告", r.name);
    println!("=================================================================");
    println!();
    println!("  【收益概览】");
    println!("    总投入资金:       ¥{:>12.2}", r.total_inflow);
    println!("    期末总资产:       ¥{:>12.2}", r.final_value);
    println!("    总收益:           ¥{:>12.2}", r.final_value - r.total_inflow);
    println!("    总收益率:                 {:>10.2}%", r.total_return * 100.0);
    println!("    年化(XIRR):               {:>10.2}%", r.xirr * 100.0);
    println!("    年化(简单口径):           {:>10.2}%", r.naive_annualized * 100.0);
    println!();
    println!("  【风险】");
    println!("    最大回撤:                 {:>10.2}%", r.max_drawdown * 100.0);
    println!("    年化波动:                 {:>10.2}%", r.risk.volatility * 100.0);
    println!("    Sharpe:                   {:>10.2}", r.risk.sharpe);
    println!("    Sortino:                  {:>10.2}", r.risk.sortino);
    println!("    Calmar:                   {:>10.2}", r.risk.calmar);
    println!();
    println!("  【成本】");
    println!("    交易与赎回费合计: ¥{:>12.2}", r.total_fees);
    println!("    现金利息收入:     ¥{:>12.2}", r.cash_interest);
    println!(
        "    费用占期末资产:           {:>10.2}%",
        if r.final_value > 0.0 { r.total_fees / r.final_value * 100.0 } else { 0.0 }
    );
    println!();
    println!("  【交易】");
    println!(
        "    买入 {:>3} 次 | 卖出 {:>3} 次 | 合计 {:>3} 次 | 年均 {:.1} 次",
        r.buy_count, r.sell_count, r.trades.len(), r.trades_per_year
    );

    let mut by_leg: HashMap<usize, (usize, f64)> = HashMap::new();
    for t in &r.trades {
        let e = by_leg.entry(t.leg).or_insert((0, 0.0));
        e.0 += 1;
        e.1 += t.amount;
    }
    let mut legs: Vec<_> = by_leg.into_iter().collect();
    legs.sort_by_key(|(k, _)| *k);
    for (leg, (n, amt)) in legs {
        println!("      {} {:>4} 次, ¥{:>12.2}", pad_to_width(LEG_NAMES[leg], 16), n, amt);
    }
    println!();

    print_yearly(r);
    print_key_trades(r);
}

/// 按年分解：中长线投资者更关心逐年表现而非单一总数
fn print_yearly(r: &BacktestResult) {
    println!("  【逐年表现】");
    let mut table = Table::new();
    table.load_preset(UTF8_FULL).apply_modifier(UTF8_ROUND_CORNERS);
    table.set_header(vec![
        Cell::new("年份"),
        Cell::new("收益"),
        Cell::new("年末总资产"),
        Cell::new("年内注资"),
        Cell::new("均仓位"),
        Cell::new("目标仓位"),
        Cell::new("情绪原始→平滑"),
        Cell::new("年末构成 美/A/金/现金"),
    ]);
    let mut years: Vec<i32> = r.monthly.iter().map(|m| m.date.year()).collect();
    years.dedup();
    for year in years {
        let ms: Vec<&Monthly> = r.monthly.iter().filter(|m| m.date.year() == year).collect();
        if ms.is_empty() {
            continue;
        }
        let ret = ms.iter().fold(1.0, |acc, m| acc * (1.0 + m.period_return)) - 1.0;
        let inflow: f64 = ms.iter().map(|m| m.inflow).sum();
        let avg_w = ms.iter().map(|m| m.risk_weight).sum::<f64>() / ms.len() as f64;
        let avg_target = ms.iter().map(|m| m.target_risk_weight).sum::<f64>() / ms.len() as f64;
        let avg_raw = ms.iter().map(|m| m.fgi).sum::<f64>() / ms.len() as f64;
        let avg_fgi = ms.iter().map(|m| m.signal).sum::<f64>() / ms.len() as f64;
        let last = ms[ms.len() - 1];
        let pct = |v: f64| {
            if last.total_value > 0.0 { v / last.total_value * 100.0 } else { 0.0 }
        };
        let composition = format!(
            "{:.0}/{:.0}/{:.0}/{:.0}%",
            pct(last.leg_values[0]),
            pct(last.leg_values[1]),
            pct(last.leg_values[2]),
            pct(last.cash)
        );
        let color = if ret >= 0.0 { Color::Green } else { Color::Red };
        table.add_row(vec![
            Cell::new(year.to_string()),
            Cell::new(format!("{:+.2}%", ret * 100.0)).fg(color),
            Cell::new(format!("¥{:.0}", last.total_value)),
            Cell::new(format!("¥{:.0}", inflow)),
            Cell::new(format!("{:.0}%", avg_w)),
            Cell::new(format!("{:.0}%", avg_target)),
            Cell::new(format!("{:.0}→{:.0} {}", avg_raw, avg_fgi, last.zone)),
            Cell::new(composition),
        ]);
    }
    println!("{}", table);
    println!();
}

/// 每年金额最大的一笔交易，便于人工核对策略在关键时点做了什么
fn print_key_trades(r: &BacktestResult) {
    if r.trades.is_empty() {
        return;
    }
    println!("  【关键交易】（每年金额最大一笔）");
    let mut by_year: HashMap<i32, &Trade> = HashMap::new();
    for t in &r.trades {
        by_year
            .entry(t.date.year())
            .and_modify(|cur| {
                if t.amount > cur.amount {
                    *cur = t;
                }
            })
            .or_insert(t);
    }
    let mut years: Vec<i32> = by_year.keys().copied().collect();
    years.sort();
    for y in years {
        let t = by_year[&y];
        println!(
            "    {} {} {} {}(FGI {:.0})  {:.2}份 @ {:.4}  ¥{:.0}  费用¥{:.2}",
            t.date,
            t.action,
            pad_to_width(LEG_NAMES[t.leg], 16),
            pad_to_width(&t.zone, 10),
            t.fgi,
            t.shares,
            t.price,
            t.amount,
            t.fee
        );
    }
    println!();
}

/// 按终端显示宽度补齐（中文占 2 列）
fn pad_to_width(s: &str, width: usize) -> String {
    let w = unicode_width::UnicodeWidthStr::width(s);
    if w >= width {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(width - w))
    }
}

pub fn print_comparison(results: &[BacktestResult]) {
    println!();
    println!("=================================================================");
    println!("   策略对比（同一数据、同一成本模型）");
    println!("=================================================================");
    println!();
    let mut table = Table::new();
    table.load_preset(UTF8_FULL).apply_modifier(UTF8_ROUND_CORNERS);
    table.set_header(vec![
        Cell::new("策略"),
        Cell::new("年化XIRR"),
        Cell::new("最大回撤"),
        Cell::new("波动"),
        Cell::new("Sharpe"),
        Cell::new("Calmar"),
        Cell::new("费用"),
        Cell::new("年均交易"),
    ]);
    for r in results {
        let c = if r.xirr >= 0.0 { Color::Green } else { Color::Red };
        table.add_row(vec![
            Cell::new(&r.name),
            Cell::new(format!("{:.2}%", r.xirr * 100.0)).fg(c),
            Cell::new(format!("{:.2}%", r.max_drawdown * 100.0)),
            Cell::new(format!("{:.1}%", r.risk.volatility * 100.0)),
            Cell::new(format!("{:.2}", r.risk.sharpe)),
            Cell::new(format!("{:.2}", r.risk.calmar)),
            Cell::new(format!("¥{:.0}", r.total_fees)),
            Cell::new(format!("{:.1}", r.trades_per_year)),
        ]);
    }
    println!("{}", table);
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn 内置数据集应可解析且无拼接断点() {
        let rows = load_main();
        assert!(rows.len() > 100, "月数 {}", rows.len());
        // 旧数据集在 2019-01 有 -97% 的拼接断点，这里逐月校验单月跳变
        for w in rows.windows(2) {
            for leg in 0..3 {
                let ch = w[1].prices[leg] / w[0].prices[leg] - 1.0;
                assert!(
                    ch.abs() < 0.35,
                    "{} 第{}腿单月跳变 {:.1}% 疑似数据断点",
                    w[1].date, leg, ch * 100.0
                );
            }
        }
    }

    #[test]
    fn 数据集应为真实值而非人工估填的整数() {
        let rows = load_main();
        // 旧数据集 112/112 为整数；真实序列几乎不应出现整数
        let ints = rows
            .iter()
            .filter(|r| r.prices.iter().all(|p| (p - p.round()).abs() < 1e-9))
            .count();
        assert!(ints <= 1, "疑似人工估填，整数行数={}", ints);
    }

    #[test]
    fn 情绪平滑窗口为一时等于原值() {
        let rows = load_main();
        let s = smooth_fgi(&rows, 1);
        assert_eq!(s[10], rows[10].fgi);
    }

    #[test]
    fn 情绪平滑应降低波动() {
        let rows = load_main();
        let raw: Vec<f64> = rows.iter().map(|r| r.fgi).collect();
        let sm = smooth_fgi(&rows, 3);
        assert!(
            metrics::stddev(&sm) < metrics::stddev(&raw),
            "平滑后波动应更小: {} vs {}",
            metrics::stddev(&sm),
            metrics::stddev(&raw)
        );
    }

    #[test]
    fn 阶梯赎回费应按批次持有天数分别计算() {
        let costs = crate::config::Costs::default();
        let mut leg = Leg::default();
        leg.buy(100.0, 1.0, d(2020, 1, 1)); // 持有 1827 天 → 免赎回费
        leg.buy(100.0, 1.0, d(2024, 12, 29)); // 持有 3 天 → 落入 <7 天惩罚档 1.5%
        // 全部卖出（min_days=0 以便同时卖到新批次）
        let (net, fee, sold) = leg.sell_fifo(200.0, 1.0, d(2025, 1, 1), &costs, 0);
        assert!((sold - 200.0).abs() < 1e-9);
        // 老批次仅 0.05% 卖出费，新批次 0.05%+1.5%
        let expect = 100.0 * 0.0005 + 100.0 * (0.0005 + 0.015);
        assert!((fee - expect).abs() < 1e-6, "fee={} expect={}", fee, expect);
        assert!((net - (200.0 - expect)).abs() < 1e-6);
        // 边界：恰好持有 7 天不属于 <7 档，应落到 0.75%
        let mut leg2 = Leg::default();
        leg2.buy(100.0, 1.0, d(2024, 12, 25));
        let (_, fee2, _) = leg2.sell_fifo(100.0, 1.0, d(2025, 1, 1), &costs, 0);
        let expect2 = 100.0 * (0.0005 + 0.0075);
        assert!((fee2 - expect2).abs() < 1e-6, "7天边界 fee={}", fee2);
    }

    #[test]
    fn 最短持有天数应阻止卖出新批次() {
        let costs = crate::config::Costs::default();
        let mut leg = Leg::default();
        leg.buy(100.0, 1.0, d(2024, 12, 25)); // 7天
        let (_, _, sold) = leg.sell_fifo(100.0, 1.0, d(2025, 1, 1), &costs, 30);
        assert_eq!(sold, 0.0, "未满最短持有期不应卖出");
        assert!((leg.shares() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn fifo_应先卖最早批次() {
        let costs = crate::config::Costs { buy_fee_pct: 0.0, sell_fee_pct: 0.0, cash_annual_yield: 0.0, redemption_tiers: vec![] };
        let mut leg = Leg::default();
        leg.buy(10.0, 1.0, d(2020, 1, 1));
        leg.buy(10.0, 5.0, d(2021, 1, 1));
        leg.sell_fifo(10.0, 3.0, d(2023, 1, 1), &costs, 0);
        // 卖掉最早的低成本批次后，剩余成本应为第二批的 5.0
        assert!((leg.avg_cost() - 5.0).abs() < 1e-9, "avg={}", leg.avg_cost());
    }

    #[test]
    fn 目标仓位框架的实际权重应贴近目标() {
        let cfg = AppConfig::default_config();
        let rows = load_main();
        let bt = BacktestConfig::default();
        let r = run(&cfg, &bt, &SignalConfig::default(), Engine::TargetWeight, &rows);
        // 跳过建仓初期，检查权重跟踪误差
        let late: Vec<&Monthly> = r.monthly.iter().skip(12).collect();
        let bad = late
            .iter()
            .filter(|m| (m.risk_weight - m.target_risk_weight).abs() > 15.0)
            .count();
        assert!(
            bad * 10 < late.len(),
            "偏离目标超15pp的月份过多: {}/{}",
            bad,
            late.len()
        );
    }

    #[test]
    fn 目标仓位框架不应出现弹药几何衰减() {
        // 旧框架在连续恐慌期会越买越少（每次只花现金的固定比例）。
        // 新框架应能把仓位推到目标附近，因此极度恐慌期末现金占比应显著低于目标留存。
        let cfg = AppConfig::default_config();
        let rows = load_main();
        let bt = BacktestConfig::default();
        let r = run(&cfg, &bt, &SignalConfig::default(), Engine::TargetWeight, &rows);
        let fear_months: Vec<&Monthly> = r
            .monthly
            .iter()
            .skip(12)
            .filter(|m| m.signal < cfg.thresholds.extreme_fear)
            .collect();
        if !fear_months.is_empty() {
            let avg_risk = fear_months.iter().map(|m| m.risk_weight).sum::<f64>()
                / fear_months.len() as f64;
            assert!(
                avg_risk > 60.0,
                "极度恐慌期平均风险仓位仅 {:.1}%，疑似弹药衰减",
                avg_risk
            );
        }
    }

    #[test]
    fn 成本模型应确实扣减收益() {
        let rows = load_main();
        let bt = BacktestConfig::default();
        let mut free = AppConfig::default_config();
        free.costs = crate::config::Costs {
            buy_fee_pct: 0.0,
            sell_fee_pct: 0.0,
            cash_annual_yield: 0.0,
            redemption_tiers: vec![],
        };
        let with_cost = AppConfig::default_config();
        let a = run(&free, &bt, &SignalConfig::default(), Engine::TargetWeight, &rows);
        let b = run(&with_cost, &bt, &SignalConfig::default(), Engine::TargetWeight, &rows);
        assert_eq!(a.total_fees, 0.0);
        assert!(b.total_fees > 0.0, "应产生费用");
    }

    #[test]
    fn 买入持有基准应使用配置的资产配置() {
        // 旧版基准硬编码 70/15/15，与被对比策略的 55/25/20 不一致。
        let mut cfg = AppConfig::default_config();
        cfg.allocation.us_stocks = 100.0;
        cfg.allocation.cn_stocks = 0.0;
        cfg.allocation.counter_cyclical = 0.0;
        let rows = load_main();
        let bt = BacktestConfig::default();
        let r = run(&cfg, &bt, &SignalConfig::default(), Engine::BuyHold, &rows);
        let last = r.monthly.last().unwrap();
        assert!(last.leg_values[1] < 1e-6, "A股腿应为空");
        assert!(last.leg_values[2] < 1e-6, "黄金腿应为空");
        assert!(last.leg_values[0] > 0.0);
    }

    #[test]
    fn 带宽越大交易越少() {
        let rows = load_main();
        let bt = BacktestConfig::default();
        let mut narrow = AppConfig::default_config();
        narrow.rebalance.band_pp = 1.0;
        let mut wide = AppConfig::default_config();
        wide.rebalance.band_pp = 12.0;
        let a = run(&narrow, &bt, &SignalConfig::default(), Engine::TargetWeight, &rows);
        let b = run(&wide, &bt, &SignalConfig::default(), Engine::TargetWeight, &rows);
        assert!(
            a.trades.len() > b.trades.len(),
            "窄带 {} 应多于宽带 {}",
            a.trades.len(),
            b.trades.len()
        );
    }

    #[test]
    fn 现金应产生利息() {
        let rows = load_main();
        let bt = BacktestConfig::default();
        let cfg = AppConfig::default_config();
        let r = run(&cfg, &bt, &SignalConfig::default(), Engine::TargetWeight, &rows);
        assert!(r.cash_interest > 0.0, "闲置现金应有货币基金收益");
    }

    #[test]
    fn 交易频率应符合中长线约束() {
        // 用户可接受每月2-3次，即年均约36次以内
        let rows = load_main();
        let bt = BacktestConfig::default();
        let cfg = AppConfig::default_config();
        let r = run(&cfg, &bt, &SignalConfig::default(), Engine::TargetWeight, &rows);
        assert!(
            r.trades_per_year <= 36.0,
            "年均交易 {:.1} 次超出中长线约束",
            r.trades_per_year
        );
    }
    #[test]
    fn 对照_趋势锚点与情绪锚点() {
        let cfg = AppConfig::default_config();
        let rows = load_main();
        let bt = BacktestConfig::spanning(&rows, 100_000.0, 50_000.0);
        println!("\n{:<22} {:>9} {:>9} {:>8} {:>8} {:>9}", "信号模式", "XIRR", "回撤", "Sharpe", "Calmar", "年均交易");
        for (name, sc) in [
            ("情绪锚(平滑1月)", SignalConfig { smooth_months: 1, ..SignalConfig::default() }),
            ("情绪锚(平滑3月)", SignalConfig::default()),
            ("情绪锚(平滑6月)", SignalConfig { smooth_months: 6, ..SignalConfig::default() }),
            ("趋势锚+情绪倾斜", SignalConfig::trend_tilt()),
            ("趋势锚(倾斜0)", SignalConfig { tilt_pp: 0.0, ..SignalConfig::trend_tilt() }),
        ] {
            let r = run(&cfg, &bt, &sc, Engine::TargetWeight, &rows);
            println!("{:<22} {:>8.2}% {:>8.2}% {:>8.2} {:>8.2} {:>9.1}",
                name, r.xirr*100.0, r.max_drawdown*100.0, r.risk.sharpe, r.risk.calmar, r.trades_per_year);
        }
        for (name, e) in [("买入持有+年度再平衡", Engine::BuyHoldRebalanced), ("买入持有", Engine::BuyHold)] {
            let r = run(&cfg, &bt, &SignalConfig::default(), e, &rows);
            println!("{:<22} {:>8.2}% {:>8.2}% {:>8.2} {:>8.2} {:>9.1}",
                name, r.xirr*100.0, r.max_drawdown*100.0, r.risk.sharpe, r.risk.calmar, r.trades_per_year);
        }
    }
}
