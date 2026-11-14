//! 绩效：金额加权收益、真实回撤与资金流水。

use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::*;

use crate::app::Snapshot;
use crate::gui::AppShell;
use crate::gui::chart;
use crate::gui::components::*;
use crate::gui::theme::*;

use super::{card_header, page_root, row};

pub fn render(shell: &AppShell, _cx: &mut Context<AppShell>) -> AnyElement {
    let mut root = page_root("page-performance");

    let Some(data) = &shell.data else {
        return root
            .child(
                card_pad().child(
                    div()
                        .text_size(px(12.5))
                        .text_color(rgb(TEXT_SECONDARY))
                        .child("正在读取本地数据…"),
                ),
            )
            .into_any_element();
    };

    let drawdown = data.drawdown.as_ref();

    root = root
        .child(row(vec![
            stat_tile(
                IconName::TrendingUp,
                Tone::Accent,
                "XIRR",
                data.xirr.map(pct_signed).unwrap_or_else(|| "—".to_owned()),
                Some((format!("追踪 {} 天", data.tracking_days), Tone::Muted)),
                None,
            ),
            stat_tile(
                IconName::PiggyBank,
                Tone::Accent,
                "累计净投入",
                money(data.net_inflow),
                Some((format!("{} 笔外部现金流", data.counts.flows), Tone::Muted)),
                None,
            ),
            stat_tile(
                IconName::Wallet,
                Tone::Accent,
                "当前总资产",
                money(data.total_assets),
                Some((format!("{} 个快照", data.counts.snapshots), Tone::Muted)),
                None,
            ),
            stat_tile(
                IconName::TrendingDown,
                Tone::Warning,
                "最大回撤",
                drawdown
                    .map(|d| format!("-{:.1}%", d.max_dd * 100.0))
                    .unwrap_or_else(|| "—".to_owned()),
                drawdown.map(|d| (d.recovery_note(), Tone::Muted)),
                None,
            ),
        ]))
        .child(discipline_card(data))
        .child(curve_card(data));

    if let Some(drawdown) = drawdown {
        root = root.child(
            div()
                .h_flex()
                .gap(px(12.))
                .items_stretch()
                .child(
                    card_pad()
                        .flex_1()
                        .gap(px(8.))
                        .child(card_header("最大回撤的来路", ""))
                        .child(kv_row(
                            "峰值",
                            format!(
                                "{} · {}",
                                drawdown.peak_date.format("%Y-%m-%d"),
                                money(drawdown.peak_value)
                            ),
                        ))
                        .child(kv_row(
                            "谷值",
                            format!(
                                "{} · {}",
                                drawdown.trough_date.format("%Y-%m-%d"),
                                money(drawdown.trough_value)
                            ),
                        ))
                        .child(kv_row(
                            "全期峰值",
                            format!(
                                "{} · {}",
                                drawdown.all_time_peak_date.format("%Y-%m-%d"),
                                money(drawdown.all_time_peak)
                            ),
                        ))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(TEXT_TERTIARY))
                                .child(drawdown.recovery_note()),
                        ),
                )
                .child(flows_card(data)),
        );
    } else {
        root = root.child(flows_card(data));
    }

    root.child(trades_card(data)).into_any_element()
}

/// 年度纪律卡：回答「我今年有没有乱动」。
///
/// 只呈现流水统计与前缀分桶。没有前缀的记录写成「未标记」并说明不推断，
/// 不把它算成计划内（也就不会替用户美化纪律）。
fn discipline_card(data: &Snapshot) -> Div {
    let review = &data.discipline;
    let over = review.over_budget();
    let total = review.total;
    let headline = if total == 0 {
        "尚未操作".to_owned()
    } else {
        format!("{total} 次")
    };
    let track = if review.budget_max == 0 {
        0.0
    } else {
        total as f32 / review.budget_max as f32
    };
    let accent = if over { WARNING } else { ACCENT };

    card_pad()
        .child(card_header("纪律复盘", "按流水标记统计"))
        .child(
            div()
                .v_flex()
                .gap(px(10.))
                .child(
                    div()
                        .h_flex()
                        .items_baseline()
                        .gap(px(10.))
                        .child(
                            div()
                                .text_size(px(28.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(if over { WARNING } else { TEXT }))
                                .child(headline),
                        )
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(rgb(TEXT_SECONDARY))
                                .child(format!("{} 年操作次数", review.year)),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(rgb(TEXT_TERTIARY))
                        .child(budget_line(review)),
                )
                .child(hbar(track, accent, 8.))
                .child(
                    div()
                        .h_flex()
                        .flex_wrap()
                        .gap(px(18.))
                        .child(bucket(
                            "计划外冷静",
                            review.out_of_plan,
                            WARNING,
                            "计划外或金额异常",
                        ))
                        .child(bucket(
                            "过热冷静",
                            review.hot_calm,
                            ACCENT,
                            "情绪过热时买入",
                        ))
                        .child(bucket("未标记", review.unmarked, TEXT_TERTIARY, "无标记")),
                ),
        )
        .child(divider())
        .child(kv_row(
            "最近一次操作",
            review
                .last_trade
                .map(|date| date.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "今年还没有操作".to_owned()),
        ))
}

/// One bucket: the count is the loud part, the label and the caveat sit beside
/// it. `hint` is where "未标记" states that nothing is inferred.
fn bucket(label: &'static str, count: usize, color: u32, hint: &'static str) -> Div {
    div()
        .v_flex()
        .gap(px(2.))
        .child(
            div()
                .h_flex()
                .items_baseline()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(18.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(if count == 0 { TEXT_TERTIARY } else { color }))
                        .child(count.to_string()),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(rgb(TEXT_SECONDARY))
                        .child(label),
                ),
        )
        .child(
            div()
                .text_size(px(11.5))
                .text_color(rgb(TEXT_TERTIARY))
                .child(hint),
        )
}

/// Budget sentence. Equal to the upper bound is still inside the budget, so the
/// wording distinguishes "还剩 0 次" from "已超 N 次".
fn budget_line(review: &crate::discipline::YearReview) -> String {
    let range = format!("{}-{} 次/年", review.budget_min, review.budget_max);
    if review.over_budget() {
        format!(
            "已超预算 {} 次（{}）——建议接下来只在决策日执行计划内交易",
            review.total - review.budget_max as usize,
            range
        )
    } else if review.total == review.budget_max as usize {
        format!("预算 {range}，今年次数已用满（操作仍以决策日计划为准）")
    } else {
        format!("预算 {range}，还剩 {} 次", review.remaining())
    }
}

fn curve_card(data: &Snapshot) -> Div {
    let mut card = card_pad().child(card_header("总资产曲线", "每日快照"));
    if data.series.len() < 2 {
        return card.child(empty_state(
            IconName::ChartLine,
            "数据点不足",
            "至少需要两个每日快照。",
        ));
    }
    let values: Vec<f64> = data.series.iter().map(|(_, value)| *value).collect();
    let peak = values.iter().copied().fold(f64::MIN, f64::max);
    card = card.child(chart::area_chart(values, 200., ACCENT)).child(
        div()
            .h_flex()
            .justify_between()
            .child(caption(
                data.series
                    .first()
                    .map(|(date, _)| date.format("%Y-%m-%d").to_string())
                    .unwrap_or_default(),
            ))
            .child(caption(format!("区间最高 {}", money(peak))))
            .child(caption(
                data.series
                    .last()
                    .map(|(date, _)| date.format("%Y-%m-%d").to_string())
                    .unwrap_or_default(),
            )),
    );
    card
}

fn flows_card(data: &Snapshot) -> Div {
    let card = card_pad().child(card_header("资金流水", "外部资金进出"));
    if data.flows.is_empty() {
        return card.child(empty_state(
            IconName::ArrowDownUp,
            "没有流水",
            "入金或出金后显示在这里。",
        ));
    }
    let rows: Vec<Div> = data
        .flows
        .iter()
        .take(8)
        .map(|flow| {
            div()
                .h_flex()
                .items_center()
                .justify_between()
                .py(px(7.))
                .border_b_1()
                .border_color(rgb(DIVIDER))
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .gap(px(8.))
                        .child(pill(
                            flow.kind_cn().to_owned(),
                            if flow.amount >= 0.0 { ACCENT } else { WARNING },
                            if flow.amount >= 0.0 {
                                ACCENT_SOFT
                            } else {
                                WARNING_SOFT
                            },
                        ))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(TEXT_TERTIARY))
                                .child(flow.date.format("%Y-%m-%d").to_string()),
                        ),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(if flow.amount >= 0.0 {
                            TEXT
                        } else {
                            TEXT_SECONDARY
                        }))
                        .child(money_signed(flow.amount)),
                )
        })
        .collect();
    card.child(div().v_flex().children(rows))
}

fn trades_card(data: &Snapshot) -> Div {
    let card = card_pad().child(card_header("交易记录", ""));
    if data.trades.is_empty() {
        return card.child(empty_state(
            IconName::Receipt,
            "没有交易",
            "记录第一笔买入后显示在这里。",
        ));
    }
    let rows: Vec<Div> = data
        .trades
        .iter()
        .map(|trade| {
            let tone = if trade.side == "buy" {
                Tone::Gain
            } else {
                Tone::Loss
            };
            div()
                .h_flex()
                .items_center()
                .justify_between()
                .py(px(8.))
                .border_b_1()
                .border_color(rgb(DIVIDER))
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .gap(px(10.))
                        .child(pill(
                            trade.side_cn().to_owned(),
                            tone_color(tone),
                            tone_soft(tone),
                        ))
                        .child(div().text_size(px(13.5)).child(format!(
                            "{} · {:.2} 份 @ ¥{:.4}",
                            trade.code, trade.shares, trade.price
                        )))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(TEXT_TERTIARY))
                                .child(trade.date.clone()),
                        ),
                )
                .child(div().text_size(px(13.)).child(money(trade.amount)))
        })
        .collect();
    card.child(div().v_flex().children(rows))
}

// 只测纯函数，因此刻意窄导入：`use super::*` 会连带引入 gpui 的 prelude，
// 把 #[test] 的宏展开撑爆（与 dashboard.rs 同样的理由）。
#[cfg(test)]
mod tests {
    use super::budget_line;
    use crate::discipline::YearReview;
    use chrono::NaiveDate;

    fn review(total: usize) -> YearReview {
        YearReview {
            year: 2026,
            total,
            out_of_plan: 0,
            hot_calm: 0,
            unmarked: 0,
            budget_min: 10,
            budget_max: 20,
            last_trade: Some(NaiveDate::from_ymd_opt(2026, 3, 9).unwrap()),
        }
    }

    #[test]
    fn 预算内说明还剩几次() {
        let text = budget_line(&review(7));
        assert!(text.contains("10-20 次/年"));
        assert!(text.contains("还剩 13 次"), "实际: {text}");
        assert!(!text.contains("超预算"));
    }

    #[test]
    fn 用满上限不算超预算() {
        let text = budget_line(&review(20));
        assert!(text.contains("已用满"), "实际: {text}");
        assert!(!text.contains("超预算"));
    }

    #[test]
    fn 超过上限给出超出笔数与收敛建议() {
        let text = budget_line(&review(23));
        assert!(text.contains("已超预算 3 次"), "实际: {text}");
        assert!(text.contains("只在决策日"));
    }
}
