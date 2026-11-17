//! 下一次唤醒的时刻表。
//!
//! 守护进程不轮询：它把「下一次该在什么时候想事情」算出来，然后一直睡到
//! 那个时刻。三个来源取最早的一个：
//!
//! - 定时巡检：`last_scheduled + 最小间隔`（从未巡检过就是现在）；
//! - 自设任务：最早一条还没被唤醒过的待办；
//! - 承诺到期：到期日当天早上（静音时段之后）。
//!
//! 任何输入变化（账本事件、GUI 或 CLI 改了状态）都会让调用方重算，所以这里
//! 是纯函数：不缓存、不持有状态、不写盘。
//!
//! 为什么不用系统计划任务（launchd / cron / 计划任务）：那些是**启动器**，
//! 每次触发都要拉起一个新进程，并且要跟着用户的时钟、时区、休眠一起漂。
//! 管家要的是「同一个常驻进程里的下一次思考」，进程活着就有意义；它已经
//! 由登录项里的 GUI 拉起，不需要第二个调度器。

use chrono::{DateTime, Local};

use super::{Butler, WakeSource};

/// 承诺到期在几点提。静音时段（默认 22:00–08:00）之后，早于这个点提会
/// 撞上「别在半夜说话」。
pub const COMMITMENT_HOUR: u32 = 9;

/// 一条已经排好时间的唤醒。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agenda {
    pub at: DateTime<Local>,
    pub source: WakeSource,
}

/// 从现在开始，最早该为什么在什么时候醒来。
///
/// `None` 只在「连定时巡检都算不出来」时出现——实际上不会发生，保留是为了
/// 让调用方不必假装时间一定存在。
pub fn next(now: DateTime<Local>, butler: &Butler) -> Option<Agenda> {
    let scheduler = &butler.state.scheduler;
    let mut rows: Vec<Agenda> = Vec::new();

    rows.push(Agenda {
        at: scheduler.next_scheduled().unwrap_or(now),
        source: WakeSource::Scheduled,
    });

    if let Some((id, due)) = butler.tasks.next_due(now, &scheduler.seen_tasks) {
        rows.push(Agenda {
            at: due,
            source: WakeSource::Task { id },
        });
    }

    let today = now.date_naive();
    if let Some(date) = butler.memory.next_commitment(today) {
        let id = format!("promise:{date}");
        if !scheduler.seen_commitments.contains(&id)
            && let Some(at) = date
                .and_hms_opt(COMMITMENT_HOUR, 0, 0)
                .and_then(|naive| naive.and_local_timezone(Local).single())
        {
            rows.push(Agenda {
                at,
                source: WakeSource::Commitment { id },
            });
        }
    }

    // 过去的时刻代表「已经该做了」：立刻醒来，而不是等下一个整点。
    let rows = rows.into_iter().map(|row| Agenda {
        at: row.at.max(now),
        ..row
    });
    rows.min_by_key(|row| row.at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use crate::butler::memory::{Fact, FactKind, Memory};
    use crate::butler::schedule::WakeScheduler;
    use crate::butler::tasks::{ButlerTask, TaskBoard};

    fn at(hour: u32, minute: u32) -> DateTime<Local> {
        date_at(17, hour, minute)
    }

    fn date_at(day: u32, hour: u32, minute: u32) -> DateTime<Local> {
        chrono::Local
            .with_ymd_and_hms(2026, 11, day, hour, minute, 0)
            .single()
            .unwrap()
    }

    fn butler() -> Butler {
        Butler::default()
    }

    #[test]
    fn 从未巡检过就立刻醒来() {
        let agenda = next(at(10, 0), &butler()).unwrap();
        assert_eq!(agenda.source, WakeSource::Scheduled);
        assert_eq!(agenda.at, at(10, 0));
    }

    #[test]
    fn 巡检过就按最小间隔排下一次() {
        let mut butler = butler();
        butler.state.scheduler = WakeScheduler::with_interval(4);
        butler.state.scheduler.last_scheduled = Some(at(9, 0));
        let agenda = next(at(10, 0), &butler).unwrap();
        assert_eq!(agenda.source, WakeSource::Scheduled);
        assert_eq!(agenda.at, at(13, 0));
    }

    #[test]
    fn 任务比巡检更早时按任务排() {
        let mut butler = butler();
        butler.state.scheduler.last_scheduled = Some(at(9, 0));
        let mut board = TaskBoard::default();
        board.tasks.push(ButlerTask {
            id: "t1".into(),
            intent: "检查今天有没有计划外操作".into(),
            due_at: at(11, 30),
            created_at: at(9, 10),
            key: "k1".into(),
            state: crate::butler::tasks::TaskState::Pending,
            finished_at: None,
        });
        butler.tasks = board;
        let agenda = next(at(10, 0), &butler).unwrap();
        assert_eq!(
            agenda.source,
            WakeSource::Task {
                id: "t1".into()
            }
        );
        assert_eq!(agenda.at, at(11, 30));
    }

    #[test]
    fn 已经唤醒过的任务不再排() {
        let mut butler = butler();
        butler.state.scheduler.last_scheduled = Some(at(9, 0));
        let mut board = TaskBoard::default();
        board.tasks.push(ButlerTask {
            id: "t1".into(),
            intent: "检查今天有没有计划外操作".into(),
            due_at: at(10, 30),
            created_at: at(9, 10),
            key: "k1".into(),
            state: crate::butler::tasks::TaskState::Pending,
            finished_at: None,
        });
        butler.tasks = board;
        butler.state.scheduler.seen_tasks.insert("t1".into());
        let agenda = next(at(10, 0), &butler).unwrap();
        assert_eq!(agenda.source, WakeSource::Scheduled, "不该再为一个已醒过的任务重排");
    }

    #[test]
    fn 承诺到期排在到期日早上九点() {
        let mut butler = butler();
        butler.state.scheduler.last_scheduled = Some(date_at(19, 8, 0));
        let mut memory = Memory::default();
        memory.facts.push(
            Fact::new(FactKind::Commitment, "说好这周补美股", at(8, 0))
                .expires_on(chrono::NaiveDate::from_ymd_opt(2026, 11, 19).unwrap()),
        );
        butler.memory = memory;
        // 08:30 时下一次巡检是 12:00，承诺在 09:00——承诺更早，按承诺排。
        let agenda = next(date_at(19, 8, 30), &butler).unwrap();
        assert_eq!(
            agenda.source,
            WakeSource::Commitment {
                id: "promise:2026-11-19".into()
            }
        );
        assert_eq!(agenda.at, date_at(19, COMMITMENT_HOUR, 0));
    }

    #[test]
    fn 提过的承诺不再重排() {
        let mut butler = butler();
        butler.state.scheduler.last_scheduled = Some(date_at(19, 8, 0));
        let mut memory = Memory::default();
        memory.facts.push(
            Fact::new(FactKind::Commitment, "说好这周补美股", at(8, 0))
                .expires_on(chrono::NaiveDate::from_ymd_opt(2026, 11, 19).unwrap()),
        );
        butler.memory = memory;
        butler
            .state
            .scheduler
            .seen_commitments
            .insert("promise:2026-11-19".into());
        let agenda = next(date_at(19, 8, 30), &butler).unwrap();
        assert_eq!(agenda.source, WakeSource::Scheduled);
    }

    #[test]
    fn 过去的任务立刻醒来而不是等下一轮() {
        let mut butler = butler();
        butler.state.scheduler.last_scheduled = Some(at(9, 0));
        let mut board = TaskBoard::default();
        board.tasks.push(ButlerTask {
            id: "t1".into(),
            intent: "检查今天有没有计划外操作".into(),
            due_at: at(9, 30),
            created_at: at(9, 10),
            key: "k1".into(),
            state: crate::butler::tasks::TaskState::Pending,
            finished_at: None,
        });
        butler.tasks = board;
        let agenda = next(at(10, 0), &butler).unwrap();
        assert_eq!(agenda.at, at(10, 0));
    }
}
