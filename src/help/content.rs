//! Help content: a CHM-style tree of chapters and pages.
//!
//! The markdown sources live in `help/` at the repo root and embedded at
//! compile time, so the shipped `mns-help` binary is self-contained. Screenshots
//! are the exception: they are resolved at runtime from a `help/` directory
//! next to the executable (see [`super::render::ImageCache`]) so they can be
//! added or replaced without a rebuild.
//!
//! Every page carries a stable slug — the markdown file stem — so other parts
//! of the product can link to it (`mns-help --page strategy-sentiment`, the
//! `?` badges in the desktop shell). Renaming a file therefore renames the
//! link target; the [`tests`] below keep both ends honest.

use gpui_kit::assets::IconName;

pub struct Page {
    /// 稳定标识：与 `help/<slug>.md` 的文件名一致，供深链使用。
    pub slug: &'static str,
    pub title: &'static str,
    pub body: &'static str,
}

pub struct Chapter {
    pub title: &'static str,
    pub icon: IconName,
    pub pages: &'static [Page],
}

/// 一页 = 一个 slug + 一个标题，正文按 slug 从 `help/` 取。
///
/// slug 与文件名合并成同一个字面量，是为了让「链接目标」和「磁盘上的文件」
/// 不可能写岔：多打一个字母就是编译期找不到文件，而不是运行时静默跳到首页。
macro_rules! page {
    ($slug:literal, $title:literal) => {
        Page {
            slug: $slug,
            title: $title,
            body: include_str!(concat!("../../help/", $slug, ".md")),
        }
    };
}

pub const CHAPTERS: &[Chapter] = &[
    Chapter {
        title: "开始",
        icon: IconName::BookOpen,
        pages: &[page!("start", "欢迎使用 MNS 帮助中心")],
    },
    Chapter {
        title: "MNS 策略系统",
        icon: IconName::Gauge,
        pages: &[
            page!("strategy-philosophy", "核心理念"),
            page!("strategy-sentiment", "情绪指数与目标仓位"),
            page!("strategy-rebalance", "再平衡、定投与冷静期"),
        ],
    },
    Chapter {
        title: "桌面端使用",
        icon: IconName::PanelLeft,
        pages: &[
            page!("desktop-overview", "总览与导航"),
            page!("desktop-portfolio", "持仓与账本"),
            page!("desktop-plan", "计划页"),
            page!("desktop-agent", "AI 助手"),
            page!("desktop-settings", "托盘、自启动与设置"),
        ],
    },
    Chapter {
        title: "CLI 使用",
        icon: IconName::Terminal,
        pages: &[
            page!("cli-quickstart", "快速上手"),
            page!("cli-reference", "命令参考"),
            page!("cli-backtest", "回测与历史审计"),
        ],
    },
    Chapter {
        title: "理财百科",
        icon: IconName::Coins,
        pages: &[
            page!("finance-intro", "导读：三个前提"),
            page!("finance-ladder", "风险收益阶梯"),
            page!("finance-benchmarks", "市场基准与购买约束"),
            page!("finance-metrics", "收益与风险指标"),
            page!("finance-cash-deposit", "现金与存款"),
            page!("finance-bonds-wm", "储蓄国债与银行理财"),
            page!("finance-insurance", "保险与个人养老金"),
            page!("finance-funds", "债券、混合与股票基金"),
            page!("finance-qdii", "跨境 QDII 与美元理财"),
            page!("finance-alternatives", "黄金、REITs 与另类"),
            page!("finance-concepts", "关键概念辨析"),
            page!("finance-mistakes", "常见误区清单"),
            page!("finance-guide", "给普通人的实用指南"),
        ],
    },
];

/// `(chapter, page)` 索引，与 [`super::PageId`] 同构。
type PageId = (usize, usize);

/// 按 slug 找页；找不到返回 `None`（调用方决定是回首页还是报错）。
pub fn page_id(slug: &str) -> Option<PageId> {
    if slug.is_empty() {
        return None;
    }
    CHAPTERS
        .iter()
        .enumerate()
        .find_map(|(chapter_index, chapter)| {
            chapter
                .pages
                .iter()
                .position(|page| page.slug == slug)
                .map(|page_index| (chapter_index, page_index))
        })
}

/// 按 slug 取页，用于给链接生成标题（悬停提示、面包屑）。
pub fn page(slug: &str) -> Option<&'static Page> {
    let (chapter, page) = page_id(slug)?;
    CHAPTERS.get(chapter)?.pages.get(page)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// slug 是深链的唯一契约：重复会让 `page_id` 只认得第一个，
    /// 大小写或空格混进来则会让 `--page` 在命令行里悄悄失效。
    #[test]
    fn slugs_are_unique_kebab_case() {
        let mut seen = Vec::new();
        for chapter in CHAPTERS {
            for page in chapter.pages {
                assert!(
                    !page.slug.is_empty() && page.slug.split('-').all(|part| !part.is_empty()),
                    "{} 的 slug `{}` 不是 kebab-case",
                    page.title,
                    page.slug
                );
                assert!(
                    page.slug
                        .chars()
                        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-'),
                    "{} 的 slug `{}` 含非常规字符",
                    page.title,
                    page.slug
                );
                assert!(
                    !seen.contains(&page.slug),
                    "slug `{}` 重复，`--page` 会指向错误的一页",
                    page.slug
                );
                seen.push(page.slug);
            }
        }
    }

    /// 每页都要能被自己的 slug 找回来——这是 `?` 徽标、`--page` 参数和
    /// 帮助窗口轮询共用的入口，任何一处错位都会表现为「点了跳到首页」。
    #[test]
    fn every_page_resolves_from_its_slug() {
        for (chapter_index, chapter) in CHAPTERS.iter().enumerate() {
            for (page_index, page) in chapter.pages.iter().enumerate() {
                assert_eq!(
                    page_id(page.slug),
                    Some((chapter_index, page_index)),
                    "slug `{}` 没找回自己",
                    page.slug
                );
                assert_eq!(
                    page_of(page.slug).map(|found| found.title),
                    Some(page.title)
                );
            }
        }
        assert_eq!(page_id(""), None);
        assert_eq!(page_id("no-such-page"), None);
    }

    fn page_of(slug: &str) -> Option<&'static Page> {
        page(slug)
    }
}
