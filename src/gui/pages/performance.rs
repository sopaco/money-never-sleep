//! 绩效：金额加权收益、真实回撤与资金流水。

use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::*;

use crate::app::Snapshot;
use crate::gui::chart;
use crate::gui::components::*;
use crate::gui::theme::*;
use crate::gui::{AppShell, Page};

use super::{card_header, page_lead, page_root, row};

pub fn render(shell: &AppShell, _cx: &mut Context<AppShell>) -> AnyElement {
    let mut root = page_root("page-performance").child(page_lead(Page::Performance.subtitle()));

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
                Some("现金流加权年化".to_owned()),
            ),
            stat_tile(
                IconName::PiggyBank,
                Tone::Accent,
                "累计净投入",
                money(data.net_inflow),
                Some((format!("{} 笔外部现金流", data.counts.flows), Tone::Muted)),
                Some("买卖不算外部资金".to_owned()),
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
                Some("每日快照口径".to_owned()),
            ),
        ]))
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
                        .child(card_header("最大回撤的来路", "峰值到谷值，再看恢复"))
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

fn curve_card(data: &Snapshot) -> Div {
    let mut card = card_pad().child(card_header(
        "总资产曲线",
        "每点是一次「更新数据」；入金当日会跳升，不代表收益",
    ));
    if data.series.len() < 2 {
        return card.child(empty_state(
            IconName::ChartLine,
            "数据点不足",
            "至少需要两个不同日期的快照才能看出走势与回撤。",
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
    let card = card_pad().child(card_header(
        "资金流水",
        "只记外部进出：入金、出金、期初与校准",
    ));
    if data.flows.is_empty() {
        return card.child(empty_state(
            IconName::ArrowDownUp,
            "没有流水",
            "入金或出金后，这里会形成可回溯的台账。",
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
    let card = card_pad().child(card_header(
        "交易记录",
        "买与卖是组合内部的现金转换，不进入收益口径",
    ));
    if data.trades.is_empty() {
        return card.child(empty_state(
            IconName::Receipt,
            "没有交易",
            "记录第一笔买入后，这里会保留完整轨迹。",
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
