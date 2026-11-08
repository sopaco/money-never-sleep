//! Design tokens for the desktop shell.
//!
//! The palette is sampled from the product mark (`assets/app_icon.png`): a deep
//! electric blue over a cool paper background. Everything else is neutral so the
//! brand colour stays the only saturated thing on screen, which is what makes a
//! data-heavy dashboard readable at a glance.

use gpui_kit::component::theme::Theme;
use gpui_kit::*;

// ── Surfaces ───────────────────────────────────────────────────────────
/// App background: a cool paper tone, one step darker than the rail so the
/// navigation reads as a distinct surface without needing a heavy border.
pub const BG: u32 = 0xf2f5fa;
/// Card surface.
pub const SURFACE: u32 = 0xffffff;
/// Inset layer: table headers, code blocks, quiet rows.
pub const LAYER: u32 = 0xf7f9fc;
/// Navigation rail.
pub const RAIL: u32 = 0xfbfcfe;
/// Hover wash for rows and ghost buttons.
pub const HOVER: u32 = 0xeef2f8;
/// Selected navigation item.
pub const SELECTED: u32 = 0xe8f0ff;

// ── Lines ──────────────────────────────────────────────────────────────
pub const BORDER: u32 = 0xe4e9f2;
pub const DIVIDER: u32 = 0xedf1f7;

// ── Text ───────────────────────────────────────────────────────────────
pub const TEXT: u32 = 0x131a2a;
pub const TEXT_SECONDARY: u32 = 0x5b6577;
pub const TEXT_TERTIARY: u32 = 0x8b94a6;
pub const TEXT_ON_ACCENT: u32 = 0xffffff;

// ── Brand ──────────────────────────────────────────────────────────────
pub const ACCENT: u32 = 0x1a5ce0;
pub const ACCENT_HOVER: u32 = 0x2a6ceb;
pub const ACCENT_PRESSED: u32 = 0x114cc2;
pub const ACCENT_SOFT: u32 = 0xe8f0ff;
pub const ACCENT_TINT: u32 = 0xf4f8ff;

// ── Semantic ───────────────────────────────────────────────────────────
/// Gains use red, losses green: this is a Chinese-market product.
pub const GAIN: u32 = 0xd0342c;
pub const GAIN_SOFT: u32 = 0xfdeceb;
pub const LOSS: u32 = 0x0f7b3f;
pub const LOSS_SOFT: u32 = 0xe7f6ec;
pub const WARNING: u32 = 0x9a5b00;
pub const WARNING_SOFT: u32 = 0xfff3d6;
pub const DANGER: u32 = 0xc42b1c;
pub const DANGER_SOFT: u32 = 0xfde7e9;
pub const SUCCESS: u32 = 0x0f7b3f;
pub const SUCCESS_SOFT: u32 = 0xdff6e4;
pub const NEUTRAL_SOFT: u32 = 0xf0f3f8;

// ── Geometry ───────────────────────────────────────────────────────────
// Squarer than the first pass: 12/8/16 read as soft consumer-app bubbles,
// which is the opposite of what a ledger should feel like.
pub const RADIUS: f32 = 8.0;
pub const RADIUS_SM: f32 = 6.0;
pub const RADIUS_LG: f32 = 10.0;

/// Resting elevation for cards. Two layers: a hairline contact shadow that
/// makes the card sit on the page, plus a wide soft shadow that separates it
/// from the background when several cards are stacked.
pub fn shadow_card() -> Vec<BoxShadow> {
    vec![
        BoxShadow::new(px(0.), px(1.), ink(0.05)).blur_radius(px(2.)),
        BoxShadow::new(px(0.), px(12.), ink(0.06))
            .blur_radius(px(28.))
            .spread_radius(px(-16.)),
    ]
}

/// Raised state, used on hover for anything clickable.
pub fn shadow_raised() -> Vec<BoxShadow> {
    vec![
        BoxShadow::new(px(0.), px(2.), ink(0.06)).blur_radius(px(4.)),
        BoxShadow::new(px(0.), px(16.), ink(0.10))
            .blur_radius(px(32.))
            .spread_radius(px(-14.)),
    ]
}

/// Popovers, dialogs and the toast layer.
pub fn shadow_overlay() -> Vec<BoxShadow> {
    vec![
        BoxShadow::new(px(0.), px(8.), ink(0.10)).blur_radius(px(20.)),
        BoxShadow::new(px(0.), px(24.), ink(0.16))
            .blur_radius(px(56.))
            .spread_radius(px(-20.)),
    ]
}

fn ink(alpha: f32) -> Hsla {
    rgb(0x0b1526).opacity(alpha).into()
}

/// Point the component library at this palette so built-in controls (buttons,
/// inputs, scrollbars) sit on the same surface as hand-built cards.
pub fn apply(cx: &mut App) {
    Theme::update(cx, |theme| {
        theme.colors.background = rgb(BG).into();
        theme.colors.foreground = rgb(TEXT).into();
        theme.colors.border = rgb(BORDER).into();
        theme.colors.muted = rgb(NEUTRAL_SOFT).into();
        theme.colors.muted_foreground = rgb(TEXT_SECONDARY).into();
        theme.colors.secondary = rgb(LAYER).into();
        theme.colors.secondary_foreground = rgb(TEXT).into();
        theme.colors.secondary_hover = rgb(HOVER).into();
        theme.colors.accent = rgb(SELECTED).into();
        theme.colors.accent_foreground = rgb(ACCENT).into();
        theme.colors.input = rgb(SURFACE).into();
        theme.colors.ring = rgb(ACCENT).into();
        theme.colors.primary = rgb(ACCENT).into();
        theme.colors.primary_hover = rgb(ACCENT_HOVER).into();
        theme.colors.primary_active = rgb(ACCENT_PRESSED).into();
        theme.colors.primary_foreground = rgb(TEXT_ON_ACCENT).into();
        theme.colors.button = rgb(SURFACE).into();
        theme.colors.button_hover = rgb(HOVER).into();
        theme.colors.button_active = rgb(0xe2e8f3).into();
        theme.colors.button_foreground = rgb(TEXT).into();
        theme.colors.button_primary = rgb(ACCENT).into();
        theme.colors.button_primary_hover = rgb(ACCENT_HOVER).into();
        theme.colors.button_primary_active = rgb(ACCENT_PRESSED).into();
        theme.colors.button_primary_foreground = rgb(TEXT_ON_ACCENT).into();
        theme.colors.danger = rgb(DANGER).into();
        theme.colors.success = rgb(SUCCESS).into();
        theme.colors.warning = rgb(WARNING).into();
        theme.colors.link = rgb(ACCENT).into();
        theme.colors.link_hover = rgb(ACCENT_HOVER).into();
        theme.colors.progress_bar = rgb(ACCENT).into();
        theme.colors.chart_bullish = rgb(GAIN).into();
        theme.colors.chart_bearish = rgb(LOSS).into();
        theme.colors.chart_grid = rgb(DIVIDER).into();
        theme.colors.scrollbar_thumb = rgb(0xc6cede).into();
        theme.colors.scrollbar_thumb_hover = rgb(0xa4afc2).into();
        theme.radius = px(RADIUS_SM);
        theme.radius_lg = px(RADIUS_LG);
    });
}
