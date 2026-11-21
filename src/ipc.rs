//! 本地进程间唤醒通道。
//!
//! 有两件事需要「另一个进程让本进程做点什么」：
//!
//! 1. 用户再次双击 `mns-gui`：已经有实例，把窗口拿到前面，自己退出；
//! 2. 管家守护进程（`mns-butlerd`）判定该开口：让 GUI 开管家窗口。
//!
//! 两者都用一条本地流连接完成，而不是轮询内核事件或心跳文件。这样做还有一个
//! 额外好处：**连接关闭本身就是存活信号**——守护进程崩溃时 GUI 立刻收到
//! `Disconnected`，不需要「90 秒没心跳就当它死了」这种阈值判断。
//!
//! 协议是 NDJSON，一行一条，行长上限 1MB。正常消息不到 100 字节；管家会话
//! 的全量快照（`ChatSnapshot`，最多 200 条消息）可以到几百 KB，所以上限按
//! 快照最坏情况放宽，仍用来挡住坏客户端。传输层按平台换，语义不变：
//! Unix 走 `SOCK_STREAM`（`<数据目录>/run/<app>.sock`），Windows 走命名管道
//! （`\\.\pipe\mns-<app>-<用户名>`）。两边共用同一份 `encode` / `decode`，
//! 所以「连接活着 = 对方活着」在哪个平台都成立。
//!
//! P4 起连接是**双向**的：管家窗口对 `mns-butlerd` 建一条长连接
//! （`connect_duplex`），上行发 `Chat*` 请求，下行收 `ChatSnapshot` /
//! `ChatDelta` 推送；服务端用 `Listener::send_to` 定向推。`poke` 仍是
//! 「连上就走」的短连接，供只需要递一句话的调用方使用。

use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

/// 一行协议能有多大。见模块注释：按快照最坏情况取 1MB。
const MAX_LINE: usize = 1024 * 1024;

/// 客户端发来的请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    /// 把主窗口拿到前面（二次启动）。
    Open,
    /// 打开管家会话窗口。
    OpenButler,
    /// 守护进程自我介绍。带上 `role`，将来 GUI 与管家可以共用一个监听。
    Hello { role: String },
    /// GUI 请守护进程立刻判定一次。`ledger` 有值是账本事件（不受巡检间隔
    /// 约束），无值是定时巡检。
    Wake { ledger: Option<String> },
    /// GUI 改了「主动开口」开关，让守护进程用自己的状态文件落盘——避免两个
    /// 进程同时写 `state.json`。
    SetProactive { enabled: bool },
    /// 托盘宿主请守护进程算一份托盘简报（数字取自账本快照）。
    ///
    /// 宿主不再常驻 AppShell，托盘上的总资产/偏离/建议只能向唯一的数据
    /// 服务方要：判定与数字同源，托盘就不可能出现「界面一套、通知一套」。
    RequestBrief,
    /// 守护进程回给宿主的简报，`json` 是 `brief::BriefData` 的序列化。
    Brief { json: String },
    /// 把窗口翻到指定页面。托盘菜单「本月计划」与二次启动都走这条：
    /// 窗口进程不在时先由宿主拉起，窗口已在时直接置前并翻页。
    OpenPage { page: String },
    /// 打开主窗口的「关于」面板（托盘「关于 MNS」）。
    OpenAbout,
    /// 守护进程 → 宿主：一条开口已并进会话存档（butlerd 单写），宿主只负责
    /// 决定要不要弹系统横幅。`json` 是 `butler::orchestrator::Notice` 的
    /// 序列化；宿主按 `notices.jsonl` 的 delivered 记录去重，重复下发
    /// 不会重复弹。投递成功后 butlerd 才推进 `pending.offset`。
    DeliverNotice { json: String },
    /// 用户点了某条系统通知：请已打开的管家窗口定位到该条开口。
    /// 窗口不在时由宿主带 `--notice <id>` 拉起，不走这条。
    NoticeFocus { notice_id: String },
    /// 管家窗口打开/获得焦点：宿主据此清掉托盘提示（会话已读标记由
    /// butlerd 在 `ChatSeen` 里写）。
    ButlerSeen,
    /// 管家窗口请宿主拉起守护进程（离线横幅的「重试」按钮）。
    /// 守护进程的生命周期始终归宿主：窗口不自己 spawn。
    StartButler,
    /// 宿主请已打开的主窗口刷新数据（托盘菜单「更新数据」）。
    RefreshData,
    /// 管家窗口 → butlerd：挂载会话（双向长连接上的第一帧），挂上即回
    /// 全量快照。连接断开就是 detach，没有显式的告别帧。
    ChatAttach,
    /// 管家窗口 → butlerd：用户发了一句话。回合进行中会被明确拒绝（快照带
    /// `restore` 让窗口把话放回输入框），不排队。
    ChatSend { text: String },
    /// 管家窗口 → butlerd：用户确认清空会话。
    ChatClear,
    /// 管家窗口 → butlerd：窗口已打开，标记会话已读。
    ChatSeen,
    /// butlerd → 管家窗口：会话全量快照。`json` 是
    /// `butler::chat::ChatSnapshotData` 的序列化——session、等待态、半截
    /// 正文、提示与错误一帧带全，窗口不维护第二份增量状态。
    ChatSnapshot { json: String },
    /// butlerd → 管家窗口：流式增量正文（只推 Message 正文块）。
    ChatDelta { text: String },
    /// 请守护进程退出（托盘里的「停止管家进程」）。
    Shutdown,
    /// 主动告别，不必等连接断开。
    Bye,
}

/// 服务端收到的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// 新连接。id 在进程内唯一，用来对上后面的断开。
    Connected(u64),
    /// 某条连接发来的请求。
    Signal(u64, Signal),
    /// 某条连接断开。守护进程崩溃也走这里。
    Disconnected(u64),
}

/// 监听端。必须存活到进程退出；`Drop` 会删掉 socket 文件（Windows 的命名
/// 管道随最后一个句柄关闭而消失，不需要清理）。
///
/// P4 起监听端还能**向某条连接推送**（`send_to`）：管家会话的快照与增量
/// 都从 butlerd 主动发往窗口的长连接。每条连接有一条有界出站队列——窗口
/// 读得慢不能阻塞 butlerd 的回合推进，队列满了就断开这条连接，窗口重连
/// 拿快照，不丢数据。
pub struct Listener {
    /// 只为了活着：`Drop` 里删 socket 文件。
    #[cfg(unix)]
    _inner: unix_impl::Listener,
    #[cfg(windows)]
    _inner: windows_impl::Listener,
}

impl Listener {
    /// 主动释放实例锁：删掉自己的 socket 文件，后续 `Drop` 不再重复。
    ///
    /// 退出前调用一次就够了：GPUI 的 `App` 全局表在事件循环结束时未必析构
    /// （任务与回调里还挂着引用），只靠 `Drop` 会把 `*.sock` 留在磁盘上。
    pub fn release(&mut self) {
        #[cfg(unix)]
        self._inner.release();
    }

    /// 向某条已连接的对端推送一帧。返回 `false` 表示连接不在了，或它的
    /// 出站队列已满——满即断开（见模块注释），调用方不需要善后：对端会
    /// 重连拿快照。
    pub fn send_to(&self, conn_id: u64, signal: &Signal) -> bool {
        #[cfg(unix)]
        {
            self._inner.send_to(conn_id, signal)
        }
        #[cfg(windows)]
        {
            self._inner.send_to(conn_id, signal)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (conn_id, signal);
            false
        }
    }
}

/// 连接注册表：`conn_id → 有界出站队列`。
///
/// accept 线程插入，写线程退出 / 读线程退出时移除；`send_to` 在队列满时
/// 也会移除（丢弃 sender 让写线程的 `recv` 返回断开，连接随之关闭）。
/// 两个平台共用同一份实现。
mod registry {
    use super::Signal;
    use std::collections::HashMap;
    use std::sync::mpsc::{SyncSender, sync_channel};
    use std::sync::Mutex;

    /// 每条连接的出站队列深度。快照最大 1MB，64 帧的内存上限足够吸收
    /// 一次慢读的突发；再堵说明对端已经不读，断开重连比无限积压好。
    pub(super) const OUTBOUND_QUEUE: usize = 64;

    #[derive(Default)]
    pub(super) struct Registry {
        senders: Mutex<HashMap<u64, SyncSender<Signal>>>,
    }

    impl Registry {
        pub(super) fn insert(&self, id: u64, sender: SyncSender<Signal>) {
            self.senders
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(id, sender);
        }

        pub(super) fn remove(&self, id: u64) {
            self.senders
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&id);
        }

        /// 队列满也返回 `false` 并把这条连接拆掉：慢读者拿到断线后重连、
        /// 重收全量快照，服务端回合不等待任何客户端。
        pub(super) fn send(&self, id: u64, signal: &Signal) -> bool {
            let guard = self.senders.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            match guard.get(&id) {
                Some(sender) => match sender.try_send(signal.clone()) {
                    Ok(()) => true,
                    Err(std::sync::mpsc::TrySendError::Full(_)) => {
                        drop(guard);
                        self.remove(id);
                        false
                    }
                    Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                        drop(guard);
                        self.remove(id);
                        false
                    }
                },
                None => false,
            }
        }
    }

    /// 建一条连接的出站队列。
    pub(super) fn channel() -> (SyncSender<Signal>, std::sync::mpsc::Receiver<Signal>) {
        sync_channel(OUTBOUND_QUEUE)
    }
}

/// 连接失败的原因，调用方只需要区分「有人占着」和「没人」。
#[derive(Debug)]
pub enum ConnectError {
    /// 没有实例在监听。
    Absent,
    /// 平台不支持。
    Unsupported,
    /// 其他 IO 错误。
    Io(std::io::Error),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::Absent => f.write_str("没有进程在监听"),
            ConnectError::Unsupported => f.write_str("本平台暂不支持本地唤醒通道"),
            ConnectError::Io(error) => write!(f, "{error}"),
        }
    }
}

/// 监听 `app` 的唤醒通道。已有实例在监听时返回 `AlreadyListening`。
pub fn listen(app: &str) -> std::io::Result<(Listener, UnboundedReceiver<Event>)> {
    let (tx, rx) = unbounded_channel();
    #[cfg(unix)]
    {
        let listener = unix_impl::listen(app, tx)?;
        Ok((Listener { _inner: listener }, rx))
    }
    #[cfg(windows)]
    {
        let listener = windows_impl::listen(app, tx)?;
        Ok((Listener { _inner: listener }, rx))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (app, tx);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "本地唤醒通道暂只支持 Unix 与 Windows",
        ))
    }
}

/// 连接已有的服务端，并立即发送一条请求。
pub fn poke(app: &str, signal: &Signal) -> Result<(), ConnectError> {
    #[cfg(unix)]
    {
        unix_impl::poke(app, signal)
    }
    #[cfg(windows)]
    {
        windows_impl::poke(app, signal)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (app, signal);
        Err(ConnectError::Unsupported)
    }
}

/// 叫醒管家守护进程：状态变了，重算下一次该在什么时候醒。
///
/// 守护进程不扫盘，只认「算好的时刻」和「有人叫醒」。所有写管家的地方
/// （GUI 记完账、CLI 加了任务/记住了承诺）都应该走这一个入口，否则用户要
/// 等到下一次定时巡检才看见效果。没有守护进程时静默失败。
pub fn poke_daemon() {
    let _ = poke("mns-butlerd", &Signal::Wake { ledger: None });
}

/// 长期连接的客户端（守护进程用）。
pub struct Client {
    #[cfg(unix)]
    inner: unix_impl::Client,
    #[cfg(windows)]
    inner: windows_impl::Client,
}

impl Client {
    pub fn send(&mut self, signal: &Signal) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            self.inner.send(signal)
        }
        #[cfg(windows)]
        {
            self.inner.send(signal)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = signal;
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "本地唤醒通道暂只支持 Unix 与 Windows",
            ))
        }
    }
}

/// 连接服务端并保持连接。断开即代表本进程退出。
pub fn connect(app: &str) -> Result<Client, ConnectError> {
    #[cfg(unix)]
    {
        unix_impl::connect(app).map(|inner| Client { inner })
    }
    #[cfg(windows)]
    {
        windows_impl::connect(app).map(|inner| Client { inner })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = app;
        Err(ConnectError::Unsupported)
    }
}

/// 连接服务端并保持**双向**连接：返回可写的 `Client` 和服务端推送的接收端。
///
/// 管家窗口对 `mns-butlerd` 用这一条连接收发会话：上行 `Chat*` 请求走
/// `Client::send`，下行的快照与增量从返回的通道里读。连接断开（服务端
/// 退出、队列溢出被拆线）时通道关闭——调用方据此进入离线态或重连。
pub fn connect_duplex(
    app: &str,
) -> Result<(Client, UnboundedReceiver<Signal>), ConnectError> {
    #[cfg(unix)]
    {
        match unix_impl::connect(app) {
            Ok(inner) => {
                let (tx, rx) = unbounded_channel();
                if let Err(error) = inner.start_reader(tx) {
                    return Err(ConnectError::Io(error));
                }
                Ok((Client { inner }, rx))
            }
            Err(error) => Err(error),
        }
    }
    #[cfg(windows)]
    {
        match windows_impl::connect(app) {
            Ok(inner) => {
                let (tx, rx) = unbounded_channel();
                if let Err(error) = inner.start_reader(tx) {
                    return Err(ConnectError::Io(error));
                }
                Ok((Client { inner }, rx))
            }
            Err(error) => Err(error),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = app;
        Err(ConnectError::Unsupported)
    }
}

fn encode(signal: &Signal) -> String {
    let value = match signal {
        Signal::Open => serde_json::json!({ "kind": "open" }),
        Signal::OpenButler => serde_json::json!({ "kind": "open-butler" }),
        Signal::Hello { role } => serde_json::json!({ "kind": "hello", "role": role }),
        Signal::Wake { ledger } => serde_json::json!({ "kind": "wake", "ledger": ledger }),
        Signal::SetProactive { enabled } => {
            serde_json::json!({ "kind": "set-proactive", "enabled": enabled })
        }
        Signal::RequestBrief => serde_json::json!({ "kind": "request-brief" }),
        Signal::Brief { json } => serde_json::json!({ "kind": "brief", "json": json }),
        Signal::OpenPage { page } => serde_json::json!({ "kind": "open-page", "page": page }),
        Signal::OpenAbout => serde_json::json!({ "kind": "open-about" }),
        Signal::DeliverNotice { json } => {
            serde_json::json!({ "kind": "deliver-notice", "json": json })
        }
        Signal::NoticeFocus { notice_id } => {
            serde_json::json!({ "kind": "notice-focus", "notice_id": notice_id })
        }
        Signal::ButlerSeen => serde_json::json!({ "kind": "butler-seen" }),
        Signal::StartButler => serde_json::json!({ "kind": "start-butler" }),
        Signal::RefreshData => serde_json::json!({ "kind": "refresh-data" }),
        Signal::ChatAttach => serde_json::json!({ "kind": "chat-attach" }),
        Signal::ChatSend { text } => serde_json::json!({ "kind": "chat-send", "text": text }),
        Signal::ChatClear => serde_json::json!({ "kind": "chat-clear" }),
        Signal::ChatSeen => serde_json::json!({ "kind": "chat-seen" }),
        Signal::ChatSnapshot { json } => {
            serde_json::json!({ "kind": "chat-snapshot", "json": json })
        }
        Signal::ChatDelta { text } => serde_json::json!({ "kind": "chat-delta", "text": text }),
        Signal::Shutdown => serde_json::json!({ "kind": "shutdown" }),
        Signal::Bye => serde_json::json!({ "kind": "bye" }),
    };
    format!("{value}\n")
}

fn decode(line: &str) -> Option<Signal> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    match value.get("kind")?.as_str()? {
        "open" => Some(Signal::Open),
        "open-butler" => Some(Signal::OpenButler),
        "hello" => Some(Signal::Hello {
            role: value
                .get("role")
                .and_then(|role| role.as_str())
                .unwrap_or("unknown")
                .to_owned(),
        }),
        "wake" => Some(Signal::Wake {
            ledger: value
                .get("ledger")
                .and_then(|ledger| ledger.as_str())
                .map(str::to_owned),
        }),
        "set-proactive" => Some(Signal::SetProactive {
            enabled: value.get("enabled").and_then(|flag| flag.as_bool())?,
        }),
        "request-brief" => Some(Signal::RequestBrief),
        "brief" => Some(Signal::Brief {
            json: value.get("json").and_then(|json| json.as_str())?.to_owned(),
        }),
        "open-page" => Some(Signal::OpenPage {
            page: value.get("page").and_then(|page| page.as_str())?.to_owned(),
        }),
        "open-about" => Some(Signal::OpenAbout),
        "deliver-notice" => Some(Signal::DeliverNotice {
            json: value.get("json").and_then(|json| json.as_str())?.to_owned(),
        }),
        "notice-focus" => Some(Signal::NoticeFocus {
            notice_id: value
                .get("notice_id")
                .and_then(|id| id.as_str())?
                .to_owned(),
        }),
        "butler-seen" => Some(Signal::ButlerSeen),
        "start-butler" => Some(Signal::StartButler),
        "refresh-data" => Some(Signal::RefreshData),
        "chat-attach" => Some(Signal::ChatAttach),
        "chat-send" => Some(Signal::ChatSend {
            text: value.get("text").and_then(|text| text.as_str())?.to_owned(),
        }),
        "chat-clear" => Some(Signal::ChatClear),
        "chat-seen" => Some(Signal::ChatSeen),
        "chat-snapshot" => Some(Signal::ChatSnapshot {
            json: value.get("json").and_then(|json| json.as_str())?.to_owned(),
        }),
        "chat-delta" => Some(Signal::ChatDelta {
            text: value.get("text").and_then(|text| text.as_str())?.to_owned(),
        }),
        "shutdown" => Some(Signal::Shutdown),
        "bye" => Some(Signal::Bye),
        _ => None,
    }
}

#[cfg(unix)]
mod unix_impl {
    use super::{Event, MAX_LINE, Signal, decode, encode, registry};
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::net::Shutdown;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use tokio::sync::mpsc::UnboundedSender;

    pub(super) fn socket_path(app: &str) -> PathBuf {
        // 单测落在临时目录：`~/.mns/run` 是真实账户目录，不该被测试碰。
        #[cfg(test)]
        let base = super::tests::test_run_dir();
        #[cfg(not(test))]
        let base = crate::config::resolve_data_path("run");
        base.join(format!("{app}.sock"))
    }

    pub(super) struct Listener {
        path: PathBuf,
        /// bind 时拿到的 inode / dev。`Drop` 只删「还是我那一份」的 socket：
        /// 进程快速关停时，后继进程可能已经清理残file并重新 bind，无条件
        /// 删除会把新人刚建好的 socket 拽掉，窗口就再也 poke 不到了。
        identity: Option<(u64, u64)>,
        /// 连接注册表：`send_to` 定向推送用。
        registry: Arc<registry::Registry>,
    }

    impl Listener {
        fn identity_of(path: &Path) -> Option<(u64, u64)> {
            use std::os::unix::fs::MetadataExt;
            fs::symlink_metadata(path)
                .ok()
                .map(|meta| (meta.dev(), meta.ino()))
        }

        pub(super) fn release(&mut self) {
            if Self::identity_of(&self.path) == self.identity {
                let _ = fs::remove_file(&self.path);
            }
            // 置空后 `Drop` 直接返回，不会误删后继进程刚 bind 的文件。
            self.identity = None;
        }

        pub(super) fn send_to(&self, conn_id: u64, signal: &Signal) -> bool {
            self.registry.send(conn_id, signal)
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            // Best effort：进程正常退出时把 socket 文件清掉，避免下次启动
            // 先探活再删。异常退出留下的残file由 `listen` 的探活处理。
            if Self::identity_of(&self.path) != self.identity {
                return;
            }
            let _ = fs::remove_file(&self.path);
        }
    }

    fn prepare_dir(path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
            // 只允许本用户进出：能连上来的人就能要求「打开窗口」。
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
        Ok(())
    }

    pub(super) fn listen(
        app: &str,
        tx: UnboundedSender<Event>,
    ) -> std::io::Result<Listener> {
        let path = socket_path(app);
        prepare_dir(&path)?;
        // 先探活：连得上说明有活实例，本次不抢。连不上只可能是崩溃残留，
        // 删掉重建——不做这一步，守护进程一崩就再也起不来。
        if UnixStream::connect(&path).is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                "已有实例在监听",
            ));
        }
        let _ = fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        let identity = Listener::identity_of(&path);

        let registry = Arc::new(registry::Registry::default());
        let thread_registry = registry.clone();

        std::thread::Builder::new()
            .name("mns-ipc-accept".to_owned())
            .spawn(move || {
                let mut next_id: u64 = 0;
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    next_id += 1;
                    let id = next_id;
                    if tx.send(Event::Connected(id)).is_err() {
                        return;
                    }
                    // 双向连接：读线程与写线程各持一份克隆的流。读线程解析
                    // 上行帧；写线程从有界出站队列取帧推送，队列满即拆线。
                    let Ok(write_half) = stream.try_clone() else {
                        let _ = tx.send(Event::Disconnected(id));
                        continue;
                    };
                    let (out_tx, out_rx) = registry::channel();
                    thread_registry.insert(id, out_tx);
                    let read_registry = thread_registry.clone();
                    // 每条连接两个线程：一个卡住的客户端不能堵住 accept，
                    // 一个读得慢的客户端也不能堵住服务端推送。
                    let tx = tx.clone();
                    let _ = std::thread::Builder::new()
                        .name(format!("mns-ipc-{id}"))
                        .spawn(move || {
                            read_connection(id, stream, tx);
                            read_registry.remove(id);
                        });
                    let _ = std::thread::Builder::new()
                        .name(format!("mns-ipc-{id}-w"))
                        .spawn(move || write_connection(write_half, out_rx));
                }
            })?;

        Ok(Listener {
            path,
            identity,
            registry,
        })
    }

    fn read_connection(id: u64, stream: UnixStream, tx: UnboundedSender<Event>) {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if line.len() > MAX_LINE {
                        break;
                    }
                    if let Some(signal) = decode(&line)
                        && tx.send(Event::Signal(id, signal)).is_err()
                    {
                        break;
                    }
                }
            }
        }
        let _ = tx.send(Event::Disconnected(id));
    }

    /// 服务端推送线程：从有界队列取帧写出。队列被拆（慢读者）或对端断开
    /// 都直接退出——退出即关闭写半边，对端读侧看到 EOF 自行重连。
    fn write_connection(stream: UnixStream, rx: std::sync::mpsc::Receiver<Signal>) {
        let mut writer = stream;
        while let Ok(signal) = rx.recv() {
            let line = encode(&signal);
            if writer.write_all(line.as_bytes()).is_err()
                || writer.flush().is_err()
            {
                break;
            }
        }
    }

    pub(super) struct Client {
        stream: UnixStream,
    }

    impl Drop for Client {
        fn drop(&mut self) {
            // 关闭即 detach：`shutdown` 作用于 socket 本身，而不是某个 fd。
            // 只 drop 手里这份 fd 不够——`connect_duplex` 的读线程持有同一
            // socket 的克隆，连接会保持半开：服务端看不到 EOF（永远收不到
            // Disconnected），读线程也永远等不来数据。`Both` 让两个方向
            // 同时收摊：服务端读侧立刻拿到 EOF，本进程读线程也随即退出并
            // 关闭下行通道。短连接（poke）同样受益：发完即 FIN。
            let _ = self.stream.shutdown(Shutdown::Both);
        }
    }

    impl Client {
        pub(super) fn send(&mut self, signal: &Signal) -> std::io::Result<()> {
            self.stream.write_all(encode(signal).as_bytes())?;
            self.stream.flush()
        }

        /// 双向连接的读半边：独立线程持续读行，解码后送进通道。对端断开
        /// （或被服务端拆线）时通道关闭，调用方感知为「连接没了」。
        pub(super) fn start_reader(
            &self,
            tx: UnboundedSender<Signal>,
        ) -> std::io::Result<()> {
            let stream = self.stream.try_clone()?;
            std::thread::Builder::new()
                .name("mns-ipc-client-read".to_owned())
                .spawn(move || {
                    let mut reader = BufReader::new(stream);
                    let mut line = String::new();
                    loop {
                        line.clear();
                        match reader.read_line(&mut line) {
                            Ok(0) | Err(_) => break,
                            Ok(_) => {
                                if line.len() > MAX_LINE {
                                    break;
                                }
                                if let Some(signal) = decode(&line)
                                    && tx.send(signal).is_err()
                                {
                                    break;
                                }
                            }
                        }
                    }
                })
                .map(|_| ())
        }
    }

    pub(super) fn connect(app: &str) -> Result<Client, super::ConnectError> {
        let path = socket_path(app);
        match UnixStream::connect(&path) {
            Ok(stream) => Ok(Client { stream }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(super::ConnectError::Absent)
            }
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                Err(super::ConnectError::Absent)
            }
            Err(error) => Err(super::ConnectError::Io(error)),
        }
    }

    pub(super) fn poke(app: &str, signal: &Signal) -> Result<(), super::ConnectError> {
        let mut client = connect(app)?;
        client
            .send(signal)
            .map_err(super::ConnectError::Io)
    }
}

/// Windows：命名管道。
///
/// 选它而不是继续用命名事件，是因为它和 Unix socket 是同一个东西：一条双向
/// 字节流，能承载 NDJSON，也能用「连接关闭」当存活信号。命名事件只能传一个
/// 比特，还得靠轮询去读——正好是这次要去掉的东西。
///
/// 名称带用户名（见 `pipe_name`）：管道是整机全局的，不像 socket 文件那样
/// 天然按用户目录隔离。
#[cfg(windows)]
mod windows_impl {
    use super::{Event, MAX_LINE, Signal, decode, encode, registry};
    use std::ffi::c_void;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
    use std::sync::Arc;
    use tokio::sync::mpsc::UnboundedSender;

    /// Win32 句柄。管道句柄可以交给 `std::fs::File` 托管：有了 `Read` /
    /// `Write`，读行逻辑就能和 Unix 版共用同一套写法。
    type Handle = *mut c_void;

    const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
    /// 只让第一条实例占名字：已有进程在监听时 `CreateNamedPipeW` 直接失败，
    /// 不必先连一次探活——探活与真正监听之间有竞态，可能起出两个监听者。
    const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x0008_0000;
    const PIPE_TYPE_BYTE: u32 = 0x0000_0000;
    const PIPE_READMODE_BYTE: u32 = 0x0000_0000;
    const PIPE_WAIT: u32 = 0x0000_0000;
    /// 只接本机客户端：这不是网络服务，没理由让同网段的机器连过来。
    const PIPE_REJECT_REMOTE_CLIENTS: u32 = 0x0000_0008;
    const PIPE_UNLIMITED_INSTANCES: u32 = 255;
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const OPEN_EXISTING: u32 = 3;

    const ERROR_FILE_NOT_FOUND: i32 = 2;
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_PIPE_BUSY: i32 = 231;
    const ERROR_PIPE_CONNECTED: i32 = 535;
    /// 客户端在服务端调用 ConnectNamedPipe 之前连了又断。GUI 的探活与
    /// poke 都是「连上就走」的短连接，这是常态路径而非故障：accept 循环
    /// 据此换一条新实例继续等，而不是退出——退出会让进程变聋，被 GUI
    /// 重复拉起后进程就攒起来。
    ///
    /// 生产代码不按错误码分支（所有 `wait_for_client` 错误一律换实例
    /// 重来），这个常量唯一的存在意义是给回归测试
    /// `error_no_data_之后必须换实例继续监听` 钉住「客户端先断必报
    /// 232」的契约，所以非测试编译允许它无引用。
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) const ERROR_NO_DATA: i32 = 232;
    const ERROR_BROKEN_PIPE: i32 = 109;
    const ERROR_PIPE_NOT_CONNECTED: i32 = 233;

    const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateNamedPipeW(
            name: *const u16,
            open_mode: u32,
            pipe_mode: u32,
            max_instances: u32,
            out_buffer_size: u32,
            in_buffer_size: u32,
            default_timeout: u32,
            attributes: *const c_void,
        ) -> Handle;
        fn ConnectNamedPipe(pipe: Handle, overlapped: *mut c_void) -> i32;
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            attributes: *const c_void,
            disposition: u32,
            flags: u32,
            template: Handle,
        ) -> Handle;
        fn WaitNamedPipeW(name: *const u16, timeout: u32) -> i32;
        /// 只看管道里有没有数据，不消费。同步管道上不能长期挂着 `ReadFile`：
        /// 同一文件对象的 I/O 是串行的，挂起的读会把另一个线程的写一起卡住。
        fn PeekNamedPipe(
            pipe: Handle,
            buffer: *mut c_void,
            buffer_size: u32,
            bytes_read: *mut u32,
            total_bytes_avail: *mut u32,
            bytes_left_this_message: *mut u32,
        ) -> i32;
        /// 取消指定句柄上挂起的 I/O。dup 出去的读句柄指向同一管道文件对象，
        /// 在任一克隆上调用都能打断另一克隆里阻塞中的 `ReadFile`。
        fn CancelIoEx(handle: Handle, overlapped: *mut c_void) -> i32;
    }

    /// 管道名：`\\.\pipe\mns-<app>-<用户名>`。
    ///
    /// 用户名进名字是因为命名管道是**整机全局**的，而 Unix socket 在
    /// `~/.mns/run` 里天然按用户隔离。不隔离的话，同一台机器上第二个用户
    /// 启动 MNS 会撞上第一个用户的管道，然后双方都以为对面「已经有人」。
    pub(super) fn pipe_name(app: &str) -> String {
        let user = std::env::var("USERNAME").unwrap_or_else(|_| "default".to_owned());
        let user: String = user
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
            .collect();
        format!(r"\\.\pipe\mns-{app}-{user}")
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub(super) struct Listener {
        /// 连接注册表：`send_to` 定向推送用。
        registry: Arc<registry::Registry>,
    }

    impl Listener {
        pub(super) fn send_to(&self, conn_id: u64, signal: &Signal) -> bool {
            self.registry.send(conn_id, signal)
        }
    }

    pub(super) fn create_instance(name: &str, first: bool) -> std::io::Result<File> {
        let name = wide(name);
        let mut open_mode = PIPE_ACCESS_DUPLEX;
        if first {
            open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        // 默认安全描述符：创建者（本用户）与管理员可访问，其他用户连不上。
        // 和 Unix 那边 socket 目录的 0700 是同一个意思。
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                MAX_LINE as u32,
                MAX_LINE as u32,
                0,
                std::ptr::null(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            let error = std::io::Error::last_os_error();
            // 名字已被占用：已有实例在监听。
            let kind = if error.raw_os_error() == Some(ERROR_ACCESS_DENIED) {
                std::io::ErrorKind::AddrInUse
            } else {
                error.kind()
            };
            return Err(std::io::Error::new(kind, error.to_string()));
        }
        // SAFETY: 句柄刚由 `CreateNamedPipeW` 创建，所有权交给 `File` 托管。
        Ok(unsafe { File::from_raw_handle(handle as RawHandle) })
    }

    /// 等一个客户端连上来。非重叠模式下阻塞到连接建立；客户端抢先连上时
    /// 返回 `ERROR_PIPE_CONNECTED`——那是成功，不是失败。
    /// `pub(super)` 是给回归测试直接驱动用的。
    pub(super) fn wait_for_client(pipe: &File) -> std::io::Result<()> {
        let ok = unsafe { ConnectNamedPipe(pipe.as_raw_handle() as Handle, std::ptr::null_mut()) };
        if ok != 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_PIPE_CONNECTED) {
            return Ok(());
        }
        Err(error)
    }

    pub(super) fn listen(app: &str, tx: UnboundedSender<Event>) -> std::io::Result<Listener> {
        let name = pipe_name(app);
        let mut pending = create_instance(&name, true)?;

        let registry = Arc::new(registry::Registry::default());
        let thread_registry = registry.clone();

        std::thread::Builder::new()
            .name("mns-ipc-accept".to_owned())
            .spawn(move || {
                let mut next_id: u64 = 0;
                loop {
                    if wait_for_client(&pending).is_err() {
                        // 客户端在「实例建好」与「本线程调 ConnectNamedPipe」之间
                        // 连了又断——GUI 启动时的探活（连上就走）与紧随其后的
                        // poke 都是这样的短连接，ConnectNamedPipe 会报
                        // ERROR_NO_DATA。这是常态路径，不是故障：换一条新实例
                        // 接着等，绝不能就此退出。accept 线程一死，本进程就
                        // 成了「活着但谁也连不上」的聋子——GUI 下次启动探活
                        // 失败会再拉一个新的，进程就攒起来了。管道名仍被手里的
                        // 实例占着，重建不会撞出第二个监听者。
                        pending = match next_instance(&name) {
                            Some(next) => next,
                            None => return,
                        };
                        continue;
                    }
                    // 先把下一条实例建出来，再处理刚连上的这条：否则读取期间
                    // 名字上没有任何空闲实例，第二次连接会撞 `ERROR_PIPE_BUSY`。
                    let next = match next_instance(&name) {
                        Some(file) => file,
                        // 连接已到手但换不出下一条实例：进程已没有能力继续
                        // 监听，退场交给上层收摊（事件通道关闭后调用方退出）。
                        None => return,
                    };
                    let connected = std::mem::replace(&mut pending, next);
                    next_id += 1;
                    let id = next_id;
                    if tx.send(Event::Connected(id)).is_err() {
                        return;
                    }
                    // 双向连接：读线程与写线程各持一份克隆的句柄。读线程解析
                    // 上行帧；写线程从有界出站队列取帧推送，队列满即拆线。
                    let Ok(write_half) = connected.try_clone() else {
                        let _ = tx.send(Event::Disconnected(id));
                        continue;
                    };
                    let (out_tx, out_rx) = registry::channel();
                    thread_registry.insert(id, out_tx);
                    let read_registry = thread_registry.clone();
                    let tx = tx.clone();
                    let _ = std::thread::Builder::new()
                        .name(format!("mns-ipc-{id}"))
                        .spawn(move || {
                            read_connection(id, connected, tx);
                            read_registry.remove(id);
                        });
                    let _ = std::thread::Builder::new()
                        .name(format!("mns-ipc-{id}-w"))
                        .spawn(move || write_connection(write_half, out_rx));
                }
            })?;

        Ok(Listener { registry })
    }

    /// 建下一条管道实例：成功路径用它「先建好下一条再收当前连接」，
    /// `ERROR_NO_DATA` 后用它顶替已不可用的那条。
    ///
    /// 失败按 200ms 退避重试，累计 5 秒仍失败就放弃（返回 `None`）：此时进程
    /// 已经没有能力继续监听，让 accept 线程退出、事件通道随之关闭，比留一个
    /// 谁也连不上的聋子进程好。守护进程在 Windows 上由 GUI 拉起且没有控制台，
    /// stderr 会被丢弃：只在放弃时留一行，不在每次重试时刷屏。
    pub(super) fn next_instance(name: &str) -> Option<File> {
        for _ in 0..25 {
            match create_instance(name, false) {
                Ok(file) => return Some(file),
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(200)),
            }
        }
        eprintln!("mns-ipc: {name} 连续 5 秒建不出下一条实例，放弃监听");
        None
    }

    /// 轮询式行读取器。先 `PeekNamedPipe` 看有没有数据，有才 `ReadFile`。
    ///
    /// Windows 的同步（非 OVERLAPPED）管道句柄在同一个文件对象上串行化 I/O：
    /// 读线程挂起的 `ReadFile` 会把写线程的 `WriteFile` 一起卡住。双向连接
    /// （管家窗口 ↔ `mns-butlerd`）只要一挂读，另一头就再也写不出去——窗口
    /// 表现为「发消息没反应」。轮询让 `ReadFile` 只在确有数据时发出，从不
    /// 长期挂起；20ms 的间隔对本地会话足够。
    struct LineReader {
        pipe: File,
        pending: Vec<u8>,
        chunk: Vec<u8>,
    }

    impl LineReader {
        fn new(pipe: File) -> Self {
            Self {
                pipe,
                pending: Vec::new(),
                chunk: Vec::new(),
            }
        }

        /// 读到一行（去掉行尾 `\r\n`）；对端关闭返回 `None`。
        fn next_line(&mut self) -> std::io::Result<Option<String>> {
            loop {
                if let Some(index) = self.pending.iter().position(|byte| *byte == b'\n') {
                    let mut raw: Vec<u8> = self.pending.drain(..=index).collect();
                    if raw.last() == Some(&b'\n') {
                        raw.pop();
                    }
                    if raw.last() == Some(&b'\r') {
                        raw.pop();
                    }
                    return Ok(Some(String::from_utf8_lossy(&raw).into_owned()));
                }
                if self.pending.len() > MAX_LINE {
                    return Ok(None);
                }
                let mut available: u32 = 0;
                let ok = unsafe {
                    PeekNamedPipe(
                        self.pipe.as_raw_handle() as Handle,
                        std::ptr::null_mut(),
                        0,
                        std::ptr::null_mut(),
                        &mut available,
                        std::ptr::null_mut(),
                    )
                };
                if ok == 0 {
                    let error = std::io::Error::last_os_error();
                    return match error.raw_os_error() {
                        Some(ERROR_BROKEN_PIPE) | Some(ERROR_PIPE_NOT_CONNECTED) => Ok(None),
                        _ => Err(error),
                    };
                }
                if available == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    continue;
                }
                let want = (available as usize).min(MAX_LINE);
                self.chunk.resize(want, 0);
                let count = self.pipe.read(&mut self.chunk)?;
                if count == 0 {
                    return Ok(None);
                }
                self.pending.extend_from_slice(&self.chunk[..count]);
            }
        }
    }

    fn read_connection(id: u64, pipe: File, tx: UnboundedSender<Event>) {
        let mut reader = LineReader::new(pipe);
        loop {
            match reader.next_line() {
                Ok(Some(line)) => {
                    if let Some(signal) = decode(&line)
                        && tx.send(Event::Signal(id, signal)).is_err()
                    {
                        break;
                    }
                }
                // 对端关闭（`ERROR_BROKEN_PIPE`）与读失败都到此为止。
                Ok(None) | Err(_) => break,
            }
        }
        let _ = tx.send(Event::Disconnected(id));
    }

    /// 服务端推送线程：从有界队列取帧写出。队列被拆（慢读者）或对端断开
    /// （`WriteFile` 报 `ERROR_BROKEN_PIPE`）都直接退出——退出即放弃句柄，
    /// 对端读侧看到 EOF 自行重连。
    fn write_connection(pipe: File, rx: std::sync::mpsc::Receiver<Signal>) {
        let mut writer = pipe;
        while let Ok(signal) = rx.recv() {
            let line = encode(&signal);
            if writer.write_all(line.as_bytes()).is_err() || writer.flush().is_err() {
                break;
            }
        }
    }

    pub(super) struct Client {
        pipe: File,
    }

    impl Drop for Client {
        fn drop(&mut self) {
            // 关闭即 detach（与 Unix 的 `shutdown(Both)` 对应）：读线程持有
            // 同一管道对象 dup 出来的句柄，单纯 drop 手里这份不会让阻塞中的
            // `ReadFile` 返回。CancelIoEx 打断挂起的读，读线程随即退出并
            // 关闭下行通道；两边句柄都放手后服务端下一次读看到断管。
            // 没有挂起 I/O 时调用只会失败（ERROR_NOT_FOUND），无害。
            unsafe {
                CancelIoEx(self.pipe.as_raw_handle() as Handle, std::ptr::null_mut());
            }
        }
    }

    impl Client {
        pub(super) fn send(&mut self, signal: &Signal) -> std::io::Result<()> {
            self.pipe.write_all(encode(signal).as_bytes())?;
            self.pipe.flush()
        }

        /// 双向连接的读半边：独立线程持续读行，解码后送进通道。对端断开
        /// （或被服务端拆线）时通道关闭，调用方感知为「连接没了」。
        pub(super) fn start_reader(
            &self,
            tx: UnboundedSender<Signal>,
        ) -> std::io::Result<()> {
            let pipe = self.pipe.try_clone()?;
            std::thread::Builder::new()
                .name("mns-ipc-client-read".to_owned())
                .spawn(move || {
                    let mut reader = LineReader::new(pipe);
                    loop {
                        match reader.next_line() {
                            Ok(Some(line)) => {
                                if let Some(signal) = decode(&line)
                                    && tx.send(signal).is_err()
                                {
                                    break;
                                }
                            }
                            // 对端关闭（`ERROR_BROKEN_PIPE`）与读失败都到此为止。
                            Ok(None) | Err(_) => break,
                        }
                    }
                })
                .map(|_| ())
        }
    }

    fn open(app: &str) -> Result<File, super::ConnectError> {
        let name = wide(&pipe_name(app));
        // 服务端「刚连上一条、还没建下一条」的瞬间会报 `ERROR_PIPE_BUSY`：
        // 等一小会儿再试一次，别白白丢掉一次唤醒。
        for attempt in 0..2 {
            let handle = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                )
            };
            if handle != INVALID_HANDLE_VALUE {
                // SAFETY: 句柄由 `CreateFileW` 打开，所有权交给 `File` 托管。
                return Ok(unsafe { File::from_raw_handle(handle as RawHandle) });
            }
            let error = std::io::Error::last_os_error();
            match error.raw_os_error() {
                // 名字本身不存在 = 没有进程在监听。
                Some(ERROR_FILE_NOT_FOUND) => return Err(super::ConnectError::Absent),
                Some(ERROR_PIPE_BUSY) if attempt == 0 => {
                    // 100ms 足够服务端把下一条实例建出来。
                    unsafe { WaitNamedPipeW(name.as_ptr(), 100) };
                }
                Some(ERROR_PIPE_BUSY) => return Err(super::ConnectError::Absent),
                _ => return Err(super::ConnectError::Io(error)),
            }
        }
        Err(super::ConnectError::Absent)
    }

    pub(super) fn connect(app: &str) -> Result<Client, super::ConnectError> {
        open(app).map(|pipe| Client { pipe })
    }

    pub(super) fn poke(app: &str, signal: &Signal) -> Result<(), super::ConnectError> {
        let mut client = connect(app)?;
        client.send(signal).map_err(super::ConnectError::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 单测专用的 socket 目录：进程内一次性建好，与真实账户目录
    /// （`~/.mns/run`）隔离——测试不该往用户的 `.mns` 里落 socket 文件。
    ///
    /// 只用**一个**固定名字的目录（不带 pid），跨测试轮次复用：不然每次
    /// `cargo test` 都会往系统临时目录里丢一个 `mns-ipc-tests-<pid>`，
    /// 永远攒下去。目录里的 socket 文件名带 pid，并发跑两份测试互不干扰；
    /// 残留的 socket 也不碍事——`listen` 对连不上的残留文件本来就是删了重建。
    /// 建目录失败直接 panic：静默退回真实的 `~/.mns/run` 恰恰是这条隔离
    /// 要挡住的污染。仅 Unix 需要：Windows 的命名管道不落文件系统。
    #[cfg(unix)]
    pub(super) fn test_run_dir() -> std::path::PathBuf {
        static DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
        DIR.get_or_init(|| {
            let dir = std::env::temp_dir().join("mns-ipc-tests");
            if let Err(error) = std::fs::create_dir_all(&dir) {
                panic!("单测建不出隔离 socket 目录 {dir:?}：{error}");
            }
            dir
        })
        .clone()
    }

    #[test]
    fn 协议是单行_ndjson() {
        let line = encode(&Signal::OpenButler);
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1);
        assert_eq!(decode(&line), Some(Signal::OpenButler));
    }

    /// 关窗/重开同样的窗口模式时，旧进程的 `Drop` 与新人 bind 会撞在一起：
    /// 旧人若无条件 `remove_file`，会把新人刚建好的 socket 一起删掉，宿主
    /// 之后再也 poke 不到窗口，只能反复 spawn 新进程。
    #[cfg(unix)]
    #[test]
    fn 旧监听退出不会删掉后继的_socket() {
        let app = format!("mns-sock-replace-{}", std::process::id());
        let path = super::unix_impl::socket_path(&app);
        let (first, _events) = listen(&app).unwrap();
        std::fs::remove_file(&path).unwrap();
        let (_second, _events) = listen(&app).unwrap();
        drop(first);
        assert!(path.exists(), "旧监听的 Drop 不应删掉新监听的 socket");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn hello_带角色() {
        let line = encode(&Signal::Hello {
            role: "daemon".to_owned(),
        });
        assert_eq!(
            decode(&line),
            Some(Signal::Hello {
                role: "daemon".to_owned()
            })
        );
    }

    #[test]
    fn 未知消息被忽略而不是报错() {
        assert_eq!(decode("{\"kind\":\"whatever\"}\n"), None);
        assert_eq!(decode("not json\n"), None);
        assert_eq!(decode("\n"), None);
    }

    #[test]
    fn 唤醒信号带账本_id() {
        let line = encode(&Signal::Wake {
            ledger: Some("trade:1794826433501".to_owned()),
        });
        assert_eq!(
            decode(&line),
            Some(Signal::Wake {
                ledger: Some("trade:1794826433501".to_owned())
            })
        );
        let line = encode(&Signal::Wake { ledger: None });
        assert_eq!(decode(&line), Some(Signal::Wake { ledger: None }));
    }

    #[test]
    fn 主动开口开关可往返() {
        for enabled in [true, false] {
            let line = encode(&Signal::SetProactive { enabled });
            assert_eq!(decode(&line), Some(Signal::SetProactive { enabled }));
        }
        // 缺字段的消息不能当成 false 处理，否则会误关用户的开关。
        assert_eq!(decode("{\"kind\":\"set-proactive\"}\n"), None);
    }

    #[test]
    fn 宿主与守护进程的新信号可往返() {
        assert_eq!(decode(&encode(&Signal::RequestBrief)), Some(Signal::RequestBrief));
        assert_eq!(decode(&encode(&Signal::ButlerSeen)), Some(Signal::ButlerSeen));
        assert_eq!(decode(&encode(&Signal::RefreshData)), Some(Signal::RefreshData));
        assert_eq!(decode(&encode(&Signal::StartButler)), Some(Signal::StartButler));
        assert_eq!(decode(&encode(&Signal::ChatAttach)), Some(Signal::ChatAttach));
        assert_eq!(decode(&encode(&Signal::ChatClear)), Some(Signal::ChatClear));
        assert_eq!(decode(&encode(&Signal::ChatSeen)), Some(Signal::ChatSeen));

        let brief = Signal::Brief {
            json: "{\"total_assets\":1.0}".to_owned(),
        };
        assert_eq!(decode(&encode(&brief)), Some(brief));

        let page = Signal::OpenPage {
            page: "plan".to_owned(),
        };
        assert_eq!(decode(&encode(&page)), Some(page));

        let notice = Signal::DeliverNotice {
            json: "{\"draft\":\"你好\"}".to_owned(),
        };
        assert_eq!(decode(&encode(&notice)), Some(notice));

        let send = Signal::ChatSend {
            text: "现在偏离多少".to_owned(),
        };
        assert_eq!(decode(&encode(&send)), Some(send));

        let delta = Signal::ChatDelta {
            text: "美股偏离".to_owned(),
        };
        assert_eq!(decode(&encode(&delta)), Some(delta));

        let snapshot = Signal::ChatSnapshot {
            json: "{\"session\":[]}".to_owned(),
        };
        assert_eq!(decode(&encode(&snapshot)), Some(snapshot));

        let focus = Signal::NoticeFocus {
            notice_id: "a1b2".to_owned(),
        };
        assert_eq!(decode(&encode(&focus)), Some(focus));

        // 缺载荷的帧不是一个「空」信号：宁可不理解，也不要把它当成有效指令。
        assert_eq!(decode("{\"kind\":\"brief\"}\n"), None);
        assert_eq!(decode("{\"kind\":\"open-page\"}\n"), None);
        assert_eq!(decode("{\"kind\":\"deliver-notice\"}\n"), None);
        assert_eq!(decode("{\"kind\":\"chat-send\"}\n"), None);
        assert_eq!(decode("{\"kind\":\"chat-delta\"}\n"), None);
        assert_eq!(decode("{\"kind\":\"chat-snapshot\"}\n"), None);
        assert_eq!(decode("{\"kind\":\"notice-focus\"}\n"), None);

        // P4 删掉的旧 kind：已经没有消费者，解码必须拒绝，不能复活成
        // 「未知但被忽略」之外的语义。
        assert_eq!(decode("{\"kind\":\"pending\"}\n"), None);
        assert_eq!(decode("{\"kind\":\"notice\"}\n"), None);
    }

    /// 命名管道能跑通「连上 → 发一帧 → 断开」，且断开会被当成存活信号。
    #[cfg(windows)]
    #[test]
    fn 命名管道能收能发() {
        let app = format!("mns-ipc-test-{}", std::process::id());
        let (listener, mut events) = listen(&app).expect("监听命名管道");
        let mut client = connect(&app).expect("连接命名管道");
        client
            .send(&Signal::Hello {
                role: "test".to_owned(),
            })
            .expect("写一帧");

        let mut greeted = false;
        while !greeted {
            match events.blocking_recv().expect("事件通道关闭") {
                Event::Signal(_, Signal::Hello { role }) => {
                    assert_eq!(role, "test");
                    greeted = true;
                }
                Event::Signal(_, other) => panic!("收到意外的信号：{other:?}"),
                Event::Connected(_) => {}
                Event::Disconnected(id) => panic!("连接 {id} 提前断开"),
            }
        }

        drop(client);
        loop {
            match events.blocking_recv().expect("事件通道关闭") {
                Event::Disconnected(_) => break,
                Event::Connected(_) | Event::Signal(..) => continue,
            }
        }
        drop(listener);
    }

    /// 没人监听时要报 `Absent`，不能把「没有实例」说成 IO 故障——调用方
    /// 靠这个判断「管家守护进程在不在」。
    #[cfg(windows)]
    #[test]
    fn 没人监听时报_absent() {
        let app = format!("mns-ipc-absent-{}", std::process::id());
        match connect(&app) {
            Err(ConnectError::Absent) => {}
            other => panic!("期望 Absent，实际 {:?}", other.map(|_| "已连接")),
        }
    }

    /// 同一个名字不允许起第二个监听者（守护进程靠这个判断「已经有人在跑」）。
    /// Unix 上同样成立：探活 connect 成功即拒绝，不删文件抢绑。
    #[test]
    fn 同一通道名不能有两个监听者() {
        let app = format!("mns-ipc-dup-{}", std::process::id());
        let (_first, _events) = listen(&app).expect("第一个监听者");
        let err = match listen(&app) {
            Ok(_) => panic!("第二个监听者不该成功"),
            Err(error) => error,
        };
        assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
    }

    /// 短连接风暴不能弄死监听者。
    ///
    /// 探活与 poke 都是「连上就走」的短连接：GUI 启动时先 connect 探活、
    /// 紧接着 poke Wake，第二条连接可能落在服务端「实例已建、还没调
    /// ConnectNamedPipe」的窗口里（Windows 报 `ERROR_NO_DATA`）。修复前
    /// accept 线程在这种连接上直接退出，进程变成「活着但谁也连不上」的
    /// 聋子——GUI 下次启动探活失败，再拉一个新的，butlerd 进程就攒起来。
    /// Unix 没有这个窗口（连接进内核积压队列），这条测试在两个平台都
    /// 守住「短连接形态不弄死监听者」的不变量。
    #[test]
    fn 短连接风暴不能弄死监听者() {
        let app = format!("mns-ipc-storm-{}", std::process::id());
        let (_listener, mut events) = listen(&app).expect("监听本地通道");
        // 连上立刻断开，循环多次：把「连了又断」的窗口尽量撞出来。
        let mut connected = 0;
        for _ in 0..200 {
            if connect(&app).is_ok() {
                connected += 1;
            }
        }
        assert!(connected >= 32, "风暴里应有足够连接成功，实际 {connected}");
        // 监听者必须仍然可用：一次正常收发。
        let mut client = connect(&app).expect("风暴后重新连接");
        client
            .send(&Signal::Hello {
                role: "test".to_owned(),
            })
            .expect("写一帧");
        loop {
            match events.blocking_recv().expect("事件通道关闭") {
                Event::Signal(_, Signal::Hello { role }) => {
                    assert_eq!(role, "test");
                    break;
                }
                Event::Connected(_) | Event::Disconnected(_) => {}
                Event::Signal(_, other) => panic!("收到意外的信号：{other:?}"),
            }
        }
    }

    /// P4 的管家会话通道：窗口 `connect_duplex` 建双向连接，上行发请求、
    /// 下行收 butlerd 的推送；断开就是 detach，服务端 `send_to` 对已断开的
    /// 连接返回 `false` 而不是阻塞。
    #[test]
    fn 双向连接能收能发_断开即_detach() {
        let app = format!("mns-ipc-duplex-{}", std::process::id());
        let (listener, mut events) = listen(&app).expect("监听本地通道");
        let (mut client, mut downstream) =
            connect_duplex(&app).expect("双向连接");

        client.send(&Signal::ChatAttach).expect("上行发 attach");

        // 服务端视角：先 Connected，再收到 ChatAttach（Signal 帧自带连接 id）。
        let conn_id = loop {
            match events.blocking_recv().expect("事件通道关闭") {
                Event::Connected(_) => {}
                Event::Signal(id, signal) => {
                    assert_eq!(signal, Signal::ChatAttach, "应收到 attach");
                    break id;
                }
                Event::Disconnected(_) => panic!("连接提前断开"),
            }
        };

        // 服务端推送一帧快照：走有界出站队列，不占事件通道。
        let snapshot = Signal::ChatSnapshot {
            json: "{\"session\":[]}".to_owned(),
        };
        assert!(
            listener.send_to(conn_id, &snapshot),
            "活跃连接的推送应成功"
        );
        // 重复推给不存在的连接：返回 false，不阻塞、不 panic。
        assert!(!listener.send_to(conn_id + 999, &snapshot), "未知连接应返回 false");

        match downstream.blocking_recv().expect("下行通道关闭") {
            got @ Signal::ChatSnapshot { .. } => assert_eq!(got, snapshot),
            other => panic!("下行应收到快照，实际 {other:?}"),
        }

        // 客户端断开：服务端收到 Disconnected；此后 send_to 报 false。
        drop(client);
        loop {
            match events.blocking_recv().expect("事件通道关闭") {
                Event::Disconnected(id) => {
                    assert_eq!(id, conn_id);
                    break;
                }
                Event::Connected(_) | Event::Signal(..) => continue,
            }
        }
        // 断开后队列可能仍在（写线程还没消费），但最终会被清理；这里只要求
        // 不阻塞。等写线程退出后再验证 remove 生效——用一个短暂的收尾窗口。
        let mut gone = false;
        for _ in 0..50 {
            if !listener.send_to(conn_id, &snapshot) {
                gone = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(gone, "断开的连接最终应从注册表消失");
        drop(listener);
    }

    /// `ERROR_NO_DATA` 之后必须换实例继续监听（确定性复现，Windows 专属）。
    ///
    /// 风暴测试撞的是概率窗口（连了又断恰好落在服务端 `ConnectNamedPipe`
    /// 之前），在 CI 里可能永远执行不到出错分支。这条测试把每一步钉死：
    /// 实例建好 → 客户端连上立刻断开 → 服务端此刻才调 `ConnectNamedPipe`。
    /// 客户端先断在 Windows 上**必然**报 `ERROR_NO_DATA`（MSDN 文档行为，
    /// 非竞态），这正是 GUI「连上就走的探活」撞上 accept 窗口时的出错点。
    ///
    /// 修复前的代码在这种错误上直接退出 accept 线程，进程变聋、被 GUI
    /// 重复拉起后攒起来；修复后的契约是：换一条新实例接着等。这里用
    /// `pending = next_instance(...)` 的赋值顺序复刻 accept 循环的时序
    /// ——新实例到手之后旧实例才被 drop，管道名始终有人占着。
    #[cfg(windows)]
    #[test]
    fn error_no_data_之后必须换实例继续监听() {
        use super::windows_impl::{
            create_instance, next_instance, pipe_name, wait_for_client, ERROR_NO_DATA,
        };

        let app = format!("mns-ipc-nodata-{}", std::process::id());
        let name = pipe_name(&app);

        // 场景回放：实例建好，客户端连上立刻断开。
        let mut pending = create_instance(&name, true).expect("建第一条实例");
        drop(connect(&app).expect("客户端连上"));
        // 服务端此刻才等连接：客户端先断，必然报 `ERROR_NO_DATA`。
        let error = wait_for_client(&pending).expect_err("客户端先断必报错");
        assert_eq!(
            error.raw_os_error(),
            Some(ERROR_NO_DATA),
            "期望 ERROR_NO_DATA，实际 {error}"
        );

        // 修复契约：不许退出，换一条新实例（旧实例还占着名字时也必须建得出来，
        // 管道名自始至终没有被释放，第二个进程永远绑不上）。
        pending = next_instance(&name).expect("ERROR_NO_DATA 后换出下一条实例");
        // 换出来的新实例必须能正常接客：客户端先连、服务端后收，
        // `ConnectNamedPipe` 报 `ERROR_PIPE_CONNECTED`，按成功处理。
        let client = connect(&app).expect("连上换出来的新实例");
        wait_for_client(&pending).expect("新实例能正常接客");
        drop(client);
    }
}
