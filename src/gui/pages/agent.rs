//! 助手：提交一个目标，让外部 Agent 连续规划并执行。
//!
//! 交互刻意不做成聊天：用户给目标，界面展示计划、工具调用、授权与结果，
//! 需要时一键停止。逐轮对话会把执行负担又推回给用户。

use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::*;

use crate::agent::{EntryKind, PermissionPolicy, RunState};
use crate::gui::components::*;
use crate::gui::theme::*;
use crate::gui::{AppShell, Page};

use super::{card_header, notice, page_lead, page_root};

pub fn render(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let agent = &shell.agent;
    let running = agent.is_running();
    let (state_color, state_bg) = match agent.state {
        RunState::Failed | RunState::Cancelled => (DANGER, DANGER_SOFT),
        RunState::Finished => (SUCCESS, SUCCESS_SOFT),
        RunState::Idle => (TEXT_TERTIARY, NEUTRAL_SOFT),
        _ => (ACCENT, ACCENT_SOFT),
    };

    let mut root = page_root("page-agent").child(
        div()
            .h_flex()
            .items_center()
            .justify_between()
            .child(page_lead(Page::Agent.subtitle()))
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap(px(10.))
                    .child(status_pill(agent.state.label(), state_color, state_bg))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(TEXT_TERTIARY))
                            .child(agent.detail.clone()),
                    ),
            ),
    );

    root = root.child(goal_card(shell, cx, running));

    if let Some(prompt) = &agent.pending {
        let id = prompt.id;
        let allow_id = prompt
            .options
            .iter()
            .find(|option| option.allow)
            .map(|option| option.id.clone())
            .unwrap_or_default();
        root = root.child(
            card_pad()
                .gap(px(10.))
                .bg(rgb(ACCENT_SOFT))
                .border_color(rgb(ACCENT).opacity(0.35))
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .gap(px(8.))
                        .child(pill("需要你确认", ACCENT, 0xffffff))
                        .child(
                            div()
                                .text_size(px(13.5))
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
                ),
        );
    }

    if let Some(error) = &agent.error {
        root = root.child(notice(
            format!("Agent 运行出错：{error}"),
            (DANGER, DANGER_SOFT),
        ));
    }

    root = root
        .child(timeline_card(shell))
        .child(connection_card(shell, cx));
    root.into_any_element()
}

fn goal_card(shell: &AppShell, cx: &mut Context<AppShell>, running: bool) -> Div {
    let tool_calls = shell
        .agent
        .timeline
        .iter()
        .filter(|entry| entry.kind == EntryKind::ToolCall)
        .count();
    let approvals = shell
        .agent
        .timeline
        .iter()
        .filter(|entry| entry.kind == EntryKind::Permission)
        .count();

    // One action at a time. A disabled twin was rendering as invisible text and
    // read like a layout bug; the state is already carried by the status pill.
    let action = if running {
        Button::new("agent-stop")
            .label("停止")
            .on_click(cx.listener(|shell, _, _, cx| shell.stop_agent(cx)))
    } else {
        Button::new("agent-start")
            .primary()
            .label("开始自主执行")
            .on_click(cx.listener(|shell, _, _, cx| shell.start_agent(cx)))
    };

    card_pad()
        .gap(px(12.))
        .child(card_header(
            "目标",
            format!("工具调用 {tool_calls} · 授权记录 {approvals}"),
        ))
        .child(
            div()
                .w_full()
                .rounded(px(RADIUS_SM))
                .child(Textarea::new(&shell.goal)),
        )
        .child(
            div()
                .h_flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .gap(px(6.))
                        .child(caption("权限策略"))
                        .children(
                            PermissionPolicy::ALL
                                .into_iter()
                                .map(|policy| policy_chip(policy, shell, cx)),
                        ),
                )
                .child(div().h_flex().gap(px(8.)).child(action)),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgb(TEXT_TERTIARY))
                .child("权限策略在任务启动时生效，运行中改动下次任务才应用。"),
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

fn timeline_card(shell: &AppShell) -> Div {
    let card = card_pad().gap(px(8.)).child(card_header(
        "执行轨迹",
        "最新在最上方；工具调用与结果分开显示",
    ));

    if shell.agent.timeline.is_empty() {
        return card.child(empty_state(
            IconName::Sparkles,
            "还没有执行记录",
            "写一句目标并开始，Agent 会自己规划、调用工具、给出结果。",
        ));
    }

    let rows: Vec<Div> = shell
        .agent
        .timeline
        .iter()
        .rev()
        .take(120)
        .map(|entry| {
            let (label, fg, bg) = entry_style(entry.kind);
            let mono = matches!(
                entry.kind,
                EntryKind::ToolCall | EntryKind::ToolResult | EntryKind::System
            );
            let mut text = div()
                .flex_1()
                .min_w(px(0.))
                .text_size(px(13.))
                .text_color(rgb(if entry.kind == EntryKind::Thought {
                    TEXT_SECONDARY
                } else {
                    TEXT
                }))
                .child(entry.text.clone());
            if mono {
                text = text.font_family(mono_family());
            }
            div()
                .h_flex()
                .items_start()
                .gap(px(10.))
                .py(px(7.))
                .border_b_1()
                .border_color(rgb(DIVIDER))
                .child(div().w(px(46.)).flex_none().child(pill(label, fg, bg)))
                .child(text)
        })
        .collect();

    card.child(
        div()
            .id("agent-timeline")
            .v_flex()
            .h(px(320.))
            .overflow_y_scroll()
            .restrict_scroll_to_axis()
            // GPUI does not isolate nested scroll containers by default. Let
            // the timeline handle the gesture first, then keep it away from
            // the outer page so both regions never move together.
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .children(rows),
    )
}

fn entry_style(kind: EntryKind) -> (&'static str, u32, u32) {
    match kind {
        EntryKind::System => ("系统", TEXT_SECONDARY, NEUTRAL_SOFT),
        EntryKind::Thought => ("思考", TEXT_TERTIARY, NEUTRAL_SOFT),
        EntryKind::Message => ("输出", TEXT, LAYER),
        EntryKind::Plan => ("计划", WARNING, WARNING_SOFT),
        EntryKind::ToolCall => ("工具", ACCENT, ACCENT_SOFT),
        EntryKind::ToolResult => ("结果", SUCCESS, SUCCESS_SOFT),
        EntryKind::Permission => ("授权", ACCENT, ACCENT_SOFT),
        EntryKind::Error => ("错误", DANGER, DANGER_SOFT),
    }
}

fn connection_card(shell: &AppShell, cx: &mut Context<AppShell>) -> Div {
    card_pad()
        .gap(px(10.))
        .child(card_header(
            "外部 Agent",
            "只配置 CLI 路径；模型与凭据留在 opencode 自己的配置里",
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
                            .on_click(cx.listener(|shell, _, _, cx| shell.save_agent_settings(cx))),
                    ),
                ),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgb(TEXT_TERTIARY))
                .child("MNS 会自动追加 `acp` 子命令；不需要在这里填 URL 或 API Key。"),
        )
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
