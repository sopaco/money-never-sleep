//! A minimal area chart painted directly with GPUI paths.
//!
//! The component library's charts exist, but they are generic over plot data
//! and bring axis/tooltip machinery this product does not need yet. One area
//! chart with a hairline grid is enough to answer "is my account growing", and
//! painting it directly keeps the Fluent palette exact.

use gpui_kit::*;

use super::theme::DIVIDER;

/// A canvas that fills the box it is given.
///
/// `canvas()` lays out with `size: auto`, and inside a block-layout wrapper
/// that means the width fills but the height collapses to 0px. `paint_area`
/// bails out on `height <= 1.0`, so the card looked empty even though the
/// canvas had been laid out at full width. Asking for a full size makes the
/// painted box equal to the wrapper.
///
/// `gauge_aura` hid the same collapse: `paint_arc` ignores the height and
/// derives its centre from the width alone, so it drew outside a 0px box.
fn filled_canvas<T: 'static>(
    prepaint: impl 'static + FnOnce(Bounds<Pixels>, &mut Window, &mut App) -> T,
    paint: impl 'static + FnOnce(Bounds<Pixels>, T, &mut Window, &mut App),
) -> impl IntoElement {
    canvas(prepaint, paint).size_full()
}

/// Filled area + 2px stroke + end marker, with three horizontal gridlines.
pub fn area_chart(values: Vec<f64>, height: f32, color: u32) -> Div {
    div().w_full().h(px(height)).child(filled_canvas(
        move |bounds, _, _| bounds,
        move |bounds, _paint_bounds, window, _| paint_area(bounds, &values, color, true, window),
    ))
}

fn paint_area(bounds: Bounds<Pixels>, values: &[f64], color: u32, grid: bool, window: &mut Window) {
    if values.len() < 2 {
        return;
    }
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = (max - min).max(1e-9);

    let width: f32 = bounds.size.width.into();
    let height: f32 = bounds.size.height.into();
    let left: f32 = bounds.origin.x.into();
    let top: f32 = bounds.origin.y.into();
    if width <= 1.0 || height <= 1.0 {
        return;
    }
    let pad_y = height * 0.10;
    let usable = (height - pad_y * 2.0).max(1.0);
    let last = values.len() - 1;

    let x_at = |index: usize| left + width * (index as f32 / last as f32);
    let y_at = |value: f64| top + pad_y + usable * (1.0 - ((value - min) / span) as f32);

    if grid {
        for step in 1..5 {
            let y = top + height * step as f32 / 5.0;
            let mut line = PathBuilder::stroke(px(1.));
            line.move_to(point(px(left), px(y)));
            line.line_to(point(px(left + width), px(y)));
            if let Ok(path) = line.build() {
                window.paint_path(path, rgb(DIVIDER));
            }
        }
    }

    let mut area = PathBuilder::fill();
    area.move_to(point(px(left), px(top + height)));
    for (index, value) in values.iter().enumerate() {
        area.line_to(point(px(x_at(index)), px(y_at(*value))));
    }
    area.line_to(point(px(left + width), px(top + height)));
    area.close();
    if let Ok(path) = area.build() {
        // A vertical wash keeps the curve readable without flattening the card
        // into a solid block of colour.
        let fill = linear_gradient(
            180.,
            linear_color_stop(rgb(color).opacity(0.28), 0.),
            linear_color_stop(rgb(color).opacity(0.01), 1.),
        );
        window.paint_path(path, fill);
    }

    let mut stroke = PathBuilder::stroke(px(2.));
    stroke.move_to(point(px(x_at(0)), px(y_at(values[0]))));
    for (index, value) in values.iter().enumerate().skip(1) {
        stroke.line_to(point(px(x_at(index)), px(y_at(*value))));
    }
    if let Ok(path) = stroke.build() {
        window.paint_path(path, rgb(color));
    }

    // End marker so the eye lands on "now", not on the curve's shape.
    let end_x = x_at(last);
    let end_y = y_at(values[last]);
    let mut marker = PathBuilder::fill();
    marker.add_polygon(&ring_points(end_x, end_y, 4.0), true);
    if let Ok(path) = marker.build() {
        window.paint_path(path, rgb(color));
    }
    let mut inner = PathBuilder::fill();
    inner.add_polygon(&ring_points(end_x, end_y, 2.0), true);
    if let Ok(path) = inner.build() {
        window.paint_path(path, rgb(0xffffff));
    }
}

fn ring_points(cx: f32, cy: f32, radius: f32) -> Vec<Point<Pixels>> {
    // A 16-gon is visually indistinguishable from a circle at 8px and avoids
    // arc flattening differences between backends.
    const SEGMENTS: usize = 16;
    (0..SEGMENTS)
        .map(|i| {
            let angle = std::f32::consts::TAU * i as f32 / SEGMENTS as f32;
            point(px(cx + radius * angle.cos()), px(cy + radius * angle.sin()))
        })
        .collect()
}

/// A wide arc band, used purely as atmosphere behind the hero panel. It echoes
/// the gauge in the product mark and reads as light, not as data.
pub fn gauge_aura(size: f32, color: u32, alpha: f32) -> Div {
    div().w(px(size)).h(px(size)).child(filled_canvas(
        move |bounds, _, _| bounds,
        move |bounds, _, window, _| paint_arc(bounds, color, alpha, window),
    ))
}

fn paint_arc(bounds: Bounds<Pixels>, color: u32, alpha: f32, window: &mut Window) {
    let width: f32 = bounds.size.width.into();
    if width < 16.0 {
        return;
    }
    let left: f32 = bounds.origin.x.into();
    let top: f32 = bounds.origin.y.into();
    let centre = (left + width / 2.0, top + width / 2.0);
    let start = std::f32::consts::PI * 0.66;
    let end = std::f32::consts::PI * 2.34;

    // Three hairlines rather than one fat band: a thick stroke reads as a
    // smudge, a set of rings reads as a dial. Each ring is painted in short
    // segments so the ends can fade out — light has no cut edge.
    const RINGS: usize = 3;
    const STEPS: usize = 48;
    let thin = (width * 0.0045).max(1.0);

    for ring in 0..RINGS {
        let radius = width * (0.335 + 0.055 * ring as f32);
        let ring_alpha = alpha * (1.0 - ring as f32 * 0.22);
        for step in 0..STEPS {
            let t0 = step as f32 / STEPS as f32;
            let t1 = (step + 1) as f32 / STEPS as f32;
            let fade = (std::f32::consts::PI * (t0 + t1) * 0.5).sin();
            let a = ring_alpha * fade;
            if a <= 0.002 {
                continue;
            }
            let angle0 = start + (end - start) * t0;
            let angle1 = start + (end - start) * t1;
            let mut segment = PathBuilder::stroke(px(thin));
            segment.move_to(point(
                px(centre.0 + radius * angle0.cos()),
                px(centre.1 + radius * angle0.sin()),
            ));
            segment.line_to(point(
                px(centre.0 + radius * angle1.cos()),
                px(centre.1 + radius * angle1.sin()),
            ));
            if let Ok(path) = segment.build() {
                window.paint_path(path, rgb(color).opacity(a));
            }
        }
    }
}

// Headless layout regressions for the canvas charts.
//
// The asset curve rendered as an empty card: the canvas box was 1096x0, so
// `paint_area` bailed out on its `height <= 1.0` guard and nothing was drawn.
// `canvas()` is `size: auto`, and in block layout that fills the width but
// leaves the height at zero. A screenshot was the only way to notice, so these
// tests pin the two facts that make the chart visible:
//
//   1. the wrapper spans the card's content box;
//   2. the canvas fills that wrapper, height included.
//
// `on_children_prepainted` reports the bounds children were actually laid out
// in, so the tests need no test-only hooks in the production elements.
//
// Run with `cargo test --features gui-test`.

#[cfg(all(test, feature = "gui-test"))]
mod layout_tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    // Explicit imports, not `use gpui_kit::*`: the glob would shadow Rust's
    // `#[test]` with gpui-kit's macro inside the expansion and recurse forever.
    use gpui_kit::base::StyledExt;
    use gpui_kit::{
        AppContext, Bounds, Context, InteractiveElement, IntoElement, ParentElement, Pixels, Render,
        StatefulInteractiveElement, Styled, TestAppContext, Window, div, px, size,
    };

    /// Renders the chart inside the same flex chain as the real dashboard.
    struct ChartProbe {
        /// Bounds of the card's children in the last completed frame.
        children: Rc<RefCell<Vec<Bounds<Pixels>>>>,
    }

    impl Render for ChartProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let children = self.children.clone();
            // AppShell → content column → page root → card, because the chart
            // is sized by a chain of stretched flex containers.
            div().v_flex().size_full().child(
                div().h_flex().flex_1().min_h(px(0.)).child(
                    div()
                        .v_flex()
                        .flex_1()
                        .h_full()
                        .overflow_hidden()
                        .min_w(px(0.))
                        .min_h(px(0.))
                        .child(
                            div()
                                .id("probe-page")
                                .v_flex()
                                .flex_1()
                                .w_full()
                                .min_h(px(0.))
                                .overflow_y_scroll()
                                .px(px(22.))
                                .pt(px(20.))
                                .pb(px(28.))
                                .gap(px(14.))
                                .child(
                                    div()
                                        .v_flex()
                                        .flex_none()
                                        .p(px(20.))
                                        .gap(px(14.))
                                        .child(super::area_chart(
                                            vec![1.0, 2.5, 1.5, 3.0],
                                            168.,
                                            0x0067c0,
                                        ))
                                        .on_children_prepainted(move |bounds, _, _| {
                                            *children.borrow_mut() = bounds;
                                        }),
                                ),
                        ),
                ),
            )
        }
    }

    /// Renders one filled canvas inside a fixed box.
    struct CanvasProbe {
        seen: Rc<RefCell<Option<Bounds<Pixels>>>>,
    }

    impl Render for CanvasProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let seen = self.seen.clone();
            div().w(px(300.)).h(px(120.)).child(super::filled_canvas(
                move |bounds, _, _| {
                    *seen.borrow_mut() = Some(bounds);
                    bounds
                },
                |_, _, _, _| {},
            ))
        }
    }

    fn draw(handle: gpui_kit::AnyWindowHandle, cx: &mut TestAppContext) {
        cx.update_window(handle, |_, window, cx| {
            // Mirrors TestWindowExt::render_frame without gpui-base's
            // test-support feature, which cannot be enabled (see Cargo.toml).
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn area_chart_fills_the_card_content_width(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let children = Rc::new(RefCell::new(Vec::new()));
        let handle = cx.open_window(size(px(1180.), px(720.)), |_, _| ChartProbe {
            children: children.clone(),
        });
        draw(handle.into(), cx);

        let measured = children.borrow();
        let chart = measured
            .first()
            .copied()
            .expect("the card must prepaint its chart child");
        // A 1180px window, 22px page padding on each side and the card's own
        // 20px padding leave this much content width.
        let expected: f32 = 1180. - 22. * 2. - 20. * 2.;
        let width: f32 = chart.size.width.into();
        let height: f32 = chart.size.height.into();
        assert!(width > 1., "chart collapsed to {width}px wide");
        assert!(
            (width - expected).abs() < 1.,
            "chart is {width}px wide, card content is {expected}px"
        );
        assert!((height - 168.).abs() < 1., "chart is {height}px tall");
    }

    #[gpui_kit::test]
    fn chart_canvas_fills_its_wrapper(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let seen = Rc::new(RefCell::new(None));
        let handle = cx.open_window(size(px(600.), px(400.)), |_, _| CanvasProbe {
            seen: seen.clone(),
        });
        draw(handle.into(), cx);

        let bounds = seen.borrow().expect("the canvas must be prepainted");
        let width: f32 = bounds.size.width.into();
        let height: f32 = bounds.size.height.into();
        assert!(
            (height - 120.).abs() < 1.,
            "canvas is {height}px tall inside a 120px box; an auto-sized canvas \
             collapses to zero height and paint_area draws nothing"
        );
        assert!((width - 300.).abs() < 1., "canvas is {width}px wide");
    }
}
