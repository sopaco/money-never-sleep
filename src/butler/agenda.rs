//! 下一次唤醒的时刻表。
//!
//! 守护进程不轮询：它把「下一次该在什么时候想事情」算出来，然后一直睡到
//! 那个时刻。五个来源取最早的一个：
//!
//! - 定时巡检：`last_scheduled + 最小间隔`（从未巡检过就是现在）；
//! - 每日巡检：设置里的 `daily_review_time`（今天已跑过就是明天）；
//! - 自设任务：最早一条还没被唤醒过的待办；
//! - 承诺到期：到期日当天早上（静音时段之后）；
//! - agent-init 下一次尝试：上次尝试 + 1 小时（失败后是退避时长）。
//!
//! 任何输入变化（账本事件、GUI 或 CLI 改了状态）都会让调用方重算，所以这里
//! 是纯函数：不缓存、不持有状态、不写盘。
//!
//! 为什么不用系统计划任务（launchd / cron / 计划任务）：那些是**启动器**，
//! 每次触发都要拉起一个新进程，并且要跟着用户的时钟、时区、休眠一起漂。
//! 管家要的是「同一个常驻进程里的下一次思考」，进程活着就有意义；它已经
//! 由登录项里的 GUI 拉起，不需要第二个调度器。

use chrono::{DateTime, Local};

use super::orchestrator::agent_init_backoff;
use super::review::DailyReview;
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
pub fn next(now: DateTime<Local>, butler: &Butler, review: Option<&DailyReview>) -> Option<Agenda> {
    let scheduler = &butler.state.scheduler;
    let mut rows: Vec<Agenda> = Vec::new();

    rows.push(Agenda {
        at: scheduler.next_scheduled().unwrap_or(now),
        source: WakeSource::Scheduled,
    });

    // 每日巡检有自己的固定时刻，不折算进 4 小时的定时巡检节奏。
    if let Some(review) = review {
        rows.push(Agenda {
            at: review.at,
            source: WakeSource::DailyReview {
                id: review.id.clone(),
            },
        });
    }

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

    // agent-init 的下一次尝试：常规节奏 1 小时，失败后按 15 分钟 / 1 小时 /
    // 6 小时退避。1 小时比 4 小时的定时巡检更密，必须单独排——否则「放宽到
    // 1 小时」只在巡检顺带触发时才生效。id 绑定「上次尝试 + 目标时刻」：
    // 同一个到点只叫醒一次，跑不成（比如用户刚聊过天）时不会反复空转。
    if butler.state.policy.enabled
        && let Some(last) = butler.state.last_agent_init
    {
        let at = last + agent_init_backoff(butler.state.agent_init_failures);
        let id = format!("init-retry:{}", last.timestamp_millis());
        if !scheduler.seen_agent_retries.contains(&id) {
            rows.push(Agenda {
                at,
                source: WakeSource::AgentInitRetry { id },
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
    use crate::butler::memory::{Fact, FactKind, Memory};
    use crate::butler::schedule::WakeScheduler;
    use crate::butler::tasks::{ButlerTask, TaskBoard};
    use chrono::TimeZone;

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
        let agenda = next(at(10, 0), &butler(), None).unwrap();
        assert_eq!(agenda.source, WakeSource::Scheduled);
        assert_eq!(agenda.at, at(10, 0));
    }

    #[test]
    fn 巡检过就按最小间隔排下一次() {
        let mut butler = butler();
        butler.state.scheduler = WakeScheduler::with_interval(4);
        butler.state.scheduler.last_scheduled = Some(at(9, 0));
        let agenda = next(at(10, 0), &butler, None).unwrap();
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
            kind: crate::butler::tasks::TaskKind::Check,
            state: crate::butler::tasks::TaskState::Pending,
            finished_at: None,
        });
        butler.tasks = board;
        let agenda = next(at(10, 0), &butler, None).unwrap();
        assert_eq!(agenda.source, WakeSource::Task { id: "t1".into() });
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
            kind: crate::butler::tasks::TaskKind::Check,
            state: crate::butler::tasks::TaskState::Pending,
            finished_at: None,
        });
        butler.tasks = board;
        butler.state.scheduler.seen_tasks.insert("t1".into());
        let agenda = next(at(10, 0), &butler, None).unwrap();
        assert_eq!(
            agenda.source,
            WakeSource::Scheduled,
            "不该再为一个已醒过的任务重排"
        );
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
        let agenda = next(date_at(19, 8, 30), &butler, None).unwrap();
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
        let agenda = next(date_at(19, 8, 30), &butler, None).unwrap();
        assert_eq!(agenda.source, WakeSource::Scheduled);
    }

    #[test]
    fn agent_init失败后按退避排重试() {
        // 巡检刚过（下次 13:00），上次 agent-init 尝试 09:00 且失败 1 次：
        // 15 分钟退避应当排在巡检前面。
        let mut case = butler();
        case.state.scheduler.last_scheduled = Some(at(9, 0));
        case.state.last_agent_init = Some(at(9, 0));
        case.state.agent_init_failures = 1;

        let agenda = next(at(9, 5), &case, None).unwrap();
        assert!(
            matches!(agenda.source, WakeSource::AgentInitRetry { .. }),
            "失败重试应早于 4 小时后的巡检，实际 {agenda:?}"
        );
        assert_eq!(agenda.at, at(9, 15), "第 1 次失败退避 15 分钟");

        // 连续失败到 6 小时档：不再比巡检早（下次巡检 13:00，重试 15:00）。
        let mut case = butler();
        case.state.scheduler.last_scheduled = Some(at(9, 0));
        case.state.last_agent_init = Some(at(9, 0));
        case.state.agent_init_failures = 3;
        let agenda = next(at(9, 5), &case, None).unwrap();
        assert_eq!(
            agenda.source,
            WakeSource::Scheduled,
            "6 小时退避晚于 4 小时后的巡检"
        );

        // 同一个重试时刻已消费过：不再重排（防止空转）。
        let mut case = butler();
        case.state.scheduler.last_scheduled = Some(at(9, 0));
        case.state.last_agent_init = Some(at(9, 0));
        case.state.agent_init_failures = 1;
        let id = format!("init-retry:{}", at(9, 0).timestamp_millis());
        case.state.scheduler.seen_agent_retries.insert(id);
        let agenda = next(at(9, 5), &case, None).unwrap();
        assert_eq!(agenda.source, WakeSource::Scheduled, "已消费的重试不再排");
    }

    #[test]
    fn 没有失败也按一小时排下一次主动开口() {
        let mut butler = butler();
        butler.state.scheduler.last_scheduled = Some(at(9, 0));
        butler.state.last_agent_init = Some(at(9, 0));
        butler.state.agent_init_failures = 0;
        let agenda = next(at(9, 5), &butler, None).unwrap();
        assert!(
            matches!(agenda.source, WakeSource::AgentInitRetry { .. }),
            "常规节拍应在 4 小时巡检之前醒来，实际 {agenda:?}"
        );
        assert_eq!(agenda.at, at(10, 0), "无失败时冷却 1 小时");
    }

    #[test]
    fn 主动开口关闭时不为它单独醒() {
        let mut butler = butler();
        butler.state.scheduler.last_scheduled = Some(at(9, 0));
        butler.state.last_agent_init = Some(at(9, 0));
        butler.state.policy.enabled = false;
        let agenda = next(at(9, 5), &butler, None).unwrap();
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
            kind: crate::butler::tasks::TaskKind::Check,
            state: crate::butler::tasks::TaskState::Pending,
            finished_at: None,
        });
        butler.tasks = board;
        let agenda = next(at(10, 0), &butler, None).unwrap();
        assert_eq!(agenda.at, at(10, 0));
    }

    #[test]
    fn 每日巡检按配置时刻排而不是按四小时间隔() {
        let mut case = butler();
        // 定时巡检下一次在 11:00；巡检设在 08:30，应排巡检。
        case.state.scheduler.last_scheduled = Some(at(7, 0));
        let review = DailyReview {
            at: at(8, 30),
            id: "review:2026-11-17".to_owned(),
        };
        let agenda = next(at(8, 0), &case, Some(&review)).unwrap();
        assert_eq!(
            agenda.source,
            WakeSource::DailyReview {
                id: "review:2026-11-17".to_owned()
            }
        );
        assert_eq!(agenda.at, at(8, 30));
    }

    #[test]
    fn 巡检与任务取更早的一个() {
        let mut case = butler();
        case.state.scheduler.last_scheduled = Some(at(7, 0));
        let mut board = TaskBoard::default();
        board.tasks.push(ButlerTask {
            id: "t1".into(),
            intent: "检查今天有没有计划外操作".into(),
            due_at: at(9, 0),
            created_at: at(7, 10),
            key: "k1".into(),
            kind: crate::butler::tasks::TaskKind::Check,
            state: crate::butler::tasks::TaskState::Pending,
            finished_at: None,
        });
        case.tasks = board;
        let review = DailyReview {
            at: at(8, 30),
            id: "review:2026-11-17".to_owned(),
        };
        let agenda = next(at(7, 30), &case, Some(&review)).unwrap();
        assert!(matches!(agenda.source, WakeSource::DailyReview { .. }));
        assert_eq!(agenda.at, at(8, 30));
    }
}
