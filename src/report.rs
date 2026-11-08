use anyhow::Result;
use chrono::{Datelike, Local};
use comfy_table::{Cell, Color, Table, modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL};
use std::fs;
use std::path::Path;

use unicode_width::UnicodeWidthStr;

use crate::config::AppConfig;

/// 按终端显示宽度补齐（中文占 2 列），避免 {:<n} 按字符数补齐导致的错位
fn pad_display(s: &str, width: usize) -> String {
    let w = UnicodeWidthStr::width(s);
    if w >= width {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(width - w))
    }
}
use crate::models::Position;
use crate::strategy::{LegPlan, RebalancePlan, RiskAdvice, RiskWarning};

/// 无动作时的真实原因：不能一律归为“在带宽内”。
#[derive(Clone)]
enum NoAction<'a> {
    /// 现金与持仓都为 0
    EmptyAccount,
    /// 所有腿都真实落在偏离带内
    InBand,
    /// 有腿被最小交易额 / 持有期 / 资金等规则挡住
    Blocked(Vec<&'a LegPlan>),
}

fn no_action_reason(plan: &RebalancePlan) -> NoAction<'_> {
    if plan.total_assets <= 0.0 {
        return NoAction::EmptyAccount;
    }
    let blocked: Vec<&LegPlan> = plan
        .legs
        .iter()
        .filter(|l| {
            !l.hold_reason
                .as_deref()
                .map(|r| r.contains("在带宽"))
                .unwrap_or(true)
        })
        .collect();
    if blocked.is_empty() {
        NoAction::InBand
    } else {
        NoAction::Blocked(blocked)
    }
}

/// 本月操作概况（用于报告 TL;DR 的第 4 行）。
#[derive(Debug, Clone, Default)]
pub struct MonthActivity {
    /// 本月已发生的买卖笔数
    pub trades: usize,
    /// 本月已发生交易的估算费用（按现行费率）
    pub estimated_fees: f64,
}

/// 只在展示层把金额向下取整到百位；所有腿金额本身保持精确。
fn round_hundred(v: f64) -> f64 {
    (v / 100.0).floor() * 100.0
}

/// 小额（<¥100）保留原值，避免费用被抹成 "¥0"。
fn fmt_money(v: f64) -> String {
    if v.abs() < 100.0 {
        format!("¥{:.0}", v)
    } else {
        format!("¥{:.0}", round_hundred(v))
    }
}

pub fn generate_report(
    config: &AppConfig,
    score: f64,
    rating: &str,
    raw_score: f64,
    raw_zone: &str,
    decision_note: &str,
    previous_close: Option<f64>,
    previous_1_week: Option<f64>,
    previous_1_month: Option<f64>,
    previous_1_year: Option<f64>,
    cash_balance: f64,
    positions: &[Position],
    plan: &RebalancePlan,
    risk_warnings: &[RiskWarning],
    activity: &MonthActivity,
    price_age_days: Option<i64>,
) -> Result<String> {
    let today = Local::now();
    let weekday = match today.weekday().num_days_from_monday() {
        0 => "Monday",
        1 => "Tuesday",
        2 => "Wednesday",
        3 => "Thursday",
        4 => "Friday",
        5 => "Saturday",
        _ => "Sunday",
    };
    let date_str = today.format("%Y-%m-%d").to_string();

    let mut report = String::new();

    // Header
    report.push_str(&format!(
        "═══════════════════════════════════════════════════\n\
         逆情绪投资助手 - 每日策略报告\n\
         {} ({})\n\
         ═════════════════════════════════════════════════\n\n",
        date_str, weekday
    ));

    if let Some(days) = price_age_days.filter(|d| *d >= 1) {
        report.push_str(&format!(
            "⚠ 价格数据为 {} 天前，以下市值与建议可能已失真，请先运行 `mns update-prices`。\n\n",
            days
        ));
    }

    // 结论先行：四行 TL;DR，长文保留在后面
    let fees = plan.estimated_fees(&config.costs);
    let no_action = no_action_reason(plan);
    let action_line = if plan.has_action() {
        if plan.total_buy > 0.0 && plan.total_sell > 0.0 {
            format!(
                "先卖出 ¥{:.0}，再买入 ¥{:.0}（净{} ¥{:.0}）",
                round_hundred(plan.total_sell),
                round_hundred(plan.total_buy),
                if plan.total_buy >= plan.total_sell { "买入" } else { "卖出" },
                round_hundred((plan.total_buy - plan.total_sell).abs())
            )
        } else if plan.total_buy > 0.0 {
            format!("买入 ¥{:.0}", round_hundred(plan.total_buy))
        } else {
            format!("卖出 ¥{:.0}", round_hundred(plan.total_sell))
        }
    } else {
        match &no_action {
            // “纪律正常运转”只留给真正的带宽内不动；空账户/规则阻塞是另一回事
            NoAction::InBand => "本周无需操作——这是纪律正常运转的状态".to_string(),
            NoAction::EmptyAccount => "本周无法给出操作建议——账户暂无资金".to_string(),
            NoAction::Blocked(_) => "本周暂不操作（受最小交易额 / 持有期等规则限制）".to_string(),
        }
    };

    let worst_leg = plan
        .legs
        .iter()
        .max_by(|a, b| a.drift_pp.abs().partial_cmp(&b.drift_pp.abs()).unwrap_or(std::cmp::Ordering::Equal));
    let why_line = if plan.has_action() {
        format!(
            "情绪 {}（{:.0} 分），目标风险仓位 {:.0}%，当前 {:.0}%",
            rating, score, plan.target_risk_weight, plan.current_risk_weight
        )
    } else {
        match &no_action {
            NoAction::InBand => match worst_leg {
                Some(l) => format!(
                    "情绪 {}（{:.0} 分），各腿偏离均在 ±{:.1}pp 内（最大 {} {:+.1}pp）",
                    rating, score, plan.band_pp, l.category_cn, l.drift_pp
                ),
                None => format!("情绪 {}（{:.0} 分），无持仓可调", rating, score),
            },
            NoAction::EmptyAccount => format!(
                "情绪 {}（{:.0} 分）——现金与持仓均为 ¥0，先 `mns cash set <金额>` 注资",
                rating, score
            ),
            NoAction::Blocked(legs) => {
                let reasons: Vec<String> = legs
                    .iter()
                    .take(2)
                    .map(|l| {
                        format!(
                            "{}：{}",
                            l.category_cn,
                            l.hold_reason.as_deref().unwrap_or("暂不建议操作")
                        )
                    })
                    .collect();
                format!("情绪 {}（{:.0} 分）——{}", rating, score, reasons.join("；"))
            }
        }
    };

    let fee_line = if fees < 1.0 {
        "预计费用：¥0（本次无需交易）".to_string()
    } else {
        format!(
            "预计费用：约 {}（按现行费率估算，含阶梯赎回费；赎回费按最早买入日估算，可能偏低）",
            fmt_money(fees)
        )
    };

    let drift_line = match &no_action {
        NoAction::EmptyAccount => "账户未注资，暂无偏离可言；先 `mns cash set <金额>` 注资".to_string(),
        _ => match worst_leg {
            Some(l) => format!(
                "本月已操作 {} 笔，累计估算费用 {}；偏离最大：{} {:+.1}pp",
                activity.trades,
                fmt_money(activity.estimated_fees),
                l.category_cn,
                l.drift_pp
            ),
            None => format!(
                "本月已操作 {} 笔，累计估算费用 {}",
                activity.trades,
                fmt_money(activity.estimated_fees)
            ),
        },
    };

    report.push_str("【本周结论】\n");
    report.push_str(&format!("  1. 操作：{}\n", action_line));
    report.push_str(&format!("  2. 原因：{}\n", why_line));
    report.push_str(&format!("  3. {}\n", fee_line));
    report.push_str(&format!("  4. {}\n\n", drift_line));

    // 市场情绪（决策分数 = 平滑/确认后的信号，见 src/signal.rs；
    // 当日原始值仅展示，不再直接驱动仓位）
    report.push_str(&format!(
        "【风险档位】{}\n",
        config.profile_label()
    ));
    report.push_str(&format!(
        "  中性目标仓位 {:.0}%（档位只收窄择时摆幅，不改你的曲线锚点）\n\n",
        config.effective_neutral_weight()
    ));
    report.push_str("【市场情绪】\n");
    report.push_str(&format!(
        "  CNN 恐贪指数(今日): {:.2} ({})\n",
        raw_score, raw_zone
    ));
    report.push_str(&format!(
        "  决策分数(平滑{}天): {:.2} ({})\n",
        config.rebalance.signal_smooth_days, score, rating
    ));
    if !decision_note.is_empty() {
        report.push_str(&format!("  信号说明: {}\n", decision_note));
    }
    if let Some(pc) = previous_close {
        report.push_str(&format!("  前日收盘: {:.2}", pc));
    }
    if let Some(pw) = previous_1_week {
        report.push_str(&format!(" | 周环比: {:.2} → {:.2}", pw, raw_score));
    }
    report.push('\n');
    if let Some(pm) = previous_1_month {
        report.push_str(&format!("  月环比: {:.2} → {:.2}", pm, raw_score));
    }
    if let Some(py) = previous_1_year {
        report.push_str(&format!(" | 年同比: {:.2} → {:.2}", py, raw_score));
    }
    report.push_str("\n\n");

    // 账户概览
    // `.sum()` 在空迭代器（无持仓）上返回 -0.0（IEEE 754 加法恒等元的符号），
    // 数值上等于 0.0 不影响后续计算，但 "{:.2}" 会原样打印成 "¥-0.00" 误导用户，
    // 因此用 "+ 0.0" 归一化符号（-0.0 + 0.0 == 0.0，对非零值无影响）。
    let total_mv: f64 = positions
        .iter()
        .map(|p| p.market_value_or_cost())
        .sum::<f64>()
        + 0.0;
    let total_assets = cash_balance + total_mv;
    let today_date = today.date_naive();

    report.push_str("【账户概览】\n");
    report.push_str(&format!("  现金余额: ¥{:.2}\n", cash_balance));
    report.push_str(&format!("  持仓市值: ¥{:.2}\n", total_mv));
    report.push_str(&format!("  总资产:   ¥{:.2}\n\n", total_assets));

    // 持仓明细
    if !positions.is_empty() {
        report.push_str("  持仓明细:\n");

        let mut table = Table::new();
        table
            .load_preset(UTF8_FULL)
            .apply_modifier(UTF8_ROUND_CORNERS);
        table.set_header(vec![
            Cell::new("代码"),
            Cell::new("名称"),
            Cell::new("份额"),
            Cell::new("成本价"),
            Cell::new("现价"),
            Cell::new("年化收益"),
            Cell::new("绝对收益"),
        ]);

        for pos in positions {
            let ann_str = match pos
                .annualized_return_with_min_days(&today_date, config.settings.min_holding_days)
            {
                Some(r) => format!("{:+.1}%", r * 100.0),
                None => "N/A".to_string(),
            };
            let abs_str = match pos.absolute_return() {
                Some(r) => format!("{:+.1}%", r * 100.0),
                None => "N/A".to_string(),
            };
            let cur_str = match pos.current_price {
                Some(p) => format!("{:.2}", p),
                None => "-".to_string(),
            };

            // 年化收益单元格着色
            let mut ann_cell = Cell::new(&ann_str);
            if let Some(r) =
                pos.annualized_return_with_min_days(&today_date, config.settings.min_holding_days)
            {
                if r * 100.0 >= config.settings.annualized_target_high {
                    ann_cell = ann_cell.fg(Color::Green);
                } else if r < 0.0 {
                    ann_cell = ann_cell.fg(Color::Red);
                }
            }

            table.add_row(vec![
                Cell::new(&pos.asset_code),
                Cell::new(&pos.asset_name),
                Cell::new(format!("{:.2}", pos.shares)),
                Cell::new(format!("{:.2}", pos.cost_price)),
                Cell::new(&cur_str),
                ann_cell,
                Cell::new(&abs_str),
            ]);
        }

        // 将表格每行缩进两个空格
        for line in table.to_string().lines() {
            report.push_str(&format!("  {}\n", line));
        }
        report.push('\n');
    }

    // ── 调仓计划（目标仓位 + 偏离带） ──
    report.push_str("【调仓计划】\n");
    report.push_str(&format!(
        "  风险资产目标 {:.1}%  当前 {:.1}%  偏离 {:+.1}pp  (带宽 ±{:.1}pp)\n",
        plan.target_risk_weight,
        plan.current_risk_weight,
        plan.current_risk_weight - plan.target_risk_weight,
        plan.band_pp
    ));
    report.push_str(&format!(
        "  总资产 ¥{:.2}（现金 ¥{:.2}）\n\n",
        plan.total_assets, plan.cash_balance
    ));

    let mut plan_table = Table::new();
    plan_table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS);
    plan_table.set_header(vec![
        Cell::new("类别"),
        Cell::new("目标"),
        Cell::new("当前"),
        Cell::new("偏离"),
        Cell::new("建议"),
    ]);
    for leg in &plan.legs {
        let action = if leg.amount > 0.0 {
            format!("买入 ¥{:.0}", leg.amount)
        } else if leg.amount < 0.0 {
            format!("卖出 ¥{:.0}", -leg.amount)
        } else {
            "不动作".to_string()
        };
        let mut drift_cell = Cell::new(format!("{:+.1}pp", leg.drift_pp));
        if leg.drift_pp.abs() > plan.band_pp {
            drift_cell = drift_cell.fg(if leg.drift_pp > 0.0 {
                Color::Red
            } else {
                Color::Green
            });
        }
        plan_table.add_row(vec![
            Cell::new(&leg.category_cn),
            Cell::new(format!("{:.1}%", leg.target_weight)),
            Cell::new(format!("{:.1}%", leg.current_weight)),
            drift_cell,
            Cell::new(action),
        ]);
    }
    for line in plan_table.to_string().lines() {
        report.push_str(&format!("  {}\n", line));
    }
    report.push('\n');

    for leg in &plan.legs {
        if leg.items.is_empty() && leg.hold_reason.is_none() {
            continue;
        }
        report.push_str(&format!("  ▸ {}\n", leg.category_cn));
        if let Some(reason) = &leg.hold_reason {
            report.push_str(&format!("    · {}\n", reason));
        }
        for item in &leg.items {
            let verb = if item.amount >= 0.0 {
                "买入"
            } else {
                "卖出"
            };
            report.push_str(&format!(
                "    · {} ({}): {} ¥{:.2}{}\n",
                item.asset_code,
                item.asset_name,
                verb,
                item.amount.abs(),
                item.note
                    .as_ref()
                    .map(|n| format!("  [{}]", n))
                    .unwrap_or_default()
            ));
        }
    }
    report.push('\n');

    // 净操作指引
    report.push_str("【净操作指引】\n");
    if plan.has_action() {
        report.push_str(&format!(
            "  {} ¥{:.2}（买入 ¥{:.2} - 卖出 ¥{:.2}）\n",
            plan.net_direction(),
            (plan.total_buy - plan.total_sell).abs(),
            plan.total_buy,
            plan.total_sell
        ));
        if plan.cash_constrained {
            report.push_str("  注: 可用资金不足以补足全部缺口，已按缺口比例分配\n");
        }
        if plan.total_sell > 0.0 {
            report.push_str("  注: 赎回费按各标的最早买入日估算会偏低（近期定投份额可能仍在费率档内）\n");
        }
    } else {
        match &no_action {
            NoAction::EmptyAccount => {
                report.push_str(
                    "  今日无需调仓 —— 账户暂无资金（现金 + 持仓市值 = ¥0）。\n",
                );
                report.push_str("  先 `mns cash set <金额>` 注资，再运行 `mns report` 看目标仓位。\n");
            }
            NoAction::InBand => {
                report.push_str("  今日无需调仓 —— 各类别偏离均在带宽内，持有即可。\n");
                report.push_str("  这是正常状态：中长线框架下多数月份都不应有动作。\n");
            }
            NoAction::Blocked(legs) => {
                report.push_str("  今日无需调仓，但原因不是偏离在带宽内：\n");
                for l in legs {
                    report.push_str(&format!(
                        "    · {}：{}\n",
                        l.category_cn,
                        l.hold_reason.as_deref().unwrap_or("暂不建议操作")
                    ));
                }
            }
        }
    }
    report.push('\n');

    // 风险警告
    if !risk_warnings.is_empty() {
        report.push_str("【风险警告】\n");
        for w in risk_warnings {
            let advice_str = match &w.advice {
                RiskAdvice::ConsiderBuyMore => {
                    "恐慌环境下浮亏，可能是加仓机会——若基本面未恶化，可考虑逆向加仓"
                }
                RiskAdvice::ReviewFundamentals => "中性环境下浮亏，建议审视基本面是否恶化",
                RiskAdvice::UrgentReview => {
                    "贪婪环境下仍浮亏，需紧急审视——市场普涨时该标的逆势下跌，可能存在结构性问题"
                }
            };
            report.push_str(&format!(
                "  ▸ {} ({}) — 浮亏 {:.1}%\n",
                w.asset_code, w.asset_name, w.loss_ratio
            ));
            report.push_str(&format!("    {}\n", advice_str));
        }
        report.push('\n');
    }

    // 不同情绪区间下的目标仓位预案
    report.push_str("【目标仓位预案】\n");
    if plan.total_assets <= 0.0 {
        // 账户尚未注资（刚 init、还没 `mns cash set`）：total_assets 为 0 会让每个区间都
        // 显示 "≈ ¥0"，看起来像计算出了结果，实际只是"没有钱"——先提示注资，不展示误导性的 ¥0 列表。
        report.push_str(
            "  账户暂无资金（现金 + 持仓市值 = ¥0），请先 `mns cash set <金额>` 注资，\n",
        );
        report.push_str("  注资后再运行 `mns report` 即可看到各情绪区间对应的目标仓位金额。\n\n");
    } else {
        report.push_str("  情绪进入各区间时的风险资产目标权重:\n");
        let zones = [
            (
                "极度恐慌",
                format!("指数 < {:.0}", config.thresholds.extreme_fear),
                config.target_weight_for(0.0),
            ),
            (
                "恐慌",
                format!(
                    "{:.0} ≤ 指数 < {:.0}",
                    config.thresholds.extreme_fear, config.thresholds.fear
                ),
                config.target_weight_for((config.thresholds.extreme_fear + config.thresholds.fear) / 2.0),
            ),
            (
                "中性",
                format!(
                    "{:.0} ≤ 指数 < {:.0}",
                    config.thresholds.fear, config.thresholds.neutral
                ),
                config.target_weight_for((config.thresholds.fear + config.thresholds.neutral) / 2.0),
            ),
            (
                "贪婪",
                format!(
                    "{:.0} ≤ 指数 < {:.0}",
                    config.thresholds.neutral, config.thresholds.greed
                ),
                config.target_weight_for((config.thresholds.neutral + config.thresholds.greed) / 2.0),
            ),
            (
                "极度贪婪",
                format!("指数 ≥ {:.0}", config.thresholds.greed),
                config.target_weight_for(100.0),
            ),
        ];
        for (name, desc, weight) in &zones {
            let value = plan.total_assets * weight / 100.0;
            report.push_str(&format!(
                "  · {} ({}): 目标 {:.0}% ≈ ¥{:.0}\n",
                pad_display(name, 10),
                desc,
                weight,
                value
            ));
        }
        report.push('\n');
    }

    report.push_str("\n═══════════════════════════════════════════════════\n");

    Ok(report)
}

/// 非决策日的观察版报告：只读状态与信号，不给买卖建议。
///
/// 与决策版同一套数据源（当日值 + 本地快照平滑），但明确不输出调仓计划，
/// 避免用户被周级噪声牵引操作。下一个决策日按 `rebalance.decision_weekday` 提示。
pub fn generate_observation_report(
    config: &AppConfig,
    signal: &crate::signal::DecisionSignal,
    cash_balance: f64,
    positions: &[Position],
    price_age_days: Option<i64>,
) -> Result<String> {
    let today = Local::now();
    let weekday = match today.weekday().num_days_from_monday() {
        0 => "Monday",
        1 => "Tuesday",
        2 => "Wednesday",
        3 => "Thursday",
        4 => "Friday",
        5 => "Saturday",
        _ => "Sunday",
    };
    let date_str = today.format("%Y-%m-%d").to_string();

    let mut report = String::new();
    report.push_str(&format!(
        "═══════════════════════════════════════════════════\n\
         逆情绪投资助手 - 观察报告（非决策日，今日不产生买卖建议）\n\
         {} ({})\n\
         ═════════════════════════════════════════════════\n\n",
        date_str, weekday
    ));

    report.push_str(&format!(
        "【风险档位】{}（中性目标 {:.0}%）\n\n",
        config.profile_label(),
        config.effective_neutral_weight()
    ));

    if let Some(days) = price_age_days.filter(|d| *d >= 1) {
        report.push_str(&format!(
            "⚠ 价格数据为 {} 天前，市值可能已失真，请先运行 `mns update-prices`。\n\n",
            days
        ));
    }

    // 观察日也给一句话结论：不操作 + 下一个决策日
    let next_note = match config.rebalance.decision_weekday {
        0 => "每日都是决策日".to_string(),
        wd => {
            let names = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];
            let today_idx = today.weekday().num_days_from_monday() as u32 + 1;
            let delta = (wd + 7 - today_idx) % 7;
            let delta = if delta == 0 { 7 } else { delta };
            format!("下一个决策日：{}（{} 天后）", names[(wd - 1) as usize], delta)
        }
    };
    report.push_str("【本周结论】\n");
    report.push_str(&format!(
        "  今日为观察日，不产生买卖建议；{}。\n",
        next_note
    ));
    report.push_str(&format!(
        "  情绪 {}（{:.0} 分），无需按当日情绪操作。\n\n",
        signal.smoothed_zone, signal.smoothed_score
    ));

    report.push_str("【市场情绪】\n");
    report.push_str(&format!(
        "  CNN 恐贪指数(今日): {:.2} ({})\n",
        signal.raw_score, signal.raw_zone
    ));
    report.push_str(&format!(
        "  决策分数(平滑{}天, 样本{}/{}): {:.2} ({})\n",
        config.rebalance.signal_smooth_days, signal.samples_used, config.rebalance.signal_smooth_days, signal.smoothed_score, signal.smoothed_zone
    ));
    report.push_str(&format!("  信号说明: {}\n", signal.note));
    if !signal.confirmed {
        report.push_str("  ⚠ 区间切换未确认，决策沿用旧区间，请勿按当日情绪操作\n");
    }
    report.push('\n');

    // 距离各阈值的余量（决策分数视角）
    report.push_str("【阈值距离】\n");
    let t = &config.thresholds;
    for (name, v) in [
        ("极度恐慌", t.extreme_fear),
        ("恐惧", t.fear),
        ("中性", t.neutral),
        ("贪婪", t.greed),
    ] {
        let d = signal.effective_score - v;
        let dir = if d < 0.0 {
            format!("尚差 {:.1} 分进入", -d)
        } else {
            format!("高出 {:.1} 分", d)
        };
        report.push_str(&format!("  进入「{}」({:.0}): {}\n", name, v, dir));
    }
    report.push('\n');

    // 账户概览（简版，只展示不动手）
    let total_mv: f64 = positions.iter().map(|p| p.market_value_or_cost()).sum::<f64>() + 0.0;
    let total = cash_balance + total_mv;
    let risk_weight = if total > 0.0 { total_mv / total * 100.0 } else { 0.0 };
    report.push_str("【账户概览】\n");
    report.push_str(&format!("  现金余额: ¥{:.2}\n", cash_balance));
    report.push_str(&format!("  持仓市值: ¥{:.2}\n", total_mv));
    report.push_str(&format!("  总资产:   ¥{:.2}\n", total));
    report.push_str(&format!("  风险资产权重: {:.1}%\n\n", risk_weight));

    // 下一决策日
    let dw = config.rebalance.decision_weekday;
    if dw >= 1 && dw <= 7 {
        let names = ["一", "二", "三", "四", "五", "六", "日"];
        let today_wd = today.weekday().num_days_from_monday() as usize + 1; // 周一=1
        let mut gap = dw as i64 - today_wd as i64;
        if gap <= 0 {
            gap += 7;
        }
        report.push_str(&format!(
            "提示: 决策日为每周{}，{} 天后出决策版报告；非决策日的建议一律忽略。\n",
            names[dw as usize - 1], gap
        ));
    }
    Ok(report)
}

pub fn save_report(config: &AppConfig, content: &str) -> Result<String> {
    let today = Local::now().format("%Y-%m-%d").to_string();
    let output_dir = &config.settings.report_output_dir;
    fs::create_dir_all(output_dir)?;

    let filepath = Path::new(output_dir).join(format!("{}.txt", today));
    fs::write(&filepath, content)?;

    Ok(filepath.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(total: f64, legs: Vec<LegPlan>) -> RebalancePlan {
        RebalancePlan {
            target_risk_weight: 60.0,
            current_risk_weight: 0.0,
            total_assets: total,
            cash_balance: total,
            band_pp: 4.0,
            legs,
            total_sell: 0.0,
            total_buy: 0.0,
            cash_constrained: false,
        }
    }

    fn leg(cat: &str, reason: Option<&str>) -> LegPlan {
        LegPlan {
            category: cat.to_string(),
            category_cn: cat.to_string(),
            target_weight: 50.0,
            current_weight: 0.0,
            drift_pp: -50.0,
            amount: 0.0,
            hold_reason: reason.map(|s| s.to_string()),
            items: Vec::new(),
        }
    }

    #[test]
    fn 无动作原因不冒充带宽内() {
        // 空账户 → 注资提示，而不是“偏离在带宽内”
        let p = plan(0.0, vec![leg("us_stocks", None)]);
        assert!(matches!(no_action_reason(&p), NoAction::EmptyAccount));

        // 真在带宽内
        let p = plan(
            100_000.0,
            vec![leg("us_stocks", Some("偏离 +2.0pp 在带宽 ±4.0pp 内"))],
        );
        assert!(matches!(no_action_reason(&p), NoAction::InBand));

        // 金额低于最小交易额：如实归因
        let p = plan(
            300.0,
            vec![leg("us_stocks", Some("金额 ¥155 低于最小交易额"))],
        );
        match no_action_reason(&p) {
            NoAction::Blocked(legs) => {
                assert_eq!(legs.len(), 1);
                assert!(legs[0].hold_reason.as_deref().unwrap().contains("最小交易额"));
            }
            _ => panic!("应识别为规则阻塞"),
        }
    }
}
