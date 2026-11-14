//! Page composition helpers and the six top-level pages.

pub mod agent;
pub mod dashboard;
pub mod performance;
pub mod plan;
pub mod portfolio;
pub mod settings;

use gpui_kit::base::StyledExt;
use gpui_kit::*;

use super::components::*;
use super::theme::*;

/// Scrollable page surface. Every page gets the same padding and rhythm, which
/// is what makes the navigation feel like one product rather than six screens.
pub(crate) fn page_root(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .v_flex()
        .flex_1()
        .w_full()
        .min_h(px(0.))
        .overflow_y_scroll()
        .px(px(22.))
        .pt(px(20.))
        .pb(px(28.))
        .gap(px(14.))
}

pub(crate) fn row(tiles: Vec<Div>) -> Div {
    div()
        .h_flex()
        .flex_none()
        .gap(px(12.))
        .items_stretch()
        .children(tiles)
}

pub(crate) fn card_header(title: impl Into<SharedString>, hint: impl Into<SharedString>) -> Div {
    let hint: SharedString = hint.into();
    let mut header = div()
        .h_flex()
        .items_baseline()
        .flex_none()
        .gap(px(9.))
        .child(section_title(title));
    if !hint.is_empty() {
        header = header.child(
            div()
                .text_size(px(12.5))
                .text_color(rgb(TEXT_TERTIARY))
                .child(hint),
        );
    }
    // A rule running out of the header to the card edge. Without it the
    // title floats in white space; with it the card reads as a printed
    // page with a running head.
    header.child(div().flex_1().h(px(1.)).bg(rgb(DIVIDER)))
}

pub(crate) fn notice(text: impl Into<SharedString>, tone: (u32, u32)) -> Div {
    div()
        .flex_none()
        .px(px(12.))
        .py(px(9.))
        .rounded(px(RADIUS_SM))
        .bg(rgb(tone.1))
        .text_size(px(12.5))
        .text_color(rgb(tone.0))
        .child(text.into())
}
