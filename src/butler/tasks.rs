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
/// 每日新建上限。
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ButlerTask {
    pub id: String,
    /// 到时判断什么。
    pub intent: String,
    pub due_at: DateTime<Local>,
    pub created_at: DateTime<Local>,
    /// 去重键：同一事件只能有一个任务，例如 `nonfarm:2026-12-04`。
    pub key: String,
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
            state: TaskState::Pending,
            finished_at: None,
        }
    }

    pub fn is_expired(&self, now: DateTime<Local>) -> bool {
        now > self.due_at + chrono::Duration::hours(EXPIRE_GRACE_HOURS)
    }
}

/// 提议一个任务的判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskVerdict {
    Accepted { id: String },
    /// 同一事件已有任务（或已归档不久）。
    Duplicate,
    /// 今天已经建满 2 个。
    DailyLimit,
    /// 活跃队列已满。
    QueueFull,
    /// 意图写成了预测，不是检查。
    IntentNotCheck(String),
}

/// 校验任务意图：必须是「到时检查什么」，不能是「预测什么」。
pub fn validate_intent(intent: &str) -> Result<(), String> {
    let text = intent.trim();
    if text.chars().count() < 4 {
        return Err("任务意图太短，写清到点检查什么".to_owned());
    }
    const CHECK_WORDS: [&str; 5] = ["检查", "核对", "确认", "看", "复盘"];
    if !CHECK_WORDS.iter().any(|word| text.contains(word)) {
        return Err("任务意图要写「到时检查什么」，例如「到点检查当天是否有计划外操作」".to_owned());
    }
    const PREDICT_WORDS: [&str; 6] = ["预计", "预测", "会涨", "会跌", "抄底", "加仓到"];
    if let Some(word) = PREDICT_WORDS.iter().find(|word| text.contains(*word)) {
        return Err(format!("任务意图不能是预测（含「{word}」）"));
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

    /// 提议新任务：校验意图 → 去重 → 每日上限 → 队列上限。
    ///
    /// 顺序不能换：先拒预测，再谈额度，否则「被额度拒掉的其实是坏任务」会被
    /// 当成「明天再来」。
    pub fn propose(&mut self, task: ButlerTask, now: DateTime<Local>) -> TaskVerdict {
        if let Err(reason) = validate_intent(&task.intent) {
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
        let created_today = self
            .tasks
            .iter()
            .chain(self.archived.iter())
            .filter(|existing| existing.created_at.date_naive() == now.date_naive())
            .count();
        if created_today >= MAX_NEW_PER_DAY {
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
    pub fn tick(&mut self, now: DateTime<Local>) -> TickReport {
        let mut report = TickReport::default();
        for task in &mut self.tasks {
            if task.state == TaskState::Pending && task.is_expired(now) {
                task.state = TaskState::Dropped;
                task.finished_at = Some(now);
                report.dropped += 1;
            }
        }
        // 已结束的任务移出活跃表，避免「已完成」继续占额度。
        //
        // 必须先搬进归档再修剪：否则本次刚搬进来的旧任务要等到下一次巡检才
        // 会被清掉，`tick.archived` 也会少报一条。
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
        let Some(task) = self.tasks.iter_mut().find(|task| task.id == id) else {
            return false;
        };
        if task.state != TaskState::Pending {
            return false;
        }
        task.state = state;
        task.finished_at = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 11, 16, 9, 0, 0).single().unwrap()
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
        board.propose(task("late", -30), now());
        let report = board.tick(now());
        assert_eq!(report.dropped, 1);
        assert!(board.pending().is_empty());
        assert_eq!(board.archived.len(), 1);
        assert_eq!(board.archived[0].state, TaskState::Dropped);
    }

    #[test]
    fn 到点任务只在宽限期内可执行() {
        let mut board = TaskBoard::default();
        board.propose(task("on-time", 0), now());
        assert_eq!(board.due(now()).len(), 1);
        assert_eq!(
            board.due(now() + Duration::hours(EXPIRE_GRACE_HOURS + 1)).len(),
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
