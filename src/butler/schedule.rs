//! 唤醒调度与开口判定——管家的两层结构。
//!
//! 第一层「唤醒」= permission to think：只有定时巡检、账本事件、自设任务
//! 三类事件能叫醒管家。第二层「开口」= duty to speak：绝大多数唤醒的正确
//! 答案是沉默。两层都是纯函数 + 可持久化状态，模型不参与。

use chrono::{DateTime, Datelike, Local, NaiveDate, TimeZone, Timelike};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// 定时巡检的最小间隔（小时）。规划写死「任两次间隔 ≥4h」。
pub const MIN_SCHEDULED_GAP_HOURS: i64 = 4;

/// 什么叫醒了管家。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeSource {
    /// 定时巡检。
    Scheduled,
    /// 账本事件：入金 / 出金 / 买入登记 / 卖出登记。`id` 用于去重。
    Ledger { id: String },
    /// 自设任务到点。`id` 是任务 id。
    Task { id: String },
    /// 承诺到期（记忆里的 `Commitment`）。`id` 是 `promise:<日期>`。
    Commitment { id: String },
    /// agent-init 的下一次尝试到点（常规 1 小时节拍或失败后的退避）。
    /// `id` 唯一定位一次尝试机会（上次尝试时间 + 退避），避免同一条
    /// 到点被重复叫醒。
    AgentInitRetry { id: String },
    /// 每日巡检到点。`id` = `review:<日期>`，同一天只叫醒一次。
    DailyReview { id: String },
}

/// 唤醒判定结果。不唤醒也要说明原因——否则「今天怎么没动静」无从回答。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeVerdict {
    Wake,
    /// 距上次定时巡检不足最小间隔。
    TooSoon {
        next_allowed: DateTime<Local>,
    },
    /// 同一事件重复到达（GUI 补写日志、重试）。
    Duplicate,
}

/// 唤醒调度器的持久状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WakeScheduler {
    /// 定时巡检间隔下限，读取配置时会被夹到 ≥4h。
    pub min_interval_hours: i64,
    pub last_scheduled: Option<DateTime<Local>>,
    /// 已消费的账本事件 id。上限之外的旧 id 会被丢弃（记账事件不会重放）。
    pub seen_ledger: BTreeSet<String>,
    /// 已消费的任务 id。
    pub seen_tasks: BTreeSet<String>,
    /// 已消费的承诺 id。
    pub seen_commitments: BTreeSet<String>,
    /// 已消费的 agent-init 尝试到点 id。到点没跑成新一轮尝试（比如用户
    /// 刚聊过天）时，防止同一个到点时刻把主循环打成空转。
    pub seen_agent_retries: BTreeSet<String>,
    /// 已消费的每日巡检 id（`review:<日期>`）。
    pub seen_reviews: BTreeSet<String>,
}

impl Default for WakeScheduler {
    fn default() -> Self {
        Self {
            min_interval_hours: MIN_SCHEDULED_GAP_HOURS,
            last_scheduled: None,
            seen_ledger: BTreeSet::new(),
            seen_tasks: BTreeSet::new(),
            seen_commitments: BTreeSet::new(),
            seen_agent_retries: BTreeSet::new(),
            seen_reviews: BTreeSet::new(),
        }
    }
}

impl WakeScheduler {
    /// 间隔夹到 ≥4h：配置写 1h 也不能把管家变成轮询器。
    pub fn with_interval(hours: i64) -> Self {
        Self {
            min_interval_hours: hours.max(MIN_SCHEDULED_GAP_HOURS),
            ..Self::default()
        }
    }

    /// 一次唤醒请求。返回 `Wake` 时会更新内部状态。
    pub fn request(&mut self, source: &WakeSource, now: DateTime<Local>) -> WakeVerdict {
        match source {
            WakeSource::Scheduled => {
                let gap =
                    chrono::Duration::hours(self.min_interval_hours.max(MIN_SCHEDULED_GAP_HOURS));
                if let Some(last) = self.last_scheduled
                    && now < last + gap
                {
                    return WakeVerdict::TooSoon {
                        next_allowed: last + gap,
                    };
                }
                self.last_scheduled = Some(now);
                WakeVerdict::Wake
            }
            WakeSource::Ledger { id } => {
                if !self.seen_ledger.insert(id.clone()) {
                    return WakeVerdict::Duplicate;
                }
                WakeVerdict::Wake
            }
            WakeSource::Task { id } => {
                if !self.seen_tasks.insert(id.clone()) {
                    return WakeVerdict::Duplicate;
                }
                WakeVerdict::Wake
            }
            WakeSource::Commitment { id } => {
                if !self.seen_commitments.insert(id.clone()) {
                    return WakeVerdict::Duplicate;
                }
                WakeVerdict::Wake
            }
            // 重试不推进 `last_scheduled`：它不占用 4 小时巡检的节奏，
            // 也不参与承诺/任务的去重语义——真正的节奏由 run_agent_init
            // 自己的退避把关，这里只防同一个到点重复叫醒。
            WakeSource::AgentInitRetry { id } => {
                if !self.seen_agent_retries.insert(id.clone()) {
                    return WakeVerdict::Duplicate;
                }
                WakeVerdict::Wake
            }
            // 巡检同样不占用 4 小时巡检节奏：它有自己的每日时刻。
            WakeSource::DailyReview { id } => {
                if !self.seen_reviews.insert(id.clone()) {
                    return WakeVerdict::Duplicate;
                }
                WakeVerdict::Wake
            }
        }
    }

    /// 下一次定时巡检最早可以发生的时刻。从未巡检过就是「现在」。
    pub fn next_scheduled(&self) -> Option<DateTime<Local>> {
        let gap = chrono::Duration::hours(self.min_interval_hours.max(MIN_SCHEDULED_GAP_HOURS));
        self.last_scheduled.map(|last| last + gap)
    }

    /// 修剪去重集合，避免长期运行后无限增长。
    pub fn trim_seen(&mut self, keep: usize) {
        trim_set(&mut self.seen_ledger, keep);
        trim_set(&mut self.seen_tasks, keep);
        trim_set(&mut self.seen_commitments, keep);
        trim_set(&mut self.seen_agent_retries, keep);
        trim_set(&mut self.seen_reviews, keep);
    }
}

fn trim_set(set: &mut BTreeSet<String>, keep: usize) {
    while set.len() > keep {
        let Some(first) = set.iter().next().cloned() else {
            break;
        };
        set.remove(&first);
    }
}

/// 允许开口的由头（规划 3.2 第二层，顺序即优先级）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SpeakReason {
    /// 自设任务到点且上下文成立。排第一：任务带时效，过期即作废。
    TaskDue,
    /// 有可执行的下一步：决策日偏离超带宽，或新钱到账且有计划。
    ActionableStep,
    /// 计划外行为。
    ///
    /// 当前**没有自动来源**：交易记录只有基金代码、没有分类，无法可靠判断
    /// 一笔交易是否在计划内；「市场异动」也缺少指数级行情。保留这个变体
    /// 是为了让旧 `state.json` 里的去重记录仍能反序列化。
    OffPlanBehavior,
    /// 用户预设的承诺到期（「等发工资补仓」）。
    PromiseDue,
    /// 每日巡检完成。
    DailyReview,
    /// 没有确定性由头，但 agent 认为此刻有话可说（主动开口）。
    AgentInit,
}

impl SpeakReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::TaskDue => "自设任务到点",
            Self::ActionableStep => "有可执行的下一步",
            Self::OffPlanBehavior => "计划外行为",
            Self::PromiseDue => "承诺到期",
            Self::DailyReview => "每日巡检",
            Self::AgentInit => "自主开口",
        }
    }
}

/// 一次「想开口」的候选。`draft` 是确定性模板文案，表达层可以改写措辞，
/// 但不能改变事实与数字。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeakCandidate {
    pub reason: SpeakReason,
    /// 稳定标识（事件 id / 日期 + 腿），用于同一事件去重。
    pub trigger: String,
    pub draft: String,
    /// 会话版文案（主动开口双通道）。`None` = 会话与横幅同文。
    /// 横幅（`draft`）仍走最严口径；这一段只进管家会话。
    pub chat: Option<String>,
}

/// 开口频控。默认：每日 ≤3、每周 ≤10、静音 22:00–08:00、同一事件 3 天内
/// 不重复。
///
/// 这些默认值可以在设置页或 `config.toml` 的 `[butler]` 节里调整；
/// `Butler::load()` 每次判定前用配置覆盖这里的数值（`enabled` 仍在
/// `state.json`，因为它是「能不能说」的开关，不是频控）。
///
/// `enabled = false` 是用户显式的「别主动找我」：唤醒与自整理照常进行，
/// 只是不再产生需要送达的通知。它与频控是两件事——频控限制说多少，开关
/// 决定能不能说。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SpeakPolicy {
    pub enabled: bool,
    pub daily_cap: usize,
    pub weekly_cap: usize,
    pub quiet_start_hour: u32,
    pub quiet_end_hour: u32,
    /// 同一 `trigger` 在多少天内不重复开口。
    pub dedupe_days: i64,
}

impl Default for SpeakPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            daily_cap: 3,
            weekly_cap: 10,
            quiet_start_hour: 22,
            quiet_end_hour: 8,
            dedupe_days: 3,
        }
    }
}

/// 已经开过口的记录（只保留最近一段，避免无限增长）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SpeakLedger {
    pub records: Vec<SpeakRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeakRecord {
    pub at: DateTime<Local>,
    pub trigger: String,
    pub reason: SpeakReason,
}

impl SpeakLedger {
    pub fn record(&mut self, at: DateTime<Local>, candidate: &SpeakCandidate) {
        self.records.push(SpeakRecord {
            at,
            trigger: candidate.trigger.clone(),
            reason: candidate.reason,
        });
    }

    pub fn trim(&mut self, now: DateTime<Local>, days: i64) {
        let cutoff = now - chrono::Duration::days(days.max(1));
        self.records.retain(|record| record.at >= cutoff);
    }

    fn spoken_since(&self, cutoff: DateTime<Local>) -> usize {
        self.records
            .iter()
            .filter(|record| record.at >= cutoff)
            .count()
    }

    fn spoke_about(&self, trigger: &str, cutoff: DateTime<Local>) -> bool {
        self.records
            .iter()
            .any(|record| record.trigger == trigger && record.at >= cutoff)
    }
}

/// 开口判定结果。`Silent` 必须带原因，便于事后审计。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeakDecision {
    Speak(SpeakCandidate),
    Silent(SilentBecause),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SilentBecause {
    /// 没有任何候选——最常见的答案。
    NothingToSay,
    /// 用户关掉了主动开口。
    Paused,
    /// 候选都被去重掉了（同一事件已经说过）。
    Duplicate,
    /// 静音时段。
    QuietHours,
    /// 当日已开口。
    DailyCap,
    /// 近 7 日已开口 3 次。
    WeeklyCap,
}

impl SilentBecause {
    pub fn label(self) -> &'static str {
        match self {
            Self::NothingToSay => "没有值得开口的事",
            Self::Paused => "你已关闭主动开口",
            Self::Duplicate => "同一件事已经说过",
            Self::QuietHours => "静音时段",
            Self::DailyCap => "今日已开过口",
            Self::WeeklyCap => "本周额度已用完",
        }
    }
}

/// 频控闸门：只看「此刻能不能说」，与有没有候选无关。
///
/// `decide` 在有候选之后调用它；`agent-init` 在确定性内核给出
/// `NothingToSay` 之后也调用它——两条路都走同一份额度，不会各算一套。
pub fn gate(
    policy: &SpeakPolicy,
    ledger: &SpeakLedger,
    now: DateTime<Local>,
) -> Result<(), SilentBecause> {
    if !policy.enabled {
        return Err(SilentBecause::Paused);
    }
    if in_quiet_hours(now, policy) {
        return Err(SilentBecause::QuietHours);
    }

    let start_of_day = now.date_naive().and_hms_opt(0, 0, 0).unwrap();
    let today_cutoff = Local
        .from_local_datetime(&start_of_day)
        .single()
        .unwrap_or(now);
    if ledger.spoken_since(today_cutoff) >= policy.daily_cap {
        return Err(SilentBecause::DailyCap);
    }
    let week_cutoff = now - chrono::Duration::days(7);
    if ledger.spoken_since(week_cutoff) >= policy.weekly_cap {
        return Err(SilentBecause::WeeklyCap);
    }
    Ok(())
}

/// 判定要不要开口。纯函数：给定候选、策略、历史与当前时间，结果可复算。
///
/// 到点任务走的是「用户自己设的闹钟」通道：它不受每日/每周额度、静音时段
/// 和主动开口开关影响。那些约束是防止管家**自己**找话说，不是用来吞掉用户
/// 亲口要的提醒。其余候选仍走完整闸门。
pub fn decide(
    candidates: &[SpeakCandidate],
    now: DateTime<Local>,
    policy: &SpeakPolicy,
    ledger: &SpeakLedger,
) -> SpeakDecision {
    if candidates.is_empty() {
        return SpeakDecision::Silent(SilentBecause::NothingToSay);
    }

    let dedupe_cutoff = now - chrono::Duration::days(policy.dedupe_days.max(1));
    let mut fresh: Vec<&SpeakCandidate> = candidates
        .iter()
        .filter(|candidate| !ledger.spoke_about(&candidate.trigger, dedupe_cutoff))
        .collect();

    if fresh.is_empty() {
        return SpeakDecision::Silent(SilentBecause::Duplicate);
    }
    // 任务带时效，过期即作废；其余按「可执行 → 计划外 → 承诺」排。
    fresh.sort_by_key(|candidate| candidate.reason);

    let chosen = fresh[0].clone();
    // 到点任务与每日巡检都是用户亲口设的闹钟，不是管家自己找话说。
    if matches!(
        chosen.reason,
        SpeakReason::TaskDue | SpeakReason::DailyReview
    ) {
        return SpeakDecision::Speak(chosen);
    }

    if let Err(reason) = gate(policy, ledger, now) {
        return SpeakDecision::Silent(reason);
    }

    SpeakDecision::Speak(chosen)
}

/// 静音时段是否覆盖 `now`。默认 22:00–08:00，跨零点。
pub fn in_quiet_hours(now: DateTime<Local>, policy: &SpeakPolicy) -> bool {
    let hour = now.hour();
    let start = policy.quiet_start_hour.min(23);
    let end = policy.quiet_end_hour.min(24);
    if start == end {
        return false;
    }
    if start < end {
        (start..end).contains(&hour)
    } else {
        hour >= start || hour < end
    }
}

/// 通知文案的硬约束：≤60 字、≤2 行。超了就是没遵守人设。
pub fn notification_violation(text: &str) -> Option<String> {
    let chars = text.chars().count();
    if chars > 60 {
        return Some(format!("通知文案 {chars} 字，超过 60 字上限"));
    }
    if text.lines().count() > 2 {
        return Some("通知文案超过两行".to_owned());
    }
    None
}

/// 通知文案（banner）的禁用话术表——源自 `mns-advisor/references/
/// evidence-boundary.md` 的禁用表。
///
/// 双人格拆分后只作用于**通知路径**（`validate_notification`）：banner 是
/// 被打扰通道，必须保持节制口径。管家的会话路径（dialogue / talk）不再
/// 查这张表——管家可以给倾向性观点，那是它作为自主人格的职责；会话路径只
/// 保留 `claims_execution` 一条诚信底线（不谎称能替用户下单）。
///
/// 只匹配真正的越界表述，不拦「本周不操作」这类结论——把正常结论拦掉会逼
/// 表达层写空话，反而更糟。
const BANNED: [(&str, &str); 14] = [
    ("现在是买点", "买卖点话术"),
    ("现在是卖点", "买卖点话术"),
    ("是买点", "买卖点话术"),
    ("是卖点", "买卖点话术"),
    ("该加仓", "择时话术"),
    ("该减仓", "择时话术"),
    ("建议买入", "越权建议"),
    ("建议卖出", "越权建议"),
    ("抄底", "择时话术"),
    ("跑赢", "收益承诺"),
    ("预计年化", "收益预测"),
    ("目标价", "价格预测"),
    ("胜率", "胜率承诺"),
    ("帮你下单", "越权执行"),
];

/// 命中禁用话术时返回命中词与其归类。
pub fn banned_phrase(text: &str) -> Option<(&'static str, &'static str)> {
    BANNED
        .iter()
        .find(|(phrase, _)| text.contains(phrase))
        .map(|(phrase, kind)| (*phrase, *kind))
}

/// 通知文本的完整校验：长度 + 禁用话术。任一不过就降级为模板文案。
pub fn validate_notification(text: &str) -> Result<(), String> {
    if let Some(violation) = notification_violation(text) {
        return Err(violation);
    }
    if let Some((phrase, kind)) = banned_phrase(text) {
        return Err(format!("命中禁用话术「{phrase}」（{kind}）"));
    }
    Ok(())
}

/// 会话路径的唯一硬底线：不谎称能替用户执行下单。
///
/// 策略类禁词（买卖点、择时、收益承诺）已按双人格边界从会话路径移除——
/// 那是自主人格的观点，不是越界。只留这一条诚信底线：没有券商通道却说
/// 能下单是撒谎，不是策略分歧。诚实的否定（「没法帮你下单」）不算越界，
/// 所以先看否定语境，别把说实话的回答也拦掉。
pub fn claims_execution(text: &str) -> Option<&'static str> {
    const PHRASE: &str = "帮你下单";
    let mut rest = text;
    while let Some(pos) = rest.find(PHRASE) {
        let before = &rest[..pos];
        let negated = ["不", "没", "没法", "无法", "不能", "别"]
            .iter()
            .any(|marker| before.ends_with(marker));
        if !negated {
            return Some(PHRASE);
        }
        rest = &rest[pos + PHRASE.len()..];
    }
    None
}

/// 会话文案的硬约束：≤300 字 + 不谎称能执行下单。
///
/// 双人格拆分后（`docs/butler-persona-upgrade-2026-11-17.md`）：管家是自主
/// 人格，策略性话术（买卖点、择时倾向）**不再**被本地否决——那是 mns 助手
/// 与通知 banner 的约束。这里只守长度（防模板 bug）与执行能力诚实（不能
/// 谎称替用户下单）。数字同源由 `adopt_*` 的数字白名单守，不在这层。
pub fn validate_dialogue(text: &str) -> Result<(), String> {
    let chars = text.chars().count();
    if chars > 300 {
        return Err(format!("会话文案 {chars} 字，超过 300 字上限"));
    }
    if let Some(phrase) = claims_execution(text) {
        return Err(format!("谎称能替用户执行下单（「{phrase}」）"));
    }
    Ok(())
}

/// 今天是不是决策日（供开口判定第 1 条使用）。
pub fn is_decision_day(now: DateTime<Local>, decision_weekday: u32) -> bool {
    // `decision_weekday` 用 1..=7（周一..周日），0 = 每天决策。
    if decision_weekday == 0 {
        return true;
    }
    now.weekday().number_from_monday() == decision_weekday
}

/// 本地日期偏移，测试里构造时间用。
pub fn local_midnight(day: NaiveDate) -> DateTime<Local> {
    let naive = day.and_hms_opt(9, 0, 0).expect("合法时间");
    Local
        .from_local_datetime(&naive)
        .single()
        .expect("本地时间唯一")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(day: u32, hour: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 11, day, hour, 0, 0)
            .single()
            .unwrap()
    }

    fn candidate(reason: SpeakReason, trigger: &str) -> SpeakCandidate {
        SpeakCandidate {
            reason,
            trigger: trigger.to_owned(),
            draft: "草稿".to_owned(),
            chat: None,
        }
    }

    #[test]
    fn 定时巡检间隔夹到至少四小时() {
        let mut scheduler = WakeScheduler::with_interval(1);
        assert_eq!(scheduler.min_interval_hours, MIN_SCHEDULED_GAP_HOURS);
        assert_eq!(
            scheduler.request(&WakeSource::Scheduled, at(16, 9)),
            WakeVerdict::Wake
        );
        assert_eq!(
            scheduler.request(&WakeSource::Scheduled, at(16, 12)),
            WakeVerdict::TooSoon {
                next_allowed: at(16, 13)
            }
        );
        assert_eq!(
            scheduler.request(&WakeSource::Scheduled, at(16, 13)),
            WakeVerdict::Wake
        );
    }

    #[test]
    fn 账本事件即触发但同一_id_只算一次() {
        let mut scheduler = WakeScheduler::default();
        let event = WakeSource::Ledger {
            id: "flow-42".to_owned(),
        };
        assert_eq!(scheduler.request(&event, at(16, 10)), WakeVerdict::Wake);
        assert_eq!(
            scheduler.request(&event, at(16, 10)),
            WakeVerdict::Duplicate
        );
        let other = WakeSource::Ledger {
            id: "flow-43".to_owned(),
        };
        assert_eq!(scheduler.request(&other, at(16, 10)), WakeVerdict::Wake);
    }

    #[test]
    fn 承诺到期即触发但同一_id_只算一次() {
        let mut scheduler = WakeScheduler::default();
        let event = WakeSource::Commitment {
            id: "promise:2026-11-19".to_owned(),
        };
        assert_eq!(scheduler.request(&event, at(19, 9)), WakeVerdict::Wake);
        assert_eq!(
            scheduler.request(&event, at(19, 10)),
            WakeVerdict::Duplicate
        );
    }

    #[test]
    fn 下一次巡检时刻按最小间隔推() {
        let mut scheduler = WakeScheduler::default();
        assert_eq!(scheduler.next_scheduled(), None);
        scheduler.request(&WakeSource::Scheduled, at(16, 9));
        assert_eq!(scheduler.next_scheduled(), Some(at(16, 13)));
    }

    #[test]
    fn 没有候选时保持沉默() {
        let decision = decide(
            &[],
            at(16, 10),
            &SpeakPolicy::default(),
            &SpeakLedger::default(),
        );
        assert_eq!(decision, SpeakDecision::Silent(SilentBecause::NothingToSay));
    }

    #[test]
    fn 关掉主动开口后即使有候选也沉默() {
        let policy = SpeakPolicy {
            enabled: false,
            ..SpeakPolicy::default()
        };
        let row = candidate(SpeakReason::ActionableStep, "2026-11-20:us_stocks");
        assert_eq!(
            decide(
                std::slice::from_ref(&row),
                at(16, 10),
                &policy,
                &SpeakLedger::default()
            ),
            SpeakDecision::Silent(SilentBecause::Paused)
        );
    }

    #[test]
    fn 到点任务不受静音额度与总开关影响() {
        // 到点任务是用户自己设的闹钟，不是管家自己找话说：频控、静音、
        // 主动开口开关都不该把它吞掉。
        let policy = SpeakPolicy {
            enabled: false,
            daily_cap: 1,
            weekly_cap: 1,
            quiet_start_hour: 23,
            quiet_end_hour: 6,
            ..SpeakPolicy::default()
        };
        let mut ledger = SpeakLedger::default();
        ledger.record(at(16, 9), &candidate(SpeakReason::ActionableStep, "x"));

        match decide(
            &[candidate(SpeakReason::TaskDue, "task:t1")],
            at(16, 23),
            &policy,
            &ledger,
        ) {
            SpeakDecision::Speak(row) => assert_eq!(row.reason, SpeakReason::TaskDue),
            other => panic!("到点任务不应被吞掉，实际 {other:?}"),
        }

        // 同一时刻的普通候选仍然按原规则拦下。
        assert_eq!(
            decide(
                &[candidate(SpeakReason::ActionableStep, "y")],
                at(16, 23),
                &policy,
                &ledger
            ),
            SpeakDecision::Silent(SilentBecause::Paused)
        );
    }

    #[test]
    fn 每日巡检不受静音额度与总开关影响() {
        let policy = SpeakPolicy {
            enabled: false,
            quiet_start_hour: 23,
            quiet_end_hour: 6,
            ..SpeakPolicy::default()
        };
        match decide(
            &[candidate(SpeakReason::DailyReview, "review:2026-11-21")],
            at(16, 23),
            &policy,
            &SpeakLedger::default(),
        ) {
            SpeakDecision::Speak(row) => assert_eq!(row.reason, SpeakReason::DailyReview),
            other => panic!("巡检是用户设的闹钟，不该被吞掉，实际 {other:?}"),
        }
    }

    #[test]
    fn 同一天的巡检只叫醒一次() {
        let mut scheduler = WakeScheduler::default();
        let event = WakeSource::DailyReview {
            id: "review:2026-11-21".to_owned(),
        };
        assert_eq!(scheduler.request(&event, at(21, 8)), WakeVerdict::Wake);
        assert_eq!(scheduler.request(&event, at(21, 9)), WakeVerdict::Duplicate);
        assert_eq!(scheduler.next_scheduled(), None, "巡检不推进 4 小时节奏");
    }

    #[test]
    fn 默认频控放宽到每天三次每周十次() {
        let policy = SpeakPolicy::default();
        assert_eq!(policy.daily_cap, 3);
        assert_eq!(policy.weekly_cap, 10);
        assert_eq!(policy.dedupe_days, 3);
    }

    #[test]
    fn 静音时段与每日每周上限各自生效() {
        // 只验证机制本身，与默认额度解耦：默认值变了不该让这条测试失去意义。
        let policy = SpeakPolicy {
            daily_cap: 1,
            weekly_cap: 3,
            ..SpeakPolicy::default()
        };
        let mut ledger = SpeakLedger::default();
        let row = candidate(SpeakReason::ActionableStep, "2026-11-20:us_stocks");

        // 23:00 静音。
        assert_eq!(
            decide(std::slice::from_ref(&row), at(16, 23), &policy, &ledger),
            SpeakDecision::Silent(SilentBecause::QuietHours)
        );

        // 当天已经说过一次 → 每日上限。
        ledger.record(at(16, 9), &row);
        assert_eq!(
            decide(
                &[candidate(SpeakReason::OffPlanBehavior, "trade-7")],
                at(16, 14),
                &policy,
                &ledger
            ),
            SpeakDecision::Silent(SilentBecause::DailyCap)
        );

        // 换一天可以再说，但 7 天内不超过 3 次。
        let mut ledger = SpeakLedger::default();
        for (day, trigger) in [(10, "a"), (11, "b"), (12, "c")] {
            ledger.record(
                at(day, 10),
                &candidate(SpeakReason::ActionableStep, trigger),
            );
        }
        assert_eq!(
            decide(
                &[candidate(SpeakReason::PromiseDue, "d")],
                at(13, 10),
                &policy,
                &ledger
            ),
            SpeakDecision::Silent(SilentBecause::WeeklyCap)
        );
    }

    #[test]
    fn 频控闸门与候选无关() {
        // agent-init 用它判断「没有确定性候选时能不能自己开口」。
        let policy = SpeakPolicy::default();
        let mut ledger = SpeakLedger::default();
        assert!(gate(&policy, &ledger, at(16, 10)).is_ok());
        for hour in [9, 10, 11] {
            ledger.record(at(16, hour), &candidate(SpeakReason::ActionableStep, "x"));
        }
        assert_eq!(
            gate(&policy, &ledger, at(16, 12)),
            Err(SilentBecause::DailyCap)
        );
        assert_eq!(
            gate(&policy, &ledger, at(16, 23)),
            Err(SilentBecause::QuietHours)
        );
    }

    #[test]
    fn 同一事件在去重窗口内不重复开口() {
        let policy = SpeakPolicy::default();
        let mut ledger = SpeakLedger::default();
        let row = candidate(SpeakReason::ActionableStep, "2026-11-20:us_stocks");
        ledger.record(at(15, 9), &row);
        assert_eq!(
            decide(std::slice::from_ref(&row), at(16, 10), &policy, &ledger),
            SpeakDecision::Silent(SilentBecause::Duplicate)
        );
        // 去重窗口（默认 3 天）之外可以再说同一件事。
        ledger.records.clear();
        ledger.record(at(12, 9), &row);
        assert!(matches!(
            decide(std::slice::from_ref(&row), at(16, 10), &policy, &ledger),
            SpeakDecision::Speak(_)
        ));
    }

    #[test]
    fn 有多个候选时任务到点优先() {
        let policy = SpeakPolicy::default();
        let rows = [
            candidate(SpeakReason::PromiseDue, "promise"),
            candidate(SpeakReason::ActionableStep, "decision"),
            candidate(SpeakReason::TaskDue, "task-1"),
        ];
        match decide(&rows, at(16, 10), &policy, &SpeakLedger::default()) {
            SpeakDecision::Speak(row) => assert_eq!(row.reason, SpeakReason::TaskDue),
            other => panic!("应当开口，实际 {other:?}"),
        }
    }

    #[test]
    fn 通知文案超过六十字或两行要被拒() {
        assert!(
            validate_notification("今天决策日，美股低配 6.2pp，补 ¥1,180？点开有逐腿明细").is_ok()
        );
        let long = "啊".repeat(61);
        assert!(validate_notification(&long).is_err());
        assert!(validate_notification("第一行\n第二行\n第三行").is_err());
        assert!(validate_notification("现在是买点，建议买入").is_err());
        assert!(
            validate_notification("本周不操作：偏离都在带宽内").is_ok(),
            "正常结论不能被禁用词误拦"
        );
    }

    #[test]
    fn 会话不再因策略话术被拒而通知照旧() {
        // 管家是自主人格：会话里可以有倾向性表述。banner 是被动打扰通道，
        // 策略约束不撤（docs/butler-persona-upgrade-2026-11-17.md §3）。
        assert!(validate_dialogue("现在是买点，我的看法是可以加仓").is_ok());
        assert!(validate_dialogue("目标价能到 45%，预计年化 8%").is_ok());
        assert!(validate_notification("现在是买点，我的看法是可以加仓").is_err());
        let long = "话".repeat(301);
        assert!(
            validate_dialogue(&long).is_err(),
            "长度上限仍然守住模板 bug"
        );
    }

    #[test]
    fn 会话唯一硬底线是不谎称能下单() {
        // 没有券商通道却说能下单是撒谎——诚信底线，不是策略约束。
        assert!(validate_dialogue("我帮你下单了").is_err());
        assert!(validate_dialogue("已经帮你下单").is_err());
        // 诚实的否定不算越界：模板和 agent 都会说「下单得你自己来」。
        assert!(validate_dialogue("没法帮你下单，得你自己来").is_ok());
        assert!(validate_dialogue("不能帮你下单").is_ok());
        // 通知路径的完整禁词约束不受影响。
        assert!(validate_notification("已经帮你下单").is_err());
    }

    #[test]
    fn 决策日判定用_1到7_与配置同源() {
        // 2026-11-20 是周五。
        assert!(is_decision_day(at(20, 10), 5));
        assert!(!is_decision_day(at(20, 10), 3));
        assert!(is_decision_day(at(19, 10), 0));
    }
}
