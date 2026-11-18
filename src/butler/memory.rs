//! 管家的轻量记忆。
//!
//! 三条硬约束，都是为了「记错不如不记」：
//!
//! 1. 每次会话最多 3 条要点，不存全文堆栈——摘要膨胀之后，管家会开始引用
//!    自己上次的措辞，而不是事实；
//! 2. 事实带 `expires_at`，过期自动归档；
//! 3. 整理只做合并与归档，**不做不可逆删除**：误删用户承诺是不可修复的，
//!    用户说「忘掉」才真的移出。

use anyhow::Result;
use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};

use super::butler_dir;
use super::store::{read_json, write_json};

/// 事实条目上限。超过后先归档过期条目，再归档最久没被用到的那条。
pub const MAX_FACTS: usize = 100;
/// 会话摘要条数上限。
pub const MAX_SUMMARIES: usize = 20;
/// 单次会话最多提炼的要点数。
pub const MAX_POINTS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactKind {
    /// 用户偏好：称呼、语气、风险自述。
    Preference,
    /// 承诺事项：「周五补美股」。
    Commitment,
    /// 关键时间线：发薪日、限购恢复日。
    Timeline,
    /// 反复出现的行为模式（由每周整理升格）。
    Pattern,
}

impl FactKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Preference => "偏好",
            Self::Commitment => "承诺",
            Self::Timeline => "时间线",
            Self::Pattern => "行为模式",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    pub id: String,
    pub kind: FactKind,
    pub text: String,
    pub created_at: DateTime<Local>,
    /// 到期日；到期后不再参与表达，转入归档。
    pub expires_at: Option<NaiveDate>,
    /// 被引用次数。整理时优先保留「常用」的记忆。
    #[serde(default)]
    pub hits: u32,
}

impl Fact {
    pub fn new(kind: FactKind, text: impl Into<String>, now: DateTime<Local>) -> Self {
        let text = text.into();
        Self {
            id: format!("f-{}", now.timestamp_millis()),
            kind,
            text,
            created_at: now,
            expires_at: None,
            hits: 0,
        }
    }

    pub fn expires_on(mut self, day: NaiveDate) -> Self {
        self.expires_at = Some(day);
        self
    }

    pub fn is_expired(&self, today: NaiveDate) -> bool {
        self.expires_at.is_some_and(|day| day < today)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    pub id: String,
    pub at: DateTime<Local>,
    pub points: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Memory {
    pub facts: Vec<Fact>,
    pub summaries: Vec<Summary>,
    /// 归档不是删除：用户还能查到「管家忘掉了什么」。
    pub archived_facts: Vec<Fact>,
    pub archived_summaries: Vec<Summary>,
}

/// 用户一句话里可以确定性识别的记忆意图。
///
/// 只认明确的动词前缀（「记住」「忘掉」），不做模糊猜测：把「周五要不要补仓」
/// 误当成「记住周五补仓」，比不记更糟。识别不了就交回普通问答。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    Remember {
        kind: FactKind,
        text: String,
        due: Option<NaiveDate>,
    },
    Forget {
        needle: String,
    },
}

/// 记住类前缀，长前缀必须排在短前缀前面（`strip_prefix_any` 按顺序匹配）。
const REMEMBER_PREFIXES: [&str; 5] = ["帮我记住", "记住", "记一下", "别忘了", "记下"];
/// 忘掉类前缀。
const FORGET_PREFIXES: [&str; 4] = ["帮我忘掉", "忘掉", "忘记", "不用记"];

/// 解析「记住… / 忘掉…」。返回 `None` 表示这句话不是在改记忆。
pub fn parse_intent(text: &str) -> Option<Intent> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(rest) = strip_prefix_any(text, &FORGET_PREFIXES) {
        let needle = trim_tail(rest.trim_start_matches(['：', ':', '，', ',', ' ']));
        if needle.is_empty() || starts_with_particle(needle) {
            return None;
        }
        return Some(Intent::Forget {
            needle: needle.to_owned(),
        });
    }
    let rest = strip_prefix_any(text, &REMEMBER_PREFIXES)?;
    let body = trim_tail(rest.trim_start_matches(['：', ':', '，', ',', ' ']));
    if body.is_empty() || starts_with_particle(body) {
        return None;
    }
    let (body, due) = extract_due(body);
    let body = body.trim();
    if body.is_empty() || starts_with_particle(body) {
        return None;
    }
    Some(Intent::Remember {
        kind: classify(body),
        text: body.to_owned(),
        due,
    })
}

/// 按内容猜一个最小类别。猜错只会影响表达层怎么提这件事，不影响事实本身。
pub fn classify(text: &str) -> FactKind {
    if contains_any(text, &["称呼", "叫我", "语气", "偏好", "别用", "别叫"]) {
        FactKind::Preference
    } else if contains_any(
        text,
        &["发薪", "发工资", "限购", "到期", "恢复", "每月", "每年"],
    ) {
        FactKind::Timeline
    } else {
        FactKind::Commitment
    }
}

fn contains_any(text: &str, words: &[&str]) -> bool {
    words.iter().any(|word| text.contains(word))
}

fn strip_prefix_any<'a>(text: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    prefixes
        .iter()
        .find_map(|prefix| text.strip_prefix(prefix))
}

fn trim_tail(text: &str) -> &str {
    text.trim_end_matches(['。', '.', '！', '!', '？', '?', ' '])
}

/// 「记住吗」「记住？」这类反问不是指令，不能把后半句当事实记下来。
fn starts_with_particle(text: &str) -> bool {
    text.starts_with(['吗', '么', '嘛', '？', '?'])
}

/// 文本里出现 `YYYY-MM-DD` 时视为到期日，并从正文里摘掉。
fn extract_due(text: &str) -> (String, Option<NaiveDate>) {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() >= 10 {
        for start in 0..=chars.len() - 10 {
            let candidate: String = chars[start..start + 10].iter().collect();
            if let Ok(day) = NaiveDate::parse_from_str(&candidate, "%Y-%m-%d") {
                let mut rest: String = chars[..start].iter().collect();
                rest.extend(chars[start + 10..].iter());
                let rest = rest.replace("截止", "");
                return (rest.trim().to_owned(), Some(day));
            }
        }
    }
    (text.to_owned(), None)
}

/// 记住一条事实的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remembered {
    /// 新条目。
    Added,
    /// 已有同义条目：合并为引用计数，不重复堆叠。
    Merged,
    /// 新增后触发归档（条数上限）。
    AddedAndArchived,
}

/// 一次性整理的结果，写进审计日志。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Compaction {
    pub merged: usize,
    pub expired: usize,
    pub summaries_archived: usize,
}

impl Memory {
    pub fn path() -> std::path::PathBuf {
        butler_dir().join("memory").join("memory.json")
    }

    pub fn load() -> Result<Self> {
        Ok(read_json(&Self::path())?.unwrap_or_default())
    }

    pub fn save(&self) -> Result<()> {
        write_json(&Self::path(), self)
    }

    /// 「重读-修改-落盘」，全程持写锁。
    ///
    /// `memory.json` 有三个写者（GUI 管家窗口、`mns-butlerd` 自整理、CLI
    /// `remember` / `forget`）。管家窗口一开几个小时，拿打开时的快照在
    /// 每轮对话后整体覆盖，会把期间 CLI 写进去的事实吞掉——所以每次变更
    /// 都从磁盘重读，只把这一笔变更叠上去。
    pub fn update<R>(apply: impl FnOnce(&mut Self) -> R) -> Result<(Self, R)> {
        // 拿不到锁就无锁继续（调用方语义不能因锁故障而丢「记住」）；
        // 临时值存活到语句结束，load→apply→save 全程持锁。
        let guard = super::writer_lock();
        Self::update_at(&Self::path(), guard.as_ref(), apply)
    }

    /// 路径可注入的版本：测试用 scratch 目录验证「重读再改」的合并语义，
    /// 不碰真实账户目录。`guard` 由调用方持有——跨多文件的读-改-写共享
    /// 同一把锁时，锁的边界归最外层。
    pub fn update_at<R>(
        path: &std::path::Path,
        guard: Option<&super::store::WriterLock>,
        apply: impl FnOnce(&mut Self) -> R,
    ) -> Result<(Self, R)> {
        let _ = guard;
        let mut memory: Self = read_json(path)?.unwrap_or_default();
        let result = apply(&mut memory);
        write_json(path, &memory)?;
        Ok((memory, result))
    }

    /// 最近一条还没到期的承诺的到期日（含今天）。
    ///
    /// 守护进程拿它算「哪一天该醒来提这件事」——承诺到期是独立唤醒源，
    /// 不该等下一次 4 小时巡检，也不该等 GUI 恰好那天被打开。
    pub fn next_commitment(&self, today: NaiveDate) -> Option<NaiveDate> {
        self.facts
            .iter()
            .filter(|fact| fact.kind == FactKind::Commitment)
            .filter(|fact| !fact.is_expired(today))
            .filter_map(|fact| fact.expires_at)
            .filter(|date| *date >= today)
            .min()
    }

    /// 未过期的事实，供表达层使用。
    pub fn active_facts(&self, today: NaiveDate) -> Vec<&Fact> {
        self.facts
            .iter()
            .filter(|fact| !fact.is_expired(today))
            .collect()
    }

    /// 记住一条事实。同义条目合并（`hits += 1`），不新增重复行。
    pub fn remember(&mut self, fact: Fact) -> Remembered {
        let normalized = normalize(&fact.text);
        if let Some(existing) = self
            .facts
            .iter_mut()
            .find(|existing| normalize(&existing.text) == normalized)
        {
            existing.hits = existing.hits.saturating_add(1);
            // 新条目带了更晚的到期日，就延长——用户又说了一次，说明还有效。
            if fact.expires_at > existing.expires_at {
                existing.expires_at = fact.expires_at;
            }
            return Remembered::Merged;
        }

        self.facts.push(fact);
        if self.enforce_fact_cap() {
            Remembered::AddedAndArchived
        } else {
            Remembered::Added
        }
    }

    /// 用户说「忘掉这件事」。命中即移入归档（可查），返回命中条数。
    pub fn forget(&mut self, needle: &str) -> Vec<Fact> {
        let needle = normalize(needle);
        let (hit, keep): (Vec<Fact>, Vec<Fact>) = self
            .facts
            .drain(..)
            .partition(|fact| normalize(&fact.text).contains(&needle));
        self.facts = keep;
        self.archived_facts.extend(hit.iter().cloned());
        hit
    }

    /// 记录一次会话摘要。要点超过 3 条直接截断——多出来的通常是自我复述。
    pub fn summarize(&mut self, at: DateTime<Local>, points: Vec<String>) {
        let mut points: Vec<String> = points
            .into_iter()
            .map(|point| point.trim().to_owned())
            .filter(|point| !point.is_empty())
            .collect();
        points.truncate(MAX_POINTS);
        if points.is_empty() {
            return;
        }
        self.summaries.push(Summary {
            id: format!("s-{}", at.timestamp_millis()),
            at,
            points,
        });
        while self.summaries.len() > MAX_SUMMARIES {
            let oldest = self.summaries.remove(0);
            self.archived_summaries.push(oldest);
        }
    }

    /// 每周自整理：合并同义事实、归档过期条目、压掉最早的摘要。
    ///
    /// 只合并不删除；重复条目保留 `hits` 之和，这样「用户反复说过的事」在
    /// 下一次整理里仍然不会被当成冷条目。
    pub fn compact(&mut self, today: NaiveDate) -> Compaction {
        let mut report = Compaction::default();

        let mut merged: Vec<Fact> = Vec::new();
        for fact in self.facts.drain(..) {
            let key = normalize(&fact.text);
            if let Some(existing) = merged.iter_mut().find(|row| normalize(&row.text) == key) {
                existing.hits = existing.hits.saturating_add(fact.hits.max(1));
                if fact.expires_at > existing.expires_at {
                    existing.expires_at = fact.expires_at;
                }
                report.merged += 1;
            } else {
                merged.push(fact);
            }
        }

        let (expired, alive): (Vec<Fact>, Vec<Fact>) = merged
            .into_iter()
            .partition(|fact| fact.is_expired(today));
        report.expired = expired.len();
        self.archived_facts.extend(expired);
        self.facts = alive;

        while self.summaries.len() > MAX_SUMMARIES {
            let oldest = self.summaries.remove(0);
            self.archived_summaries.push(oldest);
            report.summaries_archived += 1;
        }

        report
    }

    /// 把反复出现的行为（同一 `FactKind` 命中 ≥3 次）升格为模式，供表达层
    /// 主动提起用户自己的习惯。
    pub fn promote_patterns(&mut self, pattern_text: &str, now: DateTime<Local>) -> bool {
        if self
            .facts
            .iter()
            .any(|fact| fact.kind == FactKind::Pattern && normalize(&fact.text) == normalize(pattern_text))
        {
            return false;
        }
        self.remember(Fact::new(FactKind::Pattern, pattern_text, now));
        true
    }

    fn enforce_fact_cap(&mut self) -> bool {
        if self.facts.len() <= MAX_FACTS {
            return false;
        }
        // 先腾最久没被引用的：hits 少的优先，再按创建时间从早到晚。
        self.facts.sort_by(|a, b| {
            a.hits
                .cmp(&b.hits)
                .then_with(|| a.created_at.cmp(&b.created_at))
        });
        let evicted = self.facts.remove(0);
        self.facts.sort_by_key(|fact| fact.created_at);
        self.archived_facts.push(evicted);
        true
    }

    /// 共情用的最小人设素材：称呼与语气偏好。
    pub fn address_name(&self) -> Option<&str> {
        self.facts
            .iter()
            .find(|fact| fact.kind == FactKind::Preference && fact.text.contains("称呼"))
            .map(|fact| fact.text.as_str())
    }
}

/// 归一化：去空白、去标点差异，用于判断「是不是同一件事」。
fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            '，' | ',' | '。' | '.' | '！' | '!' | '？' | '?' => ' ',
            other => other,
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 11, 16, 9, 0, 0).single().unwrap()
    }

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, d).unwrap()
    }

    #[test]
    fn 同义事实合并而不是堆叠() {
        let mut memory = Memory::default();
        assert_eq!(
            memory.remember(Fact::new(FactKind::Commitment, "周五补美股", now())),
            Remembered::Added
        );
        assert_eq!(
            memory.remember(Fact::new(FactKind::Commitment, "周五 补美股。", now())),
            Remembered::Merged
        );
        assert_eq!(memory.facts.len(), 1);
        assert_eq!(memory.facts[0].hits, 1);
    }

    #[test]
    fn 忘掉会把条目移入归档而不是消失() {
        let mut memory = Memory::default();
        memory.remember(Fact::new(FactKind::Commitment, "周五补美股", now()));
        memory.remember(Fact::new(FactKind::Preference, "称呼我老王", now()));
        let hit = memory.forget("补美股");
        assert_eq!(hit.len(), 1);
        assert_eq!(memory.facts.len(), 1);
        assert_eq!(memory.archived_facts.len(), 1, "归档可查");
    }

    #[test]
    fn 会话摘要最多三条且只留最近二十次() {
        let mut memory = Memory::default();
        memory.summarize(
            now(),
            vec![
                "一".to_owned(),
                "二".to_owned(),
                "三".to_owned(),
                "四".to_owned(),
            ],
        );
        assert_eq!(memory.summaries[0].points.len(), MAX_POINTS);

        for index in 0..MAX_SUMMARIES + 5 {
            memory.summarize(now(), vec![format!("第 {index} 次")]);
        }
        // 首条 + 25 条 = 26 条，超出上限的 6 条进归档。
        assert_eq!(memory.summaries.len(), MAX_SUMMARIES);
        assert_eq!(memory.archived_summaries.len(), 6);
    }

    #[test]
    fn 整理只合并不删除且过期条目转为归档() {
        let mut memory = Memory::default();
        // 直接写进 `facts`：`remember` 已经会合并同义条目，这里模拟旧版本落盘
        // 的重复数据（或归一化规则变化后才出现的重复）。
        memory.facts.push(Fact::new(FactKind::Timeline, "发薪日 15 号", now()));
        memory.facts.push(Fact::new(FactKind::Timeline, "发薪日15号", now()));
        memory.facts.push(
            Fact::new(FactKind::Commitment, "月底补 A 股", now()).expires_on(day(10)),
        );

        let report = memory.compact(day(20));
        assert_eq!(report.merged, 1, "同义条目应合并");
        assert_eq!(report.expired, 1, "过期条目应归档");
        assert_eq!(memory.facts.len(), 1);
        assert_eq!(memory.archived_facts.len(), 1);
        assert_eq!(memory.facts[0].hits, 1);
    }

    #[test]
    fn 事实上限按引用次数与创建时间归档最冷的那条() {
        let mut memory = Memory::default();
        for index in 0..MAX_FACTS {
            memory.remember(Fact::new(
                FactKind::Timeline,
                format!("事件 {index}"),
                now(),
            ));
        }
        // 让第 0 条被反复引用，它不该是被挤掉的那条。
        memory.facts[0].hits = 9;
        assert_eq!(
            memory.remember(Fact::new(FactKind::Timeline, "第 101 条", now())),
            Remembered::AddedAndArchived
        );
        assert_eq!(memory.facts.len(), MAX_FACTS);
        assert!(
            memory.facts.iter().any(|fact| fact.text == "事件 0"),
            "常用条目不能被归档"
        );
        assert_eq!(memory.archived_facts.len(), 1);
    }

    #[test]
    fn 行为模式升格只做一次() {
        let mut memory = Memory::default();
        assert!(memory.promote_patterns("常在决策日临时加仓", now()));
        assert!(!memory.promote_patterns("常在决策日临时加仓", now()));
        assert_eq!(memory.facts.len(), 1);
        assert_eq!(memory.facts[0].kind, FactKind::Pattern);
    }

    #[test]
    fn 更新前重读磁盘不会吞掉别的写者() {
        // 复现真实丢数据场景：GUI 窗口打开（内存里是旧快照）期间，CLI
        // 写进一条事实；GUI 再记一笔，旧实现整体覆盖会把 CLI 那条吞掉。
        let dir = std::env::temp_dir().join(format!("mns-butler-memory-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("memory.json");

        // 第一笔：窗口打开时已经在的记忆。
        Memory::update_at(&path, None, |memory| {
            memory.remember(Fact::new(FactKind::Commitment, "周五补美股", now()))
        })
        .unwrap();

        // 第二笔：另一个写者直接落盘（模拟 CLI `mns-butler remember`）。
        Memory::update_at(&path, None, |memory| {
            memory.remember(Fact::new(FactKind::Timeline, "发薪日 15 号", now()))
        })
        .unwrap();

        // 第三笔：窗口里又说了一句。重读后叠加，前两笔都必须还在。
        let (memory, _) = Memory::update_at(&path, None, |memory| {
            memory.remember(Fact::new(FactKind::Preference, "称呼我老王", now()))
        })
        .unwrap();
        assert_eq!(memory.facts.len(), 3, "三条事实都应当在：{memory:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 只在明确的记住忘掉前缀下识别意图() {
        assert_eq!(
            parse_intent("记住：周五补美股"),
            Some(Intent::Remember {
                kind: FactKind::Commitment,
                text: "周五补美股".to_owned(),
                due: None,
            })
        );
        assert_eq!(
            parse_intent("帮我忘掉补美股。"),
            Some(Intent::Forget {
                needle: "补美股".to_owned()
            })
        );
        // 疑问句不是在改记忆：不能猜。
        assert_eq!(parse_intent("周五要不要补美股"), None);
        assert_eq!(parse_intent("记住"), None, "没有正文不写入");
        assert_eq!(parse_intent("记住吗？"), None, "反问句不是指令");
        assert_eq!(parse_intent("忘掉"), None, "没有关键词不动归档");
    }

    #[test]
    fn 记住时能从正文识别类别与到期日() {
        assert_eq!(
            parse_intent("叫我老王就好"),
            None,
            "没有前缀的偏好不自作聪明"
        );
        assert_eq!(
            parse_intent("记住：称呼我老王"),
            Some(Intent::Remember {
                kind: FactKind::Preference,
                text: "称呼我老王".to_owned(),
                due: None,
            })
        );
        let intent = parse_intent("记住 2026-11-20 截止 补房贷").unwrap_or_else(|| unreachable!());
        match intent {
            Intent::Remember { kind, text, due } => {
                assert_eq!(kind, FactKind::Commitment);
                assert_eq!(text, "补房贷");
                assert_eq!(due, Some(day(20)));
            }
            other => unreachable!("应识别为记住：{other:?}"),
        }
        assert_eq!(classify("发薪日 15 号"), FactKind::Timeline);
    }
}
