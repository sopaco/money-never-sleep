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
//!
//! 监听端跑在一个专用 tokio reactor 上：Windows 用 overlapped I/O 等就绪
//! （不再按 20ms 轮询 `PeekNamedPipe`），Unix 用 epoll/kqueue；每条连接只是
//! reactor 上的读、写两个任务，共享两条工作线程，不再各占两个 OS 线程。

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

/// 一行协议能有多大。见模块注释：按快照最坏情况取 1MB。
const MAX_LINE: usize = 1024 * 1024;

/// 客户端发来的请求。
///
/// `kind` 字段是协议的唯一判别式：`#[serde(tag = "kind", rename_all = "kebab-case")]`
/// 让枚举变体名与 JSON 一一对应，新增一条消息只需加一个变体，不再维护
/// `encode` / `decode` 两套对称 match。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Signal {
    /// 把主窗口拿到前面（二次启动）。
    Open,
    /// 打开管家会话窗口。
    OpenButler,
    /// 守护进程自我介绍。带上 `role`，将来 GUI 与管家可以共用一个监听。
    Hello {
        /// 老版本可能只发 `{"kind":"hello"}`，缺省角色保持兼容。
        #[serde(default = "default_role")]
        role: String,
    },
    /// GUI 请守护进程立刻判定一次。`ledger` 有值是账本事件（不受巡检间隔
    /// 约束），无值是定时巡检。
    Wake {
        #[serde(default)]
        ledger: Option<String>,
    },
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
    /// 宿主对开口的回执：投递（或确认已在会话里可见）之后才发。守护进程
    /// 收到才推进 `pending` 偏移，宿主中途崩溃时同一 id 会重投。
    NoticeDelivered { notice_id: String },
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
    ChatSend {
        text: String,
        /// 这句提问引用的巡检报告。只传 `{id,title}`，正文由 butlerd 按 id
        /// 读取——IPC 只负责说「引用了哪几份」。
        #[serde(default)]
        attachments: Vec<crate::butler::review::ReviewRef>,
    },
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
    /// 监听循环跑在一个专用 tokio reactor 上：Windows 的命名管道读写走
    /// overlapped I/O（不再 20ms 轮询），Unix 走 epoll/kqueue。reactor 只有
    /// 2 个工作线程，每条连接只是它上面的两个任务，不再各占 2 个 OS 线程。
    ///
    /// `Option` 只为 `Drop`：`Runtime::shutdown_background` 按值消费。
    _runtime: Option<tokio::runtime::Runtime>,
    /// 只为了活着：`Drop` 里删 socket 文件。
    #[cfg(unix)]
    _inner: unix_impl::Listener,
    #[cfg(windows)]
    _inner: windows_impl::Listener,
}

impl Drop for Listener {
    fn drop(&mut self) {
        // 用 `shutdown_background` 而不是让它随字段析构：`Drop` 可能发生在
        // 异步上下文里（butlerd 的 tokio main），阻塞式关停会 panic。
        if let Some(runtime) = self._runtime.take() {
            runtime.shutdown_background();
        }
    }
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
    use std::sync::Mutex;
    use tokio::sync::mpsc::{Receiver, Sender, channel};

    /// 每条连接的出站队列深度。快照最大 1MB，64 帧的内存上限足够吸收
    /// 一次慢读的突发；再堵说明对端已经不读，断开重连比无限积压好。
    pub(super) const OUTBOUND_QUEUE: usize = 64;

    #[derive(Default)]
    pub(super) struct Registry {
        senders: Mutex<HashMap<u64, Sender<Signal>>>,
    }

    impl Registry {
        pub(super) fn insert(&self, id: u64, sender: Sender<Signal>) {
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
        ///
        /// `try_send` 不依赖调用方处在异步上下文：`send_to` 由窗口与
        /// butlerd 的同步代码调用。
        pub(super) fn send(&self, id: u64, signal: &Signal) -> bool {
            let guard = self
                .senders
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match guard.get(&id) {
                Some(sender) => match sender.try_send(signal.clone()) {
                    Ok(()) => true,
                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                        drop(guard);
                        self.remove(id);
                        false
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
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
    pub(super) fn outbound() -> (Sender<Signal>, Receiver<Signal>) {
        channel(OUTBOUND_QUEUE)
    }

    /// 监听循环所在的 reactor。两条工作线程足够：每条连接只做本地小块
    /// 读写，真正耗时的判定与会话都在阻塞线程池或独立 runtime 上。
    pub(super) fn runtime() -> std::io::Result<tokio::runtime::Runtime> {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("mns-ipc")
            .enable_all()
            .build()
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
    let runtime = registry::runtime()?;
    #[cfg(unix)]
    {
        let inner = unix_impl::listen(app, tx, &runtime)?;
        Ok((
            Listener {
                _runtime: Some(runtime),
                _inner: inner,
            },
            rx,
        ))
    }
    #[cfg(windows)]
    {
        let inner = windows_impl::listen(app, tx, &runtime)?;
        Ok((
            Listener {
                _runtime: Some(runtime),
                _inner: inner,
            },
            rx,
        ))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (app, tx, runtime);
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
pub fn connect_duplex(app: &str) -> Result<(Client, UnboundedReceiver<Signal>), ConnectError> {
    #[cfg(unix)]
    {
        unix_impl::connect_duplex(app)
    }
    #[cfg(windows)]
    {
        windows_impl::connect_duplex(app)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = app;
        Err(ConnectError::Unsupported)
    }
}

fn default_role() -> String {
    "unknown".to_owned()
}

fn encode(signal: &Signal) -> String {
    let mut line = serde_json::to_string(signal).expect("Signal 总是可序列化");
    line.push('\n');
    line
}

fn decode(line: &str) -> Option<Signal> {
    serde_json::from_str(line).ok()
}

#[cfg(unix)]
mod unix_impl {
    use super::{Event, MAX_LINE, Signal, decode, encode, registry};
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::net::Shutdown;
    use std::os::unix::fs::PermissionsExt;
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
        runtime: &tokio::runtime::Runtime,
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
        listener.set_nonblocking(true)?;
        // 进 runtime 上下文再转换：`from_std` 要拿 reactor 的注册表。
        let listener = {
            let _guard = runtime.enter();
            tokio::net::UnixListener::from_std(listener)?
        };

        let registry = Arc::new(registry::Registry::default());
        let thread_registry = registry.clone();
        runtime.spawn(accept_loop(listener, tx, thread_registry));

        Ok(Listener {
            path,
            identity,
            registry,
        })
    }

    /// 接受连接：每条连接在 reactor 上起两个任务（读 / 写），不再各占一个
    /// OS 线程。两个任务共用 `Arc<UnixStream>`：tokio 的 `try_read` /
    /// `try_write` 都只借 `&self`，可并发进行。
    async fn accept_loop(
        listener: tokio::net::UnixListener,
        tx: UnboundedSender<Event>,
        registry: Arc<registry::Registry>,
    ) {
        let mut next_id: u64 = 0;
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                // 单个 accept 失败不该结束监听：退避一下继续，否则一次
                // EMFILE 就把进程变聋。
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                continue;
            };
            next_id += 1;
            let id = next_id;
            if tx.send(Event::Connected(id)).is_err() {
                return;
            }
            let (out_tx, out_rx) = registry::outbound();
            registry.insert(id, out_tx);
            let stream = Arc::new(stream);

            let writer_stream = Arc::clone(&stream);
            let writer_registry = Arc::clone(&registry);
            tokio::spawn(async move {
                let _ = write_loop(writer_stream.clone(), out_rx).await;
                // 写半边收摊即整条连接收摊：读任务随之退出，对端看到 EOF。
                let _ = writer_stream.shutdown(Shutdown::Both);
                writer_registry.remove(id);
            });

            let reader_stream = Arc::clone(&stream);
            let reader_registry = Arc::clone(&registry);
            let reader_tx = tx.clone();
            tokio::spawn(async move {
                read_loop(id, reader_stream, reader_tx).await;
                reader_registry.remove(id);
            });
        }
    }

    /// 读任务：拿 `readable()` 等数据，`try_read` 取一块，攒够一行就解码。
    async fn read_loop(id: u64, stream: Arc<tokio::net::UnixStream>, tx: UnboundedSender<Event>) {
        let mut pending: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            while let Some(index) = pending.iter().position(|byte| *byte == b'\n') {
                let mut raw: Vec<u8> = pending.drain(..=index).collect();
                raw.pop();
                if raw.last() == Some(&b'\r') {
                    raw.pop();
                }
                let line = String::from_utf8_lossy(&raw);
                if let Some(signal) = decode(&line)
                    && tx.send(Event::Signal(id, signal)).is_err()
                {
                    let _ = tx.send(Event::Disconnected(id));
                    return;
                }
            }
            if pending.len() > MAX_LINE {
                break;
            }
            if stream.readable().await.is_err() {
                break;
            }
            match stream.try_read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => pending.extend_from_slice(&chunk[..count]),
                // `readable()` 只保证「可以试一次」，并非一定有数据：假唤醒
                // 与并发读都会让 `try_read` 返回 `WouldBlock`，必须重试而
                // 不是当成断开——否则一条连接读到第一批帧之后就被自己掐掉。
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(_) => break,
            }
        }
        let _ = tx.send(Event::Disconnected(id));
    }

    /// 写任务：从有界队列取帧写下。队列被拆（慢读者）或对端断开都直接
    /// 返回——调用方随后会关掉整条连接。
    async fn write_loop(
        stream: Arc<tokio::net::UnixStream>,
        mut rx: tokio::sync::mpsc::Receiver<Signal>,
    ) -> std::io::Result<()> {
        while let Some(signal) = rx.recv().await {
            let line = encode(&signal);
            let mut offset = 0;
            while offset < line.len() {
                stream.writable().await?;
                match stream.try_write(&line.as_bytes()[offset..]) {
                    Ok(0) => return Ok(()),
                    Ok(count) => offset += count,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(())
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
        pub(super) fn start_reader(&self, tx: UnboundedSender<Signal>) -> std::io::Result<()> {
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

    /// 双向连接：写走同步句柄，读交给一个常驻线程。两端都在本机，一条
    /// 连接一个读线程已经够用（每进程最多一条）。
    pub(super) fn connect_duplex(
        app: &str,
    ) -> Result<(super::Client, tokio::sync::mpsc::UnboundedReceiver<Signal>), super::ConnectError>
    {
        let inner = connect(app)?;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        match inner.start_reader(tx) {
            Ok(()) => Ok((super::Client { inner }, rx)),
            Err(error) => Err(super::ConnectError::Io(error)),
        }
    }

    pub(super) fn poke(app: &str, signal: &Signal) -> Result<(), super::ConnectError> {
        let mut client = connect(app)?;
        client.send(signal).map_err(super::ConnectError::Io)
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
    use std::io::Write;
    use std::os::windows::io::{FromRawHandle, RawHandle};
    use std::sync::Arc;
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient, NamedPipeServer};
    use tokio::sync::mpsc::UnboundedSender;

    /// Win32 句柄。`poke` 仍用同步句柄：它只写一帧就走，不需要 overlapped。
    type Handle = *mut c_void;

    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const OPEN_EXISTING: u32 = 3;

    const ERROR_FILE_NOT_FOUND: i32 = 2;
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_PIPE_BUSY: i32 = 231;

    const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;

    #[link(name = "kernel32")]
    unsafe extern "system" {
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

    /// 建一条服务端实例。`first` 只给第一条：名字已被别的进程占用时
    /// `CreateNamedPipeW` 直接失败，不必先连一次探活——探活与真正监听之间
    /// 有竞态，可能起出两个监听者。
    pub(super) fn create_server(name: &str, first: bool) -> std::io::Result<NamedPipeServer> {
        let mut options = tokio::net::windows::named_pipe::ServerOptions::new();
        options
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .in_buffer_size(MAX_LINE as u32)
            .out_buffer_size(MAX_LINE as u32);
        options.create(name).map_err(|error| {
            // 名字已被占用：已有实例在监听。
            if error.raw_os_error() == Some(ERROR_ACCESS_DENIED) {
                std::io::Error::new(std::io::ErrorKind::AddrInUse, error.to_string())
            } else {
                error
            }
        })
    }

    pub(super) fn listen(
        app: &str,
        tx: UnboundedSender<Event>,
        runtime: &tokio::runtime::Runtime,
    ) -> std::io::Result<Listener> {
        let name = pipe_name(app);
        // `ServerOptions::create` 要把句柄注册进 reactor，必须在 runtime 上下文里调。
        let server = {
            let _guard = runtime.enter();
            create_server(&name, true)?
        };
        let registry = Arc::new(registry::Registry::default());
        let thread_registry = registry.clone();
        runtime.spawn(accept_loop(name, server, tx, thread_registry));
        Ok(Listener { registry })
    }

    /// 接受连接。读写都在 reactor 的 overlapped I/O 上等就绪（不再 20ms
    /// 轮询 `PeekNamedPipe`），每条连接只是两个任务，不再各占两个线程。
    async fn accept_loop(
        name: String,
        mut server: NamedPipeServer,
        tx: UnboundedSender<Event>,
        registry: Arc<registry::Registry>,
    ) {
        let mut next_id: u64 = 0;
        loop {
            if server.connect().await.is_err() {
                // 客户端在「实例建好」与「本任务调 connect」之间连了又断：
                // GUI 的探活与 poke 都是「连上就走」的短连接，这是常态。
                // 换一条新实例接着等，绝不能退出——accept 一死，进程就成了
                // 「活着但谁也连不上」的聋子。
                server = match next_server(&name).await {
                    Some(next) => next,
                    None => return,
                };
                continue;
            }
            // 先把下一条实例建出来，再处理刚连上的这条：否则读取期间名字上
            // 没有任何空闲实例，第二次连接会撞 `ERROR_PIPE_BUSY`。
            let connected = server;
            server = match next_server(&name).await {
                Some(next) => next,
                None => return,
            };

            next_id += 1;
            let id = next_id;
            if tx.send(Event::Connected(id)).is_err() {
                return;
            }
            let (out_tx, out_rx) = registry::outbound();
            registry.insert(id, out_tx);
            let pipe = Arc::new(connected);

            let writer_pipe = Arc::clone(&pipe);
            let writer_registry = Arc::clone(&registry);
            tokio::spawn(async move {
                let _ = write_loop(writer_pipe.clone(), out_rx).await;
                // 写半边收摊即整条连接收摊：读任务随之退出。
                let _ = writer_pipe.disconnect();
                writer_registry.remove(id);
            });

            let reader_pipe = Arc::clone(&pipe);
            let reader_registry = Arc::clone(&registry);
            let reader_tx = tx.clone();
            tokio::spawn(async move {
                read_loop(id, reader_pipe, reader_tx).await;
                reader_registry.remove(id);
            });
        }
    }

    /// 建下一条实例。失败按 200ms 退避重试，累计 5 秒仍失败就放弃：此时
    /// 进程已经没有能力继续监听，让 accept 任务退出比留一个聋子好。
    async fn next_server(name: &str) -> Option<NamedPipeServer> {
        for _ in 0..25 {
            match create_server(name, false) {
                Ok(server) => return Some(server),
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
            }
        }
        eprintln!("mns-ipc: {name} 连续 5 秒建不出下一条实例，放弃监听");
        None
    }

    async fn read_loop(id: u64, pipe: Arc<NamedPipeServer>, tx: UnboundedSender<Event>) {
        let mut pending: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            while let Some(index) = pending.iter().position(|byte| *byte == b'\n') {
                let mut raw: Vec<u8> = pending.drain(..=index).collect();
                raw.pop();
                if raw.last() == Some(&b'\r') {
                    raw.pop();
                }
                let line = String::from_utf8_lossy(&raw);
                if let Some(signal) = decode(&line)
                    && tx.send(Event::Signal(id, signal)).is_err()
                {
                    let _ = tx.send(Event::Disconnected(id));
                    return;
                }
            }
            if pending.len() > MAX_LINE {
                break;
            }
            if pipe.readable().await.is_err() {
                break;
            }
            match pipe.try_read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => pending.extend_from_slice(&chunk[..count]),
                // 同 Unix：`readable()` 只是「可以试一次」，假唤醒要重试。
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(_) => break,
            }
        }
        let _ = tx.send(Event::Disconnected(id));
    }

    async fn write_loop(
        pipe: Arc<NamedPipeServer>,
        mut rx: tokio::sync::mpsc::Receiver<Signal>,
    ) -> std::io::Result<()> {
        while let Some(signal) = rx.recv().await {
            let line = encode(&signal);
            let mut offset = 0;
            while offset < line.len() {
                pipe.writable().await?;
                match pipe.try_write(&line.as_bytes()[offset..]) {
                    Ok(0) => return Ok(()),
                    Ok(count) => offset += count,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(())
    }

    /// 客户端两种形态：
    ///
    /// - `connect` / `poke`：同步句柄，一次写一帧就走，不需要 reactor；
    /// - `connect_duplex`：overlapped 句柄，读任务在专用线程的 reactor 上
    ///   等就绪，写通过无界队列交给它，`send` 不依赖调用方处在异步上下文。
    pub(super) struct Client {
        pipe: Option<File>,
        tx: Option<tokio::sync::mpsc::UnboundedSender<Signal>>,
    }

    impl Client {
        pub(super) fn send(&mut self, signal: &Signal) -> std::io::Result<()> {
            if let Some(pipe) = self.pipe.as_mut() {
                pipe.write_all(encode(signal).as_bytes())?;
                return pipe.flush();
            }
            match self.tx.as_ref() {
                Some(tx) => tx.send(signal.clone()).map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::NotConnected, "双向连接已断开")
                }),
                None => Err(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    "连接已关闭",
                )),
            }
        }
    }

    /// 同步打开：`poke` 只用它写一帧，不涉及挂起的读，没有串行化问题。
    fn open_sync(app: &str) -> Result<File, super::ConnectError> {
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

    /// overlapped 打开：双向连接要能在等读的同时写，只能用 overlapped 句柄。
    fn open_overlapped(app: &str) -> Result<NamedPipeClient, super::ConnectError> {
        let name = pipe_name(app);
        for attempt in 0..2 {
            match ClientOptions::new().open(&name) {
                Ok(pipe) => return Ok(pipe),
                Err(error) => match error.raw_os_error() {
                    Some(ERROR_FILE_NOT_FOUND) => return Err(super::ConnectError::Absent),
                    Some(ERROR_PIPE_BUSY) if attempt == 0 => {
                        unsafe { WaitNamedPipeW(wide(&name).as_ptr(), 100) };
                    }
                    Some(ERROR_PIPE_BUSY) => return Err(super::ConnectError::Absent),
                    _ => return Err(super::ConnectError::Io(error)),
                },
            }
        }
        Err(super::ConnectError::Absent)
    }

    pub(super) fn connect(app: &str) -> Result<Client, super::ConnectError> {
        open_sync(app).map(|pipe| Client {
            pipe: Some(pipe),
            tx: None,
        })
    }

    pub(super) fn poke(app: &str, signal: &Signal) -> Result<(), super::ConnectError> {
        let mut client = connect(app)?;
        client.send(signal).map_err(super::ConnectError::Io)
    }

    /// 双向连接：一个专用线程驱动 current-thread reactor，读任务在
    /// overlapped I/O 上等数据（不再 20ms 轮询），写任务从无界队列取帧。
    /// 连接断开时下行通道关闭，调用方据此进入离线态或重连。
    pub(super) fn connect_duplex(
        app: &str,
    ) -> Result<(super::Client, tokio::sync::mpsc::UnboundedReceiver<Signal>), super::ConnectError>
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(super::ConnectError::Io)?;
        // `ClientOptions::open` 同样要把句柄注册进 reactor。
        let pipe = {
            let _guard = runtime.enter();
            open_overlapped(app)?
        };
        let (down_tx, down_rx) = tokio::sync::mpsc::unbounded_channel();
        let (up_tx, up_rx) = tokio::sync::mpsc::unbounded_channel();
        std::thread::Builder::new()
            .name("mns-ipc-duplex".to_owned())
            .spawn(move || {
                let pipe = Arc::new(pipe);
                runtime.block_on(async move {
                    let mut reader = tokio::spawn(read_client(Arc::clone(&pipe), down_tx));
                    let mut writer = tokio::spawn(write_client(Arc::clone(&pipe), up_rx));
                    // 任何一半收摊都拆掉另一半：`NamedPipeClient` 没有
                    // `disconnect`，abort 之后 Arc 释放即关闭句柄。读任务
                    // 卡在 `readable()` 上时，只有 abort 能把它叫醒。
                    tokio::select! {
                        _ = &mut reader => writer.abort(),
                        _ = &mut writer => reader.abort(),
                    }
                });
            })
            .map_err(super::ConnectError::Io)?;
        Ok((
            super::Client {
                inner: Client {
                    pipe: None,
                    tx: Some(up_tx),
                },
            },
            down_rx,
        ))
    }

    async fn read_client(
        pipe: Arc<NamedPipeClient>,
        tx: tokio::sync::mpsc::UnboundedSender<Signal>,
    ) {
        let mut pending: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            while let Some(index) = pending.iter().position(|byte| *byte == b'\n') {
                let mut raw: Vec<u8> = pending.drain(..=index).collect();
                raw.pop();
                if raw.last() == Some(&b'\r') {
                    raw.pop();
                }
                let line = String::from_utf8_lossy(&raw);
                if let Some(signal) = decode(&line)
                    && tx.send(signal).is_err()
                {
                    return;
                }
            }
            if pending.len() > MAX_LINE {
                break;
            }
            if pipe.readable().await.is_err() {
                break;
            }
            match pipe.try_read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => pending.extend_from_slice(&chunk[..count]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(_) => break,
            }
        }
    }

    async fn write_client(
        pipe: Arc<NamedPipeClient>,
        mut rx: tokio::sync::mpsc::UnboundedReceiver<Signal>,
    ) {
        while let Some(signal) = rx.recv().await {
            let line = encode(&signal);
            let mut offset = 0;
            let mut broken = false;
            while offset < line.len() {
                if pipe.writable().await.is_err() {
                    broken = true;
                    break;
                }
                match pipe.try_write(&line.as_bytes()[offset..]) {
                    Ok(0) => {
                        broken = true;
                        break;
                    }
                    Ok(count) => offset += count,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
                    Err(_) => {
                        broken = true;
                        break;
                    }
                }
            }
            if broken {
                break;
            }
        }
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
    fn wire_格式与旧协议逐字一致() {
        let cases = [
            (Signal::Open, r#"{"kind":"open"}"#),
            (Signal::OpenButler, r#"{"kind":"open-butler"}"#),
            (
                Signal::Hello {
                    role: "daemon".to_owned(),
                },
                r#"{"kind":"hello","role":"daemon"}"#,
            ),
            (
                Signal::Wake { ledger: None },
                r#"{"kind":"wake","ledger":null}"#,
            ),
            (
                Signal::Wake {
                    ledger: Some("trade:1".to_owned()),
                },
                r#"{"kind":"wake","ledger":"trade:1"}"#,
            ),
            (
                Signal::SetProactive { enabled: true },
                r#"{"kind":"set-proactive","enabled":true}"#,
            ),
            (Signal::RequestBrief, r#"{"kind":"request-brief"}"#),
            (
                Signal::Brief {
                    json: "{}".to_owned(),
                },
                r#"{"kind":"brief","json":"{}"}"#,
            ),
            (
                Signal::OpenPage {
                    page: "plan".to_owned(),
                },
                r#"{"kind":"open-page","page":"plan"}"#,
            ),
            (Signal::OpenAbout, r#"{"kind":"open-about"}"#),
            (
                Signal::DeliverNotice {
                    json: "{}".to_owned(),
                },
                r#"{"kind":"deliver-notice","json":"{}"}"#,
            ),
            (
                Signal::NoticeFocus {
                    notice_id: "n1".to_owned(),
                },
                r#"{"kind":"notice-focus","notice_id":"n1"}"#,
            ),
            (
                Signal::NoticeDelivered {
                    notice_id: "n1".to_owned(),
                },
                r#"{"kind":"notice-delivered","notice_id":"n1"}"#,
            ),
            (Signal::ButlerSeen, r#"{"kind":"butler-seen"}"#),
            (Signal::StartButler, r#"{"kind":"start-butler"}"#),
            (Signal::RefreshData, r#"{"kind":"refresh-data"}"#),
            (Signal::ChatAttach, r#"{"kind":"chat-attach"}"#),
            (
                Signal::ChatSend {
                    text: "hi".to_owned(),
                    attachments: Vec::new(),
                },
                r#"{"kind":"chat-send","text":"hi","attachments":[]}"#,
            ),
            (Signal::ChatClear, r#"{"kind":"chat-clear"}"#),
            (Signal::ChatSeen, r#"{"kind":"chat-seen"}"#),
            (
                Signal::ChatSnapshot {
                    json: "{}".to_owned(),
                },
                r#"{"kind":"chat-snapshot","json":"{}"}"#,
            ),
            (
                Signal::ChatDelta {
                    text: "hi".to_owned(),
                },
                r#"{"kind":"chat-delta","text":"hi"}"#,
            ),
            (Signal::Shutdown, r#"{"kind":"shutdown"}"#),
            (Signal::Bye, r#"{"kind":"bye"}"#),
        ];
        for (signal, expected) in cases {
            assert_eq!(encode(&signal), format!("{expected}\n"));
            assert_eq!(decode(&format!("{expected}\n")), Some(signal));
        }
    }

    #[test]
    fn 老客户端省略可选字段仍能解析() {
        assert_eq!(
            decode("{\"kind\":\"hello\"}\n"),
            Some(Signal::Hello {
                role: "unknown".to_owned()
            })
        );
        assert_eq!(
            decode("{\"kind\":\"wake\"}\n"),
            Some(Signal::Wake { ledger: None })
        );
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
        assert_eq!(
            decode(&encode(&Signal::RequestBrief)),
            Some(Signal::RequestBrief)
        );
        assert_eq!(
            decode(&encode(&Signal::ButlerSeen)),
            Some(Signal::ButlerSeen)
        );
        assert_eq!(
            decode(&encode(&Signal::RefreshData)),
            Some(Signal::RefreshData)
        );
        assert_eq!(
            decode(&encode(&Signal::StartButler)),
            Some(Signal::StartButler)
        );
        assert_eq!(
            decode(&encode(&Signal::ChatAttach)),
            Some(Signal::ChatAttach)
        );
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
            attachments: Vec::new(),
        };
        assert_eq!(decode(&encode(&send)), Some(send));

        // 旧窗口发来的帧没有 attachments 字段：必须还能解码成空引用。
        let legacy = decode(r#"{"kind":"chat-send","text":"旧帧"}"#).unwrap();
        assert_eq!(
            legacy,
            Signal::ChatSend {
                text: "旧帧".to_owned(),
                attachments: Vec::new(),
            }
        );

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
    /// ConnectNamedPipe」的窗口里（Windows 报 `ERROR_NO_DATA`）。accept
    /// 循环对这种客户端先断的连接一律换实例重来，绝不退出——退出会让进程
    /// 变成「活着但谁也连不上」的聋子，被 GUI 重复拉起后进程就攒起来。
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
        let (mut client, mut downstream) = connect_duplex(&app).expect("双向连接");

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
        assert!(listener.send_to(conn_id, &snapshot), "活跃连接的推送应成功");
        // 重复推给不存在的连接：返回 false，不阻塞、不 panic。
        assert!(
            !listener.send_to(conn_id + 999, &snapshot),
            "未知连接应返回 false"
        );

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
}
