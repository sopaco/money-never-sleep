//! 绩效与风险指标。
//!
//! 要点：存在分批注资时，`(期末/总投入)^(1/年数)` 并不是年化收益——它忽略了
//! 资金何时到账。正确的现金流加权收益是 XIRR，本模块以其为主口径。

use chrono::NaiveDate;

/// 一笔现金流：流入组合为负，流出（含期末市值）为正。
#[derive(Debug, Clone, Copy)]
pub struct CashFlow {
    pub date: NaiveDate,
    pub amount: f64,
}

/// XIRR：现金流加权年化收益率。
///
/// 先用牛顿法，失败则退回二分法，均失败返回 None。
pub fn xirr(flows: &[CashFlow]) -> Option<f64> {
    if flows.len() < 2 {
        return None;
    }
    let has_pos = flows.iter().any(|f| f.amount > 0.0);
    let has_neg = flows.iter().any(|f| f.amount < 0.0);
    if !(has_pos && has_neg) {
        return None; // 无符号变化则方程无解
    }
    let t0 = flows.iter().map(|f| f.date).min()?;
    let years: Vec<f64> = flows
        .iter()
        .map(|f| (f.date - t0).num_days() as f64 / 365.0)
        .collect();

    let npv = |r: f64| -> f64 {
        flows
            .iter()
            .zip(&years)
            .map(|(f, y)| f.amount / (1.0 + r).powf(*y))
            .sum()
    };

    // 牛顿法
    let mut rate = 0.1;
    for _ in 0..100 {
        let f = npv(rate);
        if f.abs() < 1e-9 {
            return Some(rate);
        }
        // 数值导数
        let h = 1e-6;
        let d = (npv(rate + h) - f) / h;
        if d.abs() < 1e-12 {
            break;
        }
        let next = rate - f / d;
        if !next.is_finite() || next <= -0.9999 {
            break;
        }
        if (next - rate).abs() < 1e-12 {
            return Some(next);
        }
        rate = next;
    }

    // 二分法兜底
    let (mut lo, mut hi) = (-0.9999_f64, 10.0_f64);
    let (mut flo, fhi) = (npv(lo), npv(hi));
    if flo * fhi > 0.0 {
        return None;
    }
    for _ in 0..300 {
        let mid = (lo + hi) / 2.0;
        let fm = npv(mid);
        if fm.abs() < 1e-10 {
            return Some(mid);
        }
        if flo * fm <= 0.0 {
            hi = mid;
        } else {
            lo = mid;
            flo = fm;
        }
    }
    Some((lo + hi) / 2.0)
}

/// 最大回撤（基于净值序列，返回正的小数）
pub fn max_drawdown(values: &[f64]) -> f64 {
    let mut peak = f64::NEG_INFINITY;
    let mut mdd = 0.0_f64;
    for v in values {
        if *v > peak {
            peak = *v;
        }
        if peak > 0.0 {
            mdd = mdd.max((peak - v) / peak);
        }
    }
    mdd
}

pub fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.iter().sum::<f64>() / xs.len() as f64
}

/// 样本标准差
pub fn stddev(xs: &[f64]) -> f64 {
    if xs.len() < 2 {
        return 0.0;
    }
    let m = mean(xs);
    let var = xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (xs.len() - 1) as f64;
    var.sqrt()
}

/// 下行标准差（只统计低于门槛的偏离），用于 Sortino
pub fn downside_dev(xs: &[f64], threshold: f64) -> f64 {
    if xs.len() < 2 {
        return 0.0;
    }
    let sum: f64 = xs
        .iter()
        .map(|x| if *x < threshold { (x - threshold).powi(2) } else { 0.0 })
        .sum();
    (sum / (xs.len() - 1) as f64).sqrt()
}

/// 风险调整指标。`periods_per_year` 月频传 12。
#[derive(Debug, Clone, Copy)]
pub struct RiskMetrics {
    pub sharpe: f64,
    pub sortino: f64,
    pub calmar: f64,
    pub volatility: f64,
}

/// `rf_annual` 为无风险年化（小数），`cagr` 用于 Calmar。
pub fn risk_metrics(
    returns: &[f64],
    periods_per_year: f64,
    rf_annual: f64,
    cagr: f64,
    max_dd: f64,
) -> RiskMetrics {
    let rf_period = (1.0 + rf_annual).powf(1.0 / periods_per_year) - 1.0;
    let excess: Vec<f64> = returns.iter().map(|r| r - rf_period).collect();
    let sd = stddev(returns);
    let dd = downside_dev(returns, rf_period);
    let ann = periods_per_year.sqrt();
    RiskMetrics {
        sharpe: if sd > 0.0 { mean(&excess) / sd * ann } else { 0.0 },
        sortino: if dd > 0.0 { mean(&excess) / dd * ann } else { 0.0 },
        calmar: if max_dd > 0.0 { cagr / max_dd } else { 0.0 },
        volatility: sd * ann,
    }
}

/// 确定性伪随机数（xorshift64*），避免引入 rand 依赖，且保证回测可复现。
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(if seed == 0 { 0x9E3779B97F4A7C15 } else { seed })
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next_u64() % n as u64) as usize }
    }
}

/// 分块 bootstrap：保留短期自相关（动量/均值回归）后重采样收益序列。
///
/// 返回每次重采样的 (年化收益, 最大回撤)。
pub fn block_bootstrap(
    returns: &[f64],
    block: usize,
    iterations: usize,
    periods_per_year: f64,
    seed: u64,
) -> Vec<(f64, f64)> {
    if returns.len() < block.max(2) || iterations == 0 {
        return Vec::new();
    }
    let mut rng = Rng::new(seed);
    let n = returns.len();
    let mut out = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let mut path = Vec::with_capacity(n);
        while path.len() < n {
            let start = rng.below(n);
            for k in 0..block {
                if path.len() >= n {
                    break;
                }
                path.push(returns[(start + k) % n]);
            }
        }
        let mut value = 1.0;
        let mut values = Vec::with_capacity(n + 1);
        values.push(value);
        for r in &path {
            value *= 1.0 + r;
            values.push(value);
        }
        let years = n as f64 / periods_per_year;
        let cagr = if value > 0.0 && years > 0.0 {
            value.powf(1.0 / years) - 1.0
        } else {
            -1.0
        };
        out.push((cagr, max_drawdown(&values)));
    }
    out
}

/// 取分位数（p 为 0-1，线性插值）
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let idx = p.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lo = idx.floor() as usize;
    let hi = idx.ceil() as usize;
    if lo == hi {
        return sorted[lo];
    }
    sorted[lo] + (sorted[hi] - sorted[lo]) * (idx - lo as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn xirr_单笔投入翻倍一年应为百分之百() {
        // 用非闰年跨度（365天），与 XIRR 的 365 天惯例一致
        let flows = vec![
            CashFlow { date: d(2021, 1, 1), amount: -100.0 },
            CashFlow { date: d(2022, 1, 1), amount: 200.0 },
        ];
        let r = xirr(&flows).unwrap();
        assert!((r - 1.0).abs() < 1e-4, "期望约100%, 实际{}", r);
    }

    #[test]
    fn xirr_采用365天惯例_闰年跨度略低于百分之百() {
        // 2020-01-01 -> 2021-01-01 实为 366 天，故 2^(365/366)-1 ≈ 99.62%
        let flows = vec![
            CashFlow { date: d(2020, 1, 1), amount: -100.0 },
            CashFlow { date: d(2021, 1, 1), amount: 200.0 },
        ];
        let r = xirr(&flows).unwrap();
        assert!((r - 0.99621).abs() < 1e-3, "实际{}", r);
    }

    #[test]
    fn xirr_零收益应为零() {
        let flows = vec![
            CashFlow { date: d(2020, 1, 1), amount: -100.0 },
            CashFlow { date: d(2023, 1, 1), amount: 100.0 },
        ];
        assert!(xirr(&flows).unwrap().abs() < 1e-6);
    }

    #[test]
    fn xirr_区别于简单年化_晚到的资金不应被当作全程投入() {
        // 期初10万，第2年末又投10万，期末25万。
        // 简单口径 (25/20)^(1/3)-1 ≈ 7.7%，但第二笔只投了1年，真实年化更高。
        let flows = vec![
            CashFlow { date: d(2020, 1, 1), amount: -100_000.0 },
            CashFlow { date: d(2022, 1, 1), amount: -100_000.0 },
            CashFlow { date: d(2023, 1, 1), amount: 250_000.0 },
        ];
        let x = xirr(&flows).unwrap();
        let naive = (250_000.0_f64 / 200_000.0).powf(1.0 / 3.0) - 1.0;
        assert!(x > naive, "XIRR({:.4}) 应高于简单年化({:.4})", x, naive);
    }

    #[test]
    fn xirr_亏损应为负() {
        let flows = vec![
            CashFlow { date: d(2020, 1, 1), amount: -100.0 },
            CashFlow { date: d(2021, 1, 1), amount: 50.0 },
        ];
        assert!(xirr(&flows).unwrap() < 0.0);
    }

    #[test]
    fn 最大回撤计算() {
        // 100 -> 120 -> 60 -> 90：峰值120跌到60 = 50%
        let mdd = max_drawdown(&[100.0, 120.0, 60.0, 90.0]);
        assert!((mdd - 0.5).abs() < 1e-9, "{}", mdd);
    }

    #[test]
    fn 单调上涨无回撤() {
        assert_eq!(max_drawdown(&[1.0, 2.0, 3.0]), 0.0);
    }

    #[test]
    fn 下行标准差只统计亏损侧() {
        // 全为正收益时下行偏离为0
        assert_eq!(downside_dev(&[0.05, 0.03, 0.02], 0.0), 0.0);
        assert!(downside_dev(&[-0.05, 0.03, -0.02], 0.0) > 0.0);
    }

    #[test]
    fn sortino_应高于sharpe_当亏损少于波动() {
        // 收益偏正、少量小幅回调 → 下行风险小于总波动
        let rets = vec![0.05, 0.06, -0.01, 0.07, 0.04, -0.005, 0.05];
        let m = risk_metrics(&rets, 12.0, 0.0, 0.30, 0.05);
        assert!(m.sortino > m.sharpe, "sortino={} sharpe={}", m.sortino, m.sharpe);
    }

    #[test]
    fn calmar_为年化除以最大回撤() {
        let m = risk_metrics(&[0.01, 0.02], 12.0, 0.0, 0.10, 0.25);
        assert!((m.calmar - 0.4).abs() < 1e-9, "{}", m.calmar);
    }

    #[test]
    fn bootstrap_可复现且数量正确() {
        let rets: Vec<f64> = (0..60).map(|i| if i % 5 == 0 { -0.02 } else { 0.01 }).collect();
        let a = block_bootstrap(&rets, 6, 50, 12.0, 42);
        let b = block_bootstrap(&rets, 6, 50, 12.0, 42);
        assert_eq!(a.len(), 50);
        assert_eq!(a[10].0, b[10].0, "同种子应可复现");
    }

    #[test]
    fn 分位数插值() {
        let v = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(percentile(&v, 0.0), 1.0);
        assert_eq!(percentile(&v, 1.0), 5.0);
        assert_eq!(percentile(&v, 0.5), 3.0);
    }
}
