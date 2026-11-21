//! GUI 里每个 `?` 指向哪一页帮助。
//!
//! 两类入口，用途不同，不要混：
//!
//! - **页面级**：窗口顶栏页名旁边那一个，回答「这一屏怎么用」，对应
//!   `desktop-*` 章节；
//! - **概念级**：卡片标题旁的那一个，回答「这个数字是怎么来的」，对应
//!   `strategy-*` / `finance-*` 章节。
//!
//! slug 全部集中在这里，而不是散落在各页面的字符串里：`ALL` 被测试逐条
//! 拿去 `content::page_id` 校验，改文件名时编译能过、测试会先响。

/// 一条帮助链接：目标页 + 悬停时说的那句话。
#[derive(Clone, Copy)]
pub(crate) struct HelpLink {
    pub slug: &'static str,
    pub tip: &'static str,
}

impl HelpLink {
    const fn new(slug: &'static str, tip: &'static str) -> Self {
        Self { slug, tip }
    }
}

// ── 页面级 ─────────────────────────────────────────────────────────────

pub(crate) const OVERVIEW: HelpLink = HelpLink::new("desktop-overview", "帮助：总览与导航");
pub(crate) const PORTFOLIO: HelpLink = HelpLink::new("desktop-portfolio", "帮助：持仓与账本");
pub(crate) const PLAN: HelpLink = HelpLink::new("desktop-plan", "帮助：计划页");
pub(crate) const REVIEW: HelpLink = HelpLink::new("finance-metrics", "帮助：收益与风险指标怎么读");
pub(crate) const REVIEW_PAGE: HelpLink = HelpLink::new("desktop-review", "帮助：巡检怎么用");
pub(crate) const SETTINGS: HelpLink = HelpLink::new("desktop-settings", "帮助：托盘、自启动与设置");

// ── 概念级 ─────────────────────────────────────────────────────────────

/// 配置位置卡：当前权重、目标权重与偏离从哪来。
pub(crate) const TARGET_WEIGHT: HelpLink =
    HelpLink::new("strategy-sentiment", "帮助：目标仓位与偏离怎么算");
/// 分腿分配卡：为什么只补超过带宽的那条腿。
pub(crate) const BAND_PER_LEG: HelpLink =
    HelpLink::new("strategy-rebalance", "帮助：为什么只补超过带宽的腿");
/// 纪律复盘卡：动作预算与冷静期。
pub(crate) const DISCIPLINE: HelpLink =
    HelpLink::new("strategy-rebalance", "帮助：再平衡、定投与冷静期");
/// 风险档位卡：四档的差别与中性档目标。
pub(crate) const RISK_PROFILE: HelpLink =
    HelpLink::new("strategy-sentiment", "帮助：四个风险档位的区别");

/// 测试用清单：新增链接时记得加进来。
#[cfg(test)]
pub(crate) const ALL: &[HelpLink] = &[
    OVERVIEW,
    PORTFOLIO,
    PLAN,
    REVIEW,
    REVIEW_PAGE,
    SETTINGS,
    TARGET_WEIGHT,
    BAND_PER_LEG,
    DISCIPLINE,
    RISK_PROFILE,
];
