//! 设置：风险档位、定投金额、决策日与外部 Agent。

use gpui_kit::base::StyledExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::*;

use crate::gui::components::*;
use crate::gui::theme::*;
use crate::gui::{AppShell, Page};

use super::{card_header, page_lead, page_root};

pub fn render(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let mut root = page_root("page-settings").child(page_lead(Page::Settings.subtitle()));

    let Some(data) = &shell.data else {
        return root
            .child(
                card_pad().child(
                    div()
                        .text_size(px(12.5))
                        .text_color(rgb(TEXT_SECONDARY))
                        .child("正在读取本地配置…"),
                ),
            )
            .into_any_element();
    };
    let config = &data.config;

    // ── 风险档位 ────────────────────────────────────────────────
    let profile_cards: Vec<_> = config
        .profiles
        .iter()
        .map(|profile| {
            let active = profile.active;
            let mut card = div()
                .id(SharedString::from(format!("profile-{}", profile.id)))
                .v_flex()
                .flex_1()
                .min_w(px(170.))
                .gap(px(6.))
                .p(px(14.))
                .rounded(px(RADIUS))
                .bg(rgb(if active { ACCENT_SOFT } else { SURFACE }))
                .border_1()
                .border_color(rgb(if active { ACCENT } else { BORDER }))
                .cursor_pointer()
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(if active { ACCENT } else { TEXT }))
                                .child(profile.label.clone()),
                        )
                        .child(if active {
                            pill("当前", ACCENT, 0xffffff)
                        } else {
                            div()
                        }),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(TEXT_SECONDARY))
                        .child(profile.description.clone()),
                )
                .on_click(cx.listener({
                    let id = profile.id.clone();
                    move |shell, _, _, cx| shell.apply_profile(id.clone(), cx)
                }));
            if !active {
                card = card.hover(|style| {
                    style
                        .bg(rgb(ACCENT_TINT))
                        .border_color(rgb(0xbfd4f7))
                        .shadow(shadow_raised())
                });
            }
            card
        })
        .collect();

    root = root.child(
        card_pad()
            .gap(px(12.))
            .child(card_header(
                "风险档位",
                "抬高常驻仓位并收窄择时摆幅，不改变你配置的三条腿",
            ))
            .child(
                div()
                    .h_flex()
                    .gap(px(10.))
                    .items_stretch()
                    .children(profile_cards),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(TEXT_TERTIARY))
                    .child(format!(
                        "当前中性目标仓位 {:.0}%。档位越高，曲线越接近满仓，回撤也越大。",
                        config.neutral_weight
                    )),
            ),
    );

    // ── 定投与决策日 ────────────────────────────────────────────
    let weekday_rows: Vec<Div> = (0..8u32)
        .collect::<Vec<_>>()
        .chunks(4)
        .map(|chunk| {
            let chips: Vec<Button> = chunk
                .iter()
                .map(|weekday| {
                    let active = config.decision_weekday == *weekday;
                    let mut button = Button::new(format!("weekday-{weekday}"))
                        .compact()
                        .label(crate::app::weekday_label(*weekday));
                    if active {
                        button = button.primary();
                    }
                    let weekday = *weekday;
                    button.on_click(
                        cx.listener(move |shell, _, _, cx| shell.apply_weekday(weekday, cx)),
                    )
                })
                .collect();
            div().h_flex().gap(px(6.)).children(chips)
        })
        .collect();

    root = root.child(
        div()
            .h_flex()
            .gap(px(12.))
            .items_stretch()
            .child(
                card_pad()
                    .flex_1()
                    .gap(px(10.))
                    .child(card_header("每月定投", "只买不卖；未买完的钱留在现金里"))
                    .child(
                        div()
                            .h_flex()
                            .gap(px(8.))
                            .items_end()
                            .child(
                                div()
                                    .v_flex()
                                    .flex_1()
                                    .gap(px(4.))
                                    .child(caption("金额（元）"))
                                    .child(Input::new(&shell.monthly)),
                            )
                            .child(
                                Button::new("save-monthly")
                                    .primary()
                                    .label("保存")
                                    .on_click(
                                        cx.listener(|shell, _, _, cx| shell.save_monthly(cx)),
                                    ),
                            ),
                    )
                    .child(caption(format!(
                        "再平衡带宽 ±{:.1}pp · 单笔最小 {}",
                        config.band_pp,
                        money(config.min_trade_amount)
                    ))),
            )
            .child(
                card_pad()
                    .flex_1()
                    .gap(px(10.))
                    .child(card_header(
                        "决策日",
                        "非决策日只出观察报告，不产生买卖建议",
                    ))
                    .child(div().v_flex().gap(px(6.)).children(weekday_rows))
                    .child(caption(format!("当前：{}", config.decision_label))),
            ),
    );

    // ── 外部 Agent ─────────────────────────────────────────────
    root = root.child(
        card_pad()
            .gap(px(10.))
            .child(card_header(
                "外部 Agent",
                "只配置 CLI 路径与模型名；推理端点与凭据由 opencode 自己管理",
            ))
            .child(
                div()
                    .h_flex()
                    .gap(px(12.))
                    .child(
                        div()
                            .v_flex()
                            .flex_1()
                            .gap(px(4.))
                            .child(caption("CLI 路径"))
                            .child(Input::new(&shell.cli)),
                    )
                    .child(
                        div()
                            .v_flex()
                            .flex_1()
                            .gap(px(4.))
                            .child(caption("工作目录"))
                            .child(Input::new(&shell.cwd)),
                    )
                    .child(
                        div().v_flex().gap(px(4.)).child(caption(" ")).child(
                            Button::new("settings-save-agent")
                                .primary()
                                .label("保存")
                                .on_click(
                                    cx.listener(|shell, _, _, cx| shell.save_agent_settings(cx)),
                                ),
                        ),
                    ),
            )
            .child(
                div()
                    .v_flex()
                    .gap(px(4.))
                    .child(caption("模型（可选）"))
                    .child(Input::new(&shell.model)),
            )
            .child(caption(
                "MNS 自动追加 `acp` 子命令。模型留空则沿用 opencode 自己配置的默认模型；\
                 需要指定时按 provider/model 填写，例如 opencode/deepseek-v4.1-flash-free。\
                 这里只保存名字、不内置清单——免费模型随时会变。",
            )),
    );

    // ── 数据与文件 ─────────────────────────────────────────────
    root = root.child(
        card_pad()
            .gap(px(6.))
            .child(card_header(
                "数据与文件",
                "CLI 与桌面端共用同一份配置和数据库",
            ))
            .child(kv_row("配置文件", config.config_path.clone()))
            .child(kv_row("数据库", config.db_path.clone()))
            .child(kv_row("报告目录", config.report_dir.clone()))
            .child(divider())
            .child(
                div()
                    .h_flex()
                    .gap(px(24.))
                    .child(badge_metric("持仓", data.counts.positions))
                    .child(badge_metric("每日快照", data.counts.snapshots))
                    .child(badge_metric("交易记录", data.counts.trades))
                    .child(badge_metric("资金流水", data.counts.flows))
                    .child(div().flex_1())
                    .child({
                        let report_dir = config.report_dir.clone();
                        Button::new("open-report-dir")
                            .label("打开报告目录")
                            .on_click(move |_, _, _| reveal_dir(&report_dir))
                    }),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(TEXT_TERTIARY))
                    .child(format!(
                        "数据新鲜度：{}。审计、历史回测等研究功能仍在 CLI，避免把桌面端做成研究终端。",
                        data.freshness.summary()
                    )),
            ),
    );

    root.into_any_element()
}

fn badge_metric(label: &'static str, value: usize) -> Div {
    div()
        .v_flex()
        .gap(px(2.))
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgb(TEXT_TERTIARY))
                .child(label),
        )
        .child(
            div()
                .text_size(px(16.))
                .font_weight(FontWeight::MEDIUM)
                .child(value.to_string()),
        )
}

fn reveal_dir(path: &str) {
    if path.is_empty() {
        return;
    }
    let program = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program).arg(path).spawn();
}
