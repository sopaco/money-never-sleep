//! 助手：界面上只有一件事——对话。
//!
//! 首次由 MNS 取数并组装提示词，Agent 判断后回传结构化结论；本轮结束后
//! 输入框仍是同一个输入框，发送变成追问，复用同一个 ACP session。设置、
//! 定时、权限策略与工具轨迹都收进侧边抽屉：整页只有对话区一个滚动容器，
//! 输入框永远固定在底部，不会被展开的内容顶走。

use gpui_kit::base::StyledExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::text::TextView;
use gpui_kit::*;

use crate::agent::{EntryKind, PermissionPolicy, RunState, TimelineEntry};
use crate::gui::components::*;
use crate::gui::theme::*;
use crate::gui::{AgentPanel, AppShell, CloseAgentPanel, Page};

use super::{card_header, notice, page_lead};

/// The assistant page is a fixed-height workspace, not a scrolling document:
/// chrome on top, one scrollable conversation, composer pinned at the bottom.
/// `page_root()` deliberately is not used here — its page-level scrollbar used
/// to sit outside the chat's own scrollbar, so a wheel gesture could move
/// either one depending on where the pointer happened to be.
pub fn render(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let agent = &shell.agent;
    let running = agent.is_running();
    let (state_color, state_bg) = match agent.state {
        RunState::Failed | RunState::Cancelled => (DANGER, DANGER_SOFT),
        RunState::Finished => (SUCCESS, SUCCESS_SOFT),
        RunState::Idle => (TEXT_TERTIARY, NEUTRAL_SOFT),
        _ => (ACCENT, ACCENT_SOFT),
    };
    let trace_count = agent
        .timeline
        .iter()
        .filter(|entry| !is_message(entry.kind))
        .count();
    let settings_open = shell.agent_panel == Some(AgentPanel::Settings);

    let content = div()
        .v_flex()
        .flex_1()
        .min_h(px(0.))
        .px(px(22.))
        .pt(px(20.))
        .pb(px(20.))
        .gap(px(12.))
        .child(header_bar(
            agent.state.label(),
            &agent.detail,
            state_color,
            state_bg,
            settings_open,
            cx,
        ))
        .child(daily_bar(shell, cx, running))
        .child(conversation_card(shell, cx, running, trace_count));

    div()
        .id("page-agent")
        .v_flex()
        .relative()
        .flex_1()
        .w_full()
        .min_h(px(0.))
        .child(content)
        .children(panel_layer(shell, cx))
        .into_any_element()
}

/// Page chrome: one reading line, a quiet status pill, and two low-key
/// toggles. The window header already carries the page name.
fn header_bar(
    state: &'static str,
    detail: &str,
    state_color: u32,
    state_bg: u32,
    settings_open: bool,
    cx: &mut Context<AppShell>,
) -> Div {
    let detail_label = detail.to_owned();
    div()
        .h_flex()
        .flex_none()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .child(page_lead(Page::Agent.subtitle()))
        .child(
            div()
                .h_flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(TEXT_TERTIARY))
                        .child(detail_label),
                )
                .child(status_pill(state, state_color, state_bg))
                .child(
                    Button::new("agent-settings-toggle")
                        .compact()
                        .label(if settings_open {
                            "收起设置"
                        } else {
                            "设置"
                        })
                        .on_click(cx.listener(|shell, _, window, cx| {
                            shell.toggle_agent_panel(AgentPanel::Settings, window, cx);
                        })),
                ),
        )
}

/// The one always-visible action: a scheduled review, plus a manual run.
/// Time, permissions and CLI path live in the collapsed settings card.
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
                            .child(format!("每天 {time} 自动取数并判断")),
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

/// The structured conclusion, kept out of the prose so the answer stays
/// readable. Sections only appear when the agent actually supplied them.
fn decision_card(shell: &AppShell) -> Option<Div> {
    let report = shell.agent.decision.as_ref()?;
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

    if let Some(questions) = report.value.get("questions").and_then(|v| v.as_array()) {
        let items: Vec<String> = questions
            .iter()
            .filter_map(|q| q.as_str().map(str::trim).filter(|s| !s.is_empty()))
            .map(|s| format!("· {s}"))
            .collect();
        if !items.is_empty() {
            card = card.child(
                div()
                    .v_flex()
                    .gap(px(4.))
                    .child(overline("待确认"))
                    .text_size(px(12.))
                    .text_color(rgb(TEXT_SECONDARY))
                    .children(items.into_iter().map(|text| div().child(text))),
            );
        }
    }

    Some(card)
}

/// One surface for the whole exchange: messages, optional trace, composer.
/// Keeping the composer inside the card makes it read as a conversation
/// rather than a stack of unrelated forms.
///
/// This card owns the page's ONLY scrollbar. The composer is a sibling of the
/// scroll area, not a child, so expanding anything can never cover it. The
/// structured conclusion, permission prompt and errors live in the same
/// stream as the messages instead of forming a second scroll region above it.
fn conversation_card(
    shell: &AppShell,
    cx: &mut Context<AppShell>,
    running: bool,
    trace_count: usize,
) -> Div {
    let trace_open = shell.agent_panel == Some(AgentPanel::Trace);
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
        .child(
            div()
                .h_flex()
                .items_baseline()
                .gap(px(9.))
                .child(section_title("对话"))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(TEXT_TERTIARY))
                        .child("结论在这里，执行细节在右侧抽屉"),
                ),
        )
        .child(
            Button::new("trace-toggle")
                .compact()
                .label(trace_label)
                .on_click(cx.listener(|shell, _, window, cx| {
                    shell.toggle_agent_panel(AgentPanel::Trace, window, cx);
                })),
        );

    let rows: Vec<Div> = shell
        .agent
        .timeline
        .iter()
        .enumerate()
        .filter(|(_, entry)| is_message(entry.kind))
        .map(|(index, entry)| message_row(index, entry))
        .collect();

    let mut stream = div()
        .id("agent-conversation")
        .v_flex()
        .flex_1()
        .min_h(px(0.))
        .gap(px(18.))
        .overflow_y_scroll()
        .track_scroll(&shell.conversation_scroll)
        .px(px(18.))
        .py(px(18.));

    if rows.is_empty() {
        stream = stream.child(empty_conversation());
    } else {
        stream = stream.children(rows);
    }

    // The permission prompt is the newest thing the user has to act on, so it
    // belongs at the tail, right above the composer.
    if let Some(prompt) = &shell.agent.pending {
        stream = stream.child(permission_card(prompt, cx));
    }
    if let Some(error) = &shell.agent.error {
        stream = stream.child(notice(
            format!("Agent 运行出错：{error}"),
            (DANGER, DANGER_SOFT),
        ));
    }
    if let Some(card) = decision_card(shell) {
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
        .child(composer(shell, cx, running))
}

fn empty_conversation() -> Stateful<Div> {
    div()
        .id("agent-conversation-empty")
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
                .child("还没有开始对话"),
        )
        .child(
            div()
                .text_size(px(12.5))
                .text_color(rgb(TEXT_TERTIARY))
                .child("点上方「立即巡检」，或在下面写一句话开始。"),
        )
}

/// User turns, assistant answers, errors and permission notes are the
/// conversation. Everything else is machinery and goes to the trace strip.
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
            .child(assistant_avatar())
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .pt(px(2.))
                    .text_size(px(13.5))
                    .text_color(rgb(TEXT))
                    .child(TextView::markdown(
                        ElementId::named_usize("agent-md", index),
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

fn assistant_avatar() -> Div {
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

/// A right-hand drawer over the page, never in its flow. That is the whole
/// point: opening the trace must not add a second scrollbar, resize the chat
/// or cover the composer. Swift-side state lives in `AppShell::agent_panel`.
fn panel_layer(shell: &AppShell, cx: &mut Context<AppShell>) -> Option<Div> {
    let panel = shell.agent_panel?;
    let (title, hint) = match panel {
        AgentPanel::Trace => ("执行细节", "工具调用、思考与系统消息，按时间顺序排列"),
        AgentPanel::Settings => ("高级设置", "巡检时间、权限策略与外部 Agent"),
    };
    let body = match panel {
        AgentPanel::Trace => trace_body(shell),
        AgentPanel::Settings => settings_body(shell, cx),
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
                    .id("agent-panel-backdrop")
                    .absolute()
                    .top(px(0.))
                    .left(px(0.))
                    .right(px(0.))
                    .bottom(px(0.))
                    .occlude()
                    .bg(rgb(0x0b1526).opacity(0.22))
                    .on_click(cx.listener(|shell, _, window, cx| {
                        shell.close_agent_panel(window, cx);
                    })),
            )
            .child(
                div()
                    .id("agent-panel")
                    .absolute()
                    .top(px(0.))
                    .right(px(0.))
                    .bottom(px(0.))
                    .w(px(420.))
                    .v_flex()
                    .occlude()
                    .track_focus(&shell.agent_panel_focus)
                    .key_context("agent-panel")
                    .on_action(cx.listener(|shell, _: &CloseAgentPanel, window, cx| {
                        shell.close_agent_panel(window, cx);
                    }))
                    .bg(rgb(SURFACE))
                    .border_l_1()
                    .border_color(rgb(BORDER))
                    .shadow(shadow_overlay())
                    .child(panel_header(title, hint, cx))
                    .child(
                        div()
                            .id("agent-panel-body")
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

fn panel_header(title: &'static str, hint: &'static str, cx: &mut Context<AppShell>) -> Div {
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
        .child(
            div()
                .v_flex()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(TEXT_TERTIARY))
                        .child(hint),
                ),
        )
        .child(
            Button::new("agent-panel-close")
                .compact()
                .label("关闭")
                .on_click(cx.listener(|shell, _, window, cx| {
                    shell.close_agent_panel(window, cx);
                })),
        )
}

fn trace_body(shell: &AppShell) -> Div {
    let rows: Vec<Div> = shell
        .agent
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
                    .child("开始一次巡检或提问后，工具调用、思考与系统消息会出现在这里。"),
            );
    }
    div()
        .v_flex()
        .gap(px(2.))
        .child(
            div()
                .pb(px(10.))
                .text_size(px(11.5))
                .text_color(rgb(TEXT_TERTIARY))
                .child("仅用于核对，不影响对话。点击遮罩、关闭或按 Esc 收起。"),
        )
        .children(rows)
}

/// One composer for the whole life of the session. Before the first run it
/// binds to `goal` (start), afterwards to `followup` (ask); the user never
/// sees two input boxes or a "start vs. ask" decision.
fn composer(shell: &AppShell, cx: &mut Context<AppShell>, running: bool) -> Div {
    let open = shell.agent.open;
    let input = if open {
        Textarea::new(&shell.followup)
    } else {
        Textarea::new(&shell.goal)
    };

    let action = if running {
        Button::new("agent-stop")
            .label("停止")
            .on_click(cx.listener(|shell, _, _, cx| shell.stop_agent(cx)))
    } else if open {
        Button::new("agent-ask")
            .primary()
            .label("发送")
            .on_click(cx.listener(|shell, _, window, cx| shell.ask_agent(window, cx)))
    } else {
        Button::new("agent-start")
            .primary()
            .label("发送")
            .on_click(cx.listener(|shell, _, _, cx| shell.start_agent(cx)))
    };

    let hint = if running {
        "助手正在工作，完成后可以继续追问"
    } else if open {
        "接着问就行，助手会沿用刚才的数据与结论"
    } else {
        "写一句话开始；发送后会开启一个可以持续追问的会话"
    };

    div()
        .v_flex()
        .flex_none()
        .gap(px(8.))
        .px(px(14.))
        .py(px(13.))
        .border_t_1()
        .border_color(rgb(DIVIDER))
        .bg(rgb(SURFACE))
        .child(
            div()
                .h_flex()
                .items_end()
                .gap(px(9.))
                .child(div().flex_1().min_w(px(0.)).child(input))
                .child(action),
        )
        .child(
            div()
                .text_size(px(11.5))
                .text_color(rgb(TEXT_TERTIARY))
                .child(hint),
        )
}

/// Advanced setup lives in the drawer: schedule, permissions and the external
/// CLI are things a person sets once, not a card that competes with the chat.
fn settings_body(shell: &AppShell, cx: &mut Context<AppShell>) -> Div {
    div()
        .v_flex()
        .gap(px(14.))
        .child(card_header("巡检时间", "每天到点自动运行一次"))
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
        .child(card_header(
            "权限策略",
            "默认「保护写入」，敏感操作会先问你",
        ))
        .child(
            div().h_flex().gap(px(6.)).children(
                PermissionPolicy::ALL
                    .into_iter()
                    .map(|policy| policy_chip(policy, shell, cx)),
            ),
        )
        .child(divider())
        .child(card_header(
            "外部 Agent",
            "只填 CLI 路径；模型与凭据留在它自己的配置里",
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
                        Button::new("agent-save")
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
                .child("MNS 会自动追加 acp 子命令，不需要填 URL 或 API Key。"),
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

/// The ```mns-decision fence is a machine channel: the shell already renders
/// it as a card. Leaving the raw JSON in the prose would show the reader a
/// block of syntax they never asked for.
fn strip_decision_block(text: &str) -> String {
    const FENCE: &str = "```mns-decision";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let Some(start) = rest.find(FENCE) else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let after = &rest[start + FENCE.len()..];
        match after.find("```") {
            Some(end) => {
                rest = after[end + 3..].trim_start_matches(['\r', '\n']);
            }
            // Streaming text can end mid-fence. Drop the incomplete tail.
            None => {
                rest = "";
            }
        }
    }
    let mut cleaned = out.trim_end().to_owned();
    cleaned.push('\n');
    cleaned
}

fn mono_family() -> &'static str {
    if cfg!(target_os = "windows") {
        "Consolas"
    } else if cfg!(target_os = "macos") {
        "Menlo"
    } else {
        "DejaVu Sans Mono"
    }
}

#[cfg(test)]
mod tests {
    use super::{is_message, strip_decision_block};
    use crate::agent::EntryKind;

    #[test]
    fn decision_fence_is_removed_from_visible_prose() {
        let text = "今天不操作。\n\n```mns-decision\n{\"verdict\":\"hold\"}\n```";
        assert_eq!(strip_decision_block(text), "今天不操作。\n");
    }

    #[test]
    fn text_without_a_fence_is_untouched() {
        let text = "## 结论\n\n- 现金充足\n";
        assert_eq!(strip_decision_block(text), text);
    }

    #[test]
    fn a_half_written_fence_does_not_leak_json() {
        let text = "正在写：\n```mns-decision\n{\"verdict\":";
        assert_eq!(strip_decision_block(text), "正在写：\n");
    }

    #[test]
    fn tool_rows_are_not_part_of_the_conversation() {
        assert!(is_message(EntryKind::User));
        assert!(is_message(EntryKind::Message));
        assert!(!is_message(EntryKind::ToolCall));
        assert!(!is_message(EntryKind::System));
    }
}
