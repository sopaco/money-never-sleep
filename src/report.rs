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
    /// 本年度已发生的买卖笔数（用于年度交易预算披露）
    pub year_trades: usize,
}

/// 日报口径披露：把「日频建议」与「回测结论」的口径差异写进报告本身，
/// 避免用户在看建议时误把长周期代理回测当成实盘预期。
pub fn disclosure(config: &AppConfig) -> String {
    format!(
        "【口径披露】\n\
         \x20 · 本建议基于日频情绪信号（{} 天平滑 + {} 天区间确认 + 决策周期 {:?}），\n\
         \x20\x20\x20 与 `mns backtest` 的默认审计链路同源；\n\
         \x20 · 长周期牛熊检验使用 2016 年至今的代理标的（非实盘持仓），\n\
         \x20\x20\x20 只用于稳健性比较，不得用于实盘收益预测；\n\
         \x20 · 场外 QDII 存在 T+{} 赎回到账与每日限购，指令可能无法一次成交；\n\
         \x20 · 历史回测与置换检验均不代表未来收益，本报告不构成投资建议。\n\n",
        config.rebalance.signal_smooth_days,
        config.rebalance.zone_confirm_days,
        config.backtest.decision_interval,
        config.channels.qdii_feeder.settlement_days,
    )
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
    // C3 资产曲线的追踪起点（最早每日快照日期），None = 尚未启用
    tracking_since: Option<chrono::NaiveDate>,
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
    // 无可执行标的（产品 C 批发现）：腿级金额已算出（has_action 为真）但
    // 所有腿的 items 为空——空标的池时 split_within_leg 无从分摊，个别
    // 类别缺资产同样无法分摊。此时费用为 0，若照常输出"买入 ¥9400"会与
    // "¥0 无需交易"的费用行自相矛盾。如实引导建池，类似 EmptyAccount
    // 的注资引导。
    let no_executable_items = plan.has_action() && plan.legs.iter().all(|l| l.items.is_empty());
    // 动作行用精确金额（¥{:.2}），与下方调仓明细同口径——费用是估算值，
    // 保持 fmt_money 的取整不动；动作金额是"要执行多少钱"，必须精确。
    let action_line = if no_executable_items {
        if positions.is_empty() {
            "资金已就位，但标的池为空——先 `mns add <代码> <名称> <类别>` 建池".to_string()
        } else {
            "目标类别下暂无可执行标的——用 `mns add` 补齐对应类别后再执行".to_string()
        }
    } else if plan.has_action() {
        if plan.total_buy > 0.0 && plan.total_sell > 0.0 {
            format!(
                "先卖出 ¥{:.2}，再买入 ¥{:.2}（净{} ¥{:.2}）",
                plan.total_sell,
                plan.total_buy,
                if plan.total_buy >= plan.total_sell { "买入" } else { "卖出" },
                (plan.total_buy - plan.total_sell).abs()
            )
        } else if plan.total_buy > 0.0 {
            format!("买入 ¥{:.2}", plan.total_buy)
        } else {
            format!("卖出 ¥{:.2}", plan.total_sell)
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
    let why_line = if no_executable_items {
        format!(
            "情绪 {}（{:.0} 分），目标风险仓位 {:.0}%——暂无可执行标的，建池后下个决策日即可执行",
            rating, score, plan.target_risk_weight
        )
    } else if plan.has_action() {
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

    let fee_line = if no_executable_items {
        "预计费用：—（无可执行标的）".to_string()
    } else if fees < 1.0 {
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
    report.push_str(&format!("  4. {}\n", drift_line));
    let budget = &config.backtest;
    let over = activity.year_trades > budget.annual_trade_budget_max as usize;
    report.push_str(&format!(
        "  5. 年度交易预算：本年已操作 {} 笔 / 预算 {}-{} 次{}\n\n",
        activity.year_trades,
        budget.annual_trade_budget_min,
        budget.annual_trade_budget_max,
        if over { "（已超预算，建议仅执行决策日建议）" } else { "" }
    ));

    // 市场情绪（决策分数 = 平滑/确认后的信号，见 src/signal.rs；
    // 当日原始值仅展示，不再直接驱动仓位）
    //
    // 档位语义如实披露：core_blend 把曲线向满仓收缩 = 抬高常驻仓位 + 收窄摆幅，
    // 这**提高**而不是降低风险（回撤最小的是无档位/曲线原样），不能让用户
    // 把"稳健"档误读为"更小回撤"。
    let profile_note = if config.profile.core_blend > 0.0 {
        "档位会抬高常驻仓位并收窄择时摆幅，这不降低风险；追求更小回撤可评估无档位"
    } else {
        "无档位：目标仓位完全由你的曲线决定，择时摆幅最大"
    };
    report.push_str(&format!(
        "【风险档位】{}\n",
        config.profile_label()
    ));
    report.push_str(&format!(
        "  中性目标仓位 {:.0}%（{}）\n\n",
        config.effective_neutral_weight(),
        profile_note
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
    report.push_str(&format!("  总资产:   ¥{:.2}\n", total_assets));
    if let Some(since) = tracking_since {
        report.push_str(&format!(
            "  （资产曲线自 {} 起每日追踪，此前无记录）\n",
            since.format("%Y-%m-%d")
        ));
    }
    report.push('\n');

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

    // 信号与局限：诚实披露常驻尾注（不随行情变化，固定三行）。
    // 此前该信息只在用户很少运行的 `mns backtest validate` 里；
    // 日报是用户每天看的地方，局限必须出现在这里。
    report.push_str("【信号与局限】\n");
    report.push_str(&format!(
        "  · 决策信号 = CNN 恐贪指数（{}天平滑 + {}天区间确认），为情绪锚\n",
        config.rebalance.signal_smooth_days, config.rebalance.zone_confirm_days
    ));
    report.push_str("  · 趋势锚在长周期代理回测中优于情绪锚，但需≥12个月本地价格历史，实盘暂不可用\n");
    report.push_str("  · 长周期样本 2016-2026 覆盖 2018Q4/2020/2022/2026 回调，但不含 2000-2002 与 2008 型多年期熊市\n\n");
    report.push_str(&disclosure(config));

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
    // C3 资产曲线的追踪起点（最早每日快照日期），None = 尚未启用
    tracking_since: Option<chrono::NaiveDate>,
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
        "【风险档位】{}（中性目标 {:.0}%）\n",
        config.profile_label(),
        config.effective_neutral_weight()
    ));
    report.push_str(&format!(
        "  {}\n\n",
        if config.profile.core_blend > 0.0 {
            "档位会抬高常驻仓位并收窄择时摆幅，这不降低风险；追求更小回撤可评估无档位"
        } else {
            "无档位：目标仓位完全由你的曲线决定，择时摆幅最大"
        }
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
    report.push_str(&format!("  风险资产权重: {:.1}%\n", risk_weight));
    if let Some(since) = tracking_since {
        report.push_str(&format!(
            "  （资产曲线自 {} 起每日追踪，此前无记录）\n",
            since.format("%Y-%m-%d")
        ));
    }
    report.push('\n');

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
    report.push_str(&disclosure(config));
    Ok(report)
}

pub fn save_report(config: &AppConfig, content: &str) -> Result<String> {
    let now = Local::now();
    let date_str = now.format("%Y-%m-%d").to_string();
    let output_dir = &config.settings.report_output_dir;
    fs::create_dir_all(output_dir)?;

    let filepath = Path::new(output_dir).join(format!("{}.txt", date_str));
    // 同日多次运行：追加而非覆盖。"先留档再决策"要求每次运行的快照都
    // 可回溯——如同日切换档位后重跑，两份报告应能对照（此前 fs::write
    // 会静默销毁更早的版本）。
    if filepath.exists() {
        let mut previous = fs::read_to_string(&filepath)?;
        previous.push_str(&format!(
            "\n\n═══════════ 追加运行 {} ═════════════════════════════\n\n",
            now.format("%H:%M:%S")
        ));
        previous.push_str(content);
        fs::write(&filepath, previous)?;
    } else {
        fs::write(&filepath, content)?;
    }

    Ok(filepath.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::LegItem;

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

    fn make_report(config: &AppConfig) -> String {
        let p = plan(
            100_000.0,
            vec![leg("us_stocks", Some("偏离 +2.0pp 在带宽 ±4.0pp 内"))],
        );
        generate_report(
            config,
            50.0,
            "中性",
            50.0,
            "中性",
            "",
            None,
            None,
            None,
            None,
            100_000.0,
            &[],
            &p,
            &[],
            &MonthActivity::default(),
            None,
            None,
        )
        .unwrap()
    }

    #[test]
    fn 决策报告包含信号与局限尾注() {
        let report = make_report(&AppConfig::default_config());
        assert!(report.contains("【信号与局限】"), "诚实披露尾注应存在");
        assert!(report.contains("情绪锚"));
        assert!(report.contains("趋势锚"));
        assert!(report.contains("多年期熊市"), "熊市未检验的局限应披露");
        assert!(report.contains("年度交易预算"), "应披露年度交易预算");
        assert!(report.contains("【口径披露】"), "应披露信号与回测口径差异");
        assert!(report.contains("T+7"), "应披露场外 QDII 赎回结算约束");
    }

    #[test]
    fn 观察版报告也应包含口径披露() {
        let c = AppConfig::default_config();
        let hist = crate::signal::ScoreHistory::new();
        let s = crate::signal::compute(&c, 50.0, &hist, chrono::Local::now().date_naive());
        let r = generate_observation_report(&c, &s, 0.0, &[], None, None).unwrap();
        assert!(r.contains("【口径披露】"));
    }

    #[test]
    fn TLDR动作行使用精确金额与明细同口径() {
        // P3：动作行曾是"买入 ¥1300"（百位取整）而明细是 ¥1352.31，
        // 两数并存会让人怀疑哪个才是要执行的金额——动作行必须精确。
        let config = AppConfig::default_config();
        let mut p = plan(
            100_000.0,
            vec![leg("us_stocks", Some("偏离 +2.0pp 在带宽 ±4.0pp 内"))],
        );
        p.total_buy = 1352.31;
        // 可执行明细存在（买入腿有标的），TL;DR 才走"买入"分支；
        // 全空 items 属于无可执行标的引导，另行测试
        p.legs[0].items = vec![LegItem {
            asset_code: "QQQ".into(),
            asset_name: "纳指100".into(),
            amount: 1352.31,
            note: None,
            holding_days: None,
        }];
        let report = generate_report(
            &config,
            50.0,
            "中性",
            50.0,
            "中性",
            "",
            None,
            None,
            None,
            None,
            100_000.0,
            &[],
            &p,
            &[],
            &MonthActivity::default(),
            None,
            None,
        )
        .unwrap();
        assert!(
            report.contains("1. 操作：买入 ¥1352.31"),
            "TL;DR 动作行应为精确金额，实际: {}",
            report
                .lines()
                .find(|l| l.contains("操作："))
                .unwrap_or("<未找到>")
        );
        // 卖出方向同样精确
        p.total_buy = 0.0;
        p.total_sell = 2202.00;
        p.legs[0].items = vec![LegItem {
            asset_code: "QQQ".into(),
            asset_name: "纳指100".into(),
            amount: -2202.00,
            note: None,
            holding_days: Some(365),
        }];
        let report = generate_report(
            &config,
            50.0,
            "中性",
            50.0,
            "中性",
            "",
            None,
            None,
            None,
            None,
            100_000.0,
            &[],
            &p,
            &[],
            &MonthActivity::default(),
            None,
            None,
        )
        .unwrap();
        assert!(report.contains("1. 操作：卖出 ¥2202.00"));
    }

    #[test]
    fn 空标的池时引导建池而非给出无法执行的建议() {
        // 产品 C 批发现：空池 + 有现金 → has_action 为真（腿级金额已算出）
        // 但 items 全空、费用为 0，"操作：买入 ¥9400" 与 "¥0 无需交易"
        // 自相矛盾。正确 UX 是引导 mns add 建池。
        let config = AppConfig::default_config();
        let mut p = plan(100_000.0, vec![leg("us_stocks", None)]);
        p.total_buy = 9400.00; // 腿级金额存在，但空池无从分摊 → items 为空
        p.legs[0].amount = 9400.00;
        let report = generate_report(
            &config,
            22.0,
            "恐惧",
            22.0,
            "恐惧",
            "",
            None,
            None,
            None,
            None,
            100_000.0,
            &[], // 空标的池
            &p,
            &[],
            &MonthActivity::default(),
            None,
            None,
        )
        .unwrap();
        let action = report
            .lines()
            .find(|l| l.contains("1. 操作："))
            .unwrap_or("<未找到>");
        assert!(
            action.contains("标的池为空") && action.contains("mns add"),
            "TL;DR 应引导建池，实际动作行: {}",
            action
        );
        assert!(
            !action.contains("买入"),
            "TL;DR 不得给出无法执行的动作金额，实际: {}",
            action
        );
        assert!(
            report.contains("预计费用：—（无可执行标的）"),
            "费用行应如实标注无可执行标的"
        );
    }

    #[test]
    fn 有持仓但目标类别缺资产时同样不给出无法执行的建议() {
        // items 全空的另一种形态：持仓存在，但要买入的类别下没有资产
        // （如只持有美股，恐贪走高后逆周期腿要买入却无标的可分摊）。
        let config = AppConfig::default_config();
        let mut p = plan(100_000.0, vec![leg("counter_cyclical", None)]);
        p.total_buy = 5000.00;
        p.legs[0].amount = 5000.00;
        let positions = vec![Position {
            id: 1,
            asset_code: "QQQ".into(),
            asset_name: "纳指100".into(),
            category: "us_stocks".into(),
            shares: 100.0,
            cost_price: 400.0,
            current_price: Some(500.0),
            first_buy_date: "2026-01-01".into(),
            updated_at: "2026-01-01".into(),
        }];
        let report = generate_report(
            &config,
            22.0,
            "恐惧",
            22.0,
            "恐惧",
            "",
            None,
            None,
            None,
            None,
            100_000.0,
            &positions,
            &p,
            &[],
            &MonthActivity::default(),
            None,
            None,
        )
        .unwrap();
        let action = report
            .lines()
            .find(|l| l.contains("1. 操作："))
            .unwrap_or("<未找到>");
        assert!(
            action.contains("目标类别下暂无可执行标的"),
            "TL;DR 应提示目标类别缺资产，实际: {}",
            action
        );
        assert!(!action.contains("买入"));
    }

    #[test]
    fn 决策与观察报告披露资产曲线追踪起点() {
        // C3：追踪说明让用户知道回撤曲线从哪天起算，此前无记录
        // 不会误以为曲线起点就是开户日。
        let config = AppConfig::default_config();
        let since = chrono::NaiveDate::from_ymd_opt(2026, 1, 5).unwrap();
        let p = plan(
            100_000.0,
            vec![leg("us_stocks", Some("偏离 +2.0pp 在带宽 ±4.0pp 内"))],
        );
        let report = generate_report(
            &config,
            50.0,
            "中性",
            50.0,
            "中性",
            "",
            None,
            None,
            None,
            None,
            100_000.0,
            &[],
            &p,
            &[],
            &MonthActivity::default(),
            None,
            Some(since),
        )
        .unwrap();
        assert!(
            report.contains("资产曲线自 2026-01-05 起每日追踪"),
            "决策报告的账户概览应披露追踪起点"
        );

        let signal = crate::signal::DecisionSignal {
            raw_score: 50.0,
            raw_zone: "中性".into(),
            smoothed_score: 50.0,
            smoothed_zone: "中性".into(),
            effective_score: 50.0,
            effective_zone: "中性".into(),
            confirmed: true,
            decision_day: false,
            report_kind: "观察",
            samples_used: 3,
            note: String::new(),
        };
        let obs = generate_observation_report(
            &config,
            &signal,
            100_000.0,
            &[],
            None,
            Some(since),
        )
        .unwrap();
        assert!(
            obs.contains("资产曲线自 2026-01-05 起每日追踪"),
            "观察报告的账户概览应披露追踪起点"
        );

        // 未启用（None）时不出现说明
        let obs2 =
            generate_observation_report(&config, &signal, 100_000.0, &[], None, None).unwrap();
        assert!(!obs2.contains("资产曲线自"), "未启用追踪时不应有该说明");
    }

    #[test]
    fn 档位语义应如实披露不降低风险() {
        // 无档位：说明摆幅最大
        let mut c = AppConfig::default_config();
        c.apply_profile(crate::config::RiskProfile::Base);
        let report = make_report(&c);
        assert!(report.contains("无档位：目标仓位完全由你的曲线决定"));

        // 有档位：必须说明抬高常驻仓位、不降低风险
        c.apply_profile(crate::config::RiskProfile::Steady);
        let report = make_report(&c);
        assert!(
            report.contains("不降低风险"),
            "档位文案不得暗示'稳健'=更小回撤: {}",
            report.lines().find(|l| l.contains("档位会")).unwrap_or("")
        );
        assert!(report.contains("抬高常驻仓位"));
    }

    #[test]
    fn 同日重复运行追加而非覆盖() {
        let dir = std::env::temp_dir().join(format!("mns-report-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut config = AppConfig::default_config();
        config.settings.report_output_dir = dir.to_string_lossy().to_string();

        let p1 = save_report(&config, "第一份报告").unwrap();
        let content1 = std::fs::read_to_string(&p1).unwrap();
        assert!(content1.contains("第一份报告"));
        assert!(!content1.contains("追加运行"), "首次写入应为全新文件");

        let p2 = save_report(&config, "第二份报告").unwrap();
        assert_eq!(p1, p2, "同日应写入同一文件");
        let content2 = std::fs::read_to_string(&p2).unwrap();
        assert!(
            content2.contains("第一份报告") && content2.contains("第二份报告"),
            "两份内容都应保留，实际:\n{}",
            content2
        );
        assert!(content2.contains("追加运行"));

        let _ = std::fs::remove_dir_all(&dir);
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
