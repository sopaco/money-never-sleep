//! 巡检：界面上只有一件事——看结论。
//!
//! 首次由 MNS 取数并组装提示词，Agent 判断后回传结构化结论；本轮结束后
//! 报告写入 `butler::review`，巡检页只读回放历史。自由对话与追问不在这里：
//! 原输入框的位置是一张管家引导卡。设置、定时、权限策略与工具轨迹都收进
//! 侧边抽屉：整页只有时间线一个滚动容器。

use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::component::input::Input;
use gpui_kit::component::text::TextView;
use gpui_kit::*;

use crate::agent::{DecisionReport, EntryKind, PermissionPolicy, RunState, TimelineEntry};
use crate::butler::review::{ReviewSummary, strip_decision_block};
use crate::gui::components::*;
use crate::gui::theme::*;
use crate::gui::{AppShell, CloseReviewPanel, ReviewPanel, ToastTone, ViewedReview};

use super::{card_header, notice};

/// 巡检页是固定高度的工作区，不是滚动文档：顶部是状态栏，中部是一条
/// 可滚动的时间线，底部是引导卡。这里刻意不用 `page_root()`——它的
/// 页面级滚动条会和时间线自己的滚动条叠在一起。
pub fn render(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let review = &shell.review;
    let running = review.is_running();
    let (state_color, state_bg) = match review.state {
        RunState::Failed | RunState::Cancelled => (DANGER, DANGER_SOFT),
        RunState::Finished => (SUCCESS, SUCCESS_SOFT),
        RunState::Idle => (TEXT_TERTIARY, NEUTRAL_SOFT),
        _ => (ACCENT, ACCENT_SOFT),
    };
    let trace_count = review
        .timeline
        .iter()
        .filter(|entry| !is_message(entry.kind))
        .count();
    let settings_open = shell.review_panel == Some(ReviewPanel::Settings);
    let history_open = shell.review_panel == Some(ReviewPanel::History);

    let content = div()
        .v_flex()
        .flex_1()
        .min_h(px(0.))
        .px(px(22.))
        .pt(px(20.))
        .pb(px(20.))
        .gap(px(12.))
        .child(header_bar(
            review.state.label(),
            &review.detail,
            state_color,
            state_bg,
            settings_open,
            history_open,
            shell.reviews.len(),
            cx,
        ))
        .child(daily_bar(shell, cx, running))
        .child(conversation_card(shell, cx, running, trace_count));

    div()
        .id("page-review")
        .v_flex()
        .relative()
        .flex_1()
        .w_full()
        .min_h(px(0.))
        .child(content)
        .children(panel_layer(shell, cx))
        .into_any_element()
}

/// 页面顶部：一行状态说明、一个安静的状态胶囊、两个低调的开关。
fn header_bar(
    state: &'static str,
    detail: &str,
    state_color: u32,
    state_bg: u32,
    settings_open: bool,
    history_open: bool,
    review_count: usize,
    cx: &mut Context<AppShell>,
) -> Div {
    let detail_label = detail.to_owned();
    div()
        .h_flex()
        .flex_none()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .child(div().flex_1())
        .child(
            div()
                .h_flex()
                .flex_none()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .max_w(px(190.))
                        .overflow_hidden()
                        .text_size(px(12.))
                        .text_color(rgb(TEXT_TERTIARY))
                        .child(detail_label),
                )
                .child(status_pill(state, state_color, state_bg))
                .child(reviews_toggle(history_open, review_count, cx))
                .child(
                    Button::new("review-settings-toggle")
                        .compact()
                        .label(if settings_open {
                            "收起设置"
                        } else {
                            "设置"
                        })
                        .on_click(cx.listener(|shell, _, window, cx| {
                            shell.toggle_review_panel(ReviewPanel::Settings, window, cx);
                        })),
                ),
        )
}

/// 历史入口。一个时钟图标加一个词读起来像按钮而不是装饰，条数让人知道
/// 巡检记录正在被保留。`IconName::History` 在这个图标集里不存在。
fn reviews_toggle(open: bool, count: usize, cx: &mut Context<AppShell>) -> Stateful<Div> {
    let label = if count > 0 {
        format!("历史 · {count}")
    } else {
        "历史".to_owned()
    };
    div()
        .id("review-history-toggle")
        .h_flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .px(px(9.))
        .py(px(5.))
        .rounded(px(RADIUS_SM))
        .border_1()
        .border_color(rgb(if open { ACCENT } else { BORDER }))
        .bg(rgb(if open { ACCENT_SOFT } else { SURFACE }))
        .text_size(px(12.5))
        .text_color(rgb(if open { ACCENT } else { TEXT_SECONDARY }))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(if open { ACCENT_SOFT } else { HOVER })))
        .child(div().text_size(px(14.)).child(IconName::Clock))
        .child(label)
        .on_click(cx.listener(|shell, _, window, cx| {
            shell.toggle_review_panel(ReviewPanel::History, window, cx);
        }))
}

/// 唯一常驻的动作：巡检开关与「立即巡检」。时间、权限与 CLI 路径收在
/// 折叠的设置卡里。
fn daily_bar(shell: &AppShell, cx: &mut Context<AppShell>, running: bool) -> Div {
    let enabled = shell.agent_settings.daily_review;
    let time = shell.agent_settings.daily_review_time.trim();
    let time = if time.is_empty() { "08:30" } else { time };

    let mut toggle = Button::new("daily-toggle").compact().label(if enabled {
        "已开启"
    } else {
        "已关闭"
    });
    if enabled {
        toggle = toggle.primary();
    }

    card().px(px(16.)).py(px(11.)).child(
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
                    .child(dot(if enabled { SUCCESS } else { TEXT_TERTIARY }))
                    .child(
                        div()
                            .text_size(px(13.5))
                            .font_weight(FontWeight::MEDIUM)
                            .child("每日巡检"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(TEXT_TERTIARY))
                            .child(format!("每天 {time}")),
                    ),
            )
            .child(
                div()
                    .h_flex()
                    .gap(px(8.))
                    .child(toggle.on_click(cx.listener(|shell, _, _, cx| {
                        shell.toggle_daily_review(cx);
                    })))
                    .child(
                        Button::new("daily-now")
                            .primary()
                            .label("立即巡检")
                            .disabled(running)
                            .on_click(cx.listener(|shell, _, _, cx| {
                                shell.start_daily_review(true, cx);
                            })),
                    ),
            ),
    )
}

fn permission_card(prompt: &crate::agent::PermissionPrompt, cx: &mut Context<AppShell>) -> Div {
    let id = prompt.id;
    let allow_id = prompt
        .options
        .iter()
        .find(|option| option.allow)
        .map(|option| option.id.clone())
        .unwrap_or_default();

    card_pad()
        .gap(px(10.))
        .bg(rgb(ACCENT_TINT))
        .border_color(rgb(ACCENT).opacity(0.32))
        .child(
            div()
                .h_flex()
                .items_center()
                .gap(px(8.))
                .child(pill("需要你确认", ACCENT, 0xffffff))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(prompt.title.clone()),
                ),
        )
        .child(
            div()
                .text_size(px(12.5))
                .text_color(rgb(TEXT_SECONDARY))
                .child(prompt.detail.clone()),
        )
        .child(
            div()
                .h_flex()
                .gap(px(8.))
                .child(
                    Button::new("perm-allow")
                        .primary()
                        .label("允许这一次")
                        .on_click(cx.listener(move |shell, _, _, cx| {
                            shell.grant(id, allow_id.clone(), cx);
                        })),
                )
                .child(Button::new("perm-deny").label("拒绝").on_click(cx.listener(
                    move |shell, _, _, cx| {
                        shell.deny(id, cx);
                    },
                ))),
        )
}

/// 结构化结论，正文之外单独呈现，保持正文可读。只有 Agent 真的给了字段
/// 才出现对应段落。
fn decision_card(report: Option<&DecisionReport>) -> Option<Div> {
    let report = report?;
    let verdict = report.verdict();
    let (label, fg, bg) = match verdict {
        "hold" => ("今天不操作", SUCCESS, SUCCESS_SOFT),
        "adjust" => ("建议调整", WARNING, WARNING_SOFT),
        "wait" => ("等待数据", ACCENT, ACCENT_SOFT),
        _ => ("本轮结论", TEXT_SECONDARY, NEUTRAL_SOFT),
    };

    let mut card = card_pad()
        .gap(px(12.))
        .bg(rgb(bg))
        .border_color(rgb(fg).opacity(0.26))
        .child(
            div()
                .h_flex()
                .items_center()
                .justify_between()
                .gap(px(10.))
                .child(overline("今日结论"))
                .child(pill(label, fg, 0xffffff)),
        )
        .child(
            div()
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(TEXT))
                .child(report.headline()),
        );

    let risks = report.value.get("risks").and_then(|v| v.as_array());
    if let Some(risks) = risks {
        let rows: Vec<Div> = risks
            .iter()
            .take(4)
            .filter_map(|risk| {
                let text = risk.get("text")?.as_str()?.trim();
                if text.is_empty() {
                    return None;
                }
                let level = risk.get("level").and_then(|v| v.as_str()).unwrap_or("low");
                let color = match level {
                    "high" => DANGER,
                    "medium" => WARNING,
                    _ => SUCCESS,
                };
                Some(
                    div()
                        .h_flex()
                        .items_start()
                        .gap(px(8.))
                        .child(dot(color))
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(12.5))
                                .text_color(rgb(TEXT_SECONDARY))
                                .child(text.to_owned()),
                        ),
                )
            })
            .collect();
        if !rows.is_empty() {
            card = card.child(
                div()
                    .v_flex()
                    .gap(px(7.))
                    .child(overline("需要留意"))
                    .children(rows),
            );
        }
    }

    if let Some(actions) = report.value.get("actions").and_then(|v| v.as_array()) {
        let rows: Vec<Div> = actions
            .iter()
            .take(6)
            .filter_map(|action| {
                let code = action.get("code")?.as_str()?;
                let side = action.get("side").and_then(|v| v.as_str()).unwrap_or("buy");
                let amount = action
                    .get("amount_cny")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0);
                let reason = action
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let (side_label, color) = if side == "sell" {
                    ("卖出", SUCCESS)
                } else {
                    ("买入", DANGER)
                };
                Some(
                    div()
                        .h_flex()
                        .items_center()
                        .gap(px(9.))
                        .child(pill(side_label, color, 0xffffff))
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_family(mono_family())
                                .child(format!("{code} ¥{amount:.0}")),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(12.))
                                .text_color(rgb(TEXT_TERTIARY))
                                .child(reason.to_owned()),
                        ),
                )
            })
            .collect();
        if !rows.is_empty() {
            card = card.child(
                div()
                    .v_flex()
                    .gap(px(7.))
                    .pt(px(10.))
                    .border_t_1()
                    .border_color(rgb(fg).opacity(0.18))
                    .child(overline("建议动作"))
                    .children(rows),
            );
        }
    }

    let questions = report.questions();
    if !questions.is_empty() {
        let items: Vec<String> = questions
            .into_iter()
            .map(|text| format!("· {text}"))
            .collect();
        card = card.child(
            div()
                .v_flex()
                .gap(px(4.))
                .child(overline("待确认"))
                .text_size(px(12.))
                .text_color(rgb(TEXT_SECONDARY))
                .children(items.into_iter().map(|text| div().child(text)))
                .child(
                    div()
                        .pt(px(2.))
                        .text_size(px(11.5))
                        .text_color(rgb(TEXT_TERTIARY))
                        .child("这些只有你知道；想展开可以去管家窗口引用这份巡检继续。"),
                ),
        );
    }

    Some(card)
}

/// 一个页面容器：顶部标题、中间唯一可滚动的时间线、底部引导卡。
fn conversation_card(
    shell: &AppShell,
    cx: &mut Context<AppShell>,
    running: bool,
    trace_count: usize,
) -> Div {
    if let Some(view) = &shell.viewing_review {
        return replay_card(view, shell, cx);
    }

    let trace_open = shell.review_panel == Some(ReviewPanel::Trace);
    let trace_label = if trace_open {
        "关闭执行细节".to_owned()
    } else if trace_count > 0 {
        format!("执行细节 · {trace_count}")
    } else {
        "执行细节".to_owned()
    };

    let header = div()
        .h_flex()
        .flex_none()
        .items_center()
        .justify_between()
        .px(px(18.))
        .py(px(11.))
        .border_b_1()
        .border_color(rgb(DIVIDER))
        .child(section_title("巡检时间线"))
        .child(
            div().child(
                Button::new("trace-toggle")
                    .compact()
                    .label(trace_label)
                    .on_click(cx.listener(|shell, _, window, cx| {
                        shell.toggle_review_panel(ReviewPanel::Trace, window, cx);
                    })),
            ),
        );

    let rows: Vec<Div> = shell
        .review
        .timeline
        .iter()
        .enumerate()
        .filter(|(_, entry)| is_message(entry.kind))
        .map(|(index, entry)| message_row(index, entry))
        .collect();

    let mut stream = div()
        .id("review-timeline")
        .v_flex()
        .flex_1()
        .min_h(px(0.))
        .gap(px(18.))
        .overflow_y_scroll()
        .track_scroll(&shell.conversation_scroll)
        .px(px(18.))
        .py(px(18.));

    if rows.is_empty() {
        stream = stream.child(empty_review());
    } else {
        stream = stream.children(rows);
    }

    // 权限请求是用户此刻唯一需要回应的事，放在时间线末尾、引导卡上方。
    if let Some(prompt) = &shell.review.pending {
        stream = stream.child(permission_card(prompt, cx));
    }
    if let Some(error) = &shell.review.error {
        stream = stream.child(notice(
            format!("巡检运行出错：{error}"),
            (DANGER, DANGER_SOFT),
        ));
    }
    if let Some(card) = decision_card(shell.review.decision.as_ref()) {
        stream = stream.child(card);
    }

    div()
        .v_flex()
        .flex_1()
        .min_h(px(0.))
        .bg(rgb(SURFACE))
        .border_1()
        .border_color(rgb(BORDER))
        .rounded(px(RADIUS))
        .shadow(shadow_card())
        .overflow_hidden()
        .child(header)
        .child(stream)
        .child(review_guide_card(shell, running, cx))
}

/// 只读回放：不再还原执行轨迹，只显示报告正文与结论卡片。`body` 之外
/// 的旧字段读不懂就降级，不 panic。
fn replay_card(view: &ViewedReview, shell: &AppShell, cx: &mut Context<AppShell>) -> Div {
    let title = format!("历史回放 · {}", view.report.title);
    let header = div()
        .h_flex()
        .flex_none()
        .items_center()
        .justify_between()
        .px(px(18.))
        .py(px(11.))
        .border_b_1()
        .border_color(rgb(DIVIDER))
        .child(
            div()
                .h_flex()
                .items_baseline()
                .gap(px(9.))
                .child(section_title("巡检记录"))
                .child(
                    div()
                        .max_w(px(300.))
                        .overflow_hidden()
                        .text_size(px(12.))
                        .text_color(rgb(TEXT_TERTIARY))
                        .child(title),
                ),
        )
        .child(
            div()
                .h_flex()
                .items_center()
                .gap(px(8.))
                .child(pill("只读", ACCENT, ACCENT_SOFT))
                .child(
                    Button::new("review-back-inline")
                        .compact()
                        .label("返回当前巡检")
                        .on_click(cx.listener(|shell, _, _, cx| shell.exit_review_view(cx))),
                ),
        );

    let mut stream = div()
        .id("review-replay")
        .v_flex()
        .flex_1()
        .min_h(px(0.))
        .gap(px(16.))
        .overflow_y_scroll()
        .track_scroll(&shell.conversation_scroll)
        .px(px(18.))
        .py(px(18.))
        .child(review_meta_line(view));

    if view.report.body.trim().is_empty() {
        stream = stream.child(notice(
            "这份报告没有可显示的正文（可能是旧版本或中途失败）。",
            (TEXT_TERTIARY, NEUTRAL_SOFT),
        ));
    } else {
        stream = stream.child(div().text_size(px(13.5)).text_color(rgb(TEXT)).child(
            TextView::markdown(
                ElementId::named_usize("review-replay-md", 0),
                view.report.body.clone(),
            ),
        ));
    }
    if let Some(card) = decision_card(view.decision.as_ref()) {
        stream = stream.child(card);
    }

    div()
        .v_flex()
        .flex_1()
        .min_h(px(0.))
        .bg(rgb(SURFACE))
        .border_1()
        .border_color(rgb(BORDER))
        .rounded(px(RADIUS))
        .shadow(shadow_card())
        .overflow_hidden()
        .child(header)
        .child(stream)
        .child(replay_footer(view, cx))
}

/// 日期 / 来源 / 状态一行。旧报告里的状态串认不出来时原样显示。
fn review_meta_line(view: &ViewedReview) -> Div {
    let report = &view.report;
    let date = if report.date.trim().is_empty() {
        "日期未知".to_owned()
    } else {
        report.date.clone()
    };
    let source = if report.source.trim().is_empty() {
        "巡检".to_owned()
    } else {
        report.source.clone()
    };
    div()
        .h_flex()
        .items_center()
        .gap(px(8.))
        .child(dot(ACCENT))
        .child(
            div()
                .text_size(px(12.5))
                .text_color(rgb(TEXT_SECONDARY))
                .child(format!(
                    "{date} · {source} · {}",
                    review_state_label(&report.state)
                )),
        )
}

fn review_state_label(state: &str) -> String {
    match state {
        "finished" => "已完成".to_owned(),
        "failed" => "失败".to_owned(),
        "cancelled" => "已取消".to_owned(),
        "running" => "进行中".to_owned(),
        other if other.trim().is_empty() => "状态未知".to_owned(),
        other => other.to_owned(),
    }
}

fn empty_review() -> Stateful<Div> {
    div()
        .id("review-empty")
        .v_flex()
        .flex_1()
        .min_h(px(240.))
        .w_full()
        .items_center()
        .justify_center()
        .gap(px(11.))
        .px(px(24.))
        .child(
            div()
                .h_flex()
                .items_center()
                .justify_center()
                .w(px(42.))
                .h(px(42.))
                .rounded(px(21.))
                .bg(rgb(ACCENT_SOFT))
                .child(div().text_size(px(17.)).text_color(rgb(ACCENT)).child("✦")),
        )
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::MEDIUM)
                .child("还没有巡检记录"),
        )
        .child(
            div()
                .text_size(px(12.5))
                .text_color(rgb(TEXT_TERTIARY))
                .child("点上方「立即巡检」，开启后到点也会自动巡检。"),
        )
}

/// 用户发言、巡检结论、错误与权限记录是时间线。其余都是机器细节，
/// 进「执行细节」抽屉。
fn is_message(kind: EntryKind) -> bool {
    matches!(
        kind,
        EntryKind::User | EntryKind::Message | EntryKind::Error | EntryKind::Permission
    )
}

fn message_row(index: usize, entry: &TimelineEntry) -> Div {
    match entry.kind {
        EntryKind::User => div().w_full().flex_none().h_flex().justify_end().child(
            div()
                .v_flex()
                .max_w(px(560.))
                .rounded(px(14.))
                .bg(rgb(ACCENT_SOFT))
                .px(px(14.))
                .py(px(10.))
                .text_size(px(13.5))
                .line_height(px(21.))
                .text_color(rgb(TEXT))
                .children(
                    entry
                        .text
                        .lines()
                        .map(|line| div().child(line.to_owned()))
                        .collect::<Vec<_>>(),
                ),
        ),
        EntryKind::Message => div()
            .w_full()
            .flex_none()
            .h_flex()
            .items_start()
            .gap(px(11.))
            .child(review_avatar())
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .pt(px(2.))
                    .text_size(px(13.5))
                    .text_color(rgb(TEXT))
                    .child(TextView::markdown(
                        ElementId::named_usize("review-md", index),
                        strip_decision_block(&entry.text),
                    )),
            ),
        EntryKind::Error => div()
            .w_full()
            .flex_none()
            .child(notice(entry.text.clone(), (DANGER, DANGER_SOFT))),
        EntryKind::Permission => div()
            .w_full()
            .flex_none()
            .h_flex()
            .gap(px(8.))
            .child(dot(ACCENT))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(TEXT_TERTIARY))
                    .child(entry.text.clone()),
            ),
        _ => div(),
    }
}

fn review_avatar() -> Div {
    div()
        .h_flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(26.))
        .h(px(26.))
        .rounded(px(13.))
        .bg(rgb(ACCENT))
        .child(
            div()
                .text_size(px(11.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(TEXT_ON_ACCENT))
                .child("M"),
        )
}

fn trace_row(entry: &TimelineEntry) -> Div {
    div()
        .h_flex()
        .flex_none()
        .items_start()
        .gap(px(8.))
        .text_size(px(11.5))
        .font_family(mono_family())
        .text_color(rgb(TEXT_TERTIARY))
        .child(div().w(px(58.)).flex_none().child(trace_label(entry.kind)))
        .child(div().flex_1().min_w(px(0.)).child(entry.text.clone()))
}

fn trace_label(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::System => "系统",
        EntryKind::Thought => "思考",
        EntryKind::Plan => "计划",
        EntryKind::ToolCall => "调用",
        EntryKind::ToolResult => "结果",
        _ => "记录",
    }
}

/// 右侧抽屉浮在页面上方，不进入页面流：打开执行细节不能多出一条滚动条，
/// 也不能改变时间线尺寸或盖住底部引导卡。
fn panel_layer(shell: &AppShell, cx: &mut Context<AppShell>) -> Option<Div> {
    let panel = shell.review_panel?;
    let (title, hint) = match panel {
        ReviewPanel::Trace => ("执行细节".to_owned(), String::new()),
        ReviewPanel::Settings => ("高级设置".to_owned(), String::new()),
        ReviewPanel::History => (
            "巡检历史".to_owned(),
            if shell.reviews.is_empty() {
                String::new()
            } else {
                format!("共 {} 份", shell.reviews.len())
            },
        ),
    };
    let body = match panel {
        ReviewPanel::Trace => trace_body(shell),
        ReviewPanel::Settings => settings_body(shell, cx),
        ReviewPanel::History => reviews_body(shell, cx),
    };

    Some(
        div()
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .right(px(0.))
            .bottom(px(0.))
            .child(
                div()
                    .id("review-panel-backdrop")
                    .absolute()
                    .top(px(0.))
                    .left(px(0.))
                    .right(px(0.))
                    .bottom(px(0.))
                    .occlude()
                    .bg(rgb(0x0b1526).opacity(0.22))
                    .on_click(cx.listener(|shell, _, window, cx| {
                        shell.close_review_panel(window, cx);
                    })),
            )
            .child(
                div()
                    .id("review-panel")
                    .absolute()
                    .top(px(0.))
                    .right(px(0.))
                    .bottom(px(0.))
                    .w(px(420.))
                    .v_flex()
                    .occlude()
                    .track_focus(&shell.review_panel_focus)
                    .key_context("review-panel")
                    .on_action(cx.listener(|shell, _: &CloseReviewPanel, window, cx| {
                        shell.close_review_panel(window, cx);
                    }))
                    .bg(rgb(SURFACE))
                    .border_l_1()
                    .border_color(rgb(BORDER))
                    .shadow(shadow_overlay())
                    .child(panel_header(title, hint, cx))
                    .child(
                        div()
                            .id("review-panel-body")
                            .v_flex()
                            .flex_1()
                            .min_h(px(0.))
                            .overflow_y_scroll()
                            .px(px(18.))
                            .py(px(16.))
                            .child(body),
                    ),
            ),
    )
}

fn panel_header(
    title: impl Into<SharedString>,
    hint: impl Into<SharedString>,
    cx: &mut Context<AppShell>,
) -> Div {
    let title: SharedString = title.into();
    let hint: SharedString = hint.into();
    let mut label = div().v_flex().gap(px(2.)).child(
        div()
            .text_size(px(15.))
            .font_weight(FontWeight::SEMIBOLD)
            .child(title),
    );
    if !hint.is_empty() {
        label = label.child(
            div()
                .text_size(px(11.5))
                .text_color(rgb(TEXT_TERTIARY))
                .child(hint),
        );
    }
    div()
        .h_flex()
        .flex_none()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .px(px(18.))
        .py(px(14.))
        .border_b_1()
        .border_color(rgb(DIVIDER))
        .child(label)
        .child(
            Button::new("review-panel-close")
                .compact()
                .label("关闭")
                .on_click(cx.listener(|shell, _, window, cx| {
                    shell.close_review_panel(window, cx);
                })),
        )
}

fn trace_body(shell: &AppShell) -> Div {
    let rows: Vec<Div> = shell
        .review
        .timeline
        .iter()
        .filter(|entry| !is_message(entry.kind))
        .map(trace_row)
        .collect();
    if rows.is_empty() {
        return div()
            .v_flex()
            .gap(px(8.))
            .py(px(28.))
            .child(
                div()
                    .text_size(px(13.5))
                    .font_weight(FontWeight::MEDIUM)
                    .child("还没有执行记录"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(TEXT_TERTIARY))
                    .child("巡检后显示工具调用与系统消息。"),
            );
    }
    div().v_flex().gap(px(2.)).children(rows)
}

// ── 巡检历史抽屉 ─────────────────────────────────────────────────────

/// 最新在前，一行一份报告。点击回放；垃圾桶图标需要二次确认才删除文件。
fn reviews_body(shell: &AppShell, cx: &mut Context<AppShell>) -> Div {
    let mut body = div().v_flex().gap(px(8.));
    if let Some(error) = &shell.reviews_error {
        body = body.child(notice(error.clone(), (DANGER, DANGER_SOFT)));
    }
    if !shell.reviews_loaded {
        return body.child(
            div()
                .text_size(px(12.5))
                .text_color(rgb(TEXT_TERTIARY))
                .child("正在读取巡检记录…"),
        );
    }
    if shell.reviews.is_empty() {
        return body.child(empty_state(
            IconName::Clock,
            "还没有巡检记录",
            "点上方「立即巡检」，开启后到点也会自动巡检。",
        ));
    }
    let rows: Vec<Div> = shell
        .reviews
        .iter()
        .map(|item| review_row(item, shell, cx))
        .collect();
    body.children(rows)
}

fn review_row(item: &ReviewSummary, shell: &AppShell, cx: &mut Context<AppShell>) -> Div {
    let id = item.id.clone();
    let active = shell
        .viewing_review
        .as_ref()
        .is_some_and(|view| view.report.id == id);
    let pending = shell.reviews_pending_delete.as_deref() == Some(id.as_str());

    let open_id = id.clone();
    let row = div()
        .v_flex()
        .flex_none()
        .gap(px(7.))
        .px(px(11.))
        .py(px(10.))
        .rounded(px(RADIUS_SM))
        .border_1()
        .border_color(rgb(if active { ACCENT } else { BORDER }))
        .bg(rgb(if active { ACCENT_SOFT } else { SURFACE }))
        .child(
            div()
                .h_flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .id(SharedString::from(format!("review-open-{id}")))
                        .flex_1()
                        .min_w(px(0.))
                        .v_flex()
                        .gap(px(3.))
                        .cursor_pointer()
                        .child(
                            div()
                                .w_full()
                                .overflow_hidden()
                                .text_size(px(13.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(TEXT))
                                .child(item.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(TEXT_TERTIARY))
                                .child(format!(
                                    "{} · {} · {}",
                                    item.updated_at,
                                    item.source,
                                    review_state_label(&item.state)
                                )),
                        )
                        .on_click(cx.listener(move |shell, _, _, cx| {
                            shell.open_review(open_id.clone(), cx);
                        })),
                )
                // 删除按钮是标题的兄弟节点：点垃圾桶不能顺带打开报告。
                .child(review_delete_button(id.clone(), cx)),
        )
        .child(
            div()
                .w_full()
                .overflow_hidden()
                .text_size(px(12.5))
                .text_color(rgb(TEXT_SECONDARY))
                .child(item.preview.clone()),
        );

    if pending {
        row.child(review_delete_confirm(id, cx))
    } else {
        row
    }
}

fn review_delete_button(id: String, cx: &mut Context<AppShell>) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("review-delete-{id}")))
        .flex_none()
        .w(px(28.))
        .h(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS_SM))
        .border_1()
        .border_color(rgb(BORDER))
        .text_size(px(14.))
        .text_color(rgb(TEXT_TERTIARY))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(DANGER_SOFT)))
        .child(IconName::Trash)
        .on_click(cx.listener(move |shell, _, _, cx| {
            shell.request_delete_review(id.clone(), cx);
        }))
}

fn review_delete_confirm(id: String, cx: &mut Context<AppShell>) -> Div {
    div()
        .h_flex()
        .items_center()
        .justify_between()
        .gap(px(8.))
        .pt(px(9.))
        .border_t_1()
        .border_color(rgb(DIVIDER))
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgb(DANGER))
                .child("确定删除这份巡检报告？"),
        )
        .child(
            div()
                .h_flex()
                .gap(px(6.))
                .child(
                    Button::new("review-delete-cancel")
                        .compact()
                        .label("取消")
                        .on_click(cx.listener(|shell, _, _, cx| {
                            shell.cancel_delete_review(cx);
                        })),
                )
                .child(
                    Button::new("review-delete-confirm")
                        .compact()
                        .label("删除")
                        .on_click(cx.listener(move |shell, _, _, cx| {
                            shell.confirm_delete_review(id.clone(), cx);
                        })),
                ),
        )
}

/// 原输入框的位置改成管家导流：巡检只按纪律出结论，自由对话与追问在
/// 管家窗口，那里能看到账本和记忆。
///
/// 这是接力棒而不是一个空窗口：按钮带着当前报告 id（以及结论里的第一条
/// 待确认）去管家，窗口打开即挂上这份报告、预填那句话。有 `questions` 时
/// 文案直接点名「N 件只有你知道的事」，没有时保留通用入口，引用照带。
fn review_guide_card(shell: &AppShell, running: bool, cx: &mut Context<AppShell>) -> Div {
    let stop = running.then(|| {
        Button::new("review-stop")
            .compact()
            .label("停止")
            .on_click(cx.listener(|shell, _, _, cx| shell.stop_review(cx)))
    });

    let report_id = shell
        .current_report
        .as_ref()
        .map(|meta| meta.report.id.clone())
        .filter(|id| !id.trim().is_empty());
    let questions = shell
        .review
        .decision
        .as_ref()
        .map(DecisionReport::questions)
        .unwrap_or_default();
    let first_question = questions.first().cloned();
    let (title, subtitle) = if running {
        (
            "巡检还在跑".to_owned(),
            "巡检完成后可带这份结论去管家继续追问；运行中的报告只有开场白。".to_owned(),
        )
    } else if questions.is_empty() {
        (
            "想看持仓、计划或随便聊聊？".to_owned(),
            "巡检按纪律出结论；追问与自由对话在管家窗口，能结合账本与记忆。".to_owned(),
        )
    } else {
        (
            format!("有 {} 件只有你知道的事", questions.len()),
            "带上这份巡检再回答，管家能结合账本与记忆把结论说完整。".to_owned(),
        )
    };
    let label = if questions.is_empty() {
        "去管家聊聊"
    } else {
        "带这份巡检问管家"
    };
    // 运行中的报告不能被引用：正文只有一句开工旁白，模型会拿它当依据，
    // 然后被迫解释「报告为什么坏了」。跑完再给接力入口。
    let relay = (!running).then(|| {
        Button::new("review-open-butler")
            .primary()
            .label(label)
            .on_click(cx.listener(move |shell, _, _, cx| {
                let Some(id) = report_id.clone() else {
                    // 还没有报告（首启、未跑过巡检）：普通打开。
                    let _ = crate::ipc::poke("mns-gui", &crate::ipc::Signal::OpenButler);
                    return;
                };
                let signal = crate::ipc::Signal::ButlerRef {
                    id,
                    question: first_question.clone(),
                };
                if let Err(error) = crate::ipc::poke("mns-gui", &signal) {
                    shell.show_toast(format!("无法打开管家窗口：{error}"), ToastTone::Error, cx);
                }
            }))
    });

    div()
        .v_flex()
        .flex_none()
        .gap(px(10.))
        .px(px(18.))
        .py(px(14.))
        .border_t_1()
        .border_color(rgb(DIVIDER))
        .bg(rgb(LAYER))
        .child(
            div()
                .h_flex()
                .items_center()
                .justify_between()
                .gap(px(14.))
                .child(
                    div()
                        .v_flex()
                        .flex_1()
                        .min_w(px(0.))
                        .gap(px(3.))
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
                                .child(subtitle),
                        ),
                )
                .child(
                    div()
                        .h_flex()
                        .flex_none()
                        .items_center()
                        .gap(px(8.))
                        .children(stop)
                        .children(relay),
                ),
        )
}

/// 回放时输入框本就不存在，这里说明「在看什么」，并允许把这份旧报告带去
/// 管家继续问（同样只递 id 与第一条待确认），最后才是返回入口。
///
/// 只有跑完的报告才有接力入口：历史里可能留着被中断的运行中报告，正文只有
/// 一句开工旁白，带去管家只会让模型对着半截文本解释「报告为什么坏了」。
fn replay_footer(view: &ViewedReview, cx: &mut Context<AppShell>) -> Div {
    let report_id = view.report.id.clone();
    let question = view
        .decision
        .as_ref()
        .and_then(|decision| decision.questions().into_iter().next());
    let relay = (view.report.is_quotable() && !report_id.trim().is_empty()).then(|| {
        let id = report_id.clone();
        Button::new("review-replay-butler")
            .compact()
            .label("带这份巡检问管家")
            .on_click(cx.listener(move |shell, _, _, cx| {
                let signal = crate::ipc::Signal::ButlerRef {
                    id: id.clone(),
                    question: question.clone(),
                };
                if let Err(error) = crate::ipc::poke("mns-gui", &signal) {
                    shell.show_toast(format!("无法打开管家窗口：{error}"), ToastTone::Error, cx);
                }
            }))
    });

    div()
        .v_flex()
        .flex_none()
        .gap(px(10.))
        .px(px(18.))
        .py(px(13.))
        .border_t_1()
        .border_color(rgb(DIVIDER))
        .bg(rgb(LAYER))
        .child(
            div()
                .h_flex()
                .items_center()
                .justify_between()
                .gap(px(14.))
                .child(
                    div()
                        .v_flex()
                        .flex_1()
                        .min_w(px(0.))
                        .gap(px(3.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(format!("正在查看：{}", view.report.title)),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(TEXT_TERTIARY))
                                .child(
                                    "历史报告只读；可以带着它去管家继续问，新的巡检会覆盖当前时间线。",
                                ),
                        ),
                )
                .child(
                    div()
                        .h_flex()
                        .flex_none()
                        .items_center()
                        .gap(px(8.))
                        .children(relay)
                        .child(
                            Button::new("review-back")
                                .primary()
                                .label("返回当前巡检")
                                .on_click(cx.listener(|shell, _, _, cx| {
                                    shell.exit_review_view(cx)
                                })),
                        ),
                ),
        )
}

/// 高级设置收在抽屉里：巡检时间、权限策略与外部 CLI 都是设一次的东西，
/// 不该和结论抢版面。
fn settings_body(shell: &AppShell, cx: &mut Context<AppShell>) -> Div {
    div()
        .v_flex()
        .gap(px(14.))
        .child(card_header("巡检时间", ""))
        .child(
            div()
                .h_flex()
                .items_center()
                .gap(px(8.))
                .child(div().w(px(90.)).child(Input::new(&shell.daily_time)))
                .child(
                    Button::new("daily-save")
                        .compact()
                        .label("保存时间")
                        .on_click(cx.listener(|shell, _, _, cx| shell.save_daily_time(cx))),
                ),
        )
        .child(divider())
        .child(card_header("权限策略", ""))
        .child(
            div().h_flex().gap(px(6.)).children(
                PermissionPolicy::ALL
                    .into_iter()
                    .map(|policy| policy_chip(policy, shell, cx)),
            ),
        )
        .child(
            div()
                .text_size(px(11.5))
                .text_color(rgb(TEXT_TERTIARY))
                .child(
                    "仅作用于非 opencode 的 ACP agent。使用默认 opencode 时，任务与管家都按 \
                     Agent 默认开放工具，MNS 只额外拦会改账本的 mns 子命令和 init。",
                ),
        )
        .child(divider())
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
                        Button::new("review-save")
                            .primary()
                            .label("保存")
                            .on_click(cx.listener(|shell, _, _, cx| {
                                shell.save_agent_settings(cx);
                            })),
                    ),
                ),
        )
        .child(
            div()
                .text_size(px(11.5))
                .text_color(rgb(TEXT_TERTIARY))
                .child("无需填写 URL 或 API Key。"),
        )
}

fn policy_chip(policy: PermissionPolicy, shell: &AppShell, cx: &mut Context<AppShell>) -> Button {
    let active = shell.agent_settings.permission_policy == policy;
    let mut button = Button::new(SharedString::from(format!("policy-{}", policy.label())))
        .compact()
        .label(policy.label());
    if active {
        button = button.primary();
    }
    button.on_click(cx.listener(move |shell, _, _, cx| shell.set_policy(policy, cx)))
}

#[cfg(test)]
mod tests {
    use super::is_message;
    use crate::agent::EntryKind;
    use crate::butler::review::strip_decision_block;

    #[test]
    fn decision_fence_is_removed_from_visible_prose() {
        let text = "今天不操作。\n\n```mns-decision\n{\"verdict\":\"hold\"}\n```";
        assert_eq!(strip_decision_block(text), "今天不操作。");
    }

    #[test]
    fn text_without_a_fence_is_untouched() {
        let text = "## 结论\n\n- 现金充足\n";
        assert_eq!(strip_decision_block(text), "## 结论\n\n- 现金充足");
    }

    #[test]
    fn a_half_written_fence_does_not_leak_json() {
        let text = "正在写：\n```mns-decision\n{\"verdict\":";
        assert_eq!(strip_decision_block(text), "正在写：");
    }

    #[test]
    fn tool_rows_are_not_part_of_the_conversation() {
        assert!(is_message(EntryKind::User));
        assert!(is_message(EntryKind::Message));
        assert!(!is_message(EntryKind::ToolCall));
        assert!(!is_message(EntryKind::System));
    }
}
