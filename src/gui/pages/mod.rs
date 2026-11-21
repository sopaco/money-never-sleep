//! Page composition helpers and the six top-level pages.

pub mod dashboard;
pub mod performance;
pub mod plan;
pub mod portfolio;
pub mod review;
pub mod settings;

use gpui_kit::base::StyledExt;
use gpui_kit::*;

use super::components::*;
use super::help_links::HelpLink;
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
    header_with(title.into(), hint.into(), None)
}

/// 卡片标题 + 一个跳帮助的问号。
///
/// 只给「数字背后的机制说不清」的卡片用：目标权重怎么来的、为什么只补一条
/// 腿、最大回撤怎么读。每张卡都挂一个问号，等于没有问号。
pub(crate) fn card_header_linked(
    title: impl Into<SharedString>,
    hint: impl Into<SharedString>,
    link: HelpLink,
) -> Div {
    header_with(title.into(), hint.into(), Some(link))
}

fn header_with(title: SharedString, hint: SharedString, link: Option<HelpLink>) -> Div {
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
    if let Some(link) = link {
        header = header.child(help_badge(link));
    }
    // A rule running out of the header to the card edge. Without it the
    // title floats in white space; with it the card reads as a printed
    // page with a running head.
    header.child(div().flex_1().min_w(px(8.)).h(px(1.)).bg(rgb(DIVIDER)))
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

#[cfg(all(test, feature = "gui-test"))]
mod tests {
    // 显式导入：`gpui_kit::*` 里的属性宏 `test` 会在宏展开中盖住内建的
    // `#[test]`，形成无限递归（`src/gui/chart.rs` 踩过同一个坑）。
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui_kit::{
        AppContext, Bounds, Context, IntoElement, ParentElement, Pixels, Render, Styled,
        TestAppContext, Window, div, px, size,
    };

    use super::card_header_linked;
    use crate::gui::help_links;

    /// 把卡片标题行单独渲染出来，交出子元素的几何位置。
    struct HeaderProbe {
        children: Rc<RefCell<Vec<Bounds<Pixels>>>>,
    }

    impl Render for HeaderProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let children = self.children.clone();
            div().w(px(600.)).p(px(20.)).child(
                card_header_linked("配置位置", "当前权重 / 目标权重", help_links::TARGET_WEIGHT)
                    .on_children_prepainted(move |bounds, _, _| {
                        *children.borrow_mut() = bounds;
                    }),
            )
        }
    }

    fn center_y(bounds: Bounds<Pixels>) -> f32 {
        let top: f32 = bounds.origin.y.into();
        let height: f32 = bounds.size.height.into();
        top + height / 2.0
    }

    /// 问号要对齐标题汉字的中线，且落在提示文字右侧。
    ///
    /// 这条测试替代了「靠眼睛看截图」：一行标题里字号有 15 / 12.5 两种，
    /// 对齐方式（baseline 还是 center）会让 16px 的圆偏离标题中线好几个像素，
    /// 而「盒居中」又不等于「墨迹居中」——两者差的正是那 1px。
    #[gpui_kit::test]
    fn help_badge_sits_centered_in_the_card_header(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let children = Rc::new(RefCell::new(Vec::new()));
        let handle = cx.open_window(size(px(600.), px(120.)), |_, _| HeaderProbe {
            children: children.clone(),
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();

        let bounds = children.borrow().clone();
        assert_eq!(
            bounds.len(),
            4,
            "标题、提示、问号、分隔线应各占一个子元素，实际 {}",
            bounds.len()
        );
        let (title, hint, badge) = (bounds[0], bounds[1], bounds[2]);

        let width: f32 = badge.size.width.into();
        let height: f32 = badge.size.height.into();
        assert!(
            (width - 16.0).abs() < 0.6 && (height - 16.0).abs() < 0.6,
            "问号应为 16x16，实际 {width}x{height}"
        );

        // 要对齐的是中文墨迹的中心，而不是行盒的中心：15px 字在 24.5px 行盒里，
        // 墨迹中心比盒子中心低约 1px。徽标因此应落在标题中心下方 1px 附近；
        // 一旦回到「与盒子同心」，肉眼就是之前那个浮在右上角的样子。
        let delta = center_y(badge) - center_y(title);
        assert!(
            (0.3..1.6).contains(&delta),
            "问号中心相对标题中心偏移 {delta:+.2}px，应在 +1px 附近（对齐汉字墨迹中心）"
        );

        assert!(
            badge.origin.x >= hint.origin.x + hint.size.width,
            "问号应排在提示文字右侧，实际 x={:?}，提示右边界={:?}",
            badge.origin.x,
            hint.origin.x + hint.size.width
        );
    }
}
