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
//! 协议是 NDJSON，一行一条，行长上限 64KB。传输层按平台换，语义不变：
//! Unix 走 `SOCK_STREAM`（`<数据目录>/run/<app>.sock`），Windows 走命名管道
//! （`\\.\pipe\mns-<app>-<用户名>`）。两边共用同一份 `encode` / `decode`，
//! 所以「连接活着 = 对方活着」在哪个平台都成立。

use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

/// 一行协议能有多大。正常消息不到 100 字节，这个上限只用来挡住坏客户端。
const MAX_LINE: usize = 64 * 1024;

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
    /// 守护进程刚产生一条开口，GUI 该来读 `pending.jsonl` 了。
    Pending,
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
pub struct Listener {
    /// 只为了活着：`Drop` 里删 socket 文件。
    #[cfg(unix)]
    _inner: unix_impl::Listener,
    #[cfg(windows)]
    _inner: windows_impl::Listener,
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

fn encode(signal: &Signal) -> String {
    let value = match signal {
        Signal::Open => serde_json::json!({ "kind": "open" }),
        Signal::OpenButler => serde_json::json!({ "kind": "open-butler" }),
        Signal::Hello { role } => serde_json::json!({ "kind": "hello", "role": role }),
        Signal::Wake { ledger } => serde_json::json!({ "kind": "wake", "ledger": ledger }),
        Signal::SetProactive { enabled } => {
            serde_json::json!({ "kind": "set-proactive", "enabled": enabled })
        }
        Signal::Pending => serde_json::json!({ "kind": "pending" }),
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
        "pending" => Some(Signal::Pending),
        "shutdown" => Some(Signal::Shutdown),
        "bye" => Some(Signal::Bye),
        _ => None,
    }
}

#[cfg(unix)]
mod unix_impl {
    use super::{Event, MAX_LINE, Signal, decode, encode};
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use tokio::sync::mpsc::UnboundedSender;

    pub(super) fn socket_path(app: &str) -> PathBuf {
        crate::config::resolve_data_path("run").join(format!("{app}.sock"))
    }

    pub(super) struct Listener {
        path: PathBuf,
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            // Best effort：进程正常退出时把 socket 文件清掉，避免下次启动
            // 先探活再删。异常退出留下的残file由 `listen` 的探活处理。
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
                    // 每条连接一个线程：一个卡住的客户端不能堵住 accept。
                    let tx = tx.clone();
                    let _ = std::thread::Builder::new()
                        .name(format!("mns-ipc-{id}"))
                        .spawn(move || read_connection(id, stream, tx));
                }
            })?;

        Ok(Listener { path })
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

    pub(super) struct Client {
        stream: UnixStream,
    }

    impl Client {
        pub(super) fn send(&mut self, signal: &Signal) -> std::io::Result<()> {
            self.stream.write_all(encode(signal).as_bytes())?;
            self.stream.flush()
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
    use super::{Event, MAX_LINE, Signal, decode, encode};
    use std::ffi::c_void;
    use std::fs::File;
    use std::io::{BufRead, BufReader, Write};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
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

    pub(super) struct Listener;

    fn create_instance(name: &str, first: bool) -> std::io::Result<File> {
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
    fn wait_for_client(pipe: &File) -> std::io::Result<()> {
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

        std::thread::Builder::new()
            .name("mns-ipc-accept".to_owned())
            .spawn(move || {
                let mut next_id: u64 = 0;
                loop {
                    if wait_for_client(&pending).is_err() {
                        return;
                    }
                    // 先把下一条实例建出来，再处理刚连上的这条：否则读取期间
                    // 名字上没有任何空闲实例，第二次连接会撞 `ERROR_PIPE_BUSY`。
                    let next = match create_instance(&name, false) {
                        Ok(file) => file,
                        Err(_) => return,
                    };
                    let connected = std::mem::replace(&mut pending, next);
                    next_id += 1;
                    let id = next_id;
                    if tx.send(Event::Connected(id)).is_err() {
                        return;
                    }
                    // 每条连接一个线程：一个卡住的客户端不能堵住 accept。
                    let tx = tx.clone();
                    let _ = std::thread::Builder::new()
                        .name(format!("mns-ipc-{id}"))
                        .spawn(move || read_connection(id, connected, tx));
                }
            })?;

        Ok(Listener)
    }

    fn read_connection(id: u64, pipe: File, tx: UnboundedSender<Event>) {
        let mut reader = BufReader::new(pipe);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                // 客户端关掉连接时 `ReadFile` 报 `ERROR_BROKEN_PIPE`，和读到
                // EOF 一样：这条连接到此为止。
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

    pub(super) struct Client {
        pipe: File,
    }

    impl Client {
        pub(super) fn send(&mut self, signal: &Signal) -> std::io::Result<()> {
            self.pipe.write_all(encode(signal).as_bytes())?;
            self.pipe.flush()
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

    #[test]
    fn 协议是单行_ndjson() {
        let line = encode(&Signal::OpenButler);
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1);
        assert_eq!(decode(&line), Some(Signal::OpenButler));
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
    #[cfg(windows)]
    #[test]
    fn 同一管道名不能有两个监听者() {
        let app = format!("mns-ipc-dup-{}", std::process::id());
        let (_first, _events) = listen(&app).expect("第一个监听者");
        let err = match listen(&app) {
            Ok(_) => panic!("第二个监听者不该成功"),
            Err(error) => error,
        };
        assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
    }
}
