//! 设置：风险档位、定投金额、决策日与外部 Agent。

use gpui_kit::base::StyledExt;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;

use crate::gui::AppShell;
use crate::gui::components::*;
use crate::gui::help_links;
use crate::gui::theme::*;

use super::{card_header, card_header_linked, page_root};

pub fn render(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let mut root = page_root("page-settings");

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
            .child(card_header_linked("风险档位", "", help_links::RISK_PROFILE))
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
                    .child(format!("当前中性档目标 {:.0}%", config.neutral_weight)),
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
                    .child(card_header("每月定投", "只买不卖"))
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
                    .child(card_header("决策日", ""))
                    .child(div().v_flex().gap(px(6.)).children(weekday_rows))
                    .child(caption(format!("当前：{}", config.decision_label))),
            ),
    );

    // ── 外部 Agent ─────────────────────────────────────────────
    root = root.child(
        card_pad()
            .gap(px(10.))
            .child(card_header("外部 Agent", ""))
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
                "留空则使用 opencode 默认模型；按 provider/model 填写。",
            )),
    );

    // ── 启动与后台 ─────────────────────────────────────────────
    let launch = shell.agent_settings.launch_at_login;
    let mut launch_toggle = Button::new("launch-at-login").compact().label(if launch {
        "已开启"
    } else {
        "已关闭"
    });
    if launch {
        launch_toggle = launch_toggle.primary();
    }
    root = root.child(
        card_pad()
            .gap(px(8.))
            .child(card_header("启动与后台", ""))
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap(px(9.))
                            .child(dot(if launch { SUCCESS } else { TEXT_TERTIARY }))
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("开机自启动"),
                            ),
                    )
                    .child(launch_toggle.on_click(cx.listener(|shell, _, _, cx| {
                        shell.toggle_launch_at_login(cx);
                    }))),
            )
            .child(caption(
                "登录后常驻托盘，不打开窗口；管家横幅也能及时送达。",
            )),
    );

    // ── 行为记录（管家） ───────────────────────────────────────
    let behavior = shell.behavior_enabled;
    let mut behavior_toggle = Button::new("behavior-log").compact().label(if behavior {
        "已开启"
    } else {
        "已关闭"
    });
    if behavior {
        behavior_toggle = behavior_toggle.primary();
    }
    root = root.child(
        card_pad()
            .gap(px(8.))
            .child(card_header("行为记录", "供管家参考"))
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap(px(9.))
                            .child(dot(if behavior { SUCCESS } else { TEXT_TERTIARY }))
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("本地行为记录"),
                            ),
                    )
                    .child(behavior_toggle.on_click(cx.listener(|shell, _, _, cx| {
                        shell.toggle_behavior_logging(cx);
                    }))),
            )
            .child(caption(
                "记录打开应用、切换页面、打开买入弹层与护栏命中；只存在本机，不上传，可随时关闭或清除。",
            ))
            .child(
                div().h_flex().justify_end().child(
                    Button::new("clear-behavior")
                        .compact()
                        .label("清除记录")
                        .on_click(cx.listener(|shell, _, _, cx| shell.clear_behavior_log(cx))),
                ),
            ),
    );

    // ── 管家 ─────────────────────────────────────────────────
    let proactive = shell.butler_proactive;
    let mut proactive_toggle = Button::new("butler-proactive")
        .compact()
        .label(if proactive { "已开启" } else { "已停止" });
    if proactive {
        proactive_toggle = proactive_toggle.primary();
    }
    root = root.child(
        card_pad()
            .gap(px(8.))
            .child(card_header("管家", "本地 · 只读账本"))
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap(px(9.))
                            .child(dot(if proactive { SUCCESS } else { TEXT_TERTIARY }))
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("主动开口"),
                            ),
                    )
                    .child(proactive_toggle.on_click(cx.listener(|shell, _, _, cx| {
                        shell.toggle_butler_proactive(cx);
                    }))),
            )
            .child(caption(
                "开口会弹一条系统通知（macOS / Windows 11）并在托盘菜单留一条，点开即进会话；只在本机，不外发。关掉后管家仍可回答问题。",
            ))
            .child(divider())
            .child(caption("主动开口频控（改完立即生效）"))
            .child(
                div()
                    .h_flex()
                    .items_end()
                    .gap(px(12.))
                    .child(number_field("每天最多", &shell.butler_daily, "次"))
                    .child(number_field("每周最多", &shell.butler_weekly, "次"))
                    .child(number_field("同一件事去重", &shell.butler_dedupe, "天")),
            )
            .child(
                div()
                    .h_flex()
                    .items_end()
                    .gap(px(12.))
                    .child(number_field("静音从", &shell.butler_quiet_start, "点"))
                    .child(number_field("到", &shell.butler_quiet_end, "点"))
                    .child(
                        Button::new("save-butler-policy")
                            .primary()
                            .compact()
                            .label("保存频控")
                            .on_click(cx.listener(|shell, _, _, cx| {
                                shell.save_butler_policy(cx);
                            })),
                    ),
            )
            .child(caption(
                "静音时段内不主动开口（起止相同表示不静音）；开关本身仍由上面的「主动开口」控制。频控只限制说多少，不决定能不能说。",
            ))
            .child(
                div().h_flex().justify_end().child(
                    Button::new("open-butler")
                        .compact()
                        .label("打开管家")
                        .on_click(cx.listener(|_, _, window, cx| {
                            window.dispatch_action(
                                Box::new(crate::gui::OpenButlerWindow),
                                cx,
                            );
                        })),
                ),
            ),
    );

    // ── 数据与文件 ─────────────────────────────────────────────
    root = root.child(
        card_pad()
            .gap(px(6.))
            .child(card_header("数据与文件", ""))
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
                    .child(format!("数据新鲜度：{}", data.freshness.summary())),
            ),
    );

    root.into_any_element()
}

/// 设置页里的窄数字输入：标签在上，输入框与单位在同一行。
fn number_field(label: &'static str, input: &Entity<InputState>, unit: &'static str) -> AnyElement {
    div()
        .v_flex()
        .gap(px(4.))
        .child(caption(label))
        .child(
            div()
                .h_flex()
                .items_center()
                .gap(px(6.))
                .child(div().w(px(68.)).child(Input::new(input)))
                .child(caption(unit)),
        )
        .into_any_element()
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
    let _ = crate::gui::command(program).arg(path).spawn();
}
