//! Rendering of parsed help markdown into GPUI elements.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::{
    div, img, px, rgb, AnyElement, FontStyle, FontWeight, HighlightStyle, Image, ImageFormat,
    IntoElement, ParentElement, Pixels, SharedString, Styled, StyledText, UnderlineStyle,
};
use unicode_width::UnicodeWidthStr;

use crate::gui::components::mono_family;
use crate::gui::theme::{
    ACCENT, ACCENT_SOFT, BORDER, DANGER, DANGER_SOFT, DIVIDER, LAYER, SUCCESS, SUCCESS_SOFT,
    TEXT, TEXT_SECONDARY, TEXT_TERTIARY, WARNING, WARNING_SOFT, RADIUS, RADIUS_SM,
};

use super::markdown::{Align, Block, Callout, ListItem, Span};

/// Loads and caches help images from disk.
///
/// Images are resolved against `help/` directories found next to the
/// executable and in its ancestors (the latter covers `target/debug` during
/// development). Missing images render as a dashed placeholder that names the
/// expected path, so screenshots can be dropped in later without a rebuild.
#[derive(Default)]
pub struct ImageCache {
    map: std::cell::RefCell<HashMap<String, Option<LoadedImage>>>,
}

/// A loaded screenshot plus the aspect ratio needed to size its element.
///
/// GPUI only derives the height from the intrinsic size when the width is
/// `Length::Auto`; `w_full` is a relative length, so an unsized image element
/// keeps its natural height and the drawn picture floats inside a much taller
/// box. Carrying the ratio lets `render_image` set both sides explicitly.
#[derive(Clone)]
struct LoadedImage {
    image: Arc<Image>,
    /// `width / height`; `None` when the header could not be recognised.
    aspect: Option<f32>,
}

impl ImageCache {
    fn resolve(&self, path: &str) -> Option<LoadedImage> {
        if let Some(hit) = self.map.borrow().get(path) {
            return hit.clone();
        }
        let loaded = Self::search_roots()
            .into_iter()
            .find_map(|root| Self::load(&root.join(path)));
        self.map
            .borrow_mut()
            .insert(path.to_owned(), loaded.clone());
        loaded
    }

    fn search_roots() -> Vec<PathBuf> {
        let mut roots = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            let mut dir = exe.parent().map(PathBuf::from);
            for _ in 0..5 {
                let Some(current) = dir else { break };
                roots.push(current.join("help"));
                dir = current.parent().map(PathBuf::from);
            }
        }
        roots.push(PathBuf::from("help"));
        roots
    }

    fn load(path: &PathBuf) -> Option<LoadedImage> {
        let bytes = std::fs::read(path).ok()?;
        let format = match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "png" => ImageFormat::Png,
            "jpg" | "jpeg" => ImageFormat::Jpeg,
            "gif" => ImageFormat::Gif,
            "bmp" => ImageFormat::Bmp,
            _ => ImageFormat::Png,
        };
        let aspect = image_aspect(&bytes);
        let image = Image::from_bytes(format, bytes);
        Some(LoadedImage {
            image: Arc::new(image),
            aspect,
        })
    }
}

/// Reads `width / height` from an image header.
///
/// Only the dimensions are needed, and every supported format puts them near
/// the start of the file, so this stays a few dozen lines instead of pulling a
/// decoder into the help viewer.
fn image_aspect(bytes: &[u8]) -> Option<f32> {
    let (width, height) = if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])
    {
        // PNG: IHDR is the first chunk, width/height are big-endian u32 at 16/20.
        let width = u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?);
        let height = u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?);
        (width as f32, height as f32)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        let width = u16::from_le_bytes(bytes.get(6..8)?.try_into().ok()?);
        let height = u16::from_le_bytes(bytes.get(8..10)?.try_into().ok()?);
        (width as f32, height as f32)
    } else if bytes.starts_with(b"BM") {
        let width = i32::from_le_bytes(bytes.get(18..22)?.try_into().ok()?).unsigned_abs();
        let height = i32::from_le_bytes(bytes.get(22..26)?.try_into().ok()?).unsigned_abs();
        (width as f32, height as f32)
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        jpeg_aspect(bytes)?
    } else {
        return None;
    };
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    Some(width / height)
}

/// JPEG: walk the marker segments until a start-of-frame carries the size.
fn jpeg_aspect(bytes: &[u8]) -> Option<(f32, f32)> {
    let mut i = 2;
    while i + 9 < bytes.len() {
        if bytes[i] != 0xff {
            return None;
        }
        let marker = bytes[i + 1];
        // Standalone markers (padding, restart) carry no length field.
        if marker == 0xff {
            i += 1;
            continue;
        }
        if marker == 0xd8 || (0xd0..=0xd9).contains(&marker) {
            i += 2;
            continue;
        }
        let length = u16::from_be_bytes(bytes.get(i + 2..i + 4)?.try_into().ok()?) as usize;
        // SOF0..SOF15 except DHT (0xc4), JPG (0xc8) and DAC (0xcc).
        if (0xc0..=0xcf).contains(&marker) && marker != 0xc4 && marker != 0xc8 && marker != 0xcc {
            let height = u16::from_be_bytes(bytes.get(i + 5..i + 7)?.try_into().ok()?);
            let width = u16::from_be_bytes(bytes.get(i + 7..i + 9)?.try_into().ok()?);
            return Some((width as f32, height as f32));
        }
        i += 2 + length;
    }
    None
}

/// Renders a parsed markdown document as a vertical stack of elements.
///
/// `width` is the readable column width in logical pixels; it is the only way
/// an image can compute the height that keeps its aspect ratio.
pub fn render_document(blocks: &[Block], images: &ImageCache, width: Pixels) -> gpui_kit::Div {
    // `w_full` on the root and on every block below is deliberate: without it
    // a flex child sizes to max-content, so paragraphs would never wrap and
    // would run off the right edge of the reading column.
    let mut doc = div().v_flex().w_full().gap(px(12.));
    for block in blocks {
        doc = doc.child(render_block(block, images, width));
    }
    doc
}

fn render_block(block: &Block, images: &ImageCache, width: Pixels) -> AnyElement {
    match block {
        Block::Heading { level, spans } => render_heading(*level, spans).into_any_element(),
        Block::Paragraph(spans) => render_paragraph(spans).into_any_element(),
        Block::List(items) => render_list(items).into_any_element(),
        Block::Table { aligns, rows } => render_table(aligns, rows).into_any_element(),
        Block::Code { text, .. } => render_code(text).into_any_element(),
        Block::Quote { callout, blocks } => {
            render_quote(*callout, blocks, images, width).into_any_element()
        }
        Block::Image { path, alt } => render_image(path, alt, images, width).into_any_element(),
        Block::Hr => div()
            .h(px(1.))
            .w_full()
            .bg(rgb(DIVIDER))
            .mt(px(10.))
            .into_any_element(),
    }
}

fn render_heading(level: u8, spans: &[Span]) -> gpui_kit::Div {
    let (size, weight, top, bottom) = match level {
        1 => (px(24.), FontWeight::SEMIBOLD, px(0.), px(4.)),
        2 => (px(19.), FontWeight::SEMIBOLD, px(22.), px(6.)),
        3 => (px(16.), FontWeight::SEMIBOLD, px(16.), px(4.)),
        _ => (px(14.5), FontWeight::SEMIBOLD, px(12.), px(2.)),
    };
    let heading = div()
        .w_full()
        .mt(top)
        .mb(bottom)
        .text_size(size)
        .font_weight(weight)
        .text_color(rgb(TEXT))
        .line_height(size + px(8.))
        .child(render_spans(spans));
    if level == 1 {
        // The page title doubles as the document header; give it an accent rule.
        div()
            .v_flex()
            .w_full()
            .gap(px(10.))
            .child(heading)
            .child(div().h(px(2.)).w(px(46.)).bg(rgb(ACCENT)).rounded(px(1.)))
    } else {
        heading
    }
}

fn render_paragraph(spans: &[Span]) -> gpui_kit::Div {
    div()
        .w_full()
        .text_size(px(14.))
        .line_height(px(24.))
        .text_color(rgb(TEXT))
        .child(render_spans(spans))
}

fn render_list(items: &[ListItem]) -> gpui_kit::Div {
    let mut list = div().v_flex().w_full().gap(px(5.)).pl(px(2.));
    for item in items {
        let marker = if item.ordered {
            format!("{}.", item.index)
        } else {
            "•".to_string()
        };
        list = list.child(
            div()
                .h_flex()
                .w_full()
                .items_start()
                .gap(px(8.))
                .pl(px((item.level * 18) as f32))
                .child(
                    div()
                        .flex_none()
                        .w(px(20.))
                        .text_size(px(14.))
                        .text_color(rgb(TEXT_TERTIARY))
                        .child(marker),
                )
                .child(
                    div()
                        .flex_1()
                        // Flex items refuse to shrink below min-content
                        // without this, which made long bullets run past the
                        // reading column instead of wrapping.
                        .min_w_0()
                        .text_size(px(14.))
                        .line_height(px(24.))
                        .text_color(rgb(TEXT))
                        .child(render_spans(&item.spans)),
                ),
        );
    }
    list
}

fn render_code(text: &str) -> gpui_kit::Div {
    div()
        .w_full()
        .p(px(12.))
        .rounded(px(RADIUS_SM))
        .bg(rgb(LAYER))
        .border_1()
        .border_color(rgb(BORDER))
        .font_family(mono_family())
        .text_size(px(12.5))
        .line_height(px(20.))
        .text_color(rgb(TEXT))
        .child(SharedString::from(text.to_owned()))
}

fn render_quote(
    callout: Option<Callout>,
    blocks: &[Block],
    images: &ImageCache,
    width: Pixels,
) -> gpui_kit::Div {
    match callout {
        Some(kind) => render_callout(kind, blocks, images, width),
        None => div()
            .w_full()
            .border_l_2()
            .border_color(rgb(BORDER))
            .pl(px(14.))
            .py(px(2.))
            .text_color(rgb(TEXT_SECONDARY))
            .child(render_document(blocks, images, width - px(16.))),
    }
}

fn render_callout(
    kind: Callout,
    blocks: &[Block],
    images: &ImageCache,
    width: Pixels,
) -> gpui_kit::Div {
    let (icon, label, fg, bg): (IconName, &str, u32, u32) = match kind {
        Callout::Note => (IconName::Info, "说明", ACCENT, ACCENT_SOFT),
        Callout::Tip => (IconName::Lightbulb, "提示", SUCCESS, SUCCESS_SOFT),
        Callout::Important => (IconName::TriangleAlert, "重要", WARNING, WARNING_SOFT),
        Callout::Warning => (IconName::OctagonAlert, "警告", DANGER, DANGER_SOFT),
    };
    // Callout 自身有 14px 内边距，图片要按扣掉内边距后的列宽计算高度。
    let inner_width = width - px(28.);
    let mut body = div().v_flex().w_full().gap(px(6.));
    for block in blocks {
        // Headings inside callouts render as emphasized lead-in text.
        if let Block::Heading { spans, .. } = block {
            body = body.child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(fg))
                    .child(render_spans(spans)),
            );
        } else {
            body = body.child(render_block(block, images, inner_width));
        }
    }
    div()
        .v_flex()
        .w_full()
        .gap(px(8.))
        .p(px(14.))
        .rounded(px(RADIUS))
        .bg(rgb(bg))
        .child(
            div()
                .h_flex()
                .items_center()
                .gap(px(7.))
                .text_size(px(15.))
                .text_color(rgb(fg))
                .child(icon)
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(fg))
                        .child(label.to_string()),
                ),
        )
        .child(body)
}

fn render_image(path: &str, alt: &str, images: &ImageCache, width: Pixels) -> AnyElement {
    match images.resolve(path) {
        Some(loaded) => {
            let mut picture = img(loaded.image).w(width);
            // 只给宽度时元素高度会退化成图片固有高度，绘制时按 Contain 收缩，
            // 于是图片上方留出大片空白。按宽高比显式给高度，元素与画面严丝合缝。
            if let Some(aspect) = loaded.aspect {
                picture = picture.h(px(width.as_f32() / aspect));
            }
            div()
                .v_flex()
                .w_full()
                .gap(px(6.))
                .py(px(4.))
                .child(
                    picture
                        .rounded(px(RADIUS))
                        .border_1()
                        .border_color(rgb(BORDER)),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(TEXT_TERTIARY))
                        .child(format!("图：{alt}")),
                )
                .into_any_element()
        }
        None => div()
            .v_flex()
            .w_full()
            .gap(px(8.))
            .items_center()
            .justify_center()
            .py(px(36.))
            .px(px(16.))
            .rounded(px(RADIUS))
            .border_1()
            .border_dashed()
            .border_color(rgb(BORDER))
            .bg(rgb(LAYER))
            .child(
                div()
                    .text_size(px(28.))
                    .text_color(rgb(TEXT_TERTIARY))
                    .child(IconName::ImageOff),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(TEXT_SECONDARY))
                    .child("截图待补充"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(TEXT_TERTIARY))
                    .child(format!("请将截图保存为 help/{path}")),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(TEXT_TERTIARY))
                    .child(format!("图注：{alt}")),
            )
            .into_any_element(),
    }
}

fn render_table(aligns: &[Align], rows: &[Vec<Vec<Span>>]) -> gpui_kit::Div {
    let weights = column_weights(rows, aligns.len());
    let mut table = div()
        .v_flex()
        .w_full()
        .rounded(px(RADIUS))
        .border_1()
        .border_color(rgb(BORDER))
        .overflow_hidden();
    for (row_index, row) in rows.iter().enumerate() {
        let header = row_index == 0;
        let mut row_el = div().h_flex().w_full().bg(if header {
            rgb(LAYER)
        } else {
            rgb(0xffffff)
        });
        for (col, cell) in row.iter().enumerate() {
            let weight = weights.get(col).copied().unwrap_or(1.0);
            let mut cell_el = div()
                .flex_grow(weight)
                .flex_basis(px(0.))
                // Same flex-shrink floor as list rows: without it a wide cell
                // forces the whole table past the column edge.
                .min_w_0()
                .px(px(10.))
                .py(px(7.))
                .text_size(px(12.5))
                .line_height(px(19.))
                .text_color(rgb(TEXT));
            if header {
                cell_el = cell_el.font_weight(FontWeight::SEMIBOLD);
            }
            if aligns.get(col) == Some(&Align::Center) {
                cell_el = cell_el.text_center();
            }
            row_el = row_el.child(cell_el.child(render_spans(cell)));
        }
        table = table.child(row_el);
    }
    table
}

/// Proportional flex weights per column, derived from the widest cell content.
fn column_weights(rows: &[Vec<Vec<Span>>], width: usize) -> Vec<f32> {
    let mut max_widths = vec![3usize; width];
    for row in rows {
        for (col, cell) in row.iter().enumerate() {
            if col >= width {
                break;
            }
            let text: String = cell.iter().map(|s| s.text.as_str()).collect();
            let w = UnicodeWidthStr::width(text.as_str()).max(3);
            max_widths[col] = max_widths[col].max(w);
        }
    }
    max_widths.iter().map(|w| *w as f32).collect()
}

/// Renders inline spans as one wrapping styled-text element.
fn render_spans(spans: &[Span]) -> StyledText {
    let mut text = String::new();
    let mut highlights = Vec::new();
    let mut font_overrides = Vec::new();
    for span in spans {
        let start = text.len();
        text.push_str(&span.text);
        let range = start..text.len();
        let mut style = HighlightStyle::default();
        let mut styled = false;
        if span.bold {
            style.font_weight = Some(FontWeight::BOLD);
            styled = true;
        }
        if span.italic {
            style.font_style = Some(FontStyle::Italic);
            styled = true;
        }
        if span.code {
            style.background_color = Some(rgb(LAYER).into());
            style.color = Some(rgb(ACCENT).into());
            font_overrides.push((range.clone(), mono_family()));
            styled = true;
        } else if span.link.is_some() {
            style.color = Some(rgb(ACCENT).into());
            style.underline = Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(rgb(ACCENT).alpha(0.5).into()),
                wavy: false,
            });
            styled = true;
        }
        if styled {
            highlights.push((range, style));
        }
    }
    let styled = StyledText::new(SharedString::from(text)).with_highlights(highlights);
    if font_overrides.is_empty() {
        styled
    } else {
        styled.with_font_family_overrides(
            font_overrides
                .into_iter()
                .map(|(range, family)| (range, SharedString::from(family)))
                .collect::<Vec<_>>(),
        )
    }
}
