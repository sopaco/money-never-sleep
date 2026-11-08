//! 决策信号处理：平滑、区间确认、决策日过滤。
//!
//! 背景：FGI 的均值回归周期是数周，而中长线组合的决策周期是数月——直接用
//! 当日原始值驱动仓位存在时间尺度错配（回测引擎里 `SignalConfig` 的注释已
//! 论证过这一点，那里默认 `smooth_months: 3`）。本模块把同一思想下沉到日
//! 线，供 `mns report` 使用：
//!
//! 1. **平滑**（方案 A）：决策分数用最近 `signal_smooth_days` 天原始值的均值，
//!    而非当日值；单日毛刺不再引起目标仓位跳变。
//! 2. **区间确认**（方案 B）：平滑值进入新区间需连续 `zone_confirm_days` 天
//!    才生效；未确认时沿用穿出前的旧区间，从机制上防 whipsaw。
//! 3. **决策日**（方案 C）：`decision_weekday` 指定每周哪天出决策版报告
//!    （0 = 每天），其余天只输出观察版。
//!
//! 历史数据来自本地 `fear_greed_snapshots` 表（`mns report` 每天写入），
//! 与 CNN API 格式脱耦；历史不足时逐级降级（见 `DecisionSignal::note`）。

use crate::config::AppConfig;
use chrono::{Datelike, NaiveDate};

/// 快照表中按日期取的 (日期, 原始分数) 记录，升序
pub type ScoreHistory = Vec<(NaiveDate, f64)>;

/// 一天的决策信号
#[derive(Debug, Clone)]
pub struct DecisionSignal {
    /// 当日原始分数（仅展示用，不驱动决策）
    pub raw_score: f64,
    /// 当日原始区间
    pub raw_zone: String,
    /// 平滑分数（最近 smoothing 窗口均值）
    pub smoothed_score: f64,
    /// 平滑值所在区间
    pub smoothed_zone: String,
    /// **决策用**分数：已确认时 = 平滑值；未确认时 = 旧区间下最后一次平滑值
    pub effective_score: f64,
    /// 决策用区间
    pub effective_zone: String,
    /// 区间是否已确认（连续 confirm 天在同一区间）
    pub confirmed: bool,
    /// 今天是否为决策日
    pub decision_day: bool,
    /// 今天应出的报告类型："决策" / "观察"
    pub report_kind: &'static str,
    /// 平滑实际用到的样本数（不足窗口时小于窗口长度）
    pub samples_used: usize,
    /// 降级/确认状态的用户可读说明
    pub note: String,
}

/// 以 `base` 日为终点、取窗口内样本均值（不足窗口长则用现有样本）。
///
/// `series`（切片，升序）；窗口按"最近 w 条记录"计（FGI 每天都有值，本地缺口
/// 按"缺失即跳过"处理，避免节假日清空信号）。
fn smoothed_at(series: &[(NaiveDate, f64)], window: usize) -> f64 {
    if window <= 1 {
        return series
            .last()
            .map(|(_, s)| *s)
            .unwrap_or(0.0);
    }
    let lo = series.len().saturating_sub(window);
    let slice = &series[lo..];
    // 窗口跨日历缺口时只取最近 window 天内（含缺口跳过）
    if let Some(&(last_date, _)) = series.last() {
        let cutoff = last_date - chrono::Duration::days(window as i64 - 1);
        let slice: ScoreHistory = slice.iter().filter(|(d, _)| *d >= cutoff).cloned().collect();
        if !slice.is_empty() {
            return slice.iter().map(|(_, s)| s).sum::<f64>() / slice.len() as f64;
        }
    }
    if slice.is_empty() {
        return 0.0;
    }
    slice.iter().map(|(_, s)| s).sum::<f64>() / slice.len() as f64
}

/// 历史序列（升序，不含今天）每天向前看 `window` 条的平滑值序列
fn smoothed_series(history: &ScoreHistory, window: usize) -> Vec<f64> {
    (1..=history.len())
        .map(|n| smoothed_at(&history[..n], window))
        .collect()
}

/// 计算今天的决策信号。
///
/// `today_raw` 为 CNN 最新分数，`history` 为本地快照表取出的近期记录
/// （升序，**不含今天**——调用方应先 `save_fear_greed_snapshot` 再调本函数，
/// 由本函数把今天并入序列）。
pub fn compute(
    config: &AppConfig,
    today_raw: f64,
    history: &ScoreHistory,
    today: NaiveDate,
) -> DecisionSignal {
    let r = &config.rebalance;
    let smooth_w = r.signal_smooth_days.max(1) as usize;
    let confirm_d = r.zone_confirm_days.max(1) as usize;

    // 完整序列 = 历史 + 今天
    let mut series = history.clone();
    series.push((today, today_raw));
    // 供平滑/确认使用的近期窗口（原始分数，升序，含今天）
    let window_today = smoothed_at(&series, smooth_w);
    let samples_used = {
        let cutoff = today - chrono::Duration::days(smooth_w as i64 - 1);
        series.iter().filter(|(d, _)| *d >= cutoff).count()
    };

    let raw_zone = config.sentiment_zone(today_raw).to_string();
    let smoothed_zone = config.sentiment_zone(window_today).to_string();

    // ── 区间确认（方案 B）──
    // 把"今天 + 前 lookback 个历史平滑值"按连续相同区间分段：
    // 今天的段 ≥ confirm 天 → 确认；否则回退到此前最近一个稳定段
    // （单调大跌时会连续穿过两个区间，只有"分段扫描 + 回退"才稳）。
    let smooth_series = smoothed_series(history, smooth_w);
    let lookback = confirm_d * 3;
    let mut entries: Vec<(String, f64)> = Vec::with_capacity(lookback + 1);
    let hl = smooth_series.len();
    if hl >= lookback {
        for back in (0..lookback).rev() {
            let s = smooth_series[hl - 1 - back];
            entries.push((config.sentiment_zone(s).to_string(), s));
        }
    } else {
        for s in &smooth_series {
            entries.push((config.sentiment_zone(*s).to_string(), *s));
        }
    }
    entries.push((smoothed_zone.clone(), window_today));

    // 分段：每段记录 (区间, 段内最新平滑值, 段长)
    let mut runs: Vec<(String, f64, usize)> = Vec::new();
    for e in &entries {
        if let Some(last) = runs.last_mut() {
            if last.0 == e.0 {
                last.1 = e.1;
                last.2 += 1;
                continue;
            }
        }
        runs.push((e.0.clone(), e.1, 1));
    }
    let today_run_len = runs.last().map(|r| r.2).unwrap_or(0);
    // 历史样本足够（至少 confirm_d 天的序列）时才执行确认规则
    let enough_history = series.len() >= confirm_d;

    let (confirmed, effective_score, effective_zone, mut note) = if !enough_history {
        (
            true,
            window_today,
            smoothed_zone.clone(),
            format!(
                "本地快照不足 {} 天（现有 {} 天），区间确认暂不启用，直接采用平滑值",
                confirm_d,
                series.len()
            ),
        )
    } else if today_run_len >= confirm_d {
        (
            true,
            window_today,
            smoothed_zone.clone(),
            format!(
                "区间已确认：平滑值连续 {} 天处于「{}」",
                today_run_len, smoothed_zone
            ),
        )
    } else {
        // 未确认 → 回退到今天段之前最近的稳定段（≥ confirm 天）
        let stable = (0..runs.len().saturating_sub(1))
            .rev()
            .find(|i| runs[*i].2 >= confirm_d);
        if let Some(i) = stable {
            let (zone, score, len) = &runs[i];
            (
                false,
                *score,
                zone.clone(),
                format!(
                    "区间切换未确认：平滑值进入「{}」仅 {} 天，未满确认天数 {}，\
                     决策暂沿用旧区间「{}」(已持续 {} 天)",
                    runs.last().unwrap().0,
                    today_run_len,
                    confirm_d,
                    zone,
                    len
                ),
            )
        } else {
            (
                false,
                window_today,
                smoothed_zone.clone(),
                format!(
                    "区间频繁波动，近 {} 天无连续 {} 天的稳定区间，暂按平滑值执行",
                    lookback, confirm_d
                ),
            )
        }
    };

    // ── 决策日（方案 C）──
    // decision_weekday: 0 = 每天决策；1..=7 = 周一..周日
    let wd = today.weekday().num_days_from_monday() as u32 + 1; // 周一=1
    let decision_day = r.decision_weekday == 0 || r.decision_weekday == wd;
    if r.decision_weekday != 0 && !decision_day {
        let name = match wd {
            1 => "周一",
            2 => "周二",
            3 => "周三",
            4 => "周四",
            5 => "周五",
            6 => "周六",
            _ => "周日",
        };
        note.push_str(&format!("；今天是{}，非决策日（决策日=每周{}），只出观察报告",
            name,
            match r.decision_weekday {
                1 => "一",
                2 => "二",
                3 => "三",
                4 => "四",
                5 => "五",
                6 => "六",
                _ => "日",
            }));
    }

    DecisionSignal {
        raw_score: today_raw,
        raw_zone,
        smoothed_score: window_today,
        smoothed_zone,
        effective_score,
        effective_zone,
        confirmed,
        decision_day,
        report_kind: if decision_day { "决策" } else { "观察" },
        samples_used,
        note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use chrono::{Duration, NaiveDate};

    fn cfg() -> AppConfig {
        AppConfig::default_config()
    }

    fn day(today: NaiveDate, back: i64) -> NaiveDate {
        today - Duration::days(back)
    }

    /// `raws`：近几日的原始分数，**新→旧**排列（raws[0] = 昨天）。
    /// 历史按日期升序构造（与快照表读取顺序一致）。
    fn dsi(c: &AppConfig, today_raw: f64, raws: &[f64], today: NaiveDate) -> DecisionSignal {
        let history: ScoreHistory = (0..raws.len())
            .rev()
            .map(|i| (day(today, i as i64 + 1), raws[i]))
            .collect();
        compute(c, today_raw, &history, today)
    }

    #[test]
    fn 单日毛刺不应改变平滑值() {
        // 前 6 天 50，今天突然 20（极度恐慌），7 日均值≈45.7（中性）
        let c = cfg(); // smooth=7
        let today = NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
        let s = dsi(&c, 20.0, &[50.0, 50.0, 50.0, 50.0, 50.0, 50.0], today);
        assert!((s.smoothed_score - (20.0 + 50.0 * 6.0) / 7.0).abs() < 1e-6);
        assert_eq!(s.smoothed_zone, "中性", "单日暴跌不应翻转区间");
        assert_eq!(s.effective_zone, "中性");
        assert!(s.confirmed);
    }

    #[test]
    fn 平滑值穿越需确认天数才切换区间() {
        // 用平滑=2 的短窗口把"区间穿越"与"确认天数"解耦：
        // (20+50)/2=35 → 恐慌进入只需 1 天，确认与否只取决于连续天数
        let today = NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();

        // 场景1：今天平滑值刚进入新区间（1天），历史有稳定旧区间 → 沿用旧区间
        let mut c = cfg();
        c.rebalance.signal_smooth_days = 2;
        c.rebalance.zone_confirm_days = 3;
        let s1 = dsi(&c, 20.0, &[50.0; 9], today);
        assert_eq!(s1.smoothed_zone, "恐慌", "(20+50)/2=35 应落在恐慌区间");
        assert!(!s1.confirmed);
        assert_eq!(s1.effective_zone, "中性", "未确认应沿用旧区间");
        assert!(s1.note.contains("沿用旧区间"));

        // 场景2：连续 3 天平滑值在同一区间 → 确认（单调下跌连续穿过两区间也不乱）
        let s2 = dsi(&c, 20.0, &[20.0, 20.0, 25.0, 50.0], today);
        assert!(s2.confirmed, "连续3天应确认, note={}", s2.note);
        assert_eq!(s2.effective_zone, "极度恐慌");
        assert!((s2.effective_score - 20.0).abs() < 1e-6);

        // 场景3：连续 2 天 → 仍未确认，且旧区间稳定可持续
        let s3 = dsi(&c, 20.0, &[25.0, 20.0, 50.0, 50.0, 50.0, 50.0, 50.0, 50.0], today);
        assert!(!s3.confirmed);
        assert_eq!(s3.effective_zone, "中性");
    }

    #[test]
    fn 区间频繁波动且无稳定区间时回退到平滑值() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
        let mut c = cfg();
        c.rebalance.signal_smooth_days = 1; // 不平滑：区间=每日原始值
        c.rebalance.zone_confirm_days = 2;
        // 原始值 50↔30 交替，无任何连续 2 天的区间
        let s = dsi(&c, 35.0, &[50.0, 30.0, 50.0, 30.0], today);
        assert!(!s.confirmed);
        assert_eq!(s.effective_zone, "恐慌", "无稳定区间时按平滑值(=当日35)执行");
        assert!(s.note.contains("无连续"));
    }

    #[test]
    fn 未确认时决策分数取旧区间的平滑值() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
        let mut c = cfg();
        c.rebalance.signal_smooth_days = 2;
        c.rebalance.zone_confirm_days = 3;
        let s = dsi(&c, 20.0, &[50.0; 10], today);
        assert!(
            s.effective_score >= c.thresholds.fear,
            "决策分数应落于旧区间「中性」[45,55): {}",
            s.effective_score
        );
    }

    #[test]
    fn 快照不足时平滑降级到现有样本() {
        let c = cfg();
        let today = NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
        // 只有历史 1 天 + 今天
        let s = dsi(&c, 40.0, &[40.0], today);
        assert_eq!(s.samples_used, 2, "窗口7天但只有2天样本");
        assert!((s.smoothed_score - 40.0).abs() < 1e-6);
        assert_eq!(s.effective_zone, "恐慌");
    }

    #[test]
    fn 非决策日只出观察报告() {
        let mut c = cfg();
        c.rebalance.decision_weekday = 5; // 周五
        let friday = NaiveDate::from_ymd_opt(2026, 11, 6).unwrap(); // 2026-11-06 是周五
        let monday = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap(); // 周一
        let s1 = dsi(&c, 50.0, &[50.0; 9], friday);
        let s2 = dsi(&c, 50.0, &[50.0; 9], monday);
        assert!(s1.decision_day);
        assert_eq!(s1.report_kind, "决策");
        assert!(!s2.decision_day);
        assert_eq!(s2.report_kind, "观察");
        assert!(s2.note.contains("非决策日"));
    }

    #[test]
    fn decision_weekday为0时每天决策() {
        let c = cfg(); // 默认 0? 见 default；这里显式置 0
        let mut c = c;
        c.rebalance.decision_weekday = 0;
        let today = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap(); // 周一
        let s = dsi(&c, 50.0, &[50.0; 9], today);
        assert!(s.decision_day);
    }

    #[test]
    fn 平滑值跨日历缺口只取窗口内样本() {
        let c = cfg();
        let today = NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
        // 中间有 10 天缺口：30,40,50 (前) [缺口10天] 今天 40
        let mut history: ScoreHistory = vec![
            (day(today, 30), 30.0),
            (day(today, 29), 40.0),
            (day(today, 28), 50.0),
        ];
        history.reverse();
        let s = compute(&c, 40.0, &history, today);
        // 缺口10天: 今天 40 在 cutoff(=-6) 内，历史全在窗口外
        assert_eq!(s.samples_used, 1);
        assert_eq!(s.smoothed_zone, "恐慌", "窗口内只剩今天的40，应为恐慌区间");
    }
}
