//! 总览：账户现在怎么样，今天是否需要动手。

use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::*;

use crate::app::Snapshot;
use crate::gui::chart;
use crate::gui::components::*;
use crate::gui::mood::Mood;
use crate::gui::theme::*;
use crate::gui::{AppShell, CashKind, Page, TradeSide};

use super::{card_header, notice, page_lead, page_root, row};

pub fn render(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let mut root = page_root("page-dashboard").child(page_lead(Page::Dashboard.subtitle()));

    match (&shell.data, &shell.load_error) {
        (Some(data), _) => {
            let mood = Mood::resolve(Some(data));
            if data.positions.is_empty() && data.cash.abs() < 0.005 {
                root = root.child(start_card(cx));
            } else {
                root = root.child(hero(data));
            }
            root = root
                .child(row(vec![
                    stat_tile(
                        IconName::TrendingUp,
                        Tone::Accent,
                        "金额加权收益 XIRR",
                        data.xirr.map(pct_signed).unwrap_or_else(|| "—".to_owned()),
                        Some((format!("追踪 {} 天", data.tracking_days), Tone::Muted)),
                        Some("分批投入下比简单年化更可信".to_owned()),
                    ),
                    stat_tile(
                        IconName::TrendingDown,
                        Tone::Warning,
                        "最大回撤",
                        data.drawdown
                            .as_ref()
                            .map(|d| format!("-{:.1}%", d.max_dd * 100.0))
                            .unwrap_or_else(|| "—".to_owned()),
                        data.drawdown
                            .as_ref()
                            .map(|d| (d.recovery_note(), Tone::Muted)),
                        Some("基于每日总资产快照".to_owned()),
                    ),
                    stat_tile(
                        IconName::Coins,
                        signed_tone(data.holding_pnl()),
                        "持仓浮动盈亏",
                        money_signed(data.holding_pnl()),
                        Some((format!("{} 个标的", data.positions.len()), Tone::Muted)),
                        Some("按现价与成本价之差".to_owned()),
                    ),
                    stat_tile(
                        IconName::Gauge,
                        mood.tile_tone(),
                        "情绪分数",
                        data.signal
                            .as_ref()
                            .map(|s| format!("{:.0}", s.effective_score))
                            .unwrap_or_else(|| "—".to_owned()),
                        data.signal.as_ref().map(|s| {
                            (
                                format!("{} · {}", s.effective_zone, s.report_kind),
                                Tone::Muted,
                            )
                        }),
                        Some("用于决定目标仓位，不是买卖点".to_owned()),
                    ),
                ]))
                .child(curve_card(data))
                .child(
                    div()
                        .h_flex()
                        .flex_none()
                        .gap(px(12.))
                        .items_stretch()
                        .child(allocation_card(data))
                        .child(judgment_card(data, cx)),
                )
                .child(activity_card(data, cx));
        }
        (None, Some(error)) => {
            root = root.child(notice(
                format!("读取本地数据失败：{error}"),
                (DANGER, DANGER_SOFT),
            ));
        }
        (None, _) => {
            root = root.child(skeleton());
        }
    }

    root.into_any_element()
}

fn mini_stat(label: &'static str, value: String) -> Div {
    div().v_flex().gap(px(4.)).child(overline(label)).child(
        div()
            .text_size(px(17.))
            .font_weight(FontWeight::MEDIUM)
            .child(value),
    )
}

fn hero(data: &Snapshot) -> Div {
    let mood = Mood::resolve(Some(data));
    let pnl = data.holding_pnl();
    let tone = signed_tone(pnl);
    let arrow = if pnl > 0.0001 {
        "↑"
    } else if pnl < -0.0001 {
        "↓"
    } else {
        "·"
    };
    let basis = if data.net_inflow.abs() > 0.005 {
        pnl / data.net_inflow * 100.0
    } else {
        0.0
    };

    div()
        .relative()
        .overflow_hidden()
        .h_flex()
        .flex_none()
        .items_center()
        .gap(px(32.))
        .p(px(30.))
        .bg(linear_gradient(
            128.,
            linear_color_stop(rgb(mood.glow), 0.),
            linear_color_stop(rgb(SURFACE), 0.92),
        ))
        .border_1()
        .border_color(rgb(BORDER))
        .rounded(px(RADIUS_LG))
        .shadow(shadow_card())
        // The gauge arc is the product mark, scaled up and drained of contrast:
        // it reads as a lit surface behind the number, not as a chart.
        .child(
            div()
                .absolute()
                .right(px(-210.))
                .top(px(-125.))
                .child(chart::gauge_aura(460., mood.ink, 0.30)),
        )
        .child(
            div()
                .v_flex()
                .flex_1()
                .gap(px(12.))
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .gap(px(8.))
                        .child(pill(mood.label, mood.ink, mood.wash))
                        .child(overline("总资产 · 现金与持仓市值")),
                )
                .child(
                    div()
                        .text_size(px(54.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(money_compact(data.total_assets)),
                )
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .gap(px(12.))
                        .child(pill(
                            format!("{arrow} {}", money_signed(pnl)),
                            tone_color(tone),
                            tone_soft(tone),
                        ))
                        .child(
                            div()
                                .text_size(px(13.5))
                                .text_color(rgb(TEXT_SECONDARY))
                                .child(format!("{basis:+.2}% 相对净投入")),
                        )
                        .child(div().w(px(1.)).h(px(12.)).bg(rgb(BORDER)))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(TEXT_TERTIARY))
                                // Freshness already lives in the window header;
                                // repeating it here only diluted the number's
                                // time reference. The hero keeps the "as of".
                                .child(format!("截至 {}", data.as_of.format("%Y-%m-%d"))),
                        ),
                )
                // The mood earns its colours by saying something: the same
                // sentence that tints the panel explains what the tint means.
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(rgb(mood.ink).opacity(0.92))
                        .child(mood.note),
                ),
        )
        .child(
            div()
                .h_flex()
                .gap(px(26.))
                .items_center()
                .child(mini_stat("现金", money(data.cash)))
                .child(div().w(px(1.)).h(px(40.)).bg(rgb(BORDER)))
                .child(mini_stat("持仓市值", money(data.holdings_value)))
                .child(div().w(px(1.)).h(px(40.)).bg(rgb(BORDER)))
                .child(mini_stat("累计净投入", money(data.net_inflow))),
        )
}

fn curve_card(data: &Snapshot) -> Div {
    let mut card = card_pad().child(card_header(
        "资产曲线",
        "每日快照；入金抬高曲线但不等于收益",
    ));
    if data.series.len() < 2 {
        card = card.child(empty_state(
            IconName::ChartLine,
            "还画不出曲线",
            "每天点一次「更新数据」，从第二个快照开始就能看到走势。",
        ));
    } else {
        let values: Vec<f64> = data.series.iter().map(|(_, value)| *value).collect();
        let first = data.series.first().map(|(date, _)| *date);
        let last = data.series.last().map(|(date, _)| *date);
        card = card.child(chart::area_chart(values, 168., ACCENT)).child(
            div()
                .h_flex()
                .justify_between()
                .child(caption(
                    first
                        .map(|date| date.format("%Y-%m-%d").to_string())
                        .unwrap_or_default(),
                ))
                .child(caption(
                    last.map(|date| date.format("%Y-%m-%d").to_string())
                        .unwrap_or_default(),
                )),
        );
    }
    card
}

fn category_color(key: &str) -> u32 {
    match key {
        "us_stocks" => 0x0067c0,
        "cn_stocks" => 0x8764b8,
        "counter_cyclical" => 0x038387,
        _ => 0x8a8a8a,
    }
}

fn allocation_card(data: &Snapshot) -> Div {
    let mut card = card_pad()
        .flex_1()
        .child(card_header("配置位置", "当前权重 / 目标权重"));
    if data.total_assets <= 0.0 {
        return card.child(empty_state(
            IconName::Wallet,
            "还没有资产",
            "先入金，再添加标的；这里会显示每条腿的偏离。",
        ));
    }
    for category in &data.categories {
        let drift_tone = if category.drift_pp.abs() >= 5.0 {
            Tone::Warning
        } else {
            Tone::Muted
        };
        card = card.child(
            div()
                .v_flex()
                .gap(px(5.))
                .child(
                    div()
                        .h_flex()
                        .justify_between()
                        .child(div().text_size(px(13.5)).child(category.name.clone()))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(TEXT_SECONDARY))
                                .child(format!(
                                    "{} / {}",
                                    pct_points(category.current_pct),
                                    pct_points(category.target_pct)
                                )),
                        ),
                )
                .child(hbar(
                    (category.current_pct / 100.0) as f32,
                    category_color(&category.key),
                    8.,
                ))
                .child(
                    div()
                        .h_flex()
                        .justify_between()
                        .child(caption(format!("市值 {}", money(category.market_value))))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(tone_color(drift_tone)))
                                .child(format!("偏离 {}", pp_signed(category.drift_pp))),
                        ),
                ),
        );
    }
    // Absorbs the slack when the neighbouring card is taller, so the closing
    // note sits on the bottom edge instead of floating after the last bar.
    card.child(div().flex_1())
        .child(caption("偏离超过 5pp 的腿，在计划里会被优先补足。"))
}

fn zone_tone(zone: &str) -> Tone {
    if zone.contains("恐慌") {
        Tone::Accent
    } else if zone.contains("贪婪") {
        Tone::Warning
    } else {
        Tone::Muted
    }
}

fn judgment_card(data: &Snapshot, cx: &mut Context<AppShell>) -> Div {
    let card = card_pad().flex_1().child(card_header(
        "今天要不要动手",
        "目标是仓位刻度，不是择时信号",
    ));
    let Some(signal) = &data.signal else {
        return card.child(empty_state(
            IconName::Gauge,
            "还没有情绪数据",
            "点右上角「更新数据」抓取一次，这个卡片就会给出目标仓位。",
        ));
    };
    let tone = zone_tone(&signal.effective_zone);
    let current_risk = data.risk_weight_pct();
    let mood = Mood::resolve(Some(data));

    card.child(
        div()
            .h_flex()
            .items_end()
            .gap(px(12.))
            .child(
                div()
                    .text_size(px(38.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(tone_color(tone)))
                    .child(format!("{:.0}", signal.effective_score)),
            )
            .child(
                div()
                    .v_flex()
                    .gap(px(2.))
                    .pb(px(6.))
                    .child(pill(
                        signal.effective_zone.clone(),
                        tone_color(tone),
                        tone_soft(tone),
                    ))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(TEXT_TERTIARY))
                            .child(if signal.confirmed {
                                "区间已确认"
                            } else {
                                "区间未确认，沿用旧值"
                            }),
                    ),
            )
            .child(div().flex_1()),
    )
    .child(
        div()
            .h_flex()
            .gap(px(18.))
            .child(mini_stat("当前风险仓位", pct_points(current_risk))),
    )
    .child({
        let history: Vec<f64> = data.fgi_history.iter().map(|(_, score)| *score).collect();
        if history.len() >= 2 {
            div()
                .v_flex()
                .gap(px(3.))
                .child(caption(format!("近 {} 天情绪走势", history.len())))
                .child(chart::sparkline(history, 34., tone_color(tone)))
        } else {
            div()
        }
    })
    .child(
        // A pull-quote rule instead of another icon: the colour of the bar is
        // the mood, so the sentence reads as the room speaking, not as a badge.
        // The rule is painted absolutely so the sentence stays a plain block
        // child and wraps against the card's content width.
        div()
            .relative()
            .pl(px(12.))
            .child(
                div()
                    .absolute()
                    .left(px(0.))
                    .top(px(1.))
                    .bottom(px(1.))
                    .w(px(2.))
                    .rounded(px(1.))
                    .bg(rgb(mood.ink).opacity(0.5)),
            )
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(rgb(TEXT_SECONDARY))
                    .child(signal.note.clone()),
            ),
    )
    .child(
        div()
            .h_flex()
            .gap(px(8.))
            .child(
                Button::new("goto-plan")
                    .label("看本月计划")
                    .on_click(cx.listener(|shell, _, _, cx| shell.navigate(Page::Plan, cx))),
            )
            .child(
                Button::new("goto-portfolio")
                    .label("看持仓")
                    .on_click(cx.listener(|shell, _, _, cx| shell.navigate(Page::Portfolio, cx))),
            ),
    )
}

fn activity_card(data: &Snapshot, cx: &mut Context<AppShell>) -> Div {
    let mut card = card_pad().child(card_header("最近动作", "买卖与资金流水，最新在前"));
    if data.trades.is_empty() && data.flows.is_empty() {
        card = card.child(empty_state(
            IconName::Receipt,
            "还没有记录",
            "用「记一笔」登记入金或买卖，账户才开始积累可回溯的历史。",
        ));
    }

    let trades: Vec<Div> = data
        .trades
        .iter()
        .take(5)
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
                        .gap(px(8.))
                        .child(pill(trade.side_cn(), tone_color(tone), tone_soft(tone)))
                        .child(
                            div()
                                .text_size(px(13.5))
                                .child(format!("{} · {:.2} 份", trade.code, trade.shares)),
                        )
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

    if !trades.is_empty() {
        card = card.child(div().v_flex().children(trades));
    }

    card.child(
        div()
            .h_flex()
            .gap(px(8.))
            .pt(px(4.))
            .child(
                Button::new("add-trade")
                    .label("记买入 / 卖出")
                    .on_click(cx.listener(|shell, _, window, cx| {
                        shell.open_trade(TradeSide::Buy, String::new(), window, cx);
                    })),
            )
            .child(
                Button::new("add-cash")
                    .label("记入金")
                    .on_click(cx.listener(|shell, _, window, cx| {
                        shell.open_cash(CashKind::Deposit, window, cx);
                    })),
            )
            .child(
                Button::new("out-cash")
                    .label("记出金")
                    .on_click(cx.listener(|shell, _, window, cx| {
                        shell.open_cash(CashKind::Withdraw, window, cx);
                    })),
            )
            .child(
                Button::new("goto-perf")
                    .label("看资金流水")
                    .on_click(cx.listener(|shell, _, _, cx| shell.navigate(Page::Performance, cx))),
            ),
    )
}

fn start_card(cx: &mut Context<AppShell>) -> Div {
    let step = |index: &'static str, title: &'static str, body: &'static str| {
        div()
            .v_flex()
            .flex_1()
            .gap(px(4.))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(ACCENT))
                    .child(index),
            )
            .child(
                div()
                    .text_size(px(13.5))
                    .font_weight(FontWeight::MEDIUM)
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(TEXT_SECONDARY))
                    .child(body),
            )
    };

    card_pad()
        .gap(px(14.))
        .child(
            div()
                .v_flex()
                .gap(px(3.))
                .child(section_title("三步开始"))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(TEXT_SECONDARY))
                        .child("先把真实的钱和持仓记进来，再让它每天替你盯数据。"),
                ),
        )
        .child(
            div()
                .h_flex()
                .gap(px(20.))
                .items_start()
                .child(step(
                    "01",
                    "入金",
                    "登记外部转进来的钱；这不是收益，只抬高本金。",
                ))
                .child(step(
                    "02",
                    "添加标的并买入",
                    "先建标的，再按实际成交价记录份额。",
                ))
                .child(step("03", "更新数据", "抓价格与情绪，生成今天的目标仓位。")),
        )
        .child(
            div()
                .h_flex()
                .gap(px(8.))
                .child(
                    Button::new("start-deposit")
                        .primary()
                        .label("入金")
                        .on_click(cx.listener(|shell, _, window, cx| {
                            shell.open_cash(CashKind::Deposit, window, cx);
                        })),
                )
                .child(
                    Button::new("start-asset")
                        .label("添加标的")
                        .on_click(cx.listener(|shell, _, window, cx| {
                            shell.open_add_asset(window, cx);
                        })),
                ),
        )
}

fn skeleton() -> Div {
    card_pad()
        .gap(px(10.))
        .child(
            div()
                .h(px(14.))
                .w(px(120.))
                .rounded(px(4.))
                .bg(rgb(NEUTRAL_SOFT)),
        )
        .child(
            div()
                .h(px(34.))
                .w(px(240.))
                .rounded(px(6.))
                .bg(rgb(NEUTRAL_SOFT)),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgb(TEXT_TERTIARY))
                .child("正在读取本地数据库…"),
        )
}
