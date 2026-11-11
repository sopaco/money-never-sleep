//! 纪律复盘：把交易流水按 note 前缀分桶，回答「我今年有没有乱动」。
//!
//! 这里是**纯统计**，不评价某一笔该不该交易，也不据此给出买卖建议。
//! 分桶只依据护栏写入的结构化前缀：
//!
//!   - 「计划外冷静：」→ 计划外（(a) 不在定投∪再平衡计划里，或 (b) 金额异常放大）；
//!   - 「过热冷静：」→ 过热冷静（纯 (c)：按计划买入，只是当时情绪过热）；
//!   - 没有前缀（含空 note）→ 未标记。
//!
//! 「未标记」不推断当时的意图：JSON 里没有证据，硬猜会把「按计划执行」
//! 或「CLI 买入」误报成纪律问题。UI 必须原样说明这一点。

use chrono::{Datelike, NaiveDate};

/// (a)/(b) 触发冷静期后写入的前缀（计划外买入）。
pub const OUT_OF_PLAN_PREFIX: &str = "计划外冷静：";
/// 纯 (c) 过热但按计划买入、用户主动写了理由时写入的前缀。
pub const HOT_CALM_PREFIX: &str = "过热冷静：";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteBucket {
    /// 计划外买入（护栏 (a)/(b)）。
    OutOfPlan,
    /// 过热但按计划买入（护栏 (c)）。
    HotCalm,
    /// 没有结构化前缀，无法判断当时意图。
    Unmarked,
}

impl NoteBucket {
    pub fn label(self) -> &'static str {
        match self {
            Self::OutOfPlan => "计划外",
            Self::HotCalm => "过热冷静",
            Self::Unmarked => "未标记",
        }
    }
}

/// 只认开头的前缀。用户手写的「补记计划外冷静：…」不是护栏产物，
/// 不能因为正文里出现这几个字就被算成计划外。
pub fn bucket_of(note: Option<&str>) -> NoteBucket {
    match note.map(str::trim) {
        Some(note) if note.starts_with(OUT_OF_PLAN_PREFIX) => NoteBucket::OutOfPlan,
        Some(note) if note.starts_with(HOT_CALM_PREFIX) => NoteBucket::HotCalm,
        _ => NoteBucket::Unmarked,
    }
}

/// 某一自然年的交易统计。
#[derive(Debug, Clone, PartialEq)]
pub struct YearReview {
    pub year: i32,
    /// 本年操作总笔数（买 + 卖）。
    pub total: usize,
    pub out_of_plan: usize,
    pub hot_calm: usize,
    pub unmarked: usize,
    /// 年度交易预算区间，来自 `config.backtest`；只用于展示，不阻断。
    pub budget_min: u32,
    pub budget_max: u32,
    /// 本年最近一次操作日期；没有操作时为 None。
    pub last_trade: Option<NaiveDate>,
}

impl YearReview {
    /// 超过预算上限。等于上限不算超——预算是「10–20 次」，20 次仍在内。
    pub fn over_budget(&self) -> bool {
        self.total > self.budget_max as usize
    }

    /// 距离预算上限还剩几次，已超则为 0。
    pub fn remaining(&self) -> usize {
        (self.budget_max as usize).saturating_sub(self.total)
    }
}

/// 统计 `year` 年的交易。不在该年的记录直接忽略，调用方不必先过滤。
pub fn review(
    year: i32,
    budget_min: u32,
    budget_max: u32,
    trades: impl IntoIterator<Item = (NaiveDate, Option<String>)>,
) -> YearReview {
    let mut review = YearReview {
        year,
        total: 0,
        out_of_plan: 0,
        hot_calm: 0,
        unmarked: 0,
        budget_min,
        budget_max,
        last_trade: None,
    };
    for (date, note) in trades {
        if date.year() != year {
            continue;
        }
        review.total += 1;
        match bucket_of(note.as_deref()) {
            NoteBucket::OutOfPlan => review.out_of_plan += 1,
            NoteBucket::HotCalm => review.hot_calm += 1,
            NoteBucket::Unmarked => review.unmarked += 1,
        }
        review.last_trade = Some(match review.last_trade {
            Some(seen) => seen.max(date),
            None => date,
        });
    }
    review
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn tx(y: i32, m: u32, d: u32, note: Option<&str>) -> (NaiveDate, Option<String>) {
        (date(y, m, d), note.map(str::to_owned))
    }

    #[test]
    fn 前缀分桶只认开头且大小写与正文无关() {
        assert_eq!(
            bucket_of(Some("计划外冷静：看到回调想补一笔")),
            NoteBucket::OutOfPlan
        );
        assert_eq!(
            bucket_of(Some("过热冷静：按计划执行但分两批")),
            NoteBucket::HotCalm
        );
        // 前缀只在正文中间出现，不算护栏产物。
        assert_eq!(
            bucket_of(Some("补记计划外冷静：旧流水")),
            NoteBucket::Unmarked
        );
        // 空 note 与 None 同等对待。
        assert_eq!(bucket_of(None), NoteBucket::Unmarked);
        assert_eq!(bucket_of(Some("   ")), NoteBucket::Unmarked);
        assert_eq!(bucket_of(Some("随手买的")), NoteBucket::Unmarked);
    }

    #[test]
    fn 本年统计只数本年且分桶加总等于总数() {
        let r = review(
            2026,
            10,
            20,
            vec![
                tx(2026, 1, 5, Some("计划外冷静：追高")),
                tx(2026, 3, 9, Some("过热冷静：按计划分两批")),
                tx(2026, 3, 9, None),
                tx(2026, 7, 1, Some("卖出补现金")),
                tx(2025, 12, 31, Some("计划外冷静：去年的事")),
                tx(2027, 1, 2, Some("计划外冷静：明年的事")),
            ],
        );
        assert_eq!(r.year, 2026);
        assert_eq!(r.total, 4, "跨年记录不应计入");
        assert_eq!(r.out_of_plan, 1);
        assert_eq!(r.hot_calm, 1);
        assert_eq!(r.unmarked, 2);
        assert_eq!(
            r.out_of_plan + r.hot_calm + r.unmarked,
            r.total,
            "每一笔都必须落进一个桶，不能有遗漏"
        );
    }

    #[test]
    fn 最近一次操作取本年最大日期且与输入顺序无关() {
        let ordered = review(
            2026,
            10,
            20,
            vec![
                tx(2026, 2, 1, None),
                tx(2026, 9, 30, None),
                tx(2026, 5, 20, None),
            ],
        );
        let shuffled = review(
            2026,
            10,
            20,
            vec![
                tx(2026, 9, 30, None),
                tx(2026, 5, 20, None),
                tx(2026, 2, 1, None),
            ],
        );
        assert_eq!(ordered.last_trade, Some(date(2026, 9, 30)));
        assert_eq!(ordered, shuffled);
    }

    #[test]
    fn 空年份给出零值而不是伪造最近一次操作() {
        let r = review(2026, 10, 20, Vec::new());
        assert_eq!(r.total, 0);
        assert_eq!(r.last_trade, None);
        assert!(!r.over_budget());
        assert_eq!(r.remaining(), 20);
    }

    #[test]
    fn 预算上限边界_eq_不超_gt_才超() {
        let at_limit = (0..20)
            .map(|_| tx(2026, 1, 1, None))
            .collect::<Vec<_>>();
        let r = review(2026, 10, 20, at_limit);
        assert_eq!(r.total, 20);
        assert!(!r.over_budget(), "正好用满 20 次仍在内");
        assert_eq!(r.remaining(), 0);

        let over = (0..21).map(|_| tx(2026, 1, 1, None)).collect::<Vec<_>>();
        let r = review(2026, 10, 20, over);
        assert!(r.over_budget(), "21 次应当超预算");
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn 与护栏落库前缀保持同一来源() {
        // build_note 用这两个常量拼接；这里断言拼接结果能被正确分桶，
        // 防止哪天改了一处文案而另一处没跟着改。
        let out = format!("{OUT_OF_PLAN_PREFIX}理由");
        let hot = format!("{HOT_CALM_PREFIX}理由");
        assert_eq!(bucket_of(Some(&out)), NoteBucket::OutOfPlan);
        assert_eq!(bucket_of(Some(&hot)), NoteBucket::HotCalm);
    }
}
