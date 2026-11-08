//! 计划：本月该买什么、买多少，以及为什么某些腿不动作。

use gpui_kit::base::StyledExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::*;

use crate::app::Snapshot;
use crate::gui::components::*;
use crate::gui::theme::*;
use crate::gui::{AppShell, Page};

use super::{card_header, page_lead, notice, page_root};

pub fn render(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let mut root = page_root("page-plan").child(page_lead(Page::Plan.subtitle()));

    let Some(data) = &shell.data else {
        return root
            .child(
                card_pad().child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(TEXT_SECONDARY))
                        .child("正在读取本地数据…"),
                ),
            )
            .into_any_element();
    };

    let Some(plan) = &data.plan else {
        let monthly = data.config.monthly;
        let profile = data.config.profile_label.clone();
        return root
            .child(
                card_pad()
                    .max_w(px(900.))
                    .relative()
                    .overflow_hidden()
                    .gap(px(18.))
                    // Same arc language as the hero: this card is a stage, not
                    // a document, so it gets light rather than an icon.
                    .child(
                        div()
                            .absolute()
                            .right(px(-150.))
                            .top(px(-170.))
                            .child(crate::gui::chart::gauge_aura(360., ACCENT, 0.22)),
                    )
                    .child(
                        div()
                            .v_flex()
                            .gap(px(6.))
                            .child(
                                div()
                                    .text_size(px(21.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("再补一步，计划就有数字了"),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child("计划由情绪分数与各腿的配置偏离共同生成，缺一个输入就不会出数。"),
                            ),
                    )
                    .child(notice(
                        data.plan_blocked
                            .clone()
                            .unwrap_or_else(|| "缺少生成计划所需的数据。".to_owned()),
                        (WARNING, WARNING_SOFT),
                    ))
                    .child(
                        div()
                            .h_flex()
                            .gap(px(12.))
                            .items_stretch()
                            .child(setup_step(
                                "01",
                                "确认风险档位",
                                format!("当前：{profile}"),
                                true,
                            ))
                            .child(setup_step(
                                "02",
                                "填写每月定投金额",
                                if monthly > 0.0 {
                                    format!("当前：{}", money(monthly))
                                } else {
                                    "当前：未设置，计划无法分配金额".to_owned()
                                },
                                monthly > 0.0,
                            )),
                    )
                    .child(divider())
                    .child(
                        div()
                            .v_flex()
                            .gap(px(7.))
                            .child(overline("生成后你会看到"))
                            .child(
                                div()
                                    .h_flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(caption("三条腿各买多少"))
                                    .child(dot(0xc8cfdd))
                                    .child(caption("预算用掉多少"))
                                    .child(dot(0xc8cfdd))
                                    .child(caption("每条腿为什么买或不买")),
                            ),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap(px(8.))
                            .child(
                                Button::new("plan-goto-settings")
                                    .primary()
                                    .label("去设置金额")
                                    .on_click(cx.listener(|shell, _, _, cx| {
                                        shell.navigate(Page::Settings, cx)
                                    })),
                            )
                            .child(
                                Button::new("plan-refresh")
                                    .label("更新数据")
                                    .on_click(cx.listener(|shell, _, _, cx| shell.refresh(cx))),
                            ),
                    ),
            )
            .into_any_element();
    };

    let scale = if plan.monthly > 0.0 {
        (plan.total_buy / plan.monthly) as f32
    } else {
        0.0
    };

    root = root
        .child(
            div()
                .h_flex()
                .items_center()
                .gap(px(24.))
                .p(px(26.))
                .bg(rgb(SURFACE))
                .border_1()
                .border_color(rgb(BORDER))
                .rounded(px(RADIUS_LG))
                .child(
                    div()
                        .v_flex()
                        .flex_1()
                        .gap(px(6.))
                        .child(caption("本月计划买入"))
                        .child(
                            div()
                                .text_size(px(38.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(money(plan.total_buy)),
                        )
                        .child(
                            div()
                                .h_flex()
                                .items_center()
                                .gap(px(10.))
                                .child(pill(
                                    format!("月投入 {}", money(plan.monthly)),
                                    ACCENT,
                                    ACCENT_SOFT,
                                ))
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .text_color(rgb(TEXT_TERTIARY))
                                        .child(format!(
                                            "预算使用 {:.0}%",
                                            (scale * 100.0).min(999.0)
                                        )),
                                ),
                        ),
                )
                .child(div().w(px(1.)).h(px(64.)).bg(rgb(DIVIDER)))
                .child(
                    div()
                        .h_flex()
                        .gap(px(24.))
                        .items_center()
                        .child(mini("决策分数", format!("{:.0}", plan.score)))
                        .child(mini("目标风险仓位", pct_points(plan.target_risk_pct)))
                        .child(mini("当前风险仓位", pct_points(plan.current_risk_pct)))
                        .child(mini("预计申购费", money(plan.estimated_fees))),
                ),
        )
        .child(legs_card(data))
        .child(
            div()
                .h_flex()
                .gap(px(12.))
                .items_stretch()
                .child(explain_card(data))
                .child(rules_card(data)),
        );

    if plan.cash_constrained {
        root = root.child(notice(
            "现金不足，买入金额已按比例缩减。先入金，或在设置里下调月投入。",
            (WARNING, WARNING_SOFT),
        ));
    }

    root.into_any_element()
}

fn mini(label: &'static str, value: String) -> Div {
    div()
        .v_flex()
        .gap(px(3.))
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgb(TEXT_SECONDARY))
                .child(label),
        )
        .child(
            div()
                .text_size(px(16.))
                .font_weight(FontWeight::MEDIUM)
                .child(value),
        )
}

fn leg_color(name: &str) -> u32 {
    match name {
        "美股" => 0x0067c0,
        "A股" => 0x8764b8,
        "逆周期" => 0x038387,
        _ => 0x8a8a8a,
    }
}

fn legs_card(data: &Snapshot) -> Div {
    let Some(plan) = &data.plan else {
        return card_pad();
    };
    let mut card = card_pad()
        .gap(px(0.))
        .child(div().pb(px(10.)).child(card_header(
            "分腿分配",
            "只买不卖；偏离来自目标权重与当前市值",
        )));

    let header = div()
        .h_flex()
        .items_center()
        .gap(px(10.))
        .py(px(8.))
        .border_b_1()
        .border_color(rgb(BORDER))
        .child(div().flex_1().min_w(px(90.)).child(th("腿")))
        .child(div().w(px(72.)).child(th("目标")))
        .child(div().w(px(72.)).child(th("当前")))
        .child(div().w(px(72.)).child(th("偏离")))
        .child(div().w(px(110.)).child(th("本月买入")));
    card = card.child(header);

    for leg in &plan.legs {
        let color = leg_color(&leg.category_cn);
        let mut row = div()
            .v_flex()
            .gap(px(6.))
            .py(px(10.))
            .border_b_1()
            .border_color(rgb(DIVIDER))
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap(px(7.))
                            .flex_1()
                            .min_w(px(90.))
                            .child(dot(color))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(leg.category_cn.clone()),
                            ),
                    )
                    .child(div().w(px(72.)).child(pct_points(leg.target_pct)))
                    .child(div().w(px(72.)).child(pct_points(leg.current_pct)))
                    .child(div().w(px(72.)).child(pp_signed(leg.drift_pp)))
                    .child(
                        div()
                            .w(px(110.))
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(if leg.amount.abs() < 0.5 {
                                "—".to_owned()
                            } else {
                                money(leg.amount)
                            }),
                    ),
            )
            .child(hbar((leg.current_pct / 100.0) as f32, color, 5.));

        if let Some(reason) = &leg.hold_reason {
            row = row.child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(TEXT_TERTIARY))
                    .child(reason.clone()),
            );
        }

        for item in &leg.items {
            row = row.child(
                div()
                    .h_flex()
                    .justify_between()
                    .pl(px(14.))
                    .child(
                        div()
                            .h_flex()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child(format!("{} {}", item.code, item.name)),
                            )
                            .children(item.note.clone().map(|note| {
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(TEXT_TERTIARY))
                                    .child(note)
                            })),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(TEXT_SECONDARY))
                            .child(money(item.amount)),
                    ),
            );
        }
        card = card.child(row);
    }

    card.child(
        div().h_flex().justify_end().pt(px(12.)).child(
            div()
                .text_size(px(12.5))
                .text_color(rgb(TEXT_SECONDARY))
                .child(format!(
                    "合计买入 {} · 预计费用 {}",
                    money(plan.total_buy),
                    money(plan.estimated_fees)
                )),
        ),
    )
}

fn explain_card(data: &Snapshot) -> Div {
    let Some(plan) = &data.plan else {
        return card_pad();
    };
    let gap = plan.target_risk_pct - plan.current_risk_pct;
    card_pad()
        .flex_1()
        .gap(px(8.))
        .child(section_title("为什么是这个数"))
        .child(kv_row("决策分数", format!("{:.0}", plan.score)))
        .child(kv_row("目标风险仓位", pct_points(plan.target_risk_pct)))
        .child(kv_row("当前风险仓位", pct_points(plan.current_risk_pct)))
        .child(kv_row("需要补的仓位", pp_signed(gap)))
        .child(kv_row("再平衡带宽", format!("±{:.1}pp", plan.band_pp)))
        .child(
            div()
                .text_size(px(11.5))
                .text_color(rgb(TEXT_TERTIARY))
                .child("分数只决定目标仓位；落在带宽内的腿不产生交易，避免碎单。"),
        )
}

fn rules_card(data: &Snapshot) -> Div {
    let config = &data.config;
    card_pad()
        .flex_1()
        .gap(px(8.))
        .child(section_title("这套规则的边界"))
        .child(kv_row("风险档位", config.profile_label.clone()))
        .child(kv_row("决策日", config.decision_label.clone()))
        .child(kv_row("单笔最小金额", money(config.min_trade_amount)))
        .child(kv_row(
            "卖出最短持有",
            format!("{} 天", config.min_holding_days_for_sell),
        ))
        .child(
            div()
                .text_size(px(11.5))
                .text_color(rgb(TEXT_TERTIARY))
                .child("定投不产生超额收益，提升的是储蓄率与执行纪律。系统不会自动下单。"),
        )
}

/// One row of the "finish setup" checklist: a numbered token, the promise, and
/// the current value, so the page explains itself without a modal.
fn setup_step(index: &'static str, title: &'static str, detail: String, done: bool) -> Div {
    let (fg, bg, state) = if done {
        (SUCCESS, SUCCESS_SOFT, "已完成")
    } else {
        (WARNING, WARNING_SOFT, "待填写")
    };
    div()
        .v_flex()
        .flex_1()
        .gap(px(6.))
        .p(px(14.))
        .rounded(px(RADIUS_SM))
        .bg(rgb(LAYER))
        .border_1()
        .border_color(rgb(BORDER))
        .child(
            div()
                .h_flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(fg))
                        .child(index),
                )
                .child(pill(state, fg, bg)),
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
                .text_color(rgb(TEXT_TERTIARY))
                .child(detail),
        )
}
