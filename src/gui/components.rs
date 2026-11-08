//! Small presentation primitives shared by every page.
//!
//! The look is a cool-paper dashboard: white cards with a two-layer shadow,
//! one saturated brand colour, and a type scale that keeps numbers louder than
//! labels. Primitives live here so six pages cannot drift apart.

use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::*;

use super::theme::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Gain,
    Loss,
    Accent,
    Warning,
    Muted,
}

pub fn tone_color(tone: Tone) -> u32 {
    match tone {
        Tone::Gain => GAIN,
        Tone::Loss => LOSS,
        Tone::Accent => ACCENT,
        Tone::Warning => WARNING,
        Tone::Muted => TEXT_TERTIARY,
    }
}

pub fn tone_soft(tone: Tone) -> u32 {
    match tone {
        Tone::Gain => GAIN_SOFT,
        Tone::Loss => LOSS_SOFT,
        Tone::Accent => ACCENT_SOFT,
        Tone::Warning => WARNING_SOFT,
        Tone::Muted => NEUTRAL_SOFT,
    }
}

/// Gain/loss tone for a signed number. Zero is neutral: colouring it would
/// claim a direction the data does not have.
pub fn signed_tone(value: f64) -> Tone {
    if value > 0.0001 {
        Tone::Gain
    } else if value < -0.0001 {
        Tone::Loss
    } else {
        Tone::Muted
    }
}

// ── Number formatting ──────────────────────────────────────────────────

fn group_digits(value: i64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

pub fn money(value: f64) -> String {
    let sign = if value < 0.0 { "-" } else { "" };
    let cents = (value.abs() * 100.0).round() as i64;
    format!("{sign}¥{}.{:02}", group_digits(cents / 100), cents % 100)
}

pub fn money_signed(value: f64) -> String {
    if value > 0.0 {
        format!("+{}", money(value))
    } else {
        money(value)
    }
}

/// Compact Chinese units for headline figures: ¥12.35万.
pub fn money_compact(value: f64) -> String {
    let abs = value.abs();
    let sign = if value < 0.0 { "-" } else { "" };
    if abs >= 100_000_000.0 {
        format!("{sign}¥{:.2}亿", abs / 100_000_000.0)
    } else if abs >= 10_000.0 {
        format!("{sign}¥{:.2}万", abs / 10_000.0)
    } else {
        money(value)
    }
}

/// `fraction` is 0.1234 for +12.34%.
pub fn pct_signed(fraction: f64) -> String {
    format!("{:+.2}%", fraction * 100.0)
}

/// Expression already in percentage points, e.g. a 52.3% weight.
pub fn pct_points(points: f64) -> String {
    format!("{points:.1}%")
}

pub fn pp_signed(points: f64) -> String {
    format!("{points:+.1}pp")
}

// ── Surfaces ───────────────────────────────────────────────────────────

pub fn card() -> Div {
    div()
        .v_flex()
        .flex_none()
        .bg(rgb(SURFACE))
        .border_1()
        .border_color(rgb(BORDER))
        .rounded(px(RADIUS))
        .shadow(shadow_card())
        .overflow_hidden()
}

pub fn card_pad() -> Div {
    card().p(px(20.)).gap(px(14.))
}

pub fn divider() -> Div {
    div().h(px(1.)).w_full().bg(rgb(DIVIDER))
}

pub fn caption(text: impl IntoElement) -> Div {
    div()
        .text_size(px(13.))
        .text_color(rgb(TEXT_SECONDARY))
        .child(text)
}

/// Small caps-ish label above a value.
pub fn overline(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(TEXT_SECONDARY))
        .child(text.into())
}

pub fn section_title(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(15.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(TEXT))
        .child(text.into())
}

/// A glyph bled into the corner of a panel and drained of contrast.
///
/// Placing an icon *next to* a label makes a row of bullet points; placing it
/// *behind* the content makes the panel feel material, like a printed card with
/// a watermark. `size` is in pixels and deliberately larger than the visible
/// area so the shape is clipped by the card edge.
pub fn watermark(icon: IconName, color: u32, size: f32, alpha: f32) -> Div {
    div()
        .absolute()
        .right(px(-size * 0.16))
        .top(px(-size * 0.12))
        .text_size(px(size))
        .text_color(rgb(color).opacity(alpha))
        .child(icon)
}

pub fn pill(text: impl Into<SharedString>, fg: u32, bg: u32) -> Div {
    div()
        .h_flex()
        .items_center()
        .gap(px(5.))
        .px(px(8.))
        .py(px(3.))
        .rounded(px(999.))
        .bg(rgb(bg))
        .text_size(px(12.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(fg))
        .child(text.into())
}

pub fn dot(color: u32) -> Div {
    div().w(px(6.)).h(px(6.)).rounded(px(3.)).bg(rgb(color))
}

pub fn status_pill(label: impl Into<SharedString>, fg: u32, bg: u32) -> Div {
    let label = label.into();
    div()
        .h_flex()
        .items_center()
        .gap(px(6.))
        .px(px(9.))
        .py(px(4.))
        .rounded(px(999.))
        .bg(rgb(bg))
        .child(dot(fg))
        .child(div().text_size(px(12.5)).text_color(rgb(fg)).child(label))
}

/// A headline metric: one loud number, a quiet label above it, and the reading
/// help below. The glyph lives in the corner as a watermark so the eye lands on
/// the value first — the opposite of the icon-then-text ordering every admin
/// template uses.
pub fn stat_tile(
    icon: IconName,
    tone: Tone,
    label: &'static str,
    value: String,
    sub: Option<(String, Tone)>,
    hint: Option<String>,
) -> Div {
    let fg = tone_color(tone);
    // An em-dash at 28px reads as a value that happens to look like a minus.
    // Shrinking it to the label scale turns it back into "no number yet".
    let missing = value == "—";
    let mut tile = card_pad()
        .flex_1()
        .min_w(px(168.))
        .relative()
        .p(px(18.))
        .gap(px(6.))
        .child(watermark(icon, fg, 84., 0.068))
        .child(overline(label))
        .child(if missing {
            div()
                .py(px(5.))
                .text_size(px(20.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(TEXT_TERTIARY))
                .child(value)
        } else {
            div()
                .text_size(px(28.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(TEXT))
                .child(value)
        });
    if let Some((text, tone)) = sub {
        tile = tile.child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(tone_color(tone)))
                .child(text),
        );
    }
    if let Some(hint) = hint {
        tile = tile.child(
            div()
                .pt(px(2.))
                .text_size(px(12.5))
                .text_color(rgb(TEXT_TERTIARY))
                .child(hint),
        );
    }
    tile
}

pub fn kv_row(label: &'static str, value: String) -> Div {
    div()
        .h_flex()
        .items_center()
        .justify_between()
        .py(px(7.))
        .child(
            div()
                .text_size(px(13.))
                .text_color(rgb(TEXT_SECONDARY))
                .child(label),
        )
        .child(div().text_size(px(13.)).text_color(rgb(TEXT)).child(value))
}

/// Progress track: `fraction` is 0..=1, clamped so overflow cannot paint
/// outside the track.
pub fn hbar(fraction: f32, color: u32, height: f32) -> Div {
    let width = fraction.clamp(0.0, 1.0);
    div()
        .h(px(height))
        .w_full()
        .rounded(px(height / 2.0))
        .bg(rgb(0xeef1f7))
        .child(
            div()
                .h_full()
                .w(relative(width))
                .rounded(px(height / 2.0))
                .bg(rgb(color)),
        )
}

pub fn empty_state(icon: IconName, title: &'static str, body: impl Into<SharedString>) -> Div {
    div()
        .v_flex()
        .items_center()
        .justify_center()
        .gap(px(10.))
        .py(px(22.))
        .w_full()
        .child(
            div()
                .w(px(48.))
                .h(px(48.))
                .rounded(px(999.))
                .bg(rgb(ACCENT_TINT))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(22.))
                .text_color(rgb(ACCENT))
                .child(icon),
        )
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
                .child(body.into()),
        )
}

/// Table header label, aligned with the row cells in each page.
pub fn th(text: &'static str) -> Div {
    div()
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(TEXT_TERTIARY))
        .child(text)
}
