//! Help content: a CHM-style tree of chapters and pages.
//!
//! The markdown sources live in `help/` at the repo root and are embedded at
//! compile time, so the shipped `mns-help` binary is self-contained. Screenshots
//! are the exception: they are resolved at runtime from a `help/` directory
//! next to the executable (see [`super::render::ImageCache`]) so they can be
//! added or replaced without a rebuild.

use gpui_kit::assets::IconName;

pub struct Page {
    pub title: &'static str,
    pub body: &'static str,
}

pub struct Chapter {
    pub title: &'static str,
    pub icon: IconName,
    pub pages: &'static [Page],
}

pub const CHAPTERS: &[Chapter] = &[
    Chapter {
        title: "开始",
        icon: IconName::BookOpen,
        pages: &[Page {
            title: "欢迎使用 MNS 帮助中心",
            body: include_str!("../../help/start.md"),
        }],
    },
    Chapter {
        title: "MNS 策略系统",
        icon: IconName::Gauge,
        pages: &[
            Page {
                title: "核心理念",
                body: include_str!("../../help/strategy-philosophy.md"),
            },
            Page {
                title: "情绪指数与目标仓位",
                body: include_str!("../../help/strategy-sentiment.md"),
            },
            Page {
                title: "再平衡、定投与冷静期",
                body: include_str!("../../help/strategy-rebalance.md"),
            },
        ],
    },
    Chapter {
        title: "桌面端使用",
        icon: IconName::PanelLeft,
        pages: &[
            Page {
                title: "总览与导航",
                body: include_str!("../../help/desktop-overview.md"),
            },
            Page {
                title: "持仓与账本",
                body: include_str!("../../help/desktop-portfolio.md"),
            },
            Page {
                title: "计划页",
                body: include_str!("../../help/desktop-plan.md"),
            },
            Page {
                title: "AI 助手",
                body: include_str!("../../help/desktop-agent.md"),
            },
            Page {
                title: "托盘、自启动与设置",
                body: include_str!("../../help/desktop-settings.md"),
            },
        ],
    },
    Chapter {
        title: "CLI 使用",
        icon: IconName::Terminal,
        pages: &[
            Page {
                title: "快速上手",
                body: include_str!("../../help/cli-quickstart.md"),
            },
            Page {
                title: "命令参考",
                body: include_str!("../../help/cli-reference.md"),
            },
            Page {
                title: "回测与历史审计",
                body: include_str!("../../help/cli-backtest.md"),
            },
        ],
    },
    Chapter {
        title: "理财百科",
        icon: IconName::Coins,
        pages: &[
            Page {
                title: "导读：三个前提",
                body: include_str!("../../help/finance-intro.md"),
            },
            Page {
                title: "风险收益阶梯",
                body: include_str!("../../help/finance-ladder.md"),
            },
            Page {
                title: "市场基准与购买约束",
                body: include_str!("../../help/finance-benchmarks.md"),
            },
            Page {
                title: "收益与风险指标",
                body: include_str!("../../help/finance-metrics.md"),
            },
            Page {
                title: "现金与存款",
                body: include_str!("../../help/finance-cash-deposit.md"),
            },
            Page {
                title: "储蓄国债与银行理财",
                body: include_str!("../../help/finance-bonds-wm.md"),
            },
            Page {
                title: "保险与个人养老金",
                body: include_str!("../../help/finance-insurance.md"),
            },
            Page {
                title: "债券、混合与股票基金",
                body: include_str!("../../help/finance-funds.md"),
            },
            Page {
                title: "跨境 QDII 与美元理财",
                body: include_str!("../../help/finance-qdii.md"),
            },
            Page {
                title: "黄金、REITs 与另类",
                body: include_str!("../../help/finance-alternatives.md"),
            },
            Page {
                title: "关键概念辨析",
                body: include_str!("../../help/finance-concepts.md"),
            },
            Page {
                title: "常见误区清单",
                body: include_str!("../../help/finance-mistakes.md"),
            },
            Page {
                title: "给普通人的实用指南",
                body: include_str!("../../help/finance-guide.md"),
            },
        ],
    },
];
