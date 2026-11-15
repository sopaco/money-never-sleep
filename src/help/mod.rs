//! mns-help: a standalone, CHM-style help viewer for MNS.
//!
//! Left sidebar is the contents tree, the right pane renders markdown pages
//! embedded at compile time (see [`content`]). Back/forward history behaves
//! like a browser. Screenshots resolve at runtime from a `help/` directory
//! next to the executable, or `Resources/help` inside a macOS bundle (see
//! [`render::ImageCache`]).

pub mod content;
#[cfg(target_os = "macos")]
mod macos;
pub mod markdown;
pub mod render;

use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, div, point, px, rgba, rgb, size, AnyWindowHandle, App, AppContext, ClickEvent,
    Context, ElementId, Entity, Global, InteractiveElement, IntoElement, KeyBinding,
    ParentElement, Pixels, QuitMode, Render, ScrollHandle, StatefulInteractiveElement, Styled,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};
// The menu bar is macOS-only in GPUI; the other platforms use the window's own
// system menu (or none at all).
#[cfg(target_os = "macos")]
use gpui_kit::{Menu, MenuItem, SystemMenuType};

use crate::gui::theme::{
    ACCENT, BG, BORDER, DIVIDER, HOVER, LAYER, SELECTED, SURFACE, TEXT, TEXT_TERTIARY, RADIUS,
    RADIUS_SM,
};
use content::{Chapter, Page};

/// 目录树的宽度（逻辑像素）。
const SIDEBAR_WIDTH: f32 = 248.;
/// 正文列的宽度上限（含列内左右内边距）。
///
/// 中文 15px 一行约 50 字，再宽眼睛要来回扫行；但也不能像固定 780px 那样在
/// 宽窗口中只剩中间一条。取 880 是「一屏放得满、又不丢行」的折中。
const COLUMN_MAX: f32 = 880.;
/// 正文列与面板左右边缘之间至少保留的空白。
const PANEL_GUTTER: f32 = 28.;

/// What the menu bar should read while the viewer is frontmost.
///
/// macOS draws the application menu from the bundle's `CFBundleName`, which is
/// "MNS" for every executable inside `MNS.app`. The menu is therefore installed
/// under that name and renamed to this one once it is on screen — see
/// [`macos::rename_app_menu`] for why the rename cannot happen earlier.
#[cfg(target_os = "macos")]
const APP_MENU_NAME: &str = "MNS 帮助中心";

actions!(
    mns_help,
    [
        NavigateBack,
        NavigateForward,
        NavigateHome,
        CloseHelpWindow,
        QuitHelp
    ]
);

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
        // A help window is the whole application: closing it must also end the
        // process, otherwise the single-instance lock outlives the window and
        // the tray's 「帮助中心」 can never open it again.
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run(move |cx| {
            gpui_kit::init(cx);
            crate::gui::theme::apply(cx);

            // GPUI leaves cmd-w / cmd-q to the application (its own tests assert
            // those bindings stay empty), so the viewer must bind them or the
            // window ignores the standard macOS shortcut.
            let mut bindings = vec![
                KeyBinding::new("alt-left", NavigateBack, None),
                KeyBinding::new("alt-right", NavigateForward, None),
            ];
            #[cfg(target_os = "macos")]
            bindings.extend([
                KeyBinding::new("cmd-w", CloseHelpWindow, None),
                KeyBinding::new("cmd-q", QuitHelp, None),
                KeyBinding::new("cmd-left", NavigateBack, None),
                KeyBinding::new("cmd-right", NavigateForward, None),
                KeyBinding::new("cmd-shift-h", NavigateHome, None),
            ]);
            #[cfg(not(target_os = "macos"))]
            bindings.extend([
                KeyBinding::new("ctrl-w", CloseHelpWindow, None),
                KeyBinding::new("alt-f4", CloseHelpWindow, None),
                KeyBinding::new("ctrl-q", QuitHelp, None),
            ]);
            cx.bind_keys(bindings);

            // Also required on macOS: without a menu an item click has nothing
            // to dispatch to, which reads as "the menu button does nothing".
            #[cfg(target_os = "macos")]
            cx.set_menus(help_menus());

            // Keep the default comfortable, but scale with the display instead
            // of pinning a small window on a high-resolution screen.
            let bounds = cx
                .primary_display()
                .map(|display| display.bounds().size)
                .unwrap_or_else(|| size(px(1280.), px(820.)));
            let window_size = size(
                px((bounds.width.as_f32() * 0.78).clamp(1000., 1440.)).min(bounds.width - px(48.)),
                px((bounds.height.as_f32() * 0.80).clamp(640., 980.)).min(bounds.height - px(88.)),
            );

            let options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("MNS 帮助中心".into()),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::centered(window_size, cx)),
                window_min_size: Some(size(px(860.), px(560.))),
                ..Default::default()
            };
            let (window, view) =
                gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| HelpShell::new()))
                    .expect("open help window");
            // The viewer is a standalone process: nothing else will bring it to
            // the front (the tray spawns it as a child), and the menu bar only
            // belongs to the frontmost app.
            cx.activate(true);
            cx.set_global(HelpGlobals {
                view: Some(view),
                window: Some(window),
            });

            cx.on_action(|_: &NavigateBack, cx| {
                if let Some(view) = cx.global::<HelpGlobals>().view.clone() {
                    view.update(cx, |shell, cx| {
                        shell.back();
                        cx.notify();
                    });
                }
            });
            cx.on_action(|_: &NavigateForward, cx| {
                if let Some(view) = cx.global::<HelpGlobals>().view.clone() {
                    view.update(cx, |shell, cx| {
                        shell.forward();
                        cx.notify();
                    });
                }
            });
            cx.on_action(|_: &NavigateHome, cx| {
                if let Some(view) = cx.global::<HelpGlobals>().view.clone() {
                    view.update(cx, |shell, cx| {
                        shell.home();
                        cx.notify();
                    });
                }
            });
            cx.on_action(|_: &CloseHelpWindow, cx| close_help_window(cx));
            cx.on_action(|_: &QuitHelp, cx| cx.quit());

            #[cfg(target_os = "macos")]
            sync_app_menu_name(cx);

            spawn_instance_watcher(cx, instance);
        });
}

/// `cmd-w` / 菜单「关闭窗口」: remove the window, which (with
/// [`QuitMode::LastWindowClosed`]) also ends the process.
///
/// The removal is deferred: menu actions and key bindings are dispatched while
/// the window is still leased to the update that produced the event, and
/// removing it from inside that lease finds no window to update (the removal
/// silently does nothing). `defer` runs once the window has been handed back.
fn close_help_window(cx: &mut App) {
    cx.defer(|cx| {
        let Some(window) = cx.global::<HelpGlobals>().window else {
            return;
        };
        let _ = window.update(cx, |_, window, _| window.remove_window());
    });
}

/// Rename the application menu once AppKit has installed the menu bar.
///
/// AppKit labels the application menu from `CFBundleName` at install time and
/// only repaints it when a later `setTitle:` supplies a *different* string, so
/// this runs slightly after `set_menus` instead of during it. The second pass
/// covers the gap before the app is fully activated on slower machines; both
/// calls are idempotent and cost two msg_sends.
#[cfg(target_os = "macos")]
fn sync_app_menu_name(cx: &mut App) {
    cx.spawn(async move |cx| {
        for delay in [150u64, 900] {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(delay))
                .await;
            let _ = cx.update(|_| macos::rename_app_menu(APP_MENU_NAME));
        }
    })
    .detach();
}

/// Native macOS menu bar for the viewer. Every item maps to an action
/// registered above; an item with no listener is exactly the "click does
/// nothing" symptom.
#[cfg(target_os = "macos")]
fn help_menus() -> Vec<Menu> {
    vec![
        // Installed as the bundle name on purpose: `sync_app_menu_name` only
        // takes effect when the new title differs from this one.
        Menu::new("MNS").items([
            MenuItem::os_submenu("服务", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("退出 MNS 帮助中心", QuitHelp),
        ]),
        Menu::new("浏览").items([
            MenuItem::action("后退", NavigateBack),
            MenuItem::action("前进", NavigateForward),
            MenuItem::action("首页", NavigateHome),
            MenuItem::separator(),
            MenuItem::action("关闭窗口", CloseHelpWindow),
        ]),
    ]
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
            .w(px(SIDEBAR_WIDTH))
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

    fn render_content(&self, panel_width: Pixels, cx: &Context<Self>) -> impl IntoElement {
        let blocks = &self.docs[self.current.0][self.current.1];
        let metrics = content_metrics(panel_width);
        div()
            .id("help-content")
            .v_flex()
            .items_center()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .bg(rgb(BG))
            .child(
                // 正文列居中：宽窗口下两侧留白均分，而不是把空白全甩到右边。
                div()
                    .v_flex()
                    .flex_none()
                    .w(metrics.column)
                    .px(metrics.padding)
                    .pt(px(40.))
                    .pb(px(72.))
                    .child(render::render_document(
                        blocks,
                        &self.images,
                        metrics.text,
                    ))
                    .child(self.render_pager(cx)),
            )
    }

    /// 平铺后的页码（跨章节连续计数），用于「上一页 / 下一页」。
    fn page_position(&self) -> usize {
        content::CHAPTERS[..self.current.0]
            .iter()
            .map(|chapter| chapter.pages.len())
            .sum::<usize>()
            + self.current.1
    }

    /// 把平铺页码还原成 `(chapter, page)`；越界返回 `None`。
    fn page_at(index: usize) -> Option<PageId> {
        let mut remaining = index;
        for (chapter_index, chapter) in content::CHAPTERS.iter().enumerate() {
            if remaining < chapter.pages.len() {
                return Some((chapter_index, remaining));
            }
            remaining -= chapter.pages.len();
        }
        None
    }

    /// 页尾的「上一篇 / 下一篇」。帮助中心是线性读的，读到页尾没有下一步
    /// 就只能回目录里翻，是这套 CHM 式导航最明显的缺口。
    fn render_pager(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let position = self.page_position();
        let prev = position.checked_sub(1).and_then(Self::page_at);
        let next = Self::page_at(position + 1);
        if prev.is_none() && next.is_none() {
            // 只有一页时不该留下一条空的页尾分隔线。
            return div().into_any_element();
        }
        div()
            .h_flex()
            .items_start()
            .w_full()
            .gap(px(12.))
            .mt(px(36.))
            .pt(px(22.))
            .border_t_1()
            .border_color(rgb(DIVIDER))
            .child(match prev {
                Some(id) => pager_card(
                    "上一篇",
                    self.page(id).title,
                    false,
                    cx.listener(move |shell, _, _, _| shell.navigate(id)),
                ),
                None => div().flex_1().into_any_element(),
            })
            .child(match next {
                Some(id) => pager_card(
                    "下一篇",
                    self.page(id).title,
                    true,
                    cx.listener(move |shell, _, _, _| shell.navigate(id)),
                ),
                None => div().flex_1().into_any_element(),
            })
            .into_any_element()
    }
}

impl Render for HelpShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // `viewport_size` 已经是**逻辑像素**：gpui-pre 的 macOS 后端直接取
        // NSView frame（point），Windows 后端返回 DIP。布局单位同样是逻辑
        // 像素，所以这里直接用，不能再除一次 `scale_factor` —— 除两次在
        // Retina 屏上会把正文列压成一半宽（1180pt 窗口只剩 ~260pt 文字宽）。
        let panel_width = window.viewport_size().width - px(SIDEBAR_WIDTH);
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
                    .child(self.render_content(panel_width, cx)),
            )
    }
}

/// 正文列几何，全部是逻辑像素。
struct ContentMetrics {
    /// 整列宽度（含左右内边距）。
    column: Pixels,
    /// 列内左右内边距。
    padding: Pixels,
    /// 文本可用宽度：图片高度、表格换行都按它算。
    text: Pixels,
}

/// 由内容面板宽度推出正文列几何。
///
/// 面板越窄，列越接近 `w_full`，把空间全给正文；面板越宽，列停在
/// [`COLUMN_MAX`] 并居中，两侧留白自然变大。内边距跟着列宽走，避免窄窗口
/// 下内边距吃掉正文、宽窗口下正文贴边。
fn content_metrics(panel_width: Pixels) -> ContentMetrics {
    let available = (panel_width - px(PANEL_GUTTER * 2.)).max(px(300.));
    let column = available.min(px(COLUMN_MAX));
    let padding = px((column.as_f32() * 0.05).clamp(24., 44.));
    ContentMetrics {
        column,
        padding,
        // 图片按文本宽缩放，负数会让它算出一个反向高度。
        text: (column - padding - padding).max(px(160.)),
    }
}

/// 页尾的翻页卡片：一个方向标签 + 目标页标题。
fn pager_card(
    label: &'static str,
    title: &'static str,
    forward: bool,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui_kit::AnyElement {
    let arrow = div()
        .flex_none()
        .text_size(px(15.))
        .text_color(rgb(TEXT_TERTIARY))
        .child(if forward {
            IconName::ArrowRight
        } else {
            IconName::ArrowLeft
        });
    let mut body = div().v_flex().w_full().min_w_0().gap(px(5.));
    if forward {
        // 右对齐：箭头贴右，标签和标题也向右收，两张卡片才对称。
        body = body.items_end();
    }
    let body = body
        .child(
            div()
                .text_size(px(11.5))
                .text_color(rgb(TEXT_TERTIARY))
                .child(label),
        )
        .child(
            div()
                .text_size(px(13.5))
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .text_color(rgb(TEXT))
                .child(title),
        );
    let inner = if forward {
        div()
            .h_flex()
            .items_center()
            .justify_end()
            .gap(px(10.))
            .w_full()
            .child(body)
            .child(arrow)
    } else {
        div()
            .h_flex()
            .items_center()
            .gap(px(10.))
            .w_full()
            .child(arrow)
            .child(body)
    };
    div()
        .id(label)
        .flex_1()
        .min_w_0()
        .p(px(13.))
        .rounded(px(RADIUS))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(SURFACE))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(LAYER)))
        .child(inner)
        .on_click(handler)
        .into_any_element()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 正文列必须吃满内容面板，而不是缩成中间一条。
    ///
    /// 这个数字曾经真的坏过：`viewport_size()` 在 gpui-pre 里已经是逻辑像素，
    /// 早期代码又除了一次 `scale_factor`，Retina 屏上 1180pt 的窗口只剩
    /// ~260pt 的文字宽，整页看起来像被挤在中间。
    #[test]
    fn reading_column_fills_the_content_pane() {
        let panel = px(1180. - SIDEBAR_WIDTH);
        let metrics = content_metrics(panel);
        let text: f32 = metrics.text.into();
        let column: f32 = metrics.column.into();
        let available: f32 = (panel - px(PANEL_GUTTER * 2.)).into();

        assert!(
            (760. ..=820.).contains(&text),
            "1180pt 窗口下正文宽 {text}px，应在 760~820px 之间"
        );
        assert!(
            column <= available + 0.5,
            "正文列 {column}px 超过了扣除留白后的面板宽度 {available}px"
        );
        // 图片高度按文字宽算，宽度塌了图片就会跟着变形。
        assert!(text > 0., "正文宽不能为零");
    }

    /// 宽窗口停在阅读上限并居中，窄窗口把空间全给正文。
    #[test]
    fn reading_column_is_capped_and_degrades_gracefully() {
        let wide = content_metrics(px(2200. - SIDEBAR_WIDTH));
        let wide: f32 = wide.column.into();
        assert!(
            (wide - COLUMN_MAX).abs() < 0.5,
            "2200pt 窗口下正文列应停在 {COLUMN_MAX}px，实际 {wide}px"
        );

        let narrow = content_metrics(px(940. - SIDEBAR_WIDTH));
        let narrow_text: f32 = narrow.text.into();
        let narrow_column: f32 = narrow.column.into();
        assert!(
            narrow_text > 480.,
            "940pt 窗口下正文宽 {narrow_text}px，窄窗口不该再留大边距"
        );
        assert!(
            narrow_column < wide,
            "窄窗口的列宽 {narrow_column}px 应小于宽窗口"
        );
    }
}
