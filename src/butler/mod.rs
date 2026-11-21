//! 管家（Butler）：常驻本地、按唤醒源主动评估的顾问。
//!
//! 这里只放**确定性**部分：唤醒调度、开口判定、记忆、任务队列。表达层
//! （外接 ACP 会话）与通知层（系统通知、窗口）不在这里，它们只是消费者。
//!
//! 为什么必须拆开：一旦「说不说」依赖模型，就没人能回答「今天为什么弹了
//! 这一条」。管家的全部决策都必须能在无网络、无模型的条件下复算，模型只
//! 负责把结论说得像人话。

pub mod agenda;
pub mod chat;
pub mod events;
pub mod expression;
pub mod memory;
pub mod orchestrator;
pub mod review;
pub mod schedule;
pub mod session;
pub mod store;
pub mod talk;
pub mod tasks;

use std::path::PathBuf;

pub use memory::Memory;
pub use orchestrator::{AccountFacts, Butler, ButlerState, DriftRow, WakeReport};
pub use review::{DailyReview, ReviewRef, ReviewReport, ReviewSummary};
pub use schedule::{SilentBecause, SpeakCandidate, SpeakDecision, SpeakPolicy, SpeakReason};
pub use schedule::{WakeScheduler, WakeSource, WakeVerdict};
pub use session::{Message, Role, Session};
pub use tasks::{TaskBoard, TaskKind, TaskOrigin, TaskState, TaskVerdict};

/// 管家数据根：`~/.mns/agent/butler`。
///
/// 与 GUI 助手的存档同父目录但互不写入：管家只读 `agent/` 下的助手存档作
/// 知情信息，自己的状态全在这一个子目录里，删除即「忘掉一切」。
pub fn butler_dir() -> PathBuf {
    crate::config::app_data_dir().join("agent").join("butler")
}

/// state / memory / tasks 的跨进程写锁。
///
/// 三个写者（GUI、`mns-butlerd`、`mns-butler` CLI）的「读-改-写」全程
/// 持锁：后写的先看到先写的修改，而不是拿自己启动时的旧快照整体覆盖。
/// 锁依附在文件描述符上，进程崩溃即释放，不需要清理。
///
/// 拿不到时返回 `None`，调用方降级为无锁写入（与加锁前行为一致）——
/// 「记住周五补美股」不能因为锁故障而丢失。
pub fn writer_lock() -> Option<store::WriterLock> {
    store::lock_writers(
        &butler_dir().join("writers.lock"),
        std::time::Duration::from_secs(2),
    )
}

/// 行为事件日志：`~/.mns/agent/events.jsonl`。
///
/// GUI 追加、管家只读。放在 `agent/` 而不是 `butler/`，是因为它是**账户侧**
/// 的客观记录（打开了哪个页面、点了哪个弹层），不属于管家的私有记忆。
pub fn events_path() -> PathBuf {
    crate::config::app_data_dir()
        .join("agent")
        .join("events.jsonl")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 数据根固定在应用的_agent_目录下() {
        let dir = butler_dir();
        assert!(dir.ends_with("agent/butler"), "实际 {}", dir.display());
        assert!(dir.starts_with(crate::config::app_data_dir()));
        assert!(events_path().ends_with("agent/events.jsonl"));
    }
}
