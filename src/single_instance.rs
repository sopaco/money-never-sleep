//! 进程级单实例守卫。
//!
//! `mns-gui` 常驻托盘、`mns-help` 是独立窗口，两者都不应该被重复启动：
//! 用户再次双击时，期望的是「已经有窗口，把它拿到前面来」，而不是多开一个
//! 进程。做法是 Windows 命名互斥体 + 命名事件：
//!
//! - 先创建的进程持有 `Local\MNS-<app>-instance` 互斥体，成为 Primary；
//! - 后启动的进程发现互斥体已存在，`SetEvent` 通知 Primary 唤起窗口，然后
//!   自己立刻退出；
//! - Primary 用专用线程阻塞在这个命名事件上（不做轮询），命中时走与托盘
//!   菜单「打开 MNS」完全相同的窗口路径。
//!
//! 二次启动也可能带着意图来：`mns-gui --butler` 要的是**管家窗口**，不是
//! 主界面。意图用一条独立的事件（`Local\MNS-<app>-wakeup-butler`）表达，
//! 而不是共享内存或命令行回读——事件本身就在内核里排队，Primary 还没开始
//! 等待时信号也不会丢。
//!
//! `Local\` 前缀让实例按登录会话隔离：同一台机器上多个用户各有一份，互不干扰。
//! 非 Windows 平台暂不安装全局锁，`acquire` 恒返回 Primary。

/// 单实例守卫。必须存活到进程退出；`Drop` 释放内核对象。
pub struct Guard {
    #[cfg(windows)]
    inner: windows_impl::Guard,
}

/// `acquire` 的结果。
pub enum Acquired {
    /// 本进程是唯一实例。
    Primary(Guard),
    /// 已有实例在运行，并且已被通知唤起窗口；调用方应立即结束本进程。
    AlreadyRunning,
}

/// 二次启动希望主实例做什么。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    /// 把主窗口拿到前面（默认，等同于再次双击）。
    Open,
    /// 直接唤起管家窗口（`mns-gui --butler`）。
    OpenButler,
}

#[cfg(windows)]
impl Intent {
    /// 命名事件的后缀。`Open` 沿用旧名字，升级过程中老 Primary 也认得。
    fn event_suffix(self) -> &'static str {
        match self {
            Intent::Open => "",
            Intent::OpenButler => "-butler",
        }
    }
}

/// 尝试取得 `app`（如 `"mns-gui"` / `"mns-help"`）的单实例锁。
///
/// 等价于 [`acquire_with_intent`] 带 `Intent::Open`。
pub fn acquire(app: &str) -> Acquired {
    acquire_with_intent(app, Intent::Open)
}

/// 尝试取得 `app` 的单实例锁；已有实例时按 `intent` 通知它。
pub fn acquire_with_intent(app: &str, intent: Intent) -> Acquired {
    #[cfg(windows)]
    {
        match windows_impl::acquire(app, intent) {
            Some(inner) => Acquired::Primary(Guard { inner }),
            None => Acquired::AlreadyRunning,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (app, intent);
        Acquired::Primary(Guard {})
    }
}

impl Guard {
    /// 是否有第二个实例请求唤起本进程的窗口（非阻塞探测）。
    ///
    /// 命中后立即复位事件，避免每次轮询都重复置前。
    pub fn wakeup_requested(&self) -> bool {
        #[cfg(windows)]
        {
            self.inner.wakeup_requested()
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    /// 阻塞到「又有人启动了本程序」。命中后复位事件再返回，调用方可以接着等
    /// 下一次。返回值就是那次启动带进来的意图（主窗口 / 管家窗口）。
    /// 没有可用事件（建锁失败的降级路径）时永不返回。
    ///
    /// 这是阻塞等待，不是轮询：没有第二个实例时这个线程一直睡在内核里，
    /// 不占 CPU，也不受定时器分辨率或系统节流影响。
    pub fn wait_wakeup(&self) -> Option<Intent> {
        #[cfg(windows)]
        {
            self.inner.wait_wakeup()
        }
        #[cfg(not(windows))]
        {
            None
        }
    }
}

#[cfg(windows)]
mod windows_impl {
    use super::Intent;
    use std::ffi::c_void;

    type Handle = *mut c_void;

    const ERROR_ALREADY_EXISTS: u32 = 183;
    const EVENT_MODIFY_STATE: u32 = 0x0002;
    const WAIT_OBJECT_0: u32 = 0;
    /// 第二个事件（管家窗口）命中时的返回值。
    const WAIT_OBJECT_0_BUTLER: u32 = WAIT_OBJECT_0 + 1;
    const INFINITE: u32 = u32::MAX;

    /// 事件句柄的固定顺序：`[Open, OpenButler]`。`WaitForMultipleObjects`
    /// 的返回值减 `WAIT_OBJECT_0` 就是这个数组的下标。
    const EVENT_COUNT: u32 = 2;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateMutexW(attributes: *const c_void, initial_owner: i32, name: *const u16) -> Handle;
        fn CreateEventW(
            attributes: *const c_void,
            manual_reset: i32,
            initial_state: i32,
            name: *const u16,
        ) -> Handle;
        fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> Handle;
        fn SetEvent(handle: Handle) -> i32;
        fn ResetEvent(handle: Handle) -> i32;
        fn WaitForSingleObject(handle: Handle, millis: u32) -> u32;
        fn WaitForMultipleObjects(
            count: u32,
            handles: *const Handle,
            wait_all: i32,
            millis: u32,
        ) -> u32;
        fn CloseHandle(handle: Handle) -> i32;
        fn GetLastError() -> u32;
    }

    pub struct Guard {
        mutex: Handle,
        events: [Handle; EVENT_COUNT as usize],
    }

    impl Guard {
        pub fn wakeup_requested(&self) -> bool {
            let mut requested = false;
            for event in self.events {
                if event.is_null() {
                    continue;
                }
                // SAFETY: `event` 是本进程创建的有效事件句柄，存活于 Guard 生命周期内。
                unsafe {
                    if WaitForSingleObject(event, 0) == WAIT_OBJECT_0 {
                        ResetEvent(event);
                        requested = true;
                    }
                }
            }
            requested
        }

        pub fn wait_wakeup(&self) -> Option<Intent> {
            if self.events.iter().any(|event| event.is_null()) {
                // 建锁失败时没有事件可用：永远等下去，别让调用方反复空转。
                loop {
                    std::thread::park();
                }
            }
            // SAFETY: 两个句柄都是本进程创建的有效事件句柄，存活于 Guard
            // 生命周期内；数组布局与 `EVENT_COUNT` 一致。
            unsafe {
                let index = WaitForMultipleObjects(EVENT_COUNT, self.events.as_ptr(), 0, INFINITE);
                match index {
                    WAIT_OBJECT_0 => {
                        // 手动重置事件：取出一次信号就复位，否则会反复命中。
                        ResetEvent(self.events[0]);
                        Some(Intent::Open)
                    }
                    WAIT_OBJECT_0_BUTLER => {
                        ResetEvent(self.events[1]);
                        Some(Intent::OpenButler)
                    }
                    _ => None,
                }
            }
        }
    }

    // 句柄可以跨线程等待：`wait_wakeup` 在专用线程上跑，`Drop` 仍在原线程。
    unsafe impl Send for Guard {}

    impl Drop for Guard {
        fn drop(&mut self) {
            // SAFETY: 句柄都由本进程创建，且不会被二次释放。
            unsafe {
                for event in self.events {
                    if !event.is_null() {
                        CloseHandle(event);
                    }
                }
                if !self.mutex.is_null() {
                    CloseHandle(self.mutex);
                }
            }
        }
    }

    fn event_name(app: &str, intent: Intent) -> Vec<u16> {
        wide(&format!(r"Local\MNS-{app}-wakeup{}", intent.event_suffix()))
    }

    /// `None` 表示已有实例在运行（并已按 `intent` 通知它唤起窗口）。
    pub fn acquire(app: &str, intent: Intent) -> Option<Guard> {
        let mutex_name = wide(&format!(r"Local\MNS-{app}-instance"));
        let intent_event_name = event_name(app, intent);
        let open_event_name = event_name(app, Intent::Open);
        let butler_event_name = event_name(app, Intent::OpenButler);
        // SAFETY: 名字是 NUL 结尾的 UTF-16；句柄只在成功分支里长期持有。
        unsafe {
            let mutex = CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr());
            if mutex.is_null() {
                // 建锁失败时宁可多开一个窗口，也不要让用户完全打不开程序。
                return Some(Guard {
                    mutex,
                    events: [std::ptr::null_mut(); EVENT_COUNT as usize],
                });
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                CloseHandle(mutex);
                // 先叫醒与意图对应的事件；升级过程中老 Primary 只有通用事件，
                // 打不开就退回它，至少别让这次启动悄无声息地消失。
                let mut event = OpenEventW(EVENT_MODIFY_STATE, 0, intent_event_name.as_ptr());
                if event.is_null() && intent != Intent::Open {
                    event = OpenEventW(EVENT_MODIFY_STATE, 0, open_event_name.as_ptr());
                }
                if !event.is_null() {
                    SetEvent(event);
                    CloseHandle(event);
                }
                return None;
            }
            // 手动重置事件：即使 Primary 还没开始等待，信号也不会丢。
            let events = [
                CreateEventW(std::ptr::null(), 1, 0, open_event_name.as_ptr()),
                CreateEventW(std::ptr::null(), 1, 0, butler_event_name.as_ptr()),
            ];
            Some(Guard { mutex, events })
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn primary(app: &str) -> Guard {
        match acquire(app) {
            Acquired::Primary(guard) => guard,
            Acquired::AlreadyRunning => panic!("首个实例应当成为 Primary"),
        }
    }

    fn secondary(app: &str, intent: Intent) {
        match acquire_with_intent(app, intent) {
            Acquired::AlreadyRunning => {}
            Acquired::Primary(_) => panic!("第二个实例不应成为 Primary"),
        }
    }

    /// 二次启动的意图必须原样到达 Primary：`--butler` 不能退化成
    /// 「把主窗口提到前面」。
    #[test]
    fn secondary_launch_carries_intent() {
        let app = format!("mns-test-intent-{}", std::process::id());
        let guard = primary(&app);
        assert!(!guard.wakeup_requested());

        secondary(&app, Intent::OpenButler);
        assert_eq!(guard.wait_wakeup(), Some(Intent::OpenButler));

        secondary(&app, Intent::Open);
        assert_eq!(guard.wait_wakeup(), Some(Intent::Open));
    }
}
