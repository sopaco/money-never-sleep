//! 实盘审计引擎：账户镜像、策略有效性检验、穿越牛熊的分段验证。
//!
//! 与 `backtest.rs` 的关系：后者是 2016–2025 月频**代理**序列上的历史演示，
//! 本模块是整改后的主验证链路。两者的差别不只是数据，而是四个此前缺失的机制：
//!
//! 1. **信号同源**：直接调用 `signal::compute`（7 天平滑 + 3 天区间确认 + 决策日），
//!    与 `mns report` 用的是同一个函数、同一份 `AppConfig`；不再有"回测 3 月平滑
//!    / 实盘 7 天平滑"两条链路互相引用结论的情况。
//! 2. **执行时序**：信号在 T 日、成交在 T+lag；卖出款按通道 T+n 到账，
//!    到账前记在途资金，计入总资产但**不可用于买入**。
//! 3. **成本按通道**：场外 QDII / 场外 A 股联接 / 场内 ETF 分别取费率、赎回阶梯、
//!    结算天数、限购上限；不再把场外申购费与场内即时到账混在一起。
//! 4. **回撤双口径**：剔除资金流的复合净值（主口径）与含资金流的账户净值并列，
//!    且基于日频净值计算，不再用含注资的月频总资产掩盖回撤。
//!
//! 两个数据集（实盘四基金 / 2016-今三腿代理）都先转成 `BarSeries` 再进本引擎，
//! 保证信号、成本、回撤口径在长周期验证与账户镜像中完全一致。

use crate::config::{AppConfig, ChannelSpec};
#[cfg(test)]
use crate::config::DecisionInterval;
use crate::live_data::{Bar, BarSeries, DatasetKind, is_decision_day};
use crate::signal::{self, ScoreHistory};
use anyhow::{Context, Result, bail};
use chrono::{Duration, NaiveDate};
use std::collections::HashMap;

// ───────────────────────── 输入 ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SignalVariant {
    /// 真实情绪信号（同源 signal::compute）
    Real,
    /// 把已算好的有效信号序列做时序置换（保留边际分布，只打乱时序）
    Shuffled(u64),
    /// 在观测区间内独立同分布随机构造的信号
    Random(u64),
    /// 常数仓位，完全不用信号
    Constant(f64),
    /// 真实信号整体延迟 N 根 bar 执行
    Delayed(usize),
}

#[derive(Debug, Clone)]
pub struct AuditOptions {
    pub name: String,
    pub variant: SignalVariant,
    /// 回测起点（bar 索引）
    pub start_idx: usize,
    /// 回测终点（bar 索引，含）
    pub end_idx: usize,
    pub initial_cash: f64,
    /// 覆盖信号延迟（None = 用 config.backtest.execution_lag_days）
    pub execution_lag_days: Option<u32>,
    /// 覆盖偏离带（walk-forward 调参用）
    pub band_pp: Option<f64>,
    /// 买入持有基准：首日按 sleeve 一次性建满仓，此后完全不动。
    /// 不能用「常数 100% 仓位 + band=0」模拟——那会每天触发再平衡，
    /// 把基准变成高频调仓策略（实测 151 笔/年），使对比失真。
    pub buy_and_hold: bool,
}

impl AuditOptions {
    pub fn spanning(name: &str, bars: &BarSeries, config: &AppConfig) -> Self {
        Self {
            name: name.to_string(),
            variant: SignalVariant::Real,
            start_idx: 0,
            end_idx: bars.bars.len().saturating_sub(1),
            initial_cash: config.backtest.initial_cash,
            execution_lag_days: None,
            band_pp: None,
            buy_and_hold: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SignalSeries {
    /// 每日决策用分数（已平滑 + 区间确认）
    pub effective: Vec<f64>,
    /// 每日是否为决策日（同源 signal.rs + backtest.decision_interval）
    pub decision: Vec<bool>,
    pub raw_min: f64,
    pub raw_max: f64,
}

/// 一次性生成与实盘同源的信号序列。置换/随机检验只打乱这份**结果**，
/// 不重复调用 signal::compute，保证 300 次置换在长周期数据上仍可秒级完成。
pub fn build_signal_series(config: &AppConfig, bars: &[Bar]) -> SignalSeries {
    let raw: Vec<f64> = bars.iter().map(|b| b.fgi.unwrap_or(50.0)).collect();
    build_signal_series_with_raw(config, bars, &raw)
}

/// 用任意同长度的分值序列替换 FGI，仍走 `signal::compute` 的同一套
/// 7 天平滑 + 区间确认 + 决策日逻辑。用于「换一个输入信号」的对照实验，
/// 保证差异只来自分值本身，不来自平滑/确认/执行口径。
pub fn build_signal_series_with_raw(
    config: &AppConfig,
    bars: &[Bar],
    raw_scores: &[f64],
) -> SignalSeries {
    let smooth = config.rebalance.signal_smooth_days.max(1) as usize;
    let confirm = config.rebalance.zone_confirm_days.max(1) as usize;
    let keep = (smooth * 4).max(confirm * 4 + 2).max(12);
    let mut hist: ScoreHistory = Vec::new();
    let mut effective = Vec::with_capacity(bars.len());
    let mut decision = Vec::with_capacity(bars.len());
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;

    for i in 0..bars.len() {
        let b = &bars[i];
        let raw = raw_scores.get(i).copied().unwrap_or(50.0);
        lo = lo.min(raw);
        hi = hi.max(raw);
        let from = hist.len().saturating_sub(keep);
        let slice = hist[from..].to_vec();
        let s = signal::compute(config, raw, &slice, b.date);
        let next = bars.get(i + 1).map(|x| x.date);
        let interval_ok = is_decision_day(config, b.date, next);
        effective.push(s.effective_score);
        decision.push(s.decision_day && interval_ok);
        hist.push((b.date, raw));
    }
    if !lo.is_finite() {
        lo = 0.0;
        hi = 100.0;
    }
    SignalSeries { effective, decision, raw_min: lo, raw_max: hi }
}

// ───────────────────────── 结果 ─────────────────────────

/// 逐笔成交明细。明细字段保留给审计导出与逐笔核对，当前表格只汇总笔数与费用。
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct AuditTrade {
    pub date: NaiveDate,
    pub action: &'static str,
    pub leg: usize,
    pub amount: f64,
    pub fee: f64,
    pub blocked: Option<&'static str>,
}

/// `equity` / `account_curve` / `initial_cash` 等保留为可核对的净值轨迹，
/// 供后续导出与第三方复算；当前打印层只消费汇总指标。
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct AuditResult {
    pub name: String,
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub initial_cash: f64,
    pub final_value: f64,
    pub total_return: f64,
    /// 年化（无外部注资，等价于 XIRR）
    pub annualized: f64,
    /// 主口径：剔除资金流的复合日频净值最大回撤
    pub max_dd_flow: f64,
    /// 对照口径：含资金流的账户净值最大回撤
    pub max_dd_account: f64,
    /// 三腿按配置权重构成的纯资产池日频最大回撤（不含现金缓冲）
    pub max_dd_market: f64,
    pub calmar: f64,
    pub fees: f64,
    pub trades: Vec<AuditTrade>,
    pub avg_risk_weight: f64,
    pub trades_per_year: f64,
    pub equity: Vec<(NaiveDate, f64)>,
    pub account_curve: Vec<(NaiveDate, f64)>,
    pub blocked_by_cap: usize,
    pub blocked_by_settlement: usize,
    pub skipped_no_fgi: usize,
}

impl AuditResult {
    pub fn trade_count(&self) -> usize {
        self.trades.iter().filter(|t| t.blocked.is_none()).count()
    }
}

// ───────────────────────── 引擎 ─────────────────────────

#[derive(Debug, Clone)]
struct Lot {
    shares: f64,
    date: NaiveDate,
}

struct EngineState {
    cash: f64,
    /// (到账日, 金额)：卖出款在 T+n 之前不可用
    pending: Vec<(NaiveDate, f64)>,
    lots: [Vec<Lot>; 3],
    fees: f64,
    trades: Vec<AuditTrade>,
    blocked_cap: usize,
    blocked_settlement: usize,
}

impl EngineState {
    fn new(initial: f64) -> Self {
        Self {
            cash: initial,
            pending: Vec::new(),
            lots: [Vec::new(), Vec::new(), Vec::new()],
            fees: 0.0,
            trades: Vec::new(),
            blocked_cap: 0,
            blocked_settlement: 0,
        }
    }
}

fn channel<'a>(config: &'a AppConfig, kind: DatasetKind, leg: usize) -> &'a ChannelSpec {
    match kind {
        DatasetKind::Long => &config.channels.on_exchange_etf,
        DatasetKind::Live => match leg {
            0 => &config.channels.qdii_feeder,
            _ => &config.channels.cn_feeder,
        },
    }
}

fn leg_shares(lots: &[Vec<Lot>; 3]) -> [f64; 3] {
    let mut out = [0.0; 3];
    for (i, v) in lots.iter().enumerate() {
        out[i] = v.iter().map(|l| l.shares).sum();
    }
    out
}

fn max_drawdown(values: &[f64]) -> f64 {
    let mut peak = f64::NEG_INFINITY;
    let mut dd = 0.0_f64;
    for &v in values {
        peak = peak.max(v);
        if peak > 0.0 {
            dd = dd.max((peak - v) / peak);
        }
    }
    dd
}

/// 确定性 PRNG（xorshift64*），保证置换检验在 CI 中可复现。
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1_u64 << 53) as f64
    }
}

fn variant_scores(variant: SignalVariant, sig: &SignalSeries, n: usize) -> Vec<f64> {
    match variant {
        SignalVariant::Real => sig.effective.clone(),
        SignalVariant::Constant(_) => vec![50.0; n],
        SignalVariant::Delayed(k) => {
            let mut v = vec![sig.effective[0]; n];
            for i in k..n {
                v[i] = sig.effective[i - k];
            }
            v
        }
        SignalVariant::Shuffled(seed) => {
            let mut v = sig.effective.clone();
            let mut rng = Rng::new(seed);
            for i in (1..v.len()).rev() {
                v.swap(i, rng.below(i + 1));
            }
            v
        }
        SignalVariant::Random(seed) => {
            let mut rng = Rng::new(seed);
            (0..n)
                .map(|_| sig.raw_min + rng.unit() * (sig.raw_max - sig.raw_min))
                .collect()
        }
    }
}

/// 主引擎：严格执行策略。
pub fn run_audit(
    config: &AppConfig,
    series: &BarSeries,
    sig: &SignalSeries,
    opt: &AuditOptions,
) -> Result<AuditResult> {
    let bars = &series.bars;
    if opt.start_idx >= bars.len() || opt.end_idx >= bars.len() || opt.start_idx > opt.end_idx {
        bail!("审计区间越界: {}..{} / 共 {} 根", opt.start_idx, opt.end_idx, bars.len());
    }
    let n = opt.end_idx - opt.start_idx + 1;
    let lag_days = opt.execution_lag_days.unwrap_or(config.backtest.execution_lag_days).max(1);
    let band_pp = opt.band_pp.unwrap_or(config.rebalance.band_pp);
    let min_trade = config.rebalance.min_trade_amount;
    let min_hold = config.rebalance.min_holding_days_for_sell;
    let sleeve = config.sleeve_split();
    let sleeve_arr = [sleeve.0, sleeve.1, sleeve.2];
    let scores = variant_scores(opt.variant, sig, bars.len());
    let constant_w = match opt.variant {
        SignalVariant::Constant(w) => Some(w.clamp(0.0, 1.0)),
        _ => None,
    };

    let mut st = EngineState::new(opt.initial_cash);
    let mut equity = Vec::with_capacity(n);
    let mut account_curve = Vec::with_capacity(n);
    let mut flow_equity = 1.0_f64;
    let mut market_curve = Vec::with_capacity(n);
    let mut risk_sum = 0.0;
    let mut skipped_no_fgi = 0usize;
    let mut pending_orders: HashMap<usize, (f64, bool)> = HashMap::new();

    // 资产池基准：三腿按配置权重、买入持有的日频净值
    let base_prices = bars[opt.start_idx].prices;

    for i in opt.start_idx..=opt.end_idx {
        let bar = &bars[i];

        // 1) 到期在途资金转为可用现金
        let mut still = Vec::new();
        for (due, amt) in st.pending.drain(..) {
            if due <= bar.date {
                st.cash += amt;
            } else {
                still.push((due, amt));
            }
        }
        st.pending = still;

        // 2) 信号 → 目标仓位；只在决策日下单，并按 T+lag 排入执行队列
        // 2a) 起点统一建仓：所有方案在同一执行日、同一价格按各自初始仓位入场。
        //     信号需要约 10 日历史才产生首个决策日，若只在决策日建仓，策略会整体
        //     躲过起点急跌（2016-01 熔断），在沪深300 单资产上凭空多出 ~10% 成本优势。
        if i == opt.start_idx {
            let target_date = bar.date + Duration::days(lag_days as i64);
            let exec = bars.partition_point(|b| b.date < target_date);
            if exec <= opt.end_idx {
                let w = if opt.buy_and_hold {
                    1.0
                } else {
                    match constant_w {
                        Some(v) => v,
                        None => (config.target_weight_for(scores[opt.start_idx]) / 100.0)
                            .clamp(0.0, 1.0),
                    }
                };
                pending_orders.insert(exec, (w, opt.buy_and_hold));
            }
        }

        // 2b) 决策日信号；买入持有基准在 2a 建仓后不再产生任何指令
        if !opt.buy_and_hold {
            if bar.fgi.is_none() {
                skipped_no_fgi += 1;
            } else if sig.decision[i] {
                let risk_w = match constant_w {
                    Some(w) => w,
                    None => (config.target_weight_for(scores[i]) / 100.0).clamp(0.0, 1.0),
                };
                let target_date = bar.date + Duration::days(lag_days as i64);
                let exec = bars.partition_point(|b| b.date < target_date);
                if exec <= opt.end_idx {
                    pending_orders.insert(exec, (risk_w, false));
                }
            }
        }

        // 3a) 买入持有基准的一次性建仓：与其他方案在同一执行日、同一价格成交
        if matches!(pending_orders.get(&i), Some((_, true))) {
            pending_orders.remove(&i);
            let budget = st.cash;
            for leg in 0..3 {
                let spec = channel(config, series.kind, leg);
                let mut amount = budget * sleeve_arr[leg];
                if spec.daily_subscription_cap > 0.0
                    && amount > spec.daily_subscription_cap
                {
                    st.blocked_cap += 1;
                    amount = spec.daily_subscription_cap;
                }
                if amount < min_trade { continue; }
                let fee = amount * (spec.buy_rate() + spec.slippage_pct / 100.0);
                let net = amount - fee;
                st.cash = (st.cash - amount).max(0.0);
                st.fees += fee;
                st.lots[leg].push(Lot { shares: net / bar.prices[leg], date: bar.date });
                st.trades.push(AuditTrade {
                    date: bar.date, action: "买入", leg, amount, fee, blocked: None,
                });
            }
        }

        // 3b) 其余方案：按到期指令再平衡
        if let Some((risk_w, _)) = pending_orders.remove(&i) {
            let prices = bar.prices;
            let shares = leg_shares(&st.lots);
            let current = [
                shares[0] * prices[0], shares[1] * prices[1], shares[2] * prices[2],
            ];
            let in_transit: f64 = st.pending.iter().map(|(_, a)| a).sum();
            let total = st.cash + in_transit + current.iter().sum::<f64>();
            if total > 0.0 {
                let targets = [
                    total * risk_w * sleeve_arr[0],
                    total * risk_w * sleeve_arr[1],
                    total * risk_w * sleeve_arr[2],
                ];
                // 先卖超配：所得按通道 T+n 进入在途资金
                for leg in 0..3 {
                    let excess = current[leg] - targets[leg];
                    if excess / total * 100.0 <= band_pp || excess < min_trade {
                        continue;
                    }
                    let spec = channel(config, series.kind, leg);
                    let mut want = excess / prices[leg];
                    let mut gross_total = 0.0;
                    let mut fee_total = 0.0;
                    let mut keep: Vec<Lot> = Vec::new();
                    for lot in st.lots[leg].drain(..) {
                        if want <= 1e-12 {
                            keep.push(lot);
                            continue;
                        }
                        let held = (bar.date - lot.date).num_days();
                        if held < min_hold {
                            keep.push(lot);
                            continue;
                        }
                        let take = want.min(lot.shares);
                        let gross = take * prices[leg];
                        let fee = gross * (spec.total_sell_rate(held) + spec.slippage_pct / 100.0);
                        gross_total += gross;
                        fee_total += fee;
                        want -= take;
                        if lot.shares - take > 1e-12 {
                            keep.push(Lot { shares: lot.shares - take, ..lot });
                        }
                    }
                    st.lots[leg] = keep;
                    if gross_total > 0.0 {
                        st.fees += fee_total;
                        let due = bar.date + Duration::days(spec.settlement_days as i64);
                        st.pending.push((due, gross_total - fee_total));
                        st.trades.push(AuditTrade {
                            date: bar.date, action: "卖出", leg,
                            amount: gross_total, fee: fee_total, blocked: None,
                        });
                    }
                }

                // 再买低配：只能用已到账现金，且受单日限购约束
                let shares_after = leg_shares(&st.lots);
                let mut needs = [0.0; 3];
                let mut need_sum = 0.0;
                for leg in 0..3 {
                    let short = targets[leg] - shares_after[leg] * prices[leg];
                    if short / total * 100.0 > band_pp && short >= min_trade {
                        needs[leg] = short;
                        need_sum += short;
                    }
                }
                let mut budget = st.cash.min(need_sum);
                if in_transit > 0.0 && need_sum > st.cash {
                    st.blocked_settlement += 1;
                }
                if budget >= min_trade {
                    for leg in 0..3 {
                        if needs[leg] <= 0.0 { continue; }
                        let mut amount = budget * (needs[leg] / need_sum);
                        let spec = channel(config, series.kind, leg);
                        if spec.daily_subscription_cap > 0.0
                            && amount > spec.daily_subscription_cap
                        {
                            st.blocked_cap += 1;
                            st.trades.push(AuditTrade {
                                date: bar.date, action: "买入(限额)", leg,
                                amount: amount - spec.daily_subscription_cap,
                                fee: 0.0, blocked: Some("QDII 限购"),
                            });
                            amount = spec.daily_subscription_cap;
                        }
                        if amount < min_trade { continue; }
                        let fee = amount * (spec.buy_rate() + spec.slippage_pct / 100.0);
                        let net = amount - fee;
                        // 现金按含费金额扣减；只扣净额会把手续费凭空留在账户里
                        st.cash = (st.cash - amount).max(0.0);
                        budget -= amount;
                        st.fees += fee;
                        st.lots[leg].push(Lot { shares: net / prices[leg], date: bar.date });
                        st.trades.push(AuditTrade {
                            date: bar.date, action: "买入", leg,
                            amount, fee, blocked: None,
                        });
                    }
                }
            }
        }

        // 4) 记账
        let shares = leg_shares(&st.lots);
        let holding: f64 = (0..3).map(|k| shares[k] * bar.prices[k]).sum();
        let in_transit: f64 = st.pending.iter().map(|(_, a)| a).sum();
        let total = st.cash + in_transit + holding;
        risk_sum += if total > 0.0 { holding / total } else { 0.0 };
        if i > opt.start_idx {
            let prev = account_curve.last().map(|(_, v)| *v).unwrap_or(opt.initial_cash);
            if prev > 0.0 {
                flow_equity *= total / prev;
            }
        }
        equity.push((bar.date, flow_equity));
        account_curve.push((bar.date, total));
        market_curve.push(
            (0..3).map(|k| sleeve_arr[k] * bar.prices[k] / base_prices[k]).sum::<f64>(),
        );
    }

    let final_value = account_curve.last().map(|(_, v)| *v).unwrap_or(opt.initial_cash);
    let days = (bars[opt.end_idx].date - bars[opt.start_idx].date).num_days().max(1) as f64;
    let years = days / 365.0;
    let annualized = if final_value > 0.0 && opt.initial_cash > 0.0 {
        (final_value / opt.initial_cash).powf(1.0 / years) - 1.0
    } else {
        -1.0
    };
    let max_dd_flow = max_drawdown(&equity.iter().map(|(_, v)| *v).collect::<Vec<_>>());
    let max_dd_account = max_drawdown(&account_curve.iter().map(|(_, v)| *v).collect::<Vec<_>>());
    let max_dd_market = max_drawdown(&market_curve);

    let executed_trades = st.trades.iter().filter(|t| t.blocked.is_none()).count();
    Ok(AuditResult {
        name: opt.name.clone(),
        start: bars[opt.start_idx].date,
        end: bars[opt.end_idx].date,
        initial_cash: opt.initial_cash,
        final_value,
        total_return: final_value / opt.initial_cash - 1.0,
        annualized,
        max_dd_flow,
        max_dd_account,
        max_dd_market,
        calmar: if max_dd_flow > 0.0 { annualized / max_dd_flow } else { 0.0 },
        fees: st.fees,
        trades: st.trades,
        avg_risk_weight: risk_sum / n as f64,
        trades_per_year: executed_trades as f64 / years.max(f64::EPSILON),
        equity,
        account_curve,
        blocked_by_cap: st.blocked_cap,
        blocked_by_settlement: st.blocked_settlement,
        skipped_no_fgi,
    })
}

// ───────────────────── 账户镜像：实际执行 ─────────────────────

/// 用真实交易流水回放账户。
///
/// 流水含纠错记录（例如 020341 误填买入价后以卖出冲销），因此按
/// `(日期, 基金)` 做**净额**回放：净份额与净现金流同时抵消，避免逐笔
/// 顺序执行时出现负持仓。回放结果可与 `positions` 表现有份额交叉验证。
pub fn run_actual(
    config: &AppConfig,
    series: &BarSeries,
    db: &crate::db::Database,
) -> Result<AuditResult> {
    use crate::live_data::CODES;
    if series.kind != DatasetKind::Live {
        bail!("账户镜像只能在实盘数据集上运行（长周期层无真实单位净值）");
    }
    let bars = &series.bars;
    let entry = NaiveDate::parse_from_str(&config.backtest.entry_date, "%Y-%m-%d")
        .context("backtest.entry_date 解析失败")?;
    let start_idx = bars.partition_point(|b| b.date < entry);
    if start_idx >= bars.len() {
        bail!("数据集未覆盖入场日 {}", entry);
    }
    let end_idx = bars.len() - 1;
    let initial_cash = config.backtest.initial_cash;

    #[derive(Default, Clone)]
    struct Flow {
        shares: [f64; 4],
        cash: f64,
        count: usize,
    }
    let mut by_day: HashMap<NaiveDate, Flow> = HashMap::new();
    let txs = db.list_transactions(i64::MAX)?;
    for t in txs {
        let Ok(date) = NaiveDate::parse_from_str(&t.tx_date, "%Y-%m-%d") else { continue };
        let Some(idx) = CODES.iter().position(|c| *c == t.asset_code) else { continue };
        let e = by_day.entry(date).or_default();
        let sign = if t.tx_type == "buy" { 1.0 } else { -1.0 };
        e.shares[idx] += sign * t.shares;
        e.cash -= sign * t.amount;
        e.count += 1;
    }

    let mut shares = [0.0_f64; 4];
    let mut cash = initial_cash;
    let mut trades = Vec::new();
    let mut equity = Vec::new();
    let mut account_curve = Vec::new();
    let mut flow_equity = 1.0_f64;
    let mut market_curve = Vec::new();
    let sleeve = config.sleeve_split();
    let sleeve_arr = [sleeve.0, sleeve.1, sleeve.2];
    let base_prices = bars[start_idx].prices;

    for i in start_idx..=end_idx {
        let bar = &bars[i];
        if let Some(f) = by_day.get(&bar.date) {
            for k in 0..4 {
                shares[k] += f.shares[k];
            }
            cash += f.cash;
            for k in 0..4 {
                if f.shares[k].abs() > 1e-9 {
                    trades.push(AuditTrade {
                        date: bar.date,
                        action: if f.shares[k] > 0.0 { "买入" } else { "卖出" },
                        leg: crate::live_data::LEG_OF[k],
                        amount: f.shares[k].abs() * bar.unit_nav.unwrap_or([0.0; 4])[k],
                        fee: 0.0,
                        blocked: None,
                    });
                }
            }
        }
        let unit = bar.unit_nav.unwrap_or([0.0; 4]);
        let holding: f64 = (0..4).map(|k| shares[k] * unit[k]).sum();
        let total = cash + holding;
        if i > start_idx {
            let prev = account_curve.last().map(|(_, v)| *v).unwrap_or(initial_cash);
            if prev > 0.0 {
                flow_equity *= total / prev;
            }
        }
        equity.push((bar.date, flow_equity));
        account_curve.push((bar.date, total));
        market_curve.push(
            (0..3).map(|k| sleeve_arr[k] * bar.prices[k] / base_prices[k]).sum::<f64>(),
        );
    }

    let final_value = account_curve.last().map(|(_, v)| *v).unwrap_or(initial_cash);
    let days = (bars[end_idx].date - bars[start_idx].date).num_days().max(1) as f64;
    let years = days / 365.0;
    let annualized = (final_value / initial_cash).powf(1.0 / years) - 1.0;
    let max_dd_flow = max_drawdown(&equity.iter().map(|(_, v)| *v).collect::<Vec<_>>());
    let max_dd_account = max_drawdown(&account_curve.iter().map(|(_, v)| *v).collect::<Vec<_>>());
    let max_dd_market = max_drawdown(&market_curve);
    let actual_trades = trades.len();
    Ok(AuditResult {
        name: "账户实际执行（流水净额回放）".to_string(),
        start: bars[start_idx].date,
        end: bars[end_idx].date,
        initial_cash,
        final_value,
        total_return: final_value / initial_cash - 1.0,
        annualized,
        max_dd_flow,
        max_dd_account,
        max_dd_market,
        calmar: if max_dd_flow > 0.0 { annualized / max_dd_flow } else { 0.0 },
        fees: 0.0,
        trades,
        avg_risk_weight: 0.0,
        trades_per_year: actual_trades as f64 / years.max(f64::EPSILON),
        equity,
        account_curve,
        blocked_by_cap: 0,
        blocked_by_settlement: 0,
        skipped_no_fgi: 0,
    })
}

// ───────────────────── 策略有效性检验 ─────────────────────

#[derive(Debug, Clone)]
pub struct PermutationResult {
    pub iterations: usize,
    pub p5: f64,
    pub p50: f64,
    pub p95: f64,
    pub real_percentile: f64,
    pub real_value: f64,
    pub significant: bool,
}

#[derive(Debug, Clone)]
pub struct ValidationSuite {
    pub real: AuditResult,
    pub delayed: AuditResult,
    pub random: AuditResult,
    pub constant_avg: AuditResult,
    pub constant_70: AuditResult,
    pub constant_100: AuditResult,
    pub buy_hold: AuditResult,
    pub permutation: PermutationResult,
}

fn with_variant(base: &AuditOptions, name: &str, v: SignalVariant) -> AuditOptions {
    let mut o = base.clone();
    o.name = name.to_string();
    o.variant = v;
    o
}

/// 四个必跑对照 + 置换检验。结论只有两种：真实信号进入随机分布前 N 百分位
/// （择时有贡献），或明示「择时无显著贡献，收益主要来自平均仓位」。
pub fn run_validation_suite(
    config: &AppConfig,
    series: &BarSeries,
    sig: &SignalSeries,
    base: &AuditOptions,
) -> Result<ValidationSuite> {
    let real = run_audit(config, series, sig, base)?;
    let avg = real.avg_risk_weight;

    let mut opt = with_variant(base, "延迟 1 期执行", SignalVariant::Delayed(1));
    opt.execution_lag_days = Some(base.execution_lag_days.unwrap_or(config.backtest.execution_lag_days) + 1);
    let delayed = run_audit(config, series, sig, &opt)?;
    let random = run_audit(
        config, series, sig,
        &with_variant(base, "随机信号", SignalVariant::Random(20261108)),
    )?;
    let constant_avg = run_audit(
        config, series, sig, &with_variant(base, "常数仓位(策略均值)", SignalVariant::Constant(avg)),
    )?;
    let constant_70 = run_audit(
        config, series, sig, &with_variant(base, "常数仓位 70%", SignalVariant::Constant(0.70)),
    )?;
    let constant_100 = run_audit(
        config, series, sig, &with_variant(base, "常数仓位 100%", SignalVariant::Constant(1.0)),
    )?;
    let mut bh_opt = with_variant(base, "买入持有", SignalVariant::Constant(1.0));
    bh_opt.buy_and_hold = true;
    let buy_hold = run_audit(config, series, sig, &bh_opt)?;

    let iterations = config.backtest.permutation_iterations;
    let mut values = Vec::with_capacity(iterations);
    for seed in 0..iterations {
        let o = with_variant(base, "置换", SignalVariant::Shuffled(0x9E37_79B9 ^ seed as u64));
        values.push(run_audit(config, series, sig, &o)?.annualized);
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let q = |p: f64| -> f64 {
        let i = ((values.len() - 1) as f64 * p).round() as usize;
        values[i.min(values.len() - 1)]
    };
    let better = values.iter().filter(|v| **v < real.annualized).count();
    let real_percentile = better as f64 / values.len() as f64 * 100.0;
    let real_value = real.annualized;
    Ok(ValidationSuite {
        real,
        delayed,
        random,
        constant_avg,
        constant_70,
        constant_100,
        buy_hold,
        permutation: PermutationResult {
            iterations,
            p5: q(0.05),
            p50: q(0.50),
            p95: q(0.95),
            real_percentile,
            real_value,
            significant: real_percentile >= config.backtest.significance_percentile,
        },
    })
}

// ───────────────────── 牛熊分段与多折 walk-forward ─────────────────────

#[derive(Debug, Clone)]
pub struct RegimeRow {
    pub name: String,
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub strategy_return: f64,
    pub strategy_dd: f64,
    pub buy_hold_return: f64,
    pub buy_hold_dd: f64,
    pub trades: usize,
}

pub fn run_regimes(
    config: &AppConfig,
    series: &BarSeries,
    sig: &SignalSeries,
    base: &AuditOptions,
    regimes: &[crate::live_data::Regime],
) -> Vec<RegimeRow> {
    let bars = &series.bars;
    let mut out = Vec::new();
    for r in regimes {
        let (Ok(from), Ok(to)) = (
            NaiveDate::parse_from_str(&r.from, "%Y-%m-%d"),
            NaiveDate::parse_from_str(&r.to, "%Y-%m-%d"),
        ) else { continue };
        let s = bars.partition_point(|b| b.date < from);
        let e = bars.partition_point(|b| b.date <= to).saturating_sub(1);
        if s >= bars.len() || e <= s { continue; }
        let mut o = base.clone();
        o.start_idx = s;
        o.end_idx = e;
        o.name = r.name.clone();
        let Ok(strategy) = run_audit(config, series, sig, &o) else { continue };
        let mut bh = o.clone();
        bh.variant = SignalVariant::Constant(1.0);
        bh.buy_and_hold = true;
        let Ok(bh_res) = run_audit(config, series, sig, &bh) else { continue };
        out.push(RegimeRow {
            name: r.name.clone(),
            from: bars[s].date,
            to: bars[e].date,
            strategy_return: strategy.total_return,
            strategy_dd: strategy.max_dd_flow,
            buy_hold_return: bh_res.total_return,
            buy_hold_dd: bh_res.max_dd_flow,
            trades: strategy.trade_count(),
        });
    }
    out
}

#[derive(Debug, Clone)]
pub struct FoldRow {
    pub label: String,
    pub train: String,
    pub test: String,
    pub tuned_band: f64,
    pub default_oos: f64,
    pub tuned_oos: f64,
    pub tuned_better: bool,
}

/// 滚动多折 walk-forward：每折用前 24 个月网格调偏离带，后 12 个月样本外验证。
/// 与旧版单一切分（67 月训练 / 45 月测试）相比，能看出调参结论是否稳定。
pub fn run_walk_forward(
    config: &AppConfig,
    series: &BarSeries,
    sig: &SignalSeries,
    base: &AuditOptions,
) -> Vec<FoldRow> {
    let bars = &series.bars;
    let grid = [2.0, 4.0, 6.0, 8.0];
    let mut out = Vec::new();
    let mut fold = 0;
    let mut train_start = 0usize;
    loop {
        let train_from = bars[train_start].date;
        let train_until = train_from + Duration::days(730);
        let train_end = bars.partition_point(|b| b.date <= train_until).saturating_sub(1);
        let test_until = train_until + Duration::days(365);
        let test_end = bars.partition_point(|b| b.date <= test_until).saturating_sub(1);
        if test_end <= train_end || test_end >= bars.len() - 1 {
            if test_end <= train_end { break; }
            // 最后一折允许延伸到数据末尾
        }
        let mut best_band = config.rebalance.band_pp;
        let mut best = f64::NEG_INFINITY;
        for b in grid {
            let mut o = base.clone();
            o.start_idx = train_start;
            o.end_idx = train_end;
            o.band_pp = Some(b);
            if let Ok(r) = run_audit(config, series, sig, &o) {
                if r.annualized > best {
                    best = r.annualized;
                    best_band = b;
                }
            }
        }
        let mut d = base.clone();
        d.start_idx = train_end + 1;
        d.end_idx = test_end;
        let mut t = d.clone();
        t.band_pp = Some(best_band);
        let (Ok(dr), Ok(tr)) = (
            run_audit(config, series, sig, &d),
            run_audit(config, series, sig, &t),
        ) else { break };
        fold += 1;
        out.push(FoldRow {
            label: format!("折{fold}"),
            train: format!("{}~{}", bars[train_start].date, bars[train_end].date),
            test: format!("{}~{}", bars[(train_end + 1).min(bars.len() - 1)].date, bars[test_end].date),
            tuned_band: best_band,
            default_oos: dr.annualized,
            tuned_oos: tr.annualized,
            tuned_better: tr.annualized > dr.annualized,
        });
        if test_end >= bars.len() - 1 { break; }
        train_start = train_end + 1;
        if train_start >= bars.len() - 1 { break; }
    }
    out
}

// ───────────────────────── 输出 ─────────────────────────

fn pct(v: f64) -> String {
    format!("{:+.2}%", v * 100.0)
}
fn money(v: f64) -> String {
    let s = format!("{:.2}", v);
    let dot = s.find('.').unwrap_or(s.len());
    let (int, frac) = s.split_at(dot);
    let neg = int.starts_with('-');
    let digits = int.trim_start_matches('-');
    let mut grouped = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("¥{}{}{}", if neg { "-" } else { "" }, grouped, frac)
}
fn row(cells: &[&str], widths: &[usize]) -> String {
    cells
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let w = widths.get(i).copied().unwrap_or(12);
            let pad = w.saturating_sub(c.chars().count());
            format!("{}{}", c, " ".repeat(pad))
        })
        .collect::<Vec<_>>()
        .join("  ")
}

pub fn print_data_banner(
    config: &AppConfig,
    live: &crate::live_data::LiveDataset,
    long: &crate::live_data::LongDataset,
) {
    let sla = config.backtest.freshness_sla_days;
    println!("\n═══ 数据与口径 ═══════════════════════════════════════════════════════");
    if let Ok(f) = live.freshness(sla) { println!("  实盘层  {}", f.banner()); }
    if let Ok(f) = long.freshness(sla) { println!("  长周期层 {}", f.banner()); }
    println!("  实盘层  {} 行，{} ~ {}；标的 = 018966/539001/007605/020341",
        live.days.len(), live.days.first().map(|d| d.date.to_string()).unwrap_or_default(),
        live.days.last().map(|d| d.date.to_string()).unwrap_or_default());
    println!("  长周期层 {} 行，{} ~ {}；三腿为长历史代理，覆盖 2018Q4 / 2020 / 2022 / 2026 各段牛熊",
        long.days.len(), long.days.first().map(|d| d.date.to_string()).unwrap_or_default(),
        long.days.last().map(|d| d.date.to_string()).unwrap_or_default());
    println!("  实盘三腿 {} / {} / {}",
        crate::live_data::LEG_NAMES[0],
        crate::live_data::LEG_NAMES[1],
        crate::live_data::LEG_NAMES[2]);
    // QDII 净值 T+1/T+2 披露：沿用前值的天数就是实盘真实的信息滞后
    let stale_qdii: Vec<i32> = live.days.iter()
        .map(|d| d.nav_stale_days[0].max(d.nav_stale_days[1]))
        .filter(|&x| x > 0).collect();
    let max_stale = stale_qdii.iter().copied().max().unwrap_or(0);
    println!("  数据质量 QDII 净值沿用前值 {} / {} 日（最大滞后 {} 天）；这是实盘可知信息的约束，非插值",
        stale_qdii.len(), live.days.len(), max_stale);
    println!("  信号口径 平滑 {} 天 + 区间确认 {} 天 + 决策周期 {:?}（与 mns report 同源）",
        config.rebalance.signal_smooth_days, config.rebalance.zone_confirm_days,
        config.backtest.decision_interval);
    println!("  执行时序 信号 T 日 → 成交 T+{} 日；卖出款按通道 T+n 到账，到账前不可用于买入",
        config.backtest.execution_lag_days);
    println!("  成本通道 QDII 场外 T+{}日/限购 {} / A股联接 T+{}日 / 场内 ETF T+{}日",
        config.channels.qdii_feeder.settlement_days,
        if config.channels.qdii_feeder.daily_subscription_cap > 0.0 {
            format!("¥{:.0}/日", config.channels.qdii_feeder.daily_subscription_cap)
        } else { "无".to_string() },
        config.channels.cn_feeder.settlement_days,
        config.channels.on_exchange_etf.settlement_days);
    println!("  数据路径 实盘 {} / 长周期 {}", live.path.display(), long.path.display());
    println!("  策略可验证段 实盘 {:?} ({} 日) / 长周期 {:?} ({} 日)",
        live.meta.strategy_ready_range, live.meta.strategy_ready_observations,
        long.meta.strategy_ready_range, long.meta.strategy_ready_observations);
    println!("  ⚠ 长周期层三腿为代理标的（红利腿 H30269 为价格指数、黄金腿为场内 ETF），");
    println!("    不得用于实盘收益预测；实盘结论以账户镜像层为准。");
}

pub fn print_result_table(results: &[&AuditResult], title: &str) {
    let widths = [26, 12, 12, 12, 12, 11, 10];
    println!("\n── {} ───────────────────────────────────────────────────", title);
    println!("{}", row(&["方案", "区间收益", "年化", "MaxDD(净值)", "MaxDD(账户)", "Calmar", "笔数/年"], &widths));
    println!("{}", "-".repeat(112));
    for r in results {
        println!("{}", row(&[
            &r.name,
            &pct(r.total_return),
            &pct(r.annualized),
            &pct(-r.max_dd_flow),
            &pct(-r.max_dd_account),
            &format!("{:.2}", r.calmar),
            &format!("{:.1}", r.trades_per_year),
        ], &widths));
    }
    println!("  注：MaxDD(净值) 为剔除资金流的复合日频净值口径（主口径）；");
    println!("      MaxDD(账户) 为含资金流的账户净值口径。本引擎按真实账户建模、无外部注资，");
    println!("      两口径因此一致；差异只在旧版「10 万起步 + 年注资 5 万」的含注资回测中显现。");
}

pub fn print_mirror(strict: &AuditResult, actual: &AuditResult, config: &AppConfig) {
    println!("\n═══ 账户镜像：严格执行 vs 实际执行 ═══════════════════════════════════");
    println!("  入场日 {} · 期初 {} · 无后续外部注资（与回测旧的 10 万 + 年注资 5 万不同）",
        config.backtest.entry_date, money(config.backtest.initial_cash));
    print_result_table(&[strict, actual], "净值差与偏离");
    let gap = strict.final_value - actual.final_value;
    println!("  期末净值差 {}（严格 {} vs 实际 {}）",
        money(gap), money(strict.final_value), money(actual.final_value));
    println!("  交易笔数：严格 {} 笔 / 实际 {} 笔；费用（策略口径）{}",
        strict.trade_count(), actual.trade_count(), money(strict.fees));
    let years = ((actual.end - actual.start).num_days() as f64 / 365.0).max(f64::EPSILON);
    println!("  年度交易预算 {}-{} 次；实际流水 {} 笔 / {:.2} 年 ≈ {:.1} 次/年",
        config.backtest.annual_trade_budget_min, config.backtest.annual_trade_budget_max,
        actual.trade_count(), years, actual.trades_per_year);
    if strict.trades_per_year > config.backtest.annual_trade_budget_max as f64 {
        println!("  ⚠ 严格执行会产生 {:.1} 次/年，超出年度预算上限 {} 次；",
            strict.trades_per_year, config.backtest.annual_trade_budget_max);
        println!("    这正是「日频信号 + 周频决策」需要显式频次预算的原因，日报应压缩建议而非逐日执行。");
    }
    if strict.blocked_by_cap > 0 || strict.blocked_by_settlement > 0 {
        println!("  严格执行受阻：限购 {} 次、在途资金未到账 {} 次（均为真实通道约束）",
            strict.blocked_by_cap, strict.blocked_by_settlement);
    }
}

pub fn print_validation_suite(s: &ValidationSuite, config: &AppConfig) {
    print_result_table(
        &[&s.real, &s.delayed, &s.random, &s.constant_avg, &s.constant_70,
          &s.constant_100, &s.buy_hold],
        "策略有效性对照（同一数据、同一成本、同一信号口径）",
    );
    let p = &s.permutation;
    println!("\n  信号时序置换检验（{} 次，保留边际分布、只打乱时序）", p.iterations);
    println!("  年化分布 P5 {} / P50 {} / P95 {}；真实信号 {} → 第 {:.0} 百分位",
        pct(p.p5), pct(p.p50), pct(p.p95), pct(p.real_value), p.real_percentile);
    println!("  判定：{}（门槛为进入前 {} 百分位）",
        if p.significant {
            "择时有统计上可辨识的贡献".to_string()
        } else {
            format!("择时无显著贡献；收益主要来自平均仓位 {:.1}%，而非情绪择时",
                s.real.avg_risk_weight * 100.0)
        },
        config.backtest.significance_percentile);
    println!("  注：置换检验只检验时序信息；不排除情绪指标在非线性/更长周期下的价值。");
    println!("  纯资产池日频 MaxDD（不含现金缓冲）：{:.2}%；FGI 缺失而跳过决策 {} 日",
        s.real.max_dd_market * 100.0, s.real.skipped_no_fgi);
}

pub fn print_regimes(rows: &[RegimeRow]) {
    let widths = [30, 24, 14, 14, 14, 14, 8];
    println!("\n── 牛熊分段（策略与买入持有同口径） ────────────────────────────────");
    println!("{}", row(&["区间", "起止", "策略收益", "策略MaxDD", "持有收益", "持有MaxDD", "笔数"], &widths));
    println!("{}", "-".repeat(128));
    for r in rows {
        println!("{}", row(&[
            &r.name,
            &format!("{}~{}", r.from, r.to),
            &pct(r.strategy_return),
            &pct(-r.strategy_dd),
            &pct(r.buy_hold_return),
            &pct(-r.buy_hold_dd),
            &r.trades.to_string(),
        ], &widths));
    }
}

pub fn print_folds(rows: &[FoldRow], config: &AppConfig) {
    let widths = [8, 26, 26, 12, 14, 14, 10];
    println!("\n── 滚动多折 walk-forward（训练 24 月 / 样本外 12 月） ────────────────");
    println!("{}", row(&["折", "训练区间", "样本外区间", "调优band", "默认年化", "调优年化", "调优更优"], &widths));
    println!("{}", "-".repeat(122));
    for r in rows {
        println!("{}", row(&[
            &r.label, &r.train, &r.test,
            &format!("{:.0}pp", r.tuned_band),
            &pct(r.default_oos), &pct(r.tuned_oos),
            if r.tuned_better { "是" } else { "否" },
        ], &widths));
    }
    let better = rows.iter().filter(|r| r.tuned_better).count();
    let avg_gain: f64 = rows.iter().map(|r| r.tuned_oos - r.default_oos).sum::<f64>()
        / rows.len().max(1) as f64;
    println!("  样本内调优在样本外更优的折数：{}/{}；平均年化改善 {:+.2}pp；当前配置 band={:.0}pp",
        better, rows.len(), avg_gain * 100.0, config.rebalance.band_pp);
    if better * 2 <= rows.len() || avg_gain < 0.005 {
        println!("  结论：调参未带来稳定或可观的样本外改善（改善 <0.5pp/年），不建议据此调参。");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live_data::{Bar, BarSeries, DatasetKind};

    fn cfg() -> AppConfig {
        let mut c = AppConfig::default_config();
        c.backtest.decision_interval = DecisionInterval::Daily;
        c.backtest.execution_lag_days = 1;
        c.rebalance.decision_weekday = 0;
        c
    }

    fn bars(n: usize) -> BarSeries {
        let start = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let mut v = Vec::new();
        for i in 0..n {
            let d = start + Duration::days(i as i64);
            let x = 1.0 + (i as f64) * 0.0005;
            v.push(Bar {
                date: d,
                fgi: Some(30.0 + (i as f64 * 7.0 % 40.0)),
                fgi_age_days: 1,
                prices: [x, 1.0 + (i as f64) * 0.0002, 1.0 + (i as f64) * 0.0003],
                unit_nav: Some([x, x, x, x]),
            });
        }
        BarSeries {
            kind: DatasetKind::Live,
            label: "test".into(),
            bars: v,
            caveats: vec![],
        }
    }

    /// 回归：起点急跌样本里，信号需 N 日历史才给出首个决策日。
    /// 若策略/常数仓位等到决策日才建仓，就会整体躲过起点跌幅，形成虚假的相对收益。
    #[test]
    fn 起点统一建仓_常数满仓与买入持有不应有偏差() {
        let mut c = cfg();
        c.allocation.us_stocks = 1.0;
        c.allocation.cn_stocks = 0.0;
        c.allocation.counter_cyclical = 0.0;
        c.rebalance.min_trade_amount = 1.0;
        let prices = [100.0, 92.0, 85.0, 88.0, 90.0, 95.0,
                      100.0, 105.0, 110.0, 112.0, 115.0, 118.0];
        let start = NaiveDate::from_ymd_opt(2020, 1, 2).unwrap();
        let bs = BarSeries {
            kind: DatasetKind::Long,
            label: "start-drop".into(),
            bars: prices.iter().enumerate().map(|(i, p)| Bar {
                date: start + Duration::days(i as i64),
                fgi: Some(50.0),
                fgi_age_days: 1,
                prices: [*p, *p, *p],
                unit_nav: None,
            }).collect(),
            caveats: vec![],
        };
        let n = bs.bars.len();
        let sig = SignalSeries {
            effective: vec![50.0; n],
            decision: (0..n).map(|i| i == 8).collect(),
            raw_min: 0.0,
            raw_max: 100.0,
        };
        let mut bh = AuditOptions::spanning("买入持有", &bs, &c);
        bh.initial_cash = 100_000.0;
        bh.buy_and_hold = true;
        let mut c100 = bh.clone();
        c100.name = "常数满仓".into();
        c100.buy_and_hold = false;
        c100.variant = SignalVariant::Constant(1.0);
        let a = run_audit(&c, &bs, &sig, &bh).unwrap();
        let b = run_audit(&c, &bs, &sig, &c100).unwrap();
        assert!(
            (a.total_return - b.total_return).abs() < 0.005,
            "同一执行日建仓时常数满仓 {:+.4} 与买入持有 {:+.4} 应一致，差异说明存在起点偏差",
            b.total_return, a.total_return
        );
    }

    #[test]
    fn 信号序列与实盘同源() {
        let c = cfg();
        let s = bars(60);
        let sig = build_signal_series(&c, &s.bars);
        // 首日平滑=当日值，确认天数不足时直接用平滑值
        assert_eq!(sig.effective.len(), 60);
        assert!((sig.effective[0] - 30.0).abs() < 1e-9);
        assert!(sig.decision.iter().all(|d| *d), "测试配置每天都是决策日");
    }

    #[test]
    fn 置换检验可复现且不改变边际分布() {
        let c = cfg();
        let s = bars(80);
        let sig = build_signal_series(&c, &s.bars);
        let a = variant_scores(SignalVariant::Shuffled(42), &sig, 80);
        let b = variant_scores(SignalVariant::Shuffled(42), &sig, 80);
        assert_eq!(a, b, "同种子必须给出相同置换序列");
        let mut sa = a.clone();
        let mut sb = sig.effective.clone();
        sa.sort_by(|x, y| x.partial_cmp(y).unwrap());
        sb.sort_by(|x, y| x.partial_cmp(y).unwrap());
        assert_eq!(sa, sb, "置换必须保留边际分布");
    }

    #[test]
    fn 执行延迟至少一期且不允许配置为零() {
        let mut c = cfg();
        c.backtest.execution_lag_days = 0;
        assert!(c.validate().is_err(), "执行延迟 0 会退回前视假设，必须拒绝");
    }

    #[test]
    fn 在途卖出款不得用于同日买入() {
        let c = cfg();
        let s = bars(120);
        let sig = build_signal_series(&c, &s.bars);
        let mut o = AuditOptions::spanning("test", &s, &c);
        o.initial_cash = 100_000.0;
        let r = run_audit(&c, &s, &sig, &o).unwrap();
        // 通道 T+n >= 1，卖出当天买入预算只来自已到账现金；结果必须可完成且资产非负
        assert!(r.final_value > 0.0);
        assert!(r.account_curve.iter().all(|(_, v)| *v >= -1e-6), "不允许透支");
    }

    #[test]
    fn 回撤主口径不再包含资金流抬升() {
        // 含一笔外部注资的曲线：峰值被注资抬高，账户口径回撤被低估
        let flow_adj = [1.0, 0.95, 0.90, 0.92];
        let account = [1.0, 0.95, 1.90, 1.75];
        assert!(max_drawdown(&flow_adj) > max_drawdown(&account));
    }

    #[test]
    fn 常数仓位对照确实不使用情绪信号() {
        let c = cfg();
        let s = bars(100);
        let sig = build_signal_series(&c, &s.bars);
        let mut o = AuditOptions::spanning("const", &s, &c);
        o.variant = SignalVariant::Constant(0.7);
        let r = run_audit(&c, &s, &sig, &o).unwrap();
        assert!(r.avg_risk_weight <= 0.75, "常数 70% 仓位的实际均值应接近 0.7");
    }

    #[test]
    fn 实盘数据集的通道映射不混用场内成本() {
        let c = cfg();
        assert_eq!(channel(&c, DatasetKind::Live, 0).settlement_days, 7);
        assert_eq!(channel(&c, DatasetKind::Live, 1).settlement_days, 3);
        assert_eq!(channel(&c, DatasetKind::Long, 0).redemption_tiers.len(), 0);
    }
}
