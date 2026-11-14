//! mns-help: a standalone, CHM-style help viewer for MNS.
//!
//! Left sidebar is the contents tree, the right pane renders markdown pages
//! embedded at compile time (see [`content`]). Back/forward history behaves
//! like a browser. Screenshots resolve at runtime from a `help/` directory
//! next to the executable (see [`render::ImageCache`]).

pub mod content;
pub mod markdown;
pub mod render;

use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, div, point, px, rgba, rgb, size, AnyWindowHandle, App, AppContext, ClickEvent,
    Context, ElementId, Entity, Global, InteractiveElement, IntoElement, KeyBinding,
    ParentElement, Pixels, Render, ScrollHandle, StatefulInteractiveElement, Styled,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};

use crate::gui::theme::{
    ACCENT, BG, BORDER, HOVER, SELECTED, SURFACE, TEXT, TEXT_TERTIARY, RADIUS_SM,
};
use content::{Chapter, Page};

actions!(mns_help, [NavigateBack, NavigateForward]);

struct HelpGlobals {
    view: Option<Entity<HelpShell>>,
    /// 单实例唤起需要拿到窗口句柄；`App::activate` 在 Windows 上是空实现，
    /// 只有窗口级的 `activate_window` 才能还原并置前。
    window: Option<AnyWindowHandle>,
}

impl Global for HelpGlobals {}

/// Runs the mns-help application (called from `src/bin/mns-help.rs`).
pub fn run() {
    // 单实例：已经在看帮助时再次双击，应该把已有窗口提到前面，而不是多开一个。
    let instance = match crate::single_instance::acquire("mns-help") {
        crate::single_instance::Acquired::Primary(guard) => guard,
        crate::single_instance::Acquired::AlreadyRunning => return,
    };

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            crate::gui::theme::apply(cx);

            cx.bind_keys([
                KeyBinding::new("alt-left", NavigateBack, None),
                KeyBinding::new("alt-right", NavigateForward, None),
            ]);

            // Keep the default comfortable on a normal screen, but never open
            // larger than the primary display — on a 1024px-wide screen a fixed
            // 1080px window would hang off the right edge and clip the column.
            let requested = size(px(1160.), px(740.));
            let bounds = cx
                .primary_display()
                .map(|display| display.bounds().size)
                .unwrap_or(requested);
            let window_size = size(
                px(1160.).min(bounds.width - px(48.)),
                px(740.).min(bounds.height - px(88.)),
            );

            let options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("MNS 帮助中心".into()),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::centered(window_size, cx)),
                window_min_size: Some(size(px(1000.), px(560.))),
                ..Default::default()
            };
            let (window, view) =
                gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| HelpShell::new()))
                    .expect("open help window");
            cx.set_global(HelpGlobals {
                view: Some(view),
                window: Some(window),
            });

            cx.on_action(|_: &NavigateBack, cx| {
                if let Some(view) = cx.global::<HelpGlobals>().view.clone() {
                    view.update(cx, |shell, _| shell.back());
                }
            });
            cx.on_action(|_: &NavigateForward, cx| {
                if let Some(view) = cx.global::<HelpGlobals>().view.clone() {
                    view.update(cx, |shell, _| shell.forward());
                }
            });

            spawn_instance_watcher(cx, instance);
        });
}

/// 轮询「二次启动」信号，把帮助窗口还原并置前。
fn spawn_instance_watcher(cx: &mut App, guard: crate::single_instance::Guard) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(400))
                .await;
            if !guard.wakeup_requested() {
                continue;
            }
            let _ = cx.update(|cx| {
                if let Some(window) = cx.global::<HelpGlobals>().window {
                    let _ = window.update(cx, |_, window, _| window.activate_window());
                }
            });
        }
    })
    .detach();
}

/// A `(chapter, page)` index into [`content::CHAPTERS`].
type PageId = (usize, usize);

pub(crate) struct HelpShell {
    expanded: Vec<bool>,
    current: PageId,
    history: Vec<PageId>,
    cursor: usize,
    docs: Vec<Vec<Vec<markdown::Block>>>,
    images: render::ImageCache,
    scroll: ScrollHandle,
}

impl HelpShell {
    fn new() -> Self {
        let docs = content::CHAPTERS
            .iter()
            .map(|chapter| {
                chapter
                    .pages
                    .iter()
                    .map(|page| markdown::parse(page.body))
                    .collect()
            })
            .collect();
        Self {
            expanded: content::CHAPTERS.iter().map(|_| true).collect(),
            current: (0, 0),
            history: vec![(0, 0)],
            cursor: 0,
            docs,
            images: render::ImageCache::default(),
            scroll: ScrollHandle::new(),
        }
    }

    fn page(&self, id: PageId) -> &'static Page {
        &content::CHAPTERS[id.0].pages[id.1]
    }

    fn chapter(&self, id: PageId) -> &'static Chapter {
        &content::CHAPTERS[id.0]
    }

    fn navigate(&mut self, target: PageId) {
        if target == self.current {
            return;
        }
        self.history.truncate(self.cursor + 1);
        self.history.push(target);
        self.cursor = self.history.len() - 1;
        self.current = target;
        self.expanded[target.0] = true;
        self.scroll.set_offset(point(px(0.), px(0.)));
    }

    fn back(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.current = self.history[self.cursor];
            self.scroll.set_offset(point(px(0.), px(0.)));
        }
    }

    fn forward(&mut self) {
        if self.cursor + 1 < self.history.len() {
            self.cursor += 1;
            self.current = self.history[self.cursor];
            self.scroll.set_offset(point(px(0.), px(0.)));
        }
    }

    fn home(&mut self) {
        self.navigate((0, 0));
    }

    fn render_toolbar(&self, cx: &Context<Self>) -> gpui_kit::Div {
        let can_back = self.cursor > 0;
        let can_forward = self.cursor + 1 < self.history.len();
        div()
            .h_flex()
            .items_center()
            .gap(px(6.))
            .px(px(14.))
            .h(px(50.))
            .flex_none()
            .bg(rgb(SURFACE))
            .border_b_1()
            .border_color(rgb(BORDER))
            // Brand
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap(px(7.))
                    .mr(px(12.))
                    .text_size(px(16.))
                    .text_color(rgb(ACCENT))
                    .child(IconName::BookText),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_color(rgb(TEXT))
                    .child("MNS 帮助中心"),
            )
            // Navigation buttons
            .child(nav_button(
                "nav-back",
                IconName::Undo2,
                "后退",
                can_back,
                cx.listener(|shell, _, _, _| shell.back()),
            ))
            .child(nav_button(
                "nav-forward",
                IconName::Redo2,
                "前进",
                can_forward,
                cx.listener(|shell, _, _, _| shell.forward()),
            ))
            .child(nav_button(
                "nav-home",
                IconName::House,
                "首页",
                true,
                cx.listener(|shell, _, _, _| shell.home()),
            ))
            // Current location breadcrumb
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_flex()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .text_size(px(12.5))
                    .text_color(rgb(TEXT_TERTIARY))
                    .overflow_hidden()
                    .child(self.chapter(self.current).title)
                    .child("/")
                    .child(self.page(self.current).title),
            )
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(TEXT_TERTIARY))
                    .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
            )
    }

    fn render_sidebar(&self, cx: &Context<Self>) -> impl IntoElement {
        let mut sidebar = div()
            .v_flex()
            .w(px(232.))
            .flex_none()
            .h_full()
            .min_h_0()
            .bg(rgb(SURFACE))
            .border_r_1()
            .border_color(rgb(BORDER))
            .id("help-sidebar")
            .overflow_y_scroll()
            .py(px(8.));

        for (chapter_index, chapter) in content::CHAPTERS.iter().enumerate() {
            let expanded = self.expanded[chapter_index];
            sidebar = sidebar.child(
                div()
                    .id(("help-chapter", chapter_index as u64))
                    .h_flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .h(px(34.))
                    .flex_none()
                    .rounded(px(RADIUS_SM))
                    .mx(px(6.))
                    .mt(px(6.))
                    .text_size(px(13.))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(rgb(TEXT))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(HOVER)))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(rgb(TEXT_TERTIARY))
                            .child(if expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(14.))
                            .text_color(rgb(TEXT_TERTIARY))
                            .child(chapter.icon),
                    )
                    .child(chapter.title)
                    .on_click(cx.listener(move |shell, _, _, _| {
                        shell.expanded[chapter_index] = !shell.expanded[chapter_index];
                    })),
            );
            if expanded {
                for (page_index, page) in chapter.pages.iter().enumerate() {
                    let selected = self.current == (chapter_index, page_index);
                    sidebar = sidebar.child(
                        div()
                            .id(("help-page", (chapter_index * 100 + page_index) as u64))
                            .h_flex()
                            .items_center()
                            .ml(px(12.))
                            .mr(px(6.))
                            // Indent to the chapter title, not to the chapter
                            // icon, so the tree reads as one column of text.
                            .pl(px(48.))
                            .h(px(32.))
                            .flex_none()
                            .rounded(px(RADIUS_SM))
                            .text_size(px(12.5))
                            .text_color(if selected {
                                rgb(ACCENT)
                            } else {
                                rgb(TEXT)
                            })
                            .font_weight(if selected {
                                gpui_kit::FontWeight::MEDIUM
                            } else {
                                gpui_kit::FontWeight::NORMAL
                            })
                            .bg(if selected {
                                rgb(SELECTED)
                            } else {
                                // Fully transparent, same Rgba type as above so
                                // the two branches unify and the surface shows.
                                rgba(0x0000_0000)
                            })
                            .when(!selected, |el| {
                                el.cursor_pointer().hover(|style| style.bg(rgb(HOVER)))
                            })
                            .child(page.title)
                            .on_click(cx.listener(move |shell, _, _, _| {
                                shell.navigate((chapter_index, page_index));
                            })),
                    );
                }
            }
        }
        sidebar
    }

    fn render_content(&self, viewport_width: Pixels) -> impl IntoElement {
        let blocks = &self.docs[self.current.0][self.current.1];
        // Window bounds are physical pixels but layout units are logical, so the
        // column must be measured from the (logical) viewport instead of being
        // assumed. `w_full` inside the document then resolves to this definite
        // width, which is what lets paragraphs wrap instead of running wide.
        const SIDEBAR: f32 = 232.;
        let column = (viewport_width - px(SIDEBAR) - px(16.))
            .min(px(780.))
            .max(px(420.));
        div()
            .id("help-content")
            .v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .bg(rgb(BG))
            .child(
                div()
                    .v_flex()
                    .w(column)
                    .px(px(44.))
                    .pt(px(32.))
                    .pb(px(88.))
                    .child(render::render_document(
                        blocks,
                        &self.images,
                        column - px(88.),
                    )),
            )
    }
}

impl Render for HelpShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // `viewport_size` here is in physical pixels while layout units are
        // logical, so divide by the display scale before using it as a width.
        let scale = window.scale_factor();
        let viewport = window.viewport_size();
        let logical_width = px(viewport.width.as_f32() / scale);
        div()
            .id("help-root")
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(BG))
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_sidebar(cx))
                    .child(self.render_content(logical_width)),
            )
    }
}

/// A compact toolbar button with icon + label; renders inert when disabled.
fn nav_button(
    id: impl Into<ElementId>,
    icon: IconName,
    label: &str,
    enabled: bool,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui_kit::AnyElement {
    let mut button = div()
        .id(id)
        .h_flex()
        .items_center()
        .gap(px(5.))
        .px(px(9.))
        .h(px(30.))
        .flex_none()
        .rounded(px(RADIUS_SM))
        .text_size(px(12.5))
        .text_color(if enabled {
            rgb(TEXT)
        } else {
            rgb(TEXT_TERTIARY)
        });
    if enabled {
        button = button
            .cursor_pointer()
            .hover(|style| style.bg(rgb(HOVER)))
            .on_click(handler);
    }
    button
        .child(div().flex_none().text_size(px(14.)).child(icon))
        .child(label.to_string())
        .into_any_element()
}
