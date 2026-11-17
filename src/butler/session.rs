//! 管家自己的会话存档。
//!
//! 与 GUI 助手**互不共享**：助手存 `agent/conversations/`，管家存
//! `agent/butler/conversation.json`。两份文件没有交叉写入，删除管家目录
//! 不会影响助手的历史，反之亦然。
//!
//! 只保留最近 200 条：管家不是聊天记录仓库，长期堆积会让每次开口都带着
//! 上个月的措辞，反而更像复读机。

use anyhow::Result;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::butler_dir;
use super::schedule::SpeakReason;
use super::store::{read_json, write_json};

/// 会话保留条数上限。
pub const MAX_MESSAGES: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Butler,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub at: DateTime<Local>,
    pub role: Role,
    pub text: String,
    /// 主动开口时记下触发原因，便于事后回答「这条是谁授权的」。
    #[serde(default)]
    pub reason: Option<SpeakReason>,
    #[serde(default)]
    pub trigger: Option<String>,
}

impl Message {
    pub fn user(at: DateTime<Local>, text: impl Into<String>) -> Self {
        Self {
            at,
            role: Role::User,
            text: text.into(),
            reason: None,
            trigger: None,
        }
    }

    pub fn butler(at: DateTime<Local>, text: impl Into<String>) -> Self {
        Self {
            at,
            role: Role::Butler,
            text: text.into(),
            reason: None,
            trigger: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub messages: Vec<Message>,
    /// 用户上次打开管家窗口的时间，用来算未读数。
    pub last_read_at: Option<DateTime<Local>>,
}

impl Session {
    pub fn path() -> PathBuf {
        butler_dir().join("conversation.json")
    }

    pub fn load() -> Result<Self> {
        Self::load_from(&Self::path())
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::path())
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        Ok(read_json(path)?.unwrap_or_default())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        write_json(path, self)
    }

    /// 追加一条，超出上限时丢弃最早的——与记忆不同，会话可以真的丢掉
    /// 最早的消息；需要长期保留的结论应该已经进了记忆。
    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
        if self.messages.len() > MAX_MESSAGES {
            let overflow = self.messages.len() - MAX_MESSAGES;
            self.messages.drain(0..overflow);
        }
    }

    pub fn mark_read(&mut self, now: DateTime<Local>) {
        self.last_read_at = Some(now);
    }

    /// 未读：上次打开之后、由管家主动发出的消息数。
    pub fn unread(&self) -> usize {
        let since = self.last_read_at;
        self.messages
            .iter()
            .filter(|message| message.role == Role::Butler)
            .filter(|message| since.is_none_or(|at| message.at > at))
            .count()
    }

    pub fn last_butler(&self) -> Option<&Message> {
        self.messages
            .iter()
            .rev()
            .find(|message| message.role == Role::Butler)
    }

    /// 用户清除会话。只删这一份文件，不碰助手存档与账本。
    pub fn clear() -> Result<()> {
        match std::fs::remove_file(Self::path()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(minute: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 11, 16, 9, minute, 0)
            .single()
            .unwrap()
    }

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("mns-butler-session-{name}-{}", std::process::id()))
            .join("conversation.json")
    }

    #[test]
    fn 会话往返且只保留最近两百条() {
        let path = scratch("cap");
        let mut session = Session::default();
        for index in 0..MAX_MESSAGES + 10 {
            session.push(Message::butler(at(0), format!("第 {index} 条")));
        }
        session.save_to(&path).unwrap();
        let loaded = Session::load_from(&path).unwrap();
        assert_eq!(loaded.messages.len(), MAX_MESSAGES);
        assert_eq!(loaded.messages[0].text, "第 10 条");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn 存储路径与助手存档互不重叠() {
        let path = Session::path().to_string_lossy().into_owned();
        assert!(
            path.ends_with(".mns/agent/butler/conversation.json"),
            "管家会话应落在自己的目录，实际 {path}"
        );
        assert!(
            !path.contains("/agent/conversations/"),
            "不能写进助手存档目录，实际 {path}"
        );
    }

    #[test]
    fn 损坏的存档返回错误而不是静默清空() {
        let path = scratch("corrupt");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{\"messages\":[{\"role\":\"drift\"}]}").unwrap();
        assert!(
            Session::load_from(&path).is_err(),
            "读不出来就让调用方决定，别当成空会话再覆盖回去"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn 未读只算上次打开之后管家的消息() {
        let mut session = Session::default();
        session.push(Message::butler(at(1), "第一条"));
        session.push(Message::user(at(2), "用户回复"));
        assert_eq!(session.unread(), 1, "没有 last_read_at 时全部算未读");

        session.mark_read(at(3));
        assert_eq!(session.unread(), 0);

        session.push(Message::butler(at(4), "第二条"));
        session.push(Message::user(at(5), "又回复"));
        assert_eq!(session.unread(), 1);
    }
}
