//! 总览：账户现在怎么样，今天是否需要动手。

use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::*;

use crate::app::{RebalanceView, Snapshot};
use chrono::NaiveDate;
use crate::gui::chart;
use crate::gui::components::*;
use crate::gui::mood::Mood;
use crate::gui::theme::*;
use crate::gui::{AppShell, CashKind, Page, TradeSide};

use super::{card_header, notice, page_lead, page_root, row};

pub fn render(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    // 720 首屏预算：首页比其它页更紧一档（顶部留白与卡片间距各收 3-6px），
    // 换来「资产曲线」标题能在四连指标卡下方露头，用户才知道下面还有内容。
    let mut root = page_root("page-dashboard")
        .pt(px(14.))
        .gap(px(10.))
        .child(page_lead(Page::Dashboard.subtitle()));

    match (&shell.data, &shell.load_error) {
        (Some(data), _) => {
            let mood = Mood::resolve(Some(data));
            if data.positions.is_empty() && data.cash.abs() < 0.005 {
                root = root.child(start_card(cx));
            } else {
                root = root.child(hero(data));
            }
            root = root
                .child(conclusion_card(data, cx))
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
                .child(allocation_card(data))
                .child(activity_card(data, cx))
                .child(report_card(shell, cx));
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
        // 首屏预算：hero 是最大的一块，收紧内边距与行距比压曲线更划算
        // （曲线高度留给折叠线以下的图形，标题必须先露头）。
        .p(px(24.))
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
                .gap(px(9.))
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
                        .text_size(px(46.))
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
    // 独占满宽后不再需要 flex_1：那一份伸缩是给「与结论卡并排」准备的，
    // 放进纵向页面里会让卡片被拉高。
    let mut card = card_pad().child(card_header("配置位置", "当前权重 / 目标权重"));
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
    card.child(caption("偏离超过 5pp 的腿，在计划里会被优先补足。"))
}

/// F1 结论推导（纯函数，便于回归测试）。
///
/// 结论只有决策日语义（与 `mns report` 决策版同源）。非决策日永远先给
/// "本周不操作"——偏离数字只是背景，不构成当日可执行建议；这样卡片和
/// CLI 观察版报告永远说同一句话。`rebalance` 为 None（本地还没有情绪
/// 数据）时返回 None，由调用方给空态。
fn weekly_conclusion(
    decision_day: bool,
    rebalance: Option<&RebalanceView>,
    next_day: Option<NaiveDate>,
    decision_weekday: u32,
) -> Option<(&'static str, Tone, Vec<String>)> {
    let rebal = rebalance?;
    let (headline, tone, detail) = if decision_day {
        if rebal.has_orders() {
            let mut lines: Vec<String> = rebal
                .legs
                .iter()
                .filter(|leg| leg.amount.abs() >= 1.0)
                .map(|leg| {
                    if leg.amount > 0.0 {
                        format!(
                            "{}腿低配 {:.1}pp，建议补 {}",
                            leg.category_cn,
                            -leg.drift_pp,
                            money(leg.amount)
                        )
                    } else {
                        format!(
                            "{}腿超配 {:.1}pp，建议减 {}",
                            leg.category_cn,
                            leg.drift_pp,
                            money(-leg.amount)
                        )
                    }
                })
                .collect();
            // 部分腿买不到：金额已被引擎清零，但用户需要知道这条腿为什么没被安排。
            // 只披露「无可买标的」这一种，带宽内/低金额不占版面（最多 2 条：
            // 三条腿里至少有一条可执行，否则会走下面的无订单分支）。
            for leg in rebal.legs.iter().filter(|leg| {
                leg.amount.abs() < 1.0
                    && leg.hold_reason.as_deref() == Some(crate::strategy::NO_INSTRUMENT_REASON)
            }) {
                lines.push(format!(
                    "{}腿{}",
                    leg.category_cn,
                    crate::gui::hold_reason_text(crate::strategy::NO_INSTRUMENT_REASON)
                ));
            }
            if rebal.cash_constrained {
                lines.push("现金不足，买入金额已按可用资金缩减".to_owned());
            }
            ("本周操作", Tone::Accent, lines)
        } else {
            // 决策日无订单：如实给出每条腿不动的原因（带宽内/低于
            // 最小交易额/持有期未满），而不是笼统一句"没问题"。
            // 三条腿原因相同时去重（例如都没标的），避免同一句话抄三遍。
            let mut lines: Vec<String> = Vec::new();
            for leg in &rebal.legs {
                let Some(reason) = &leg.hold_reason else { continue };
                if !lines.contains(reason) {
                    lines.push(reason.clone());
                }
            }
            let all_no_instrument = !lines.is_empty()
                && lines
                    .iter()
                    .all(|line| line == crate::strategy::NO_INSTRUMENT_REASON);
            let lines = if lines.is_empty() {
                vec!["偏离都在带宽内，无需操作".to_owned()]
            } else if all_no_instrument {
                // 与 `mns report` 建池引导同口径：不报金额，只给下一步。
                vec![
                    crate::gui::hold_reason_text(crate::strategy::NO_INSTRUMENT_REASON),
                    "今天不动，建池后下个决策日执行".to_owned(),
                ]
            } else {
                lines.iter().map(|line| crate::gui::hold_reason_text(line)).collect()
            };
            ("本周不操作", Tone::Muted, lines)
        }
    } else {
        // 非决策日：结论固定为"不操作"，下次决策日 + 最大偏离只作背景。
        let next_line = match next_day {
            Some(day) => format!(
                "下次决策日 {}（{}）",
                day.format("%m-%d"),
                crate::app::weekday_label(decision_weekday)
            ),
            None => "每天都是决策日".to_owned(),
        };
        let drift_line = rebal
            .legs
            .iter()
            .max_by(|a, b| {
                a.drift_pp
                    .abs()
                    .partial_cmp(&b.drift_pp.abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|leg| {
                if leg.drift_pp.abs() > rebal.band_pp {
                    // 措辞与决策日分支一致：低配/超配，非专业用户不需要换算正负号。
                    let direction = if leg.drift_pp < 0.0 { "低配" } else { "超配" };
                    format!(
                        "{}腿{} {:.1}pp，留到决策日再处理",
                        leg.category_cn,
                        direction,
                        leg.drift_pp.abs()
                    )
                } else {
                    "目前偏离都在带宽内".to_owned()
                }
            })
            .unwrap_or_else(|| "目前偏离都在带宽内".to_owned());
        ("本周不操作", Tone::Muted, vec![next_line, drift_line])
    };
    Some((headline, tone, detail))
}

/// F1 本周结论卡（满宽，首屏第一诉求）。
///
/// 只留「操作还是不操作」与具体动作；分数、区间、情绪走势交给 hero 与
/// 「情绪分数」瓦片，同一屏里不重复第三遍。底部一行脚注守住边界：情绪只
/// 决定目标仓位，不构成买卖依据。
///
/// 左列放结论与入口，右列放动作明细：这样决策日的 3-4 条腿不会把卡片撑高，
/// 资产曲线还能在 720 首屏露出标题。
fn conclusion_card(data: &Snapshot, cx: &mut Context<AppShell>) -> Div {
    // 比通用卡片再紧一档：这张卡要在 720 首屏里和 hero、四连指标卡共存。
    let card = card_pad()
        .p(px(18.))
        .gap(px(12.))
        .child(card_header("本周结论", "操作与否一次说清，情绪只是背景"));
    let Some(signal) = &data.signal else {
        return card.child(conclusion_empty(
            IconName::Gauge,
            "还没有情绪数据",
            "点右上角「更新数据」抓取一次，这里就能给出本周的操作结论。",
        ));
    };
    let conclusion = weekly_conclusion(
        signal.decision_day,
        data.rebalance.as_ref(),
        data.next_decision_day,
        data.config.decision_weekday,
    );
    let Some((headline, tone, detail)) = conclusion else {
        return card.child(conclusion_empty(
            IconName::Gauge,
            "还没有可执行的结论",
            "情绪历史攒够区间确认后，这里会写明本周该不该动手。",
        ));
    };

    card.child(
        div()
            .h_flex()
            .items_start()
            .gap(px(28.))
            .child(
                div()
                    .v_flex()
                    .w(px(380.))
                    .flex_none()
                    .gap(px(7.))
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                div()
                                    .text_size(px(25.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(tone_color(tone)))
                                    .child(headline),
                            )
                            .child(pill(
                                if signal.decision_day { "决策日" } else { "观察日" },
                                tone_color(Tone::Muted),
                                tone_soft(Tone::Muted),
                            ))
                            // 金额口径：决策日的数字是「按全部现金再平衡」算的，
                            // 与计划页的月度定投不是一回事。与 pill 同排、12px
                            // tertiary，不新增行、不抬卡高（headline 25px 撑着
                            // 这一行，12px 文本不会撑高）；观察日不渲染。
                            .children(signal.decision_day.then(|| {
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(TEXT_TERTIARY))
                                    .child("按全部现金再平衡")
                            })),
                    )
                    .child(
                        // 目标仓位与两个入口同排：少一行就少 ~25px，首屏预算里
                        // 这 25px 决定「资产曲线」标题能否露头。
                        div()
                            .h_flex()
                            .items_center()
                            .gap(px(10.))
                            .child(caption(format!(
                                "目标风险仓位 {}",
                                pct_points(rebal_target_risk(data))
                            )))
                            .child(
                                Button::new("goto-plan")
                                    .label("看本月计划")
                                    .on_click(cx.listener(|shell, _, _, cx| {
                                        shell.navigate(Page::Plan, cx)
                                    })),
                            )
                            .child(
                                Button::new("goto-portfolio")
                                    .label("看持仓")
                                    .on_click(cx.listener(|shell, _, _, cx| {
                                        shell.navigate(Page::Portfolio, cx)
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    // 决策日 3-4 条腿也要压在同一高度：行距比正文字号更占地方。
                    .gap(px(3.))
                    .children(detail.into_iter().map(|line| {
                        div()
                            .text_size(px(12.5))
                            .text_color(rgb(TEXT_SECONDARY))
                            .child(line)
                    }))
                    .child(
                        div()
                            .pt(px(2.))
                            .text_size(px(12.))
                            .text_color(rgb(TEXT_TERTIARY))
                            .child(EMOTION_BOUNDARY_NOTE),
                    ),
            ),
    )
}

/// 结论卡脚注。≤12 字是产品给的硬约束（一行、不占版面），
/// 内容是防止「情绪偏冷 → 该买」被读成因果。
const EMOTION_BOUNDARY_NOTE: &str = "情绪只调仓位，不决定买卖";

/// 结论卡的窄空态。
///
/// 竖排空态（图标 + 标题 + 说明，py 22）在满宽卡里会把卡片撑到普通形态的
/// 两倍高，首屏就被顶出去了。这里改成一行横排：图标变成陪衬，引导文案占
/// 主位，卡高只有 ~50px —— 「卡高不塌」指的是不能塌成一条线，不是要撑满。
fn conclusion_empty(icon: IconName, title: &'static str, body: &'static str) -> Div {
    div()
        .h_flex()
        .items_center()
        .gap(px(12.))
        .py(px(8.))
        .child(
            div()
                .w(px(38.))
                .h(px(38.))
                .flex_none()
                .rounded(px(999.))
                .bg(rgb(ACCENT_TINT))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(19.))
                .text_color(rgb(ACCENT))
                .child(icon),
        )
        .child(
            div()
                .v_flex()
                .gap(px(3.))
                .child(
                    div()
                        .text_size(px(14.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(TEXT))
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(TEXT_TERTIARY))
                        .child(body),
                ),
        )
}

/// 目标风险仓位（%）。有再平衡结论时用它的目标值，否则退回当前仓位，
/// 保证卡片在没有结论数据时也不显示空数字。
fn rebal_target_risk(data: &Snapshot) -> f64 {
    data.rebalance
        .as_ref()
        .map(|r| r.target_risk_pct)
        .unwrap_or_else(|| data.risk_weight_pct())
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

/// F4 最近报告卡：`reports/` 最新一份存档的只读回看。
///
/// 展开后由页面自身的滚动条承接全文——不做嵌套滚动区（与助手页
/// "一页只有一条滚动条"的约定一致）。等宽字体是必须的：报告里的
/// 分节线和对齐按固定宽度排版，比例字体会把它们排歪。
fn report_card(shell: &AppShell, cx: &mut Context<AppShell>) -> Div {
    let mut card = card_pad().child(card_header("最近报告", "每周决策的完整存档，只读"));
    // 不用 and_then：闭包的匿名生命周期会把借用到闭包调用为止。
    let report = match shell.data.as_ref() {
        Some(data) => data.latest_report.as_ref(),
        None => None,
    };
    let Some(report) = report else {
        return card.child(empty_state(
            IconName::CalendarCheck,
            "还没有报告",
            "点右上角「更新数据」生成一次报告，这里就能随时回看全文。",
        ));
    };

    // 决策版标题是「每日策略报告」（report.rs），全文没有"决策报告"四个字；
    // 观察版含「决策版报告」（带"版"字）不会误匹配到这里。
    let (kind, kind_tone) = if report.content.contains("观察报告") {
        ("观察报告", Tone::Muted)
    } else if report.content.contains("每日策略报告") {
        ("决策报告", Tone::Accent)
    } else {
        ("报告", Tone::Muted)
    };

    let lines: Vec<&str> = report.content.lines().collect();
    let visible: Vec<String> = if shell.report_expanded {
        lines.iter().map(|line| line.to_string()).collect()
    } else {
        lines.iter().take(10).map(|line| line.to_string()).collect()
    };

    let text_rows: Vec<Div> = visible
        .iter()
        .map(|line| {
            div()
                .text_size(px(11.5))
                .line_height(px(18.))
                .font_family(mono_family())
                .text_color(rgb(TEXT_SECONDARY))
                .child(if line.is_empty() {
                    " ".to_owned()
                } else {
                    line.clone()
                })
        })
        .collect();

    card = card.child(
        div()
            .h_flex()
            .items_center()
            .gap(px(8.))
            .child(pill(kind, tone_color(kind_tone), tone_soft(kind_tone)))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(report.date.clone()),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(TEXT_TERTIARY))
                    .child(format!("{} 行", lines.len())),
            )
            .child(div().flex_1())
            .child(
                Button::new(if shell.report_expanded {
                    "report-collapse"
                } else {
                    "report-expand"
                })
                .label(if shell.report_expanded {
                    "收起"
                } else if lines.len() > 10 {
                    "展开全文"
                } else {
                    "展开"
                })
                .on_click(cx.listener(|shell, _, _, cx| shell.toggle_report(cx))),
            ),
    );

    if shell.report_expanded {
        card = card.child(
            div()
                .v_flex()
                .mt(px(8.))
                .p(px(12.))
                .bg(rgb(LAYER))
                .border_1()
                .border_color(rgb(DIVIDER))
                .rounded(px(RADIUS_SM))
                .children(text_rows),
        );
    } else {
        card = card.child(
            div()
                .v_flex()
                .mt(px(8.))
                .children(text_rows)
                .child(if lines.len() > 10 {
                    div().text_size(px(12.)).text_color(rgb(TEXT_TERTIARY)).child("…")
                } else {
                    div()
                }),
        );
    }

    card
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

// 测试只测纯函数（weekly_conclusion），因此刻意窄导入——
// `use super::*` 会连带引入 gpui 的 prelude，把 #[test] 的宏展开撑爆。
#[cfg(test)]
mod tests {
    use super::weekly_conclusion;
    use crate::app::{LegView, RebalanceView};
    use crate::gui::components::Tone;
    use chrono::NaiveDate;

    fn rebalance(drifts: [f64; 3], amounts: [f64; 3], band: f64) -> RebalanceView {
        let categories = ["美股", "A股", "逆周期"];
        RebalanceView {
            score: 50.0,
            target_risk_pct: 84.0,
            current_risk_pct: 70.0,
            band_pp: band,
            total_buy: amounts.iter().sum(),
            total_sell: 0.0,
            cash_constrained: false,
            legs: categories
                .iter()
                .enumerate()
                .map(|(i, name)| LegView {
                    category_cn: name.to_string(),
                    target_pct: 30.0,
                    current_pct: 30.0,
                    drift_pp: drifts[i],
                    amount: amounts[i],
                    hold_reason: if amounts[i] == 0.0 {
                        Some(format!("{}腿不动原因", name))
                    } else {
                        None
                    },
                    items: Vec::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn 决策日有订单时给操作结论与金额() {
        let rebal = rebalance([-6.2, 0.5, 0.0], [1180.0, 0.0, 0.0], 4.0);
        let (headline, tone, lines) =
            weekly_conclusion(true, Some(&rebal), None, 5).expect("有再平衡数据必有结论");
        assert_eq!(headline, "本周操作");
        assert!(matches!(tone, Tone::Accent));
        assert!(
            lines.iter().any(|l| l.contains("美股腿低配 6.2pp") && l.contains("补")),
            "{lines:?}"
        );
        // 带宽内的腿不进结论行
        assert!(!lines.iter().any(|l| l.contains("A股") || l.contains("逆周期")));
    }

    #[test]
    fn 决策日部分腿缺标的时披露原因且不报该腿金额() {
        // 引擎已把缺标的的腿清零（2026-11-11 修正），卡面不能再给它编一条
        // "建议补 ¥X"，但也不能装作这条腿不存在——用户需要知道下一步做什么。
        let mut rebal = rebalance([-6.2, -3.0, -16.6], [1180.0, 900.0, 0.0], 4.0);
        rebal.legs[2].hold_reason = Some(crate::strategy::NO_INSTRUMENT_REASON.to_owned());
        let (headline, _, lines) = weekly_conclusion(true, Some(&rebal), None, 5).unwrap();
        assert_eq!(headline, "本周操作");
        assert!(
            lines.iter().any(|l| l.contains("逆周期腿目标类别暂无可买标的")),
            "缺标的的腿必须给出补齐指引: {lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.contains("逆周期") && l.contains("建议补")),
            "缺标的的腿不得报金额: {lines:?}"
        );
        // 三条腿里两条有金额、一条缺标的 → 三行封顶，卡高不变。
        assert!(lines.len() <= 3, "结论行不能撑高卡片: {lines:?}");
    }

    #[test]
    fn 决策日全腿缺标的时给两句引导且不报金额() {
        let mut rebal = rebalance([-6.2, -3.0, -16.6], [0.0, 0.0, 0.0], 4.0);
        for leg in &mut rebal.legs {
            leg.hold_reason = Some(crate::strategy::NO_INSTRUMENT_REASON.to_owned());
        }
        let (headline, tone, lines) = weekly_conclusion(true, Some(&rebal), None, 5).unwrap();
        assert_eq!(headline, "本周不操作");
        assert!(matches!(tone, Tone::Muted));
        assert_eq!(lines.len(), 2, "同一句原因不应抄三遍: {lines:?}");
        // 原因来自引擎常量（判等仍用它），动作句由 GUI 渲染层拼。
        assert!(lines[0].contains(crate::strategy::NO_INSTRUMENT_REASON));
        assert!(lines[0].contains("去持仓页添加标的"), "应给出 GUI 的下一步: {lines:?}");
        assert!(lines[1].contains("建池后下个决策日执行"));
        assert!(
            !lines.iter().any(|l| l.contains("建议补")),
            "无可执行标的时不得报金额: {lines:?}"
        );
    }

    #[test]
    fn 决策日无订单时如实给出不动原因() {
        let rebal = rebalance([0.5, -1.0, 0.0], [0.0, 0.0, 0.0], 4.0);
        let (headline, _, lines) = weekly_conclusion(true, Some(&rebal), None, 5).unwrap();
        assert_eq!(headline, "本周不操作");
        assert!(
            lines.iter().any(|l| l.contains("不动原因")),
            "必须给出腿级原因而不是笼统话术: {lines:?}"
        );
    }

    #[test]
    fn 非决策日永远先说不操作且金额不可执行() {
        let rebal = rebalance([-6.2, 0.5, 0.0], [1180.0, 0.0, 0.0], 4.0);
        let friday = NaiveDate::from_ymd_opt(2026, 11, 13).unwrap();
        let (headline, _, lines) =
            weekly_conclusion(false, Some(&rebal), Some(friday), 5).unwrap();
        assert_eq!(headline, "本周不操作");
        assert!(
            !lines.iter().any(|l| l.contains("建议补") || l.contains("建议减")),
            "非决策日不得出现可执行金额建议: {lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains("下次决策日 11-13") && l.contains("周五")));
        assert!(
            lines.iter().any(|l| l.contains("美股腿低配 6.2pp") && l.contains("留到决策日再处理")),
            "超带偏离措辞须与决策日分支一致（低配/超配）: {lines:?}"
        );
    }

    #[test]
    fn 非决策日带内偏离给确定感() {
        let rebal = rebalance([1.2, -0.5, 0.0], [0.0, 0.0, 0.0], 4.0);
        let (_, _, lines) = weekly_conclusion(false, Some(&rebal), None, 0).unwrap();
        assert!(lines.iter().any(|l| l.contains("目前偏离都在带宽内")));
        // decision_weekday = 0（每天决策）时不应出现"下次决策日"
        assert!(!lines.iter().any(|l| l.contains("下次决策日")));
    }

    #[test]
    fn 没有再平衡数据时卡片走空态而不是伪造结论() {
        assert!(weekly_conclusion(true, None, None, 5).is_none());
    }

    #[test]
    fn 现金不足时结论必须披露缩减() {
        let mut rebal = rebalance([-6.2, 0.5, 0.0], [1180.0, 0.0, 0.0], 4.0);
        rebal.cash_constrained = true;
        let (_, _, lines) = weekly_conclusion(true, Some(&rebal), None, 5).unwrap();
        assert!(lines.iter().any(|l| l.contains("现金不足")));
    }

    #[test]
    fn 结论卡脚注不超过十二字且写明情绪不决定买卖() {
        let note = super::EMOTION_BOUNDARY_NOTE;
        assert!(
            note.chars().count() <= 12,
            "脚注必须能压成一行（≤12 字），实际 {} 字：{note}",
            note.chars().count()
        );
        assert!(
            note.contains("不决定买卖"),
            "脚注必须守住「情绪不是买卖点」的边界：{note}"
        );
    }
}
