//! 持仓：成本、现值与配置位置。

use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::*;

use crate::app::Snapshot;
use crate::gui::components::*;
use crate::gui::theme::*;
use crate::gui::{AppShell, CashKind, Page, TradeSide};

use super::{card_header, page_lead, page_root, row};

pub fn render(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let mut root = page_root("page-portfolio").child(page_lead(Page::Portfolio.subtitle()));

    match &shell.data {
        Some(data) => {
            root = root.child(row(vec![
                stat_tile(
                    IconName::Briefcase,
                    Tone::Accent,
                    "持仓市值",
                    money(data.holdings_value),
                    Some((format!("{} 个标的", data.positions.len()), Tone::Muted)),
                    None,
                ),
                stat_tile(
                    IconName::TrendingUp,
                    signed_tone(data.holding_pnl()),
                    "浮动盈亏",
                    money_signed(data.holding_pnl()),
                    Some((
                        format!(
                            "占持仓 {:.1}%",
                            if data.holdings_value > 0.0 {
                                data.holding_pnl() / data.holdings_value * 100.0
                            } else {
                                0.0
                            }
                        ),
                        signed_tone(data.holding_pnl()),
                    )),
                    Some("未计入申赎费用".to_owned()),
                ),
                stat_tile(
                    IconName::Wallet,
                    Tone::Accent,
                    "现金",
                    money(data.cash),
                    Some((
                        format!("{:.1}% 的总资产", {
                            if data.total_assets > 0.0 {
                                data.cash / data.total_assets * 100.0
                            } else {
                                0.0
                            }
                        }),
                        Tone::Muted,
                    )),
                    None,
                ),
            ]));
            root = root.child(table_card(data, cx));
        }
        None => {
            root = root.child(
                card_pad().child(
                    div()
                        .text_size(px(12.5))
                        .text_color(rgb(TEXT_SECONDARY))
                        .child("正在读取持仓…"),
                ),
            );
        }
    }

    root.into_any_element()
}

fn numeric(text: impl Into<SharedString>, width: f32) -> Div {
    div()
        .w(px(width))
        .flex_none()
        .text_size(px(13.))
        .justify_end()
        .h_flex()
        .child(text.into())
}

fn th_cell(text: &'static str, width: f32) -> Div {
    div().w(px(width)).flex_none().child(th(text))
}

fn th_num(text: &'static str, width: f32) -> Div {
    div()
        .w(px(width))
        .flex_none()
        .h_flex()
        .justify_end()
        .child(th(text))
}

fn table_card(data: &Snapshot, cx: &mut Context<AppShell>) -> Div {
    let card = card_pad()
        .gap(px(0.))
        .child(div().pb(px(10.)).child(card_header(
            "全部持仓",
            "现价缺失时按成本价估算市值，并在价格列标出",
        )));

    if data.positions.is_empty() {
        return card
            .child(empty_state(
                IconName::Briefcase,
                "还没有持仓",
                "先入金，再添加标的并记录第一笔买入。",
            ))
            .child(
                div()
                    .h_flex()
                    .gap(px(8.))
                    .pt(px(10.))
                    .child(
                        Button::new("empty-deposit")
                            .primary()
                            .label("入金")
                            .on_click(cx.listener(|shell, _, window, cx| {
                                shell.open_cash(CashKind::Deposit, window, cx);
                            })),
                    )
                    .child(
                        Button::new("empty-add-asset")
                            .label("添加标的")
                            .on_click(cx.listener(|shell, _, window, cx| {
                                shell.open_add_asset(window, cx);
                            })),
                    ),
            );
    }

    let header = div()
        .h_flex()
        .items_center()
        .gap(px(8.))
        .py(px(8.))
        .px(px(4.))
        .border_b_1()
        .border_color(rgb(BORDER))
        .child(div().flex_1().min_w(px(150.)).child(th("标的")))
        .child(th_cell("类别", 56.))
        .child(th_num("份额", 74.))
        .child(th_num("成本", 74.))
        .child(th_num("现价", 74.))
        .child(th_num("市值", 96.))
        .child(th_num("盈亏", 78.))
        .child(th_num("权重", 58.))
        .child(th_num("年化", 62.))
        .child(div().w(px(92.)).flex_none());

    let rows: Vec<Div> = data
        .positions
        .iter()
        .map(|position| {
            let pnl_tone = position.pnl_pct.map(signed_tone).unwrap_or(Tone::Muted);
            let mut row = div()
                .h_flex()
                .items_center()
                .gap(px(8.))
                .py(px(9.))
                .px(px(4.))
                .border_b_1()
                .border_color(rgb(DIVIDER))
                .hover(|style| style.bg(rgb(LAYER)))
                .child(
                    div()
                        .v_flex()
                        .flex_1()
                        .min_w(px(150.))
                        .gap(px(1.))
                        .child(
                            div()
                                .text_size(px(13.5))
                                .font_weight(FontWeight::MEDIUM)
                                .child(position.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(rgb(TEXT_TERTIARY))
                                .child(position.code.clone()),
                        ),
                )
                .child(
                    div()
                        .w(px(56.))
                        .flex_none()
                        .h_flex()
                        .items_center()
                        .gap(px(6.))
                        .child(dot(category_color(&position.category_cn)))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(rgb(TEXT_SECONDARY))
                                .child(position.category_cn.clone()),
                        ),
                )
                .child(numeric(format!("{:.2}", position.shares), 74.))
                .child(numeric(format!("{:.4}", position.cost_price), 74.));

            let price_cell = match position.price {
                Some(price) => numeric(format!("{price:.4}"), 74.),
                None => div()
                    .w(px(74.))
                    .flex_none()
                    .h_flex()
                    .justify_end()
                    .child(pill("缺价", WARNING, WARNING_SOFT)),
            };

            row = row
                .child(price_cell)
                .child(numeric(money(position.market_value), 96.))
                .child(
                    div().w(px(78.)).flex_none().h_flex().justify_end().child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(tone_color(pnl_tone)))
                            .child(
                                position
                                    .pnl_pct
                                    .map(|value| {
                                        format!(
                                            "{} {}",
                                            if value > 0.0 {
                                                "↑"
                                            } else if value < 0.0 {
                                                "↓"
                                            } else {
                                                "·"
                                            },
                                            pct_signed(value)
                                        )
                                    })
                                    .unwrap_or_else(|| "—".to_owned()),
                            ),
                    ),
                )
                .child(numeric(pct_points(position.weight_pct), 58.))
                .child(
                    div().w(px(62.)).flex_none().h_flex().justify_end().child(
                        div()
                            .text_size(px(12.5))
                            .text_color(rgb(TEXT_SECONDARY))
                            .child(
                                position
                                    .annualized
                                    .map(pct_signed)
                                    .unwrap_or_else(|| "—".to_owned()),
                            ),
                    ),
                )
                .child(
                    div()
                        .h_flex()
                        .gap(px(4.))
                        .w(px(92.))
                        .flex_none()
                        .justify_end()
                        .child(small_action(
                            format!("buy-{}", position.code),
                            "买",
                            {
                                let code = position.code.clone();
                                move |shell: &mut AppShell, window, cx| {
                                    shell.open_trade(TradeSide::Buy, code.clone(), window, cx);
                                }
                            },
                            cx,
                        ))
                        .child(small_action(
                            format!("sell-{}", position.code),
                            "卖",
                            {
                                let code = position.code.clone();
                                move |shell: &mut AppShell, window, cx| {
                                    shell.open_trade(TradeSide::Sell, code.clone(), window, cx);
                                }
                            },
                            cx,
                        )),
                );
            row
        })
        .collect();

    card.child(header)
        .child(div().v_flex().children(rows))
        .child(
            div()
                .h_flex()
                .justify_between()
                .pt(px(12.))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(TEXT_TERTIARY))
                        .child("权重 = 市值 / 总资产；年化在持仓不足门槛天数时留空。"),
                )
                .child(
                    Button::new("portfolio-add")
                        .label("添加标的")
                        .on_click(cx.listener(|shell, _, window, cx| {
                            shell.open_add_asset(window, cx);
                        })),
                ),
        )
}

fn small_action(
    id: String,
    label: &'static str,
    action: impl Fn(&mut AppShell, &mut Window, &mut Context<AppShell>) + 'static,
    cx: &mut Context<AppShell>,
) -> Button {
    Button::new(id)
        .compact()
        .outline()
        .label(label)
        .on_click(cx.listener(move |shell, _, window, cx| action(shell, window, cx)))
}

/// The three legs keep one colour each across pages, so a glance at the dot
/// already tells you which sleeve a holding belongs to.
fn category_color(category_cn: &str) -> u32 {
    match category_cn {
        "美股" => ACCENT,
        "A股" => 0x8764b8,
        "逆周期" => 0x0e8a8f,
        _ => TEXT_TERTIARY,
    }
}
