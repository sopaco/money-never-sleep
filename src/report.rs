use anyhow::Result;
use chrono::{Datelike, Local};
use std::fs;
use std::path::Path;

use crate::config::AppConfig;
use crate::models::Position;
use crate::strategy::{BuySuggestion, RiskAdvice, RiskWarning, SellReason, SellSuggestion};

pub fn generate_report(
    config: &AppConfig,
    score: f64,
    rating: &str,
    previous_close: Option<f64>,
    previous_1_week: Option<f64>,
    previous_1_month: Option<f64>,
    previous_1_year: Option<f64>,
    cash_balance: f64,
    positions: &[Position],
    buy_suggestion: &BuySuggestion,
    sell_suggestions: &[SellSuggestion],
    risk_warnings: &[RiskWarning],
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
         逆向投资助手 - 每日策略报告\n\
         {} ({})\n\
         ═════════════════════════════════════════════════\n\n",
        date_str, weekday
    ));

    // 市场情绪
    report.push_str("【市场情绪】\n");
    report.push_str(&format!("  CNN 恐贪指数: {:.2} ({})\n", score, rating));
    if let Some(pc) = previous_close {
        report.push_str(&format!("  前日收盘: {:.2}", pc));
    }
    if let Some(pw) = previous_1_week {
        report.push_str(&format!(" | 周环比: {:.2} → {:.2}", pw, score));
    }
    report.push('\n');
    if let Some(pm) = previous_1_month {
        report.push_str(&format!("  月环比: {:.2} → {:.2}", pm, score));
    }
    if let Some(py) = previous_1_year {
        report.push_str(&format!(" | 年同比: {:.2} → {:.2}", py, score));
    }
    report.push_str("\n\n");

    // 账户概览
    let total_mv: f64 = positions.iter().map(|p| p.market_value_or_cost()).sum();
    let total_assets = cash_balance + total_mv;
    let today_date = today.date_naive();

    report.push_str("【账户概览】\n");
    report.push_str(&format!("  现金余额: ¥{:.2}\n", cash_balance));
    report.push_str(&format!("  持仓市值: ¥{:.2}\n", total_mv));
    report.push_str(&format!("  总资产:   ¥{:.2}\n\n", total_assets));

    // 持仓明细
    if !positions.is_empty() {
        report.push_str("  持仓明细:\n");
        report.push_str("  ┌──────────┬──────────────┬──────────┬──────────┬──────────┬──────────┬──────────┐\n");
        report.push_str("  │ 代码     │ 名称         │ 份额     │ 成本价   │ 现价     │ 年化收益 │ 绝对收益 │\n");
        report.push_str("  ├──────────┼──────────────┼──────────┼──────────┼──────────┼──────────┤──────────┤\n");

        for pos in positions {
            let ann_str = match pos.annualized_return_with_min_days(&today_date, config.settings.min_holding_days) {
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
            report.push_str(&format!(
                "  │ {:<8} │ {:<12} │ {:>8.2} │ {:>8.2} │ {:>8} │ {:>8} │ {:>8} │\n",
                pos.asset_code, pos.asset_name, pos.shares, pos.cost_price, cur_str, ann_str, abs_str
            ));
        }
        report.push_str("  └──────────┴──────────────┴──────────┴──────────┴──────────┴──────────┴──────────┘\n\n");
    }

    // 卖出建议
    if !sell_suggestions.is_empty() {
        report.push_str("【卖出建议】\n");
        for s in sell_suggestions {
            let reason_str = match &s.reason {
                SellReason::AnnualizedHigh => {
                    if let Some(ann) = s.annualized_return {
                        if ann * 100.0 >= config.settings.annualized_target_high {
                            format!("年化 {:.1}% ≥ {}% 高线", ann * 100.0, config.settings.annualized_target_high)
                        } else if ann * 100.0 >= config.settings.annualized_target_low {
                            format!("年化 {:.1}% ≥ {}% 低线", ann * 100.0, config.settings.annualized_target_low)
                        } else {
                            "情绪驱动减仓".to_string()
                        }
                    } else {
                        "情绪驱动减仓".to_string()
                    }
                }
                SellReason::AbsoluteProfit => {
                    format!("绝对收益 {:.0}%（长期持有获利了结）", s.absolute_return * 100.0)
                }
            };
            report.push_str(&format!(
                "  ▸ {} ({}) — {}\n",
                s.asset_code, s.asset_name, reason_str
            ));
            report.push_str(&format!(
                "    建议: 减仓 {:.0}%，卖出 {:.2} 份，预计回收 ¥{:.2}\n",
                s.sell_ratio, s.sell_shares, s.sell_amount
            ));
        }
        report.push('\n');
    }

    // 买入建议
    report.push_str("【买入建议】\n");
    if buy_suggestion.total_amount > 0.0 {
        let zone = config.sentiment_zone(score);
        let sell_proceeds: f64 = sell_suggestions.iter().map(|s| s.sell_amount).sum();
        let effective_cash = cash_balance + sell_proceeds;
        report.push_str(&format!(
            "  当前市场\"{}\"，建议投入 ¥{:.2}（可用资金 ¥{:.2} 的 {:.0}%）\n",
            zone, buy_suggestion.total_amount, effective_cash, config.buy_ratio_for(score)
        ));
        if sell_proceeds > 0.0 {
            report.push_str(&format!(
                "  注: 可用资金含卖出回收 ¥{:.2}\n",
                sell_proceeds
            ));
        }
        report.push_str(&format!(
            "    - 美股 ¥{:.2} | A股 ¥{:.2} | 逆周期 ¥{:.2}\n",
            buy_suggestion.us_amount, buy_suggestion.cn_amount, buy_suggestion.counter_amount
        ));
        if !buy_suggestion.details.is_empty() {
            report.push_str("  分配明细（逆向加权：浮亏标的获得更多资金）:\n");
            for d in &buy_suggestion.details {
                report.push_str(&format!("    · {} ({}): ¥{:.2}\n", d.asset_code, d.asset_name, d.amount));
            }
        }
        if !buy_suggestion.excluded.is_empty() {
            report.push_str("  以下标的因高浮亏暂停加仓:\n");
            for e in &buy_suggestion.excluded {
                report.push_str(&format!("    ✗ {} ({}) — 浮亏 {:.0}%: {}\n", e.asset_code, e.asset_name, e.loss_ratio, e.reason));
            }
        }
    } else {
        report.push_str("  当前市场情绪偏高，建议暂停买入。\n");
        report.push_str("  可用资金继续持有，等待市场回调。\n");
    }
    report.push('\n');

    // 净操作指引
    let total_sell: f64 = sell_suggestions.iter().map(|s| s.sell_amount).sum();
    let net_flow = buy_suggestion.total_amount - total_sell;
    report.push_str("【净操作指引】\n");
    if net_flow > 0.0 {
        report.push_str(&format!(
            "  今日净买入 ¥{:.2}（买入 ¥{:.2} - 卖出 ¥{:.2}）\n",
            net_flow, buy_suggestion.total_amount, total_sell
        ));
        report.push_str("  操作方向: 加仓，整体偏逆向买入\n");
    } else if net_flow < 0.0 {
        report.push_str(&format!(
            "  今日净卖出 ¥{:.2}（买入 ¥{:.2} - 卖出 ¥{:.2}）\n",
            -net_flow, buy_suggestion.total_amount, total_sell
        ));
        report.push_str("  操作方向: 减仓，获利了结为主\n");
    } else if buy_suggestion.total_amount > 0.0 {
        report.push_str("  买入与卖出金额基本持平，维持当前仓位\n");
    } else {
        report.push_str("  今日无操作建议，持有观望\n");
    }
    report.push('\n');

    // 风险警告
    if !risk_warnings.is_empty() {
        report.push_str("【风险警告】\n");
        for w in risk_warnings {
            let advice_str = match &w.advice {
                RiskAdvice::ConsiderBuyMore => "恐慌环境下浮亏，可能是加仓机会——若基本面未恶化，可考虑逆向加仓",
                RiskAdvice::ReviewFundamentals => "中性环境下浮亏，建议审视基本面是否恶化",
                RiskAdvice::UrgentReview => "贪婪环境下仍浮亏，需紧急审视——市场普涨时该标的逆势下跌，可能存在结构性问题",
            };
            report.push_str(&format!(
                "  ▸ {} ({}) — 浮亏 {:.1}%\n",
                w.asset_code, w.asset_name, w.loss_ratio
            ));
            report.push_str(&format!("    {}\n", advice_str));
        }
        report.push('\n');
    }

    // 资金分配预案
    let sell_proceeds_for_plan: f64 = sell_suggestions.iter().map(|s| s.sell_amount).sum();
    let effective_cash_for_plan = cash_balance + sell_proceeds_for_plan;

    report.push_str("【资金分配预案】\n");
    report.push_str("  若市场回调至不同区间的投入预案:\n");
    if sell_proceeds_for_plan > 0.0 {
        report.push_str(&format!(
            "  注: 预案基于可用资金 ¥{:.2}（现金 ¥{:.2} + 卖出回收 ¥{:.2}）\n",
            effective_cash_for_plan, cash_balance, sell_proceeds_for_plan
        ));
    }

    let zones = [
        ("极度恐慌", config.thresholds.extreme_fear, config.buy_ratio.extreme_fear),
        ("恐慌", config.thresholds.fear, config.buy_ratio.fear),
        ("中性", config.thresholds.neutral, config.buy_ratio.neutral),
        ("贪婪", config.thresholds.greed, config.buy_ratio.greed),
    ];

    for (i, (name, threshold, ratio)) in zones.iter().enumerate() {
        let amount = effective_cash_for_plan * (ratio / 100.0);
        // 极度恐慌: 指数 < threshold; 其他: 指数 < threshold 的该区间
        let threshold_desc = if i == 0 {
            format!("指数 < {:.0}", threshold)
        } else {
            format!("{:.0} ≤ 指数 < {:.0}", zones[i - 1].1, threshold)
        };
        if *ratio > 0.0 {
            report.push_str(&format!(
                "  · {}   ({}): 投入 ¥{:.2} ({:.0}%)\n",
                name, threshold_desc, amount, ratio
            ));
            report.push_str(&format!(
                "    - 美股 ¥{:.2} | A股 ¥{:.2} | 逆周期 ¥{:.2}\n",
                amount * config.allocation.us_stocks / 100.0,
                amount * config.allocation.cn_stocks / 100.0,
                amount * config.allocation.counter_cyclical / 100.0,
            ));
        } else {
            report.push_str(&format!(
                "  · {}   ({}): 暂停买入，持有观望\n",
                name, threshold_desc
            ));
        }
    }

    report.push_str("\n═══════════════════════════════════════════════════\n");

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
