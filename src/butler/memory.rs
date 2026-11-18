//! 管家的轻量记忆。
//!
//! 三条硬约束，都是为了「记错不如不记」：
//!
//! 1. 每次会话最多 3 条要点，不存全文堆栈——摘要膨胀之后，管家会开始引用
//!    自己上次的措辞，而不是事实；
//! 2. 事实带 `expires_at`，过期自动归档；
//! 3. 整理只做合并与归档，**不做不可逆删除**：误删用户承诺是不可修复的，
//!    用户说「忘掉」才真的移出。归档自身有上限（`MAX_ARCHIVED_FACTS` /
//!    `MAX_ARCHIVED_SUMMARIES`）：它是「忘掉了什么」的查询窗口，不是
//!    永久回收站，超出后从最旧的清出。

use anyhow::Result;
use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};

use super::butler_dir;
use super::store::{read_json, write_json};

/// 事实条目上限。超过后先归档过期条目，再归档最久没被用到的那条。
pub const MAX_FACTS: usize = 100;
/// 会话摘要条数上限。
pub const MAX_SUMMARIES: usize = 20;
/// 归档事实条数上限。
///
/// 归档按进入顺序排列，超出后从最旧的清出——「忘掉了什么」是查询窗口，
/// 不是永久回收站。上限取得宽松（年量级的整理才会触到），只为兜住
/// memory.json 的无限增长。
pub const MAX_ARCHIVED_FACTS: usize = 200;
/// 归档摘要条数上限。摘要只用于审计，没有读它的表达路径，上限可以更紧。
pub const MAX_ARCHIVED_SUMMARIES: usize = 200;
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
    /// 反复出现的行为模式。当前没有生产路径产生新条目（升格机制未接线，
    /// 见整理路径 `compact`）；保留枚举值是为了继续读旧数据。
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fact {
    pub id: String,
    pub kind: FactKind,
    pub text: String,
    pub created_at: DateTime<Local>,
    /// 到期日；到期后不再参与表达，转入归档。
    pub expires_at: Option<NaiveDate>,
    /// 用户重复说过的次数：同义条目再记一次 +1。它衡量的是**用户的强调**，
    /// 不是管家引用过几次——整理与驱逐拿它保住「用户反复提过的事」。
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
        /// 用户带日期指定要忘哪条（「忘掉下周五补美股」）。日期是同文
        /// 不同到期日条目的唯一区分符。
        due: Option<NaiveDate>,
    },
}

/// 记住类前缀，长前缀必须排在短前缀前面（`strip_prefix_any` 按顺序匹配）。
const REMEMBER_PREFIXES: [&str; 5] = ["帮我记住", "记住", "记一下", "别忘了", "记下"];
/// 忘掉类前缀。
const FORGET_PREFIXES: [&str; 4] = ["帮我忘掉", "忘掉", "忘记", "不用记"];

/// 解析「记住… / 忘掉…」。返回 `None` 表示这句话不是在改记忆。
///
/// `today` 用于把「明天 / 周五 / 下周三」这类相对日期词落到具体日期。
pub fn parse_intent(text: &str, today: NaiveDate) -> Option<Intent> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(rest) = strip_prefix_any(text, &FORGET_PREFIXES) {
        let body = trim_tail(rest.trim_start_matches(['：', ':', '，', ',', ' ']));
        if body.is_empty() || starts_with_particle(body) {
            return None;
        }
        // 与「记住」同一套日期抽取：带日期的忘掉只动那一天的那条。
        let (needle, due) = extract_due(body, today);
        let needle = needle.trim();
        if needle.is_empty() {
            return None;
        }
        return Some(Intent::Forget {
            needle: needle.to_owned(),
            due,
        });
    }
    let rest = strip_prefix_any(text, &REMEMBER_PREFIXES)?;
    let body = trim_tail(rest.trim_start_matches(['：', ':', '，', ',', ' ']));
    if body.is_empty() || starts_with_particle(body) {
        return None;
    }
    let (body, due) = extract_due(body, today);
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
    prefixes.iter().find_map(|prefix| text.strip_prefix(prefix))
}

fn trim_tail(text: &str) -> &str {
    text.trim_end_matches(['。', '.', '！', '!', '？', '?', ' '])
}

/// 「记住吗」「记住？」这类反问不是指令，不能把后半句当事实记下来。
fn starts_with_particle(text: &str) -> bool {
    text.starts_with(['吗', '么', '嘛', '？', '?'])
}

/// 正文里的到期日：`YYYY-MM-DD`，以及几类无歧义的相对日期词。识别后把
/// 日期从正文里摘掉。
///
/// 相对词只认这一组：**今天/明天/后天/大后天**、**周X/礼拜X**（最近的那个
/// X，含今天）、**下周X/下礼拜X**（下一周的 X，周一算一周之始）。认不到就
/// 没有到期日——「月底」「发薪日后」这类模糊表述宁可不小题大做。注意
/// 代价：形如「下周一切…」的句子会把「下周一」当成日期，关键词识别的
/// 精度上限如此，与记住/忘掉前缀走同一套取舍。
fn extract_due(text: &str, today: NaiveDate) -> (String, Option<NaiveDate>) {
    if let Some((start, end, day)) = absolute_due(text).or_else(|| relative_due(text, today)) {
        let mut rest = String::new();
        rest.push_str(&text[..start]);
        rest.push_str(&text[end..]);
        let rest = clean_connectors(&rest);
        return (rest.trim().to_owned(), Some(day));
    }
    (text.to_owned(), None)
}

/// 全字母数字的日期窗口不跨多字节字符：按字节滑窗，非 ASCII 边界处
/// `get` 返回 `None`，自然跳过。
fn absolute_due(text: &str) -> Option<(usize, usize, NaiveDate)> {
    if text.len() < 10 {
        return None;
    }
    (0..=text.len() - 10).find_map(|start| {
        let window = text.get(start..start + 10)?;
        let day = NaiveDate::parse_from_str(window, "%Y-%m-%d").ok()?;
        Some((start, start + 10, day))
    })
}

/// 相对日期词。取**出现最早**的那个（同位取更长的词：「下周五」必须整体
/// 命中，不能拆成「周五」）。
fn relative_due(text: &str, today: NaiveDate) -> Option<(usize, usize, NaiveDate)> {
    relative_words(today)
        .into_iter()
        .filter_map(|(word, day)| text.find(&word).map(|at| (at, word.len(), day)))
        .min_by_key(|(at, len, _)| (*at, std::cmp::Reverse(*len)))
        .map(|(at, len, day)| (at, at + len, day))
}

/// 生成 `today` 对应的全部相对日期词。
fn relative_words(today: NaiveDate) -> Vec<(String, NaiveDate)> {
    use chrono::{Datelike, Duration};
    let mut words: Vec<(String, NaiveDate)> =
        [("大后天", 3i64), ("后天", 2), ("明天", 1), ("今天", 0)]
            .into_iter()
            .map(|(word, offset)| (word.to_owned(), today + Duration::days(offset)))
            .collect();
    // 月末：先放「下月底」，同位更长的词优先，否则「下月底」会被拆成
    // 「月底」算错一个月。
    words.push(("下月底".to_owned(), month_end(today, 1)));
    words.push(("月底".to_owned(), month_end(today, 0)));
    // 「下周X」是**下一周**的 X（周一算一周之始）。不能拿「最近的 X + 7 天」
    // 算：今天是周三时，最近的周一本就是下周一，再加 7 天就落到下下周了。
    let monday = today - Duration::days(i64::from(today.weekday().num_days_from_monday()));
    let next_monday = monday + Duration::days(7);
    for (name, weekday) in WEEKDAY_NAMES {
        let coming = coming_weekday(today, weekday);
        words.push((format!("周{name}"), coming));
        words.push((format!("礼拜{name}"), coming));
        let next_week = next_monday + Duration::days(i64::from(weekday) - 1);
        words.push((format!("下周{name}"), next_week));
        words.push((format!("下礼拜{name}"), next_week));
    }
    words
}

/// `today` 所在月（或其后第 `months_ahead` 个月）的最后一天。
fn month_end(today: NaiveDate, months_ahead: u32) -> NaiveDate {
    use chrono::{Datelike, Duration};
    let total = today.year() * 12 + today.month() as i32 - 1 + months_ahead as i32;
    let year = total.div_euclid(12);
    let month = total.rem_euclid(12) + 1; // 1..=12
    let first_of_next = NaiveDate::from_ymd_opt(
        if month == 12 { year + 1 } else { year },
        if month == 12 { 1 } else { month as u32 + 1 },
        1,
    )
    .expect("合法年月");
    first_of_next - Duration::days(1)
}

/// 一周七天的中文名与序号（周一为 1，与 `chrono::Datelike::weekday` 同口径）。
const WEEKDAY_NAMES: [(&str, u32); 8] = [
    ("一", 1),
    ("二", 2),
    ("三", 3),
    ("四", 4),
    ("五", 5),
    ("六", 6),
    ("日", 7),
    ("天", 7),
];

/// 从 `today` 起最近的那个工作日（就是今天则返回今天）。
fn coming_weekday(today: NaiveDate, target: u32) -> NaiveDate {
    use chrono::{Datelike, Duration};
    let current = today.weekday().number_from_monday();
    let offset = (target as i64 - current as i64 + 7) % 7;
    today + Duration::days(offset)
}

/// 摘掉日期后清理残留的连接词：「截止」随处可删（旧行为），「之前/以前/的」
/// 只清开头——它们紧跟在日期词后面（「周五**之前**补美股」），出现在结尾的
/// （「比以前」）动不得。
fn clean_connectors(text: &str) -> String {
    text.replace("截止", "")
        .trim()
        .trim_start_matches("之前")
        .trim_start_matches("以前")
        .trim_start_matches('的')
        .to_owned()
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

/// 「忘掉」的结果。
///
/// 宁可不删，也不误删：子串命中多条时把候选列出来，让用户说得更具体——
/// 误删用户承诺是这次操作唯一不可修复的坏结果。
#[derive(Debug, Clone, PartialEq)]
pub enum Forgot {
    /// 没有命中。
    NotFound,
    /// 命中且无歧义：已移入归档。
    Removed(Vec<Fact>),
    /// 子串命中多条：什么都没动，候选列出来等用户说得更具体。
    Ambiguous(Vec<Fact>),
}

/// 一次性整理的结果，写进审计日志。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Compaction {
    pub merged: usize,
    pub expired: usize,
    pub summaries_archived: usize,
    /// 归档超出上限后被清出的条数（事实 + 摘要）。
    pub archived_trimmed: usize,
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

    /// 记住一条事实。同文**且同到期日**才合并（`hits += 1`），否则新起一条。
    ///
    /// 合并键必须带到期日：日期词会被从正文摘出，「补美股」这种正文在
    /// 「下周五补美股」和「月底补美股」里是同一个；单看正文合并会把两次
    /// 不同日期的承诺压成一条、悄悄延长到期日，早的那次唤醒源就没了。
    pub fn remember(&mut self, fact: Fact) -> Remembered {
        let normalized = normalize(&fact.text);
        if let Some(existing) = self.facts.iter_mut().find(|existing| {
            normalize(&existing.text) == normalized && existing.expires_at == fact.expires_at
        }) {
            existing.hits = existing.hits.saturating_add(1);
            return Remembered::Merged;
        }

        self.facts.push(fact);
        if self.enforce_fact_cap() {
            Remembered::AddedAndArchived
        } else {
            Remembered::Added
        }
    }

    /// 用户说「忘掉这件事」。命中且无歧义才移入归档（可查）。
    ///
    /// 三段语义：**精确命中**（归一化后与条目全文相等）直接删——用户念了
    /// 全句，不会有歧义；只说关键词时**恰好命中一条**也删——「忘掉补美股」
    /// 的便利要保住；**命中多条**则什么都不动，把候选列出来让用户说得更
    /// 具体——误删比多问一句糟得多。
    ///
    /// `due` 是用户话里的日期（「忘掉下周五补美股」）：同文不同到期日可以
    /// 并存之后，日期是唯一区分符。带日期时候选限缩到 `expires_at == due`；
    /// 不带日期时同文多条按歧义处理，等用户把日期补上。
    pub fn forget(&mut self, needle: &str, due: Option<NaiveDate>) -> Forgot {
        let needle = normalize(needle);
        let body_matches = |fact: &Fact, exact: bool| {
            let text = normalize(&fact.text);
            if exact {
                text == needle
            } else {
                text.contains(&needle)
            }
        };
        // 带日期时先按到期日收窄；不带日期时所有条目都是候选。
        let in_scope = |fact: &Fact| due.is_none_or(|day| fact.expires_at == Some(day));
        // 精确正文优先；没有精确再退到子串——「忘掉补美股」不把
        // 「补美股的预算」一起卷走。命中项连同索引一起收集：`Fact::new`
        // 的 id 是毫秒时间戳，同毫秒两条会撞 id，按 id 回查可能移错位置。
        let exact: Vec<(usize, Fact)> = self
            .facts
            .iter()
            .enumerate()
            .filter(|(_, fact)| in_scope(fact) && body_matches(fact, true))
            .map(|(index, fact)| (index, fact.clone()))
            .collect();
        let hits: Vec<(usize, Fact)> = if exact.is_empty() {
            self.facts
                .iter()
                .enumerate()
                .filter(|(_, fact)| in_scope(fact) && body_matches(fact, false))
                .map(|(index, fact)| (index, fact.clone()))
                .collect()
        } else {
            exact
        };
        match hits.len() {
            0 => Forgot::NotFound,
            1 => {
                let (index, hit) = hits.into_iter().next().expect("len 已确认");
                self.facts.remove(index);
                self.archived_facts.push(hit.clone());
                Forgot::Removed(vec![hit])
            }
            _ => Forgot::Ambiguous(hits.into_iter().map(|(_, fact)| fact).collect()),
        }
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

    /// 每周自整理：合并同义事实、归档过期条目、压掉最早的摘要、清出超限
    /// 的归档。
    ///
    /// 合并键是「正文 + 到期日」：不同到期日的同文条目是两件事（用户可能
    /// 就是要分两次补仓），不能压成一条、更不能悄悄延长早的那次。只合并
    /// 不删除；重复条目保留 `hits` 之和，这样「用户反复说过的事」在下一次
    /// 整理里仍然不会被当成冷条目。
    pub fn compact(&mut self, today: NaiveDate) -> Compaction {
        let mut report = Compaction::default();

        let mut merged: Vec<Fact> = Vec::new();
        for fact in self.facts.drain(..) {
            let key = normalize(&fact.text);
            if let Some(existing) = merged
                .iter_mut()
                .find(|row| normalize(&row.text) == key && row.expires_at == fact.expires_at)
            {
                existing.hits = existing.hits.saturating_add(fact.hits.max(1));
                report.merged += 1;
            } else {
                merged.push(fact);
            }
        }

        let (expired, alive): (Vec<Fact>, Vec<Fact>) =
            merged.into_iter().partition(|fact| fact.is_expired(today));
        report.expired = expired.len();
        self.archived_facts.extend(expired);
        self.facts = alive;

        while self.summaries.len() > MAX_SUMMARIES {
            let oldest = self.summaries.remove(0);
            self.archived_summaries.push(oldest);
            report.summaries_archived += 1;
        }

        // 归档上限：从最旧的清出（进入顺序 = 归档顺序）。
        if self.archived_facts.len() > MAX_ARCHIVED_FACTS {
            let overflow = self.archived_facts.len() - MAX_ARCHIVED_FACTS;
            self.archived_facts.drain(0..overflow);
            report.archived_trimmed += overflow;
        }
        if self.archived_summaries.len() > MAX_ARCHIVED_SUMMARIES {
            let overflow = self.archived_summaries.len() - MAX_ARCHIVED_SUMMARIES;
            self.archived_summaries.drain(0..overflow);
            report.archived_trimmed += overflow;
        }

        report
    }

    fn enforce_fact_cap(&mut self) -> bool {
        if self.facts.len() <= MAX_FACTS {
            return false;
        }
        // 先腾用户最少重复提过的：hits 少的优先，再按创建时间从早到晚。
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
        Local
            .with_ymd_and_hms(2026, 11, 16, 9, 0, 0)
            .single()
            .unwrap()
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
    fn 同文不同到期日不合并() {
        // e2e 复现：「记住下周五补美股」紧接「记住月底补美股」时，日期词被
        // 摘出正文后正文都成了「补美股」；单按正文合并会把两条压成一条、
        // 悄悄把到期日延到月底，早的那次唤醒源就没了。
        let mut memory = Memory::default();
        assert_eq!(
            memory.remember(Fact::new(FactKind::Commitment, "补美股", now()).expires_on(day(27))),
            Remembered::Added
        );
        assert_eq!(
            memory.remember(Fact::new(FactKind::Commitment, "补美股", now()).expires_on(day(30))),
            Remembered::Added,
            "到期日不同应各存一条，而不是延长早的那条"
        );
        assert_eq!(memory.facts.len(), 2);

        // 同句同日期才是真实重复：照常合并、引用计数 +1。
        assert_eq!(
            memory.remember(Fact::new(FactKind::Commitment, "补美股", now()).expires_on(day(30))),
            Remembered::Merged
        );
        assert_eq!(memory.facts.len(), 2);
    }

    #[test]
    fn 整理不合并不同到期日的同文事实() {
        let mut memory = Memory::default();
        memory
            .facts
            .push(Fact::new(FactKind::Commitment, "补美股", now()).expires_on(day(27)));
        memory
            .facts
            .push(Fact::new(FactKind::Commitment, "补美股", now()).expires_on(day(30)));

        let report = memory.compact(day(18));
        assert_eq!(report.merged, 0, "到期日不同不是同义条目");
        assert_eq!(memory.facts.len(), 2, "自整理也不能把两条并回去");
    }

    #[test]
    fn 忘掉会把条目移入归档而不是消失() {
        let mut memory = Memory::default();
        memory.remember(Fact::new(FactKind::Commitment, "周五补美股", now()));
        memory.remember(Fact::new(FactKind::Preference, "称呼我老王", now()));
        // 只有一条含「补美股」：子串唯一命中，直接删。
        let Forgot::Removed(hit) = memory.forget("补美股", None) else {
            panic!("唯一命中应直接移入归档");
        };
        assert_eq!(hit.len(), 1);
        assert_eq!(memory.facts.len(), 1);
        assert_eq!(memory.archived_facts.len(), 1, "归档可查");
    }

    #[test]
    fn 忘掉精确命中只动那一条() {
        let mut memory = Memory::default();
        memory.remember(Fact::new(FactKind::Commitment, "周五补美股", now()));
        memory.remember(Fact::new(FactKind::Commitment, "月底补美股", now()));

        // 念全句：精确命中，哪怕另一条也含同样的关键词。
        let Forgot::Removed(hit) = memory.forget("周五补美股", None) else {
            panic!("精确命中应直接删");
        };
        assert_eq!(hit.len(), 1);
        assert_eq!(memory.facts.len(), 1, "另一条不该被卷走");
    }

    #[test]
    fn 忘掉子串命中多条时不删而是列候选() {
        // review 指出的误伤场景：forget("补美股") 不该把所有提到补美股的
        // 条目一起卷进归档。命中多条时什么都不动，等用户说得更具体。
        let mut memory = Memory::default();
        memory.remember(Fact::new(FactKind::Commitment, "周五补美股", now()));
        memory.remember(Fact::new(FactKind::Commitment, "月底补美股", now()));

        let Forgot::Ambiguous(candidates) = memory.forget("补美股", None) else {
            panic!("多条命中应是歧义，不是全删");
        };
        assert_eq!(candidates.len(), 2, "候选要列全");
        assert_eq!(memory.facts.len(), 2, "歧义时一条都不能动");
        assert!(memory.archived_facts.is_empty(), "归档不能被歧义操作污染");

        // 用户说得更具体之后（念全句），才真的动那一条。
        let Forgot::Removed(hit) = memory.forget("月底补美股", None) else {
            panic!("念全句后应精确命中");
        };
        assert_eq!(hit.len(), 1);
        assert_eq!(memory.facts.len(), 1);

        // 没命中就是没命中，不装作做过什么。
        assert_eq!(memory.forget("不存在的事", None), Forgot::NotFound);
    }

    #[test]
    fn 忘掉按日期区分同文事实() {
        // B 的副作用：同文不同到期日可以并存后，不带日期的「忘掉补美股」
        // 会一次删光，带日期却无匹配。日期必须参与匹配。
        let mut memory = Memory::default();
        memory.remember(Fact::new(FactKind::Commitment, "补美股", now()).expires_on(day(27)));
        memory.remember(Fact::new(FactKind::Commitment, "补美股", now()).expires_on(day(30)));

        // 不带日期：两条同文，一条都不能动，候选带日期交给用户挑。
        let Forgot::Ambiguous(candidates) = memory.forget("补美股", None) else {
            panic!("同文多条应按歧义处理");
        };
        assert_eq!(candidates.len(), 2);
        assert_eq!(memory.facts.len(), 2, "歧义时一条都不能动");

        // 带日期：只删那一天的那条。
        let Forgot::Removed(hit) = memory.forget("补美股", Some(day(27))) else {
            panic!("带日期应精确命中一条");
        };
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].expires_at, Some(day(27)), "删错日期比不删更糟");
        assert_eq!(memory.facts.len(), 1);

        // 只剩一条后，不带日期也删得掉。
        let Forgot::Removed(hit) = memory.forget("补美股", None) else {
            panic!("唯一命中应直接删");
        };
        assert_eq!(hit.len(), 1);
        assert!(memory.facts.is_empty());

        // 日期对不上就是没找到。
        assert_eq!(memory.forget("补美股", Some(day(27))), Forgot::NotFound);
    }

    #[test]
    fn 同id两条也只删命中的那条() {
        // `Fact::new` 的 id 是毫秒时间戳，同毫秒创建会撞 id。命中项不是
        // 同 id 的第一条时，按 id 回查会移错位置——必须按命中索引删。
        let mut memory = Memory::default();
        let first = Fact::new(FactKind::Commitment, "补美股", now()).expires_on(day(27));
        let mut second = Fact::new(FactKind::Commitment, "补美股", now()).expires_on(day(30));
        second.id = first.id.clone();
        memory.facts.push(first);
        memory.facts.push(second);

        let Forgot::Removed(hit) = memory.forget("补美股", Some(day(30))) else {
            panic!("带日期应命中 11-30 那条");
        };
        assert_eq!(hit[0].expires_at, Some(day(30)));
        assert_eq!(memory.facts.len(), 1);
        assert_eq!(memory.facts[0].expires_at, Some(day(27)), "移错位置了");
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
    fn 归档超限从最旧的清出() {
        let mut memory = Memory::default();
        for index in 0..MAX_ARCHIVED_FACTS + 5 {
            memory.archived_facts.push(Fact::new(
                FactKind::Timeline,
                format!("归档事件 {index}"),
                now(),
            ));
        }
        for index in 0..MAX_ARCHIVED_SUMMARIES + 3 {
            memory.archived_summaries.push(Summary {
                id: format!("s-{index}"),
                at: now(),
                points: vec![format!("第 {index} 条")],
            });
        }

        let report = memory.compact(day(16));
        assert_eq!(report.archived_trimmed, 5 + 3, "清出数量要进整理报告");
        assert_eq!(memory.archived_facts.len(), MAX_ARCHIVED_FACTS);
        assert_eq!(memory.archived_summaries.len(), MAX_ARCHIVED_SUMMARIES);
        // 进的是头 5 条（最旧）和头 3 条摘要：留下的从「归档事件 5」开始。
        assert_eq!(memory.archived_facts[0].text, "归档事件 5");
        assert_eq!(
            memory.archived_facts.last().expect("非空").text,
            format!("归档事件 {}", MAX_ARCHIVED_FACTS + 4),
            "最新的归档条目必须留下"
        );
        assert_eq!(memory.archived_summaries[0].id, "s-3");
    }

    #[test]
    fn 整理只合并不删除且过期条目转为归档() {
        let mut memory = Memory::default();
        // 直接写进 `facts`：`remember` 已经会合并同义条目，这里模拟旧版本落盘
        // 的重复数据（或归一化规则变化后才出现的重复）。
        memory
            .facts
            .push(Fact::new(FactKind::Timeline, "发薪日 15 号", now()));
        memory
            .facts
            .push(Fact::new(FactKind::Timeline, "发薪日15号", now()));
        memory
            .facts
            .push(Fact::new(FactKind::Commitment, "月底补 A 股", now()).expires_on(day(10)));

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
    fn 更新前重读磁盘不会吞掉别的写者() {
        // 复现真实丢数据场景：GUI 窗口打开（内存里是旧快照）期间，CLI
        // 写进一条事实；GUI 再记一笔，旧实现整体覆盖会把 CLI 那条吞掉。
        let dir =
            std::env::temp_dir().join(format!("mns-butler-memory-merge-{}", std::process::id()));
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
        // 2026-11-16 是周一：正文里的「周五」落到本周五（11-20）。
        assert_eq!(
            parse_intent("记住：周五补美股", day(16)),
            Some(Intent::Remember {
                kind: FactKind::Commitment,
                text: "补美股".to_owned(),
                due: Some(day(20)),
            })
        );
        assert_eq!(
            parse_intent("帮我忘掉补美股。", day(16)),
            Some(Intent::Forget {
                needle: "补美股".to_owned(),
                due: None,
            })
        );
        // 忘掉也认日期：下周五 = 11-27（今天周一）。
        assert_eq!(
            parse_intent("忘掉下周五补美股", day(16)),
            Some(Intent::Forget {
                needle: "补美股".to_owned(),
                due: Some(day(27)),
            })
        );
        // 疑问句不是在改记忆：不能猜。
        assert_eq!(parse_intent("周五要不要补美股", day(16)), None);
        assert_eq!(parse_intent("记住", day(16)), None, "没有正文不写入");
        assert_eq!(parse_intent("记住吗？", day(16)), None, "反问句不是指令");
        assert_eq!(parse_intent("忘掉", day(16)), None, "没有关键词不动归档");
    }

    #[test]
    fn 相对日期词也能定下到期日() {
        // 2026-11-16 是周一。用户说「下周五补美股」时，承诺必须落到具体
        // 日期，否则永远进不了承诺唤醒源——这是三类开口由头之一。
        let dec = |d: u32| NaiveDate::from_ymd_opt(2026, 12, d).unwrap();
        let cases: [(&str, NaiveDate); 10] = [
            ("记住明天补美股", day(17)),
            ("记住后天复盘", day(18)),
            ("记住大后天还房贷", day(19)),
            ("记住周五补美股", day(20)),   // 本周五
            ("记一下周日看核账", day(22)), // 本周日
            ("记住下周五补美股", day(27)), // 下周五
            ("记住下周一交房租", day(23)), // 下周一
            ("记住今天收盘后看一眼", day(16)),
            ("记住月底还信用卡", day(30)), // 11 月底 = 11-30
            ("记住下月底交房租", dec(31)), // 下月底 = 12-31，不能算成 11-30
        ];
        for (text, due) in cases {
            let intent = parse_intent(text, day(16)).unwrap_or_else(|| unreachable!("{text}"));
            match intent {
                Intent::Remember { due: found, .. } => {
                    assert_eq!(found, Some(due), "「{text}」的到期日应落对")
                }
                other => unreachable!("应识别为记住：{other:?}"),
            }
        }
    }

    #[test]
    fn 周中说下周一指的是下一个周一() {
        // 2026-11-18 是周三，下一周从 11-23（周一）开始：「下周一」= 11-23、
        // 「下周三」= 11-25、「下周日」= 11-29。旧实现算「最近的 X + 7 天」，
        // 周中时「最近的周一」本就是下周一，再加 7 天会落到下下周。
        let wed = day(18);
        let cases: [(&str, NaiveDate); 3] = [
            ("记住下周一交房租", day(23)),
            ("记住下周三对账", day(25)),
            ("记住下周日复盘", day(29)),
        ];
        for (text, due) in cases {
            let intent = parse_intent(text, wed).unwrap_or_else(|| unreachable!("{text}"));
            match intent {
                Intent::Remember { due: found, .. } => {
                    assert_eq!(found, Some(due), "「{text}」的到期日应落对")
                }
                other => unreachable!("应识别为记住：{other:?}"),
            }
        }
    }

    #[test]
    fn 今天就是那个工作日时周X指今天() {
        // 2026-11-18 是周三：「周三」不该被算到下周去。
        let intent = parse_intent("记住周三核账", day(18)).unwrap_or_else(|| unreachable!());
        match intent {
            Intent::Remember { text, due, .. } => {
                assert_eq!(text, "核账");
                assert_eq!(due, Some(day(18)));
            }
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn 相对日期词摘掉后清理连接词() {
        let intent = parse_intent("记住周五之前补美股", day(16)).unwrap_or_else(|| unreachable!());
        match intent {
            Intent::Remember { text, due, .. } => {
                assert_eq!(text, "补美股", "「之前」是连接词，不该留在正文里");
                assert_eq!(due, Some(day(20)));
            }
            other => unreachable!("{other:?}"),
        }
        let intent = parse_intent("记住周五的复盘要点", day(16)).unwrap_or_else(|| unreachable!());
        match intent {
            Intent::Remember { text, .. } => assert_eq!(text, "复盘要点"),
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn 不是日期的近似词不误判() {
        // 「周末」「一周后」「前天」都不是无歧义的到期日，不能硬猜。
        for text in ["记住周末好好休息", "记住一周后复盘", "记住前天说的事"] {
            let intent = parse_intent(text, day(16)).unwrap_or_else(|| unreachable!());
            match intent {
                Intent::Remember { due, .. } => assert_eq!(due, None, "「{text}」不该有到期日"),
                other => unreachable!("{other:?}"),
            }
        }
    }

    #[test]
    fn 相对日期承诺要成为唤醒源() {
        // review 第 4 条的端到端验收：「记住下周五补美股」不再是 due=None
        // 的死承诺，next_commitment 能算出唤醒日，守护进程才会那天开口。
        let mut memory = Memory::default();
        let intent = parse_intent("记住下周五补美股", day(16)).unwrap_or_else(|| unreachable!());
        let Intent::Remember {
            kind, text, due, ..
        } = intent
        else {
            unreachable!();
        };
        let Some(due) = due else {
            unreachable!("下周五必须落成到期日")
        };
        memory.remember(Fact::new(kind, text, now()).expires_on(due));

        assert_eq!(memory.next_commitment(day(16)), Some(day(27)));
        assert_eq!(
            memory.next_commitment(day(26)),
            Some(day(27)),
            "到期前一天还要在唤醒日程上"
        );
        assert_eq!(
            memory.next_commitment(day(27)),
            Some(day(27)),
            "到期当天含今天"
        );
    }

    #[test]
    fn 记住时能从正文识别类别与到期日() {
        assert_eq!(
            parse_intent("叫我老王就好", day(16)),
            None,
            "没有前缀的偏好不自作聪明"
        );
        assert_eq!(
            parse_intent("记住：称呼我老王", day(16)),
            Some(Intent::Remember {
                kind: FactKind::Preference,
                text: "称呼我老王".to_owned(),
                due: None,
            })
        );
        let intent =
            parse_intent("记住 2026-11-20 截止 补房贷", day(16)).unwrap_or_else(|| unreachable!());
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
