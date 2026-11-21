//! 管家给自己设的一次性时点任务。
//!
//! 任务存在的唯一理由是「到点检查」，不是「提前预测」。因此意图文案必须写
//! 到时候判断什么；写「预计下周会跌」的提议会被校验层直接拒掉——那是预测，
//! 不是任务。

use anyhow::Result;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use super::butler_dir;
use super::store::{read_json, write_json};

/// 活跃任务上限。
pub const MAX_ACTIVE: usize = 10;
/// 每日新建上限。只约束管家自己提议的检查任务；用户亲口要的提醒不占这个额度。
pub const MAX_NEW_PER_DAY: usize = 2;
/// 到期后多久还没执行就作废。
pub const EXPIRE_GRACE_HOURS: i64 = 24;
/// 已完成/已作废任务在归档里保留多久。
pub const ARCHIVE_KEEP_DAYS: i64 = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Pending,
    Done,
    Dropped,
}

/// 任务到点后说什么。
///
/// `Check` 是默认：送达一句「到点了：<检查什么>」，提醒用户自己去核对。
/// `Talk` 是用户明确要管家到点开口：送达一句问候，不复读任务原文。
/// 两者都只有一句话——到点任务不是对话通道。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    #[default]
    Check,
    Talk,
}

/// 任务是谁发起的。
///
/// 管家自己提议的检查任务要过措辞校验与每日上限；用户亲口要的提醒不该被
/// 管家的自我约束挡住——那是闹钟，不是管家找话说。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TaskOrigin {
    #[default]
    Butler,
    User,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ButlerTask {
    pub id: String,
    /// 到时提醒/判断什么。`Check` 写成检查对象，`Talk` 写成要聊的话题。
    pub intent: String,
    pub due_at: DateTime<Local>,
    pub created_at: DateTime<Local>,
    /// 去重键：同一事件只能有一个任务，例如 `nonfarm:2026-12-04`。
    pub key: String,
    /// 到点说什么。旧 `tasks.json` 没有这个字段时按 `Check` 读。
    #[serde(default)]
    pub kind: TaskKind,
    pub state: TaskState,
    pub finished_at: Option<DateTime<Local>>,
}

impl ButlerTask {
    pub fn new(
        key: impl Into<String>,
        intent: impl Into<String>,
        due_at: DateTime<Local>,
        now: DateTime<Local>,
    ) -> Self {
        Self {
            id: format!("t-{}", now.timestamp_millis()),
            intent: intent.into(),
            due_at,
            created_at: now,
            key: key.into(),
            kind: TaskKind::Check,
            state: TaskState::Pending,
            finished_at: None,
        }
    }

    /// 到点主动开口，而不是回显任务原文。
    pub fn with_kind(mut self, kind: TaskKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn is_expired(&self, now: DateTime<Local>) -> bool {
        now > self.due_at + chrono::Duration::hours(EXPIRE_GRACE_HOURS)
    }
}

/// 提议一个任务的判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskVerdict {
    Accepted {
        id: String,
    },
    /// 同一事件已有任务（或已归档不久）。
    Duplicate,
    /// 今天已经建满 2 个。
    DailyLimit,
    /// 活跃队列已满。
    QueueFull,
    /// 到点时间已经过去：闹钟不能设在昨天。
    TooSoon {
        now: DateTime<Local>,
        due_at: DateTime<Local>,
    },
    /// 意图写成了预测，不是检查。
    IntentNotCheck(String),
}

/// 不能写成预测的措辞。用户亲口要的提醒也不该是预测。
const PREDICT_WORDS: [&str; 6] = ["预计", "预测", "会涨", "会跌", "抄底", "加仓到"];

/// 校验管家自己提议的任务意图：必须是「到时检查什么」，不能是「预测什么」。
pub fn validate_intent(intent: &str) -> Result<(), String> {
    let text = intent.trim();
    if text.chars().count() < 4 {
        return Err("任务意图太短，写清到点检查什么".to_owned());
    }
    const CHECK_WORDS: [&str; 5] = ["检查", "核对", "确认", "看", "复盘"];
    if !CHECK_WORDS.iter().any(|word| text.contains(word)) {
        return Err(
            "任务意图要写「到时检查什么」，例如「到点检查当天是否有计划外操作」".to_owned(),
        );
    }
    if let Some(word) = PREDICT_WORDS.iter().find(|word| text.contains(*word)) {
        return Err(format!("任务意图不能是预测（含「{word}」）"));
    }
    Ok(())
}

/// 用户亲口要的提醒：不要求写成「检查」，但仍然不能是预测。
///
/// 「5 分钟后提醒我喝水」是闹钟，不是检查任务；拿管家的措辞校验去拦它，
/// 只会逼模型把用户的话改写成另一件事。
pub fn validate_user_intent(intent: &str) -> Result<(), String> {
    let text = intent.trim();
    if text.chars().count() < 2 {
        return Err("提醒内容太短，写清到点提醒什么".to_owned());
    }
    if let Some(word) = PREDICT_WORDS.iter().find(|word| text.contains(*word)) {
        return Err(format!("提醒内容不能是预测（含「{word}」）"));
    }
    Ok(())
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskBoard {
    pub tasks: Vec<ButlerTask>,
    pub archived: Vec<ButlerTask>,
}

/// 巡检时处理到期任务的结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TickReport {
    pub dropped: usize,
    pub archived: usize,
}

/// 每周复盘的结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReviewReport {
    pub merged: usize,
    pub dropped_stale: usize,
    pub archived: usize,
}

impl TaskBoard {
    pub fn path() -> std::path::PathBuf {
        butler_dir().join("tasks.json")
    }

    pub fn load() -> Result<Self> {
        Ok(read_json(&Self::path())?.unwrap_or_default())
    }

    pub fn save(&self) -> Result<()> {
        write_json(&Self::path(), self)
    }

    pub fn pending(&self) -> Vec<&ButlerTask> {
        self.tasks
            .iter()
            .filter(|task| task.state == TaskState::Pending)
            .collect()
    }

    /// 提议新任务：到点必须是未来 → 校验意图 → 去重 → 每日上限 → 队列上限。
    ///
    /// 顺序不能换：先拒「已经过点的闹钟」，再拒预测，最后才谈额度——否则
    /// 「被额度拒掉的其实是坏任务」会被当成「明天再来」。
    ///
    /// 这是管家自己提议的路径（措辞必须写成「检查什么」，每日最多 2 条）。
    pub fn propose(&mut self, task: ButlerTask, now: DateTime<Local>) -> TaskVerdict {
        self.propose_as(task, now, TaskOrigin::Butler)
    }

    /// 带来源的提议。用户亲口要的提醒跳过「必须是检查」的措辞校验与每日
    /// 上限，但仍然要过「到点在未来」、预测词、去重与活跃队列上限。
    pub fn propose_as(
        &mut self,
        task: ButlerTask,
        now: DateTime<Local>,
        origin: TaskOrigin,
    ) -> TaskVerdict {
        if task.due_at < now {
            return TaskVerdict::TooSoon {
                now,
                due_at: task.due_at,
            };
        }
        let validation = match origin {
            TaskOrigin::Butler => validate_intent(&task.intent),
            TaskOrigin::User => validate_user_intent(&task.intent),
        };
        if let Err(reason) = validation {
            return TaskVerdict::IntentNotCheck(reason);
        }
        if self
            .archived
            .iter()
            .chain(self.tasks.iter())
            .any(|existing| existing.key == task.key)
        {
            return TaskVerdict::Duplicate;
        }
        // 只数今天还在待办的：完成/作废的任务已经不再占用注意力，不该继续
        // 顶着「今天已经建了 2 条」把第三条挡在门外。
        let created_today = self
            .tasks
            .iter()
            .filter(|existing| existing.state == TaskState::Pending)
            .filter(|existing| existing.created_at.date_naive() == now.date_naive())
            .count();
        if origin == TaskOrigin::Butler && created_today >= MAX_NEW_PER_DAY {
            return TaskVerdict::DailyLimit;
        }
        if self.pending().len() >= MAX_ACTIVE {
            return TaskVerdict::QueueFull;
        }

        let id = task.id.clone();
        self.tasks.push(task);
        TaskVerdict::Accepted { id }
    }

    /// 最早的「还没被唤醒过」的到点时刻。`None` = 没有待办任务。
    ///
    /// 守护进程用它算下一次该在什么时候醒来——不要每 5 分钟扫一遍磁盘。
    pub fn next_due(
        &self,
        now: DateTime<Local>,
        seen: &std::collections::BTreeSet<String>,
    ) -> Option<(String, DateTime<Local>)> {
        self.tasks
            .iter()
            .filter(|task| task.state == TaskState::Pending)
            .filter(|task| !task.is_expired(now))
            .filter(|task| !seen.contains(&task.id))
            .map(|task| (task.id.clone(), task.due_at))
            .min_by_key(|(_, due)| *due)
    }

    /// 到点且未作废的任务。
    pub fn due(&self, now: DateTime<Local>) -> Vec<ButlerTask> {
        self.tasks
            .iter()
            .filter(|task| {
                task.state == TaskState::Pending && task.due_at <= now && !task.is_expired(now)
            })
            .cloned()
            .collect()
    }

    pub fn complete(&mut self, id: &str, now: DateTime<Local>) -> bool {
        self.finish(id, TaskState::Done, now)
    }

    pub fn drop_task(&mut self, id: &str, now: DateTime<Local>) -> bool {
        self.finish(id, TaskState::Dropped, now)
    }

    /// 每次巡检调用：到期超过 24h 仍未执行的作废；归档里超过 7 天的清掉。
    ///
    /// `task done` / `drop_task` 已经当场把任务搬进归档，这里不再搬已结束的
    /// 任务，只补做「过期作废」和归档修剪。
    pub fn tick(&mut self, now: DateTime<Local>) -> TickReport {
        let mut report = TickReport::default();
        for task in &mut self.tasks {
            if task.state == TaskState::Pending && task.is_expired(now) {
                task.state = TaskState::Dropped;
                task.finished_at = Some(now);
                report.dropped += 1;
            }
        }
        // 兜底：手改过 tasks.json 或旧版本留下的已结束任务，仍然搬进归档。
        let (finished, alive): (Vec<ButlerTask>, Vec<ButlerTask>) = self
            .tasks
            .drain(..)
            .partition(|task| task.state != TaskState::Pending);
        self.tasks = alive;
        self.archived.extend(finished);

        let cutoff = now - chrono::Duration::days(ARCHIVE_KEEP_DAYS);
        let before = self.archived.len();
        self.archived
            .retain(|task| task.finished_at.is_none_or(|at| at >= cutoff));
        report.archived = before - self.archived.len();
        report
    }

    /// 每周复盘：同事件合并（保留最早到期的一条）、丢弃失效任务、归档旧记录。
    pub fn review(&mut self, now: DateTime<Local>) -> ReviewReport {
        let mut report = ReviewReport::default();
        let mut kept: Vec<ButlerTask> = Vec::new();
        for task in self.tasks.drain(..) {
            if let Some(existing) = kept.iter().position(|row| row.key == task.key) {
                // 保留更早到期的那个：它才是「到点该检查」的那一次。
                if task.due_at < kept[existing].due_at {
                    kept[existing] = task;
                }
                report.merged += 1;
            } else {
                kept.push(task);
            }
        }
        let (stale, alive): (Vec<ButlerTask>, Vec<ButlerTask>) = kept
            .into_iter()
            .partition(|task| task.state == TaskState::Pending && task.is_expired(now));
        for mut task in stale {
            task.state = TaskState::Dropped;
            task.finished_at = Some(now);
            self.archived.push(task);
            report.dropped_stale += 1;
        }
        self.tasks = alive;

        let tick = self.tick(now);
        report.archived += tick.archived;
        report
    }

    fn finish(&mut self, id: &str, state: TaskState, now: DateTime<Local>) -> bool {
        let Some(index) = self.tasks.iter().position(|task| task.id == id) else {
            return false;
        };
        if self.tasks[index].state != TaskState::Pending {
            return false;
        }
        // 状态一改就搬进归档：留着会在 `tasks` 里以「已结束」继续出现，
        // 管家读列表时会以为提醒还挂着，可能重复建任务或重复回答。
        let mut task = self.tasks.remove(index);
        task.state = state;
        task.finished_at = Some(now);
        self.archived.push(task);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn now() -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 11, 16, 9, 0, 0)
            .single()
            .unwrap()
    }

    fn task(key: &str, hours: i64) -> ButlerTask {
        ButlerTask::new(
            key,
            "到点检查当天是否有计划外操作",
            now() + Duration::hours(hours),
            now(),
        )
    }

    #[test]
    fn 预测型意图被拒而检查型通过() {
        assert!(validate_intent("到点检查当天是否有计划外操作").is_ok());
        assert!(validate_intent("核对本周补仓计划是否受影响").is_ok());
        assert!(validate_intent("预计下周会跌，到点加仓到八成").is_err());
        assert!(validate_intent("看看").is_err());
    }

    #[test]
    fn 同一事件只允许一个任务() {
        let mut board = TaskBoard::default();
        assert!(matches!(
            board.propose(task("nonfarm:2026-12-04", 24), now()),
            TaskVerdict::Accepted { .. }
        ));
        assert_eq!(
            board.propose(task("nonfarm:2026-12-04", 48), now()),
            TaskVerdict::Duplicate
        );
    }

    #[test]
    fn 每日新建不超过两个() {
        let mut board = TaskBoard::default();
        assert!(matches!(
            board.propose(task("a", 12), now()),
            TaskVerdict::Accepted { .. }
        ));
        assert!(matches!(
            board.propose(task("b", 13), now()),
            TaskVerdict::Accepted { .. }
        ));
        assert_eq!(board.propose(task("c", 14), now()), TaskVerdict::DailyLimit);
    }

    #[test]
    fn 活跃队列上限十个() {
        let mut board = TaskBoard::default();
        for index in 0..MAX_ACTIVE {
            let mut row = task(&format!("k{index}"), index as i64 + 1);
            // 摊到不同日期，绕开每日上限。
            row.created_at = now() - Duration::days(index as i64 + 1);
            assert!(matches!(
                board.propose(row, now()),
                TaskVerdict::Accepted { .. }
            ));
        }
        let mut extra = task("overflow", 30);
        extra.created_at = now() - Duration::days(30);
        assert_eq!(board.propose(extra, now()), TaskVerdict::QueueFull);
    }

    #[test]
    fn 到期超一天未执行自动作废并归档() {
        let mut board = TaskBoard::default();
        // 直接放进队列：`propose` 现在会拒绝过去的时间，这里要模拟的是
        // 「旧版本留下的、已经过期但还没被巡检处理」的任务。
        board.tasks.push(task("late", -30));
        let report = board.tick(now());
        assert_eq!(report.dropped, 1);
        assert!(board.pending().is_empty());
        assert_eq!(board.archived.len(), 1);
        assert_eq!(board.archived[0].state, TaskState::Dropped);
    }

    #[test]
    fn 已经过点的闹钟会被拒绝() {
        let mut board = TaskBoard::default();
        let verdict = board.propose(task("past", -1), now());
        assert!(
            matches!(verdict, TaskVerdict::TooSoon { .. }),
            "实际 {verdict:?}"
        );
        assert!(board.pending().is_empty(), "被拒的任务不能留在队列里");
        // 正好等于 now 仍然接受：测试和「立刻」语义都靠它。
        assert!(matches!(
            board.propose(task("instant", 0), now()),
            TaskVerdict::Accepted { .. }
        ));
    }

    #[test]
    fn 完成的任务不再占每日额度() {
        let mut board = TaskBoard::default();
        let TaskVerdict::Accepted { id } = board.propose(task("done", 6), now()) else {
            panic!("应被接受");
        };
        assert!(board.complete(&id, now()));
        assert!(matches!(
            board.propose(task("b", 7), now()),
            TaskVerdict::Accepted { .. }
        ));
        assert!(matches!(
            board.propose(task("c", 8), now()),
            TaskVerdict::Accepted { .. }
        ));
        assert_eq!(
            board.propose(task("d", 9), now()),
            TaskVerdict::DailyLimit,
            "仍然只允许两条『待办』，但已完成的不算"
        );
    }

    #[test]
    fn 用户提醒跳过检查措辞与每日额度() {
        let mut board = TaskBoard::default();
        for index in 0..4 {
            let row = ButlerTask::new(
                format!("user-{index}"),
                "提醒我喝水",
                now() + Duration::minutes(index as i64 + 5),
                now(),
            );
            assert!(
                matches!(
                    board.propose_as(row, now(), TaskOrigin::User),
                    TaskVerdict::Accepted { .. }
                ),
                "用户亲口要的提醒不该被管家的自我约束挡住"
            );
        }
        // 管家自己提议同样的话仍然要过检查措辞。
        let butler = ButlerTask::new("butler", "提醒我喝水", now() + Duration::hours(1), now());
        assert!(matches!(
            board.propose(butler, now()),
            TaskVerdict::IntentNotCheck(_)
        ));
    }

    #[test]
    fn 到点问候作为任务类型可以持久化() {
        let kind = ButlerTask::new("talk", "到点聊两句", now() + Duration::hours(1), now())
            .with_kind(TaskKind::Talk);
        let text = serde_json::to_string(&kind).unwrap();
        let back: ButlerTask = serde_json::from_str(&text).unwrap();
        assert_eq!(back.kind, TaskKind::Talk);

        // 旧文件没有 kind 字段：按 Check 读，不能反序列化失败。
        let old = r#"{"id":"t1","intent":"看计划","due_at":"2026-11-16T10:00:00+08:00","created_at":"2026-11-16T09:00:00+08:00","key":"k","state":"pending","finished_at":null}"#;
        let back: ButlerTask = serde_json::from_str(old).unwrap();
        assert_eq!(back.kind, TaskKind::Check);
    }

    #[test]
    fn 到点任务只在宽限期内可执行() {
        let mut board = TaskBoard::default();
        board.propose(task("on-time", 0), now());
        assert_eq!(board.due(now()).len(), 1);
        assert_eq!(
            board
                .due(now() + Duration::hours(EXPIRE_GRACE_HOURS + 1))
                .len(),
            0,
            "超过宽限期就不再是「该检查」而是「早就错过」"
        );
    }

    #[test]
    fn 完成后不再占额度且七天后清出归档() {
        let mut board = TaskBoard::default();
        let TaskVerdict::Accepted { id } = board.propose(task("done", 6), now()) else {
            panic!("应被接受");
        };
        assert!(board.complete(&id, now()));
        assert!(board.pending().is_empty(), "完成的任务不占活跃额度");

        let tick = board.tick(now() + Duration::days(ARCHIVE_KEEP_DAYS + 1));
        assert_eq!(tick.archived, 1);
        assert!(board.archived.is_empty());
    }

    #[test]
    fn 标记完成当刻就移入归档而不是等每周巡检() {
        let mut board = TaskBoard::default();
        let TaskVerdict::Accepted { id } = board.propose(task("now", 2), now()) else {
            panic!("应被接受");
        };
        assert!(board.complete(&id, now()));
        assert!(
            board.tasks.is_empty(),
            "已结束的任务不能留在 tasks 里被当成待执行"
        );
        assert_eq!(board.archived.len(), 1);
        assert_eq!(board.archived[0].id, id);
        assert_eq!(board.archived[0].state, TaskState::Done);
        assert!(board.archived[0].finished_at.is_some());

        // 第二次完成同一个 id 不再生效，也不会重复归档。
        assert!(!board.complete(&id, now()));
        assert_eq!(board.archived.len(), 1);
    }

    #[test]
    fn 每周复盘合并同事件并清掉失效任务() {
        let mut board = TaskBoard::default();
        board.tasks.push(task("dup", 5));
        board.tasks.push(task("dup", 9));
        board.tasks.push(task("stale", -40));
        let report = board.review(now());
        assert_eq!(report.merged, 1);
        assert_eq!(report.dropped_stale, 1);
        assert_eq!(board.pending().len(), 1, "同事件只留一条");
        assert_eq!(board.tasks[0].due_at, now() + Duration::hours(5));
    }
}
