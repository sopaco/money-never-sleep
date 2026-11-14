//! 进程级单实例守卫。
//!
//! `mns-gui` 常驻托盘、`mns-help` 是独立窗口，两者都不应该被重复启动：
//! 用户再次双击时，期望的是「已经有窗口，把它拿到前面来」，而不是多开一个
//! 进程。做法是 Windows 命名互斥体 + 命名事件：
//!
//! - 先创建的进程持有 `Local\MNS-<app>-instance` 互斥体，成为 Primary；
//! - 后启动的进程发现互斥体已存在，`SetEvent` 通知 Primary 唤起窗口，然后
//!   自己立刻退出；
//! - Primary 用一个轻量轮询（见各 GUI 的 400ms 定时任务）检查事件，命中时
//!   走与托盘菜单「打开 MNS」完全相同的窗口路径。
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

/// 尝试取得 `app`（如 `"mns-gui"` / `"mns-help"`）的单实例锁。
pub fn acquire(app: &str) -> Acquired {
    #[cfg(windows)]
    {
        match windows_impl::acquire(app) {
            Some(inner) => Acquired::Primary(Guard { inner }),
            None => Acquired::AlreadyRunning,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        Acquired::Primary(Guard {})
    }
}

impl Guard {
    /// 是否有第二个实例请求唤起本进程的窗口。
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
}

#[cfg(windows)]
mod windows_impl {
    use std::ffi::c_void;

    type Handle = *mut c_void;

    const ERROR_ALREADY_EXISTS: u32 = 183;
    const EVENT_MODIFY_STATE: u32 = 0x0002;
    const WAIT_OBJECT_0: u32 = 0;

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
        fn CloseHandle(handle: Handle) -> i32;
        fn GetLastError() -> u32;
    }

    pub struct Guard {
        mutex: Handle,
        event: Handle,
    }

    impl Guard {
        pub fn wakeup_requested(&self) -> bool {
            if self.event.is_null() {
                return false;
            }
            // SAFETY: `event` 是本进程创建的有效事件句柄，存活于 Guard 生命周期内。
            unsafe {
                if WaitForSingleObject(self.event, 0) != WAIT_OBJECT_0 {
                    return false;
                }
                ResetEvent(self.event);
                true
            }
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            // SAFETY: 两个句柄都由本进程创建，且不会被二次释放。
            unsafe {
                if !self.event.is_null() {
                    CloseHandle(self.event);
                }
                if !self.mutex.is_null() {
                    CloseHandle(self.mutex);
                }
            }
        }
    }

    /// `None` 表示已有实例在运行（并已通知它唤起窗口）。
    pub fn acquire(app: &str) -> Option<Guard> {
        let mutex_name = wide(&format!(r"Local\MNS-{app}-instance"));
        let event_name = wide(&format!(r"Local\MNS-{app}-wakeup"));
        // SAFETY: 名字是 NUL 结尾的 UTF-16；句柄只在成功分支里长期持有。
        unsafe {
            let mutex = CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr());
            if mutex.is_null() {
                // 建锁失败时宁可多开一个窗口，也不要让用户完全打不开程序。
                return Some(Guard {
                    mutex,
                    event: std::ptr::null_mut(),
                });
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                CloseHandle(mutex);
                let event = OpenEventW(EVENT_MODIFY_STATE, 0, event_name.as_ptr());
                if !event.is_null() {
                    SetEvent(event);
                    CloseHandle(event);
                }
                return None;
            }
            // 手动重置事件：即使 Primary 还没开始轮询，信号也不会丢。
            let event = CreateEventW(std::ptr::null(), 1, 0, event_name.as_ptr());
            Some(Guard { mutex, event })
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }
}
