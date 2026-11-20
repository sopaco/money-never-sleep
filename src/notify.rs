//! 系统通知：管家判定「该说」且窗口没开时送达一次。
//!
//! 这里只负责送达，不判断该不该说——开口判定在 `butler::schedule`。macOS
//! 走 `UNUserNotificationCenter`：打包后的 `MNS.app` 有 bundle id，横幅会以
//! 本应用名义出现；点击通知会通过通道唤醒 GUI 打开管家窗口。
//!
//! Windows / Linux 暂不弹系统通知：Windows Toast 需要 AUMID 与快捷方式注册，
//! 属于安装包侧工作；这两个平台仍以托盘提示为准。

use std::sync::OnceLock;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// 点击事件走通道而不是轮询：回调在哪个线程都能 `send`，GUI 侧用
/// `recv().await` 挂起等待，空闲时零唤醒。
///
/// 载荷是通知 id（旧版通知没有 userInfo 时为 `None`）。宿主拿它把点击落到
/// 具体那一条开口上，而不是只开一个空窗。
static OPEN_TX: OnceLock<UnboundedSender<Option<String>>> = OnceLock::new();

/// 点击通知会让系统激活整个 app，`.on_activate` 会把隐藏的主窗口顶到前面。
/// 这个标记告诉 `ShowMainWindow`：这次激活来自管家通知，别抢焦点。
///
/// 用时间戳而不是布尔：如果 app 本来就是激活状态，`on_activate` 不会触发，
/// 标记不能被下一次「用户点 Dock 图标」误消费。
static CLICK_AT: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// 这次点击对应哪条通知。与 `CLICK_AT` 分开存：`on_reopen` 只知道「是通知
/// 点进来的」，真正把 id 注入窗口的是通道消费端；两边都能读到同一个 id。
static CLICK_NOTICE: std::sync::Mutex<Option<(std::time::Instant, Option<String>)>> =
    std::sync::Mutex::new(None);

/// 建立点击通道并安装通知回调，返回点击事件的接收端。
///
/// 送达只能由 app 主可执行文件做：`mns-butlerd` 与 GUI 同 bundle，但它不是
/// 系统登记的那个 app 进程，`requestAuthorization` 会返回 `UNErrorDomain 1`
/// （notificationsNotAllowed）。所以**判定**可以拆进程，**送达**必须留在
/// `mns-gui`；守护进程产生的开口通过 `pending.jsonl` 交给 GUI 发。
///
/// 测试态与 `cargo run`（无 bundle）下空操作，但通道仍然可用。
pub fn init() -> UnboundedReceiver<Option<String>> {
    let (tx, rx) = unbounded_channel();
    let _ = OPEN_TX.set(tx);
    #[cfg(target_os = "macos")]
    macos::install();
    rx
}

/// 送达一条通知。正文已经过开口判定的长度与话术校验。
///
/// `notice_id` 写进 `userInfo`：系统把整条 `UNNotificationResponse` 交回来
/// 时，宿主能知道用户点的是哪一条（只靠正文匹配会在两条同文案时串号）。
///
/// 返回 `true` 表示已交给系统投递（不是「用户已看到」）；未打包的开发
/// 二进制、非 macOS 平台返回 `false`，调用方据此不要把审计写成「已送达」。
pub fn send(text: &str, notice_id: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::send(text, notice_id)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (text, notice_id);
        false
    }
}

/// 消费「这次激活是点通知来的」标记。只有一次；没点过通知就返回 false。
pub fn take_click_pending() -> bool {
    let Ok(mut slot) = CLICK_AT.lock() else {
        return false;
    };
    match slot.take() {
        // 系统激活发生在点击之后几百毫秒内，两秒足够宽裕。
        Some(at) => at.elapsed() < std::time::Duration::from_secs(2),
        None => false,
    }
}

/// 最近一次通知点击的 id（非消费）。`on_reopen` 与通道消费端都会读它，
/// 谁先到都能拿到同一份；10 秒之外的旧记录视为无关，不再返回。
pub fn recent_click_notice() -> Option<Option<String>> {
    let slot = CLICK_NOTICE.lock().ok()?;
    let (at, id) = slot.as_ref()?;
    if at.elapsed() < std::time::Duration::from_secs(10) {
        Some(id.clone())
    } else {
        None
    }
}

/// 由 `UNUserNotificationCenterDelegate` 调用，可能在任意线程。
///
/// 只有 macOS 有通知回调；其他平台没有调用方，别留下一个永不被使用的函数
/// 让 Windows 构建报警告。
#[cfg(target_os = "macos")]
fn on_click(notice_id: Option<String>) {
    if let Ok(mut slot) = CLICK_AT.lock() {
        *slot = Some(std::time::Instant::now());
    }
    if let Ok(mut slot) = CLICK_NOTICE.lock() {
        *slot = Some((std::time::Instant::now(), notice_id.clone()));
    }
    if let Some(tx) = OPEN_TX.get() {
        // 接收端只在进程退出时消失；发送失败说明 GUI 已在关闭，忽略即可。
        let _ = tx.send(notice_id);
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use block2::{DynBlock, RcBlock};
    use objc2::ClassType;
    use objc2::define_class;
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Bool, ProtocolObject};
    use objc2_foundation::{NSBundle, NSDictionary, NSError, NSObject, NSObjectProtocol, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
        UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
        UNNotificationSettings, UNNotificationSound, UNUserNotificationCenter,
        UNUserNotificationCenterDelegate,
    };
    use std::ptr::NonNull;

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "MNSButlerNotificationDelegate"]
        #[ivars = ()]
        struct NotificationDelegate;

        unsafe impl NSObjectProtocol for NotificationDelegate {}

        unsafe impl UNUserNotificationCenterDelegate for NotificationDelegate {
            // 应用在前台时也把横幅放出来：窗口没开时用户未必看着托盘。
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                completion_handler: &DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                completion_handler.call((
                    UNNotificationPresentationOptions::Banner
                        | UNNotificationPresentationOptions::Sound,
                ));
            }

            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(
                &self,
                _center: &UNUserNotificationCenter,
                response: &UNNotificationResponse,
                completion_handler: &DynBlock<dyn Fn()>,
            ) {
                // `userInfo` 里的 notice_id 由发送端写入。旧版通知没有这个键，
                // 读不到就退回「只开管家窗」的旧行为，不能让点击变成空操作。
                let content = response.notification().request().content();
                let info = content.userInfo();
                let key = NSString::from_str("notice_id");
                let notice_id = unsafe {
                    let raw: *mut AnyObject = msg_send![&*info, objectForKey: &*key];
                    if raw.is_null() {
                        None
                    } else {
                        let utf8: *const std::ffi::c_char = msg_send![raw, UTF8String];
                        if utf8.is_null() {
                            None
                        } else {
                            Some(
                                std::ffi::CStr::from_ptr(utf8)
                                    .to_string_lossy()
                                    .into_owned(),
                            )
                        }
                    }
                };
                eprintln!("[mns-gui] 收到通知点击（id={notice_id:?}），准备打开管家");
                super::on_click(notice_id);
                completion_handler.call(());
            }
        }
    );

    /// 只有打包后的 `MNS.app` 才有 bundle id；无 bundle 的进程调用通知中心
    /// 会直接报内部错误，所以开发态与测试态静默跳过。
    fn bundled() -> bool {
        NSBundle::mainBundle().bundleIdentifier().is_some()
    }

    pub(super) fn install() {
        if !bundled() {
            return;
        }
        unsafe {
            let center = UNUserNotificationCenter::currentNotificationCenter();
            let delegate: Retained<NotificationDelegate> =
                msg_send![NotificationDelegate::class(), new];
            center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            // delegate 是 weak 属性：泄漏一个实例换取进程级生命周期，避免系统
            // 提前回收后点击回调变成空操作。
            std::mem::forget(delegate);
        }
        // 打包 app 的通知权限只问一次；这里把真实状态打出来，否则「没弹横幅」
        // 无法区分「话没说」和「说了但系统没让显示」。
        log_status("启动");
    }

    /// 打印当前授权状态。`UNUserNotificationCenter` 的设置在系统设置里可以
    /// 随时改，所以每次送达前都会再读一次，而不是缓存启动时的值。
    pub(super) fn log_status(tag: &str) {
        if !bundled() {
            return;
        }
        let label = tag.to_owned();
        let handler = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
            let settings = unsafe { settings.as_ref() };
            let status = settings.authorizationStatus();
            let status = if status == objc2_user_notifications::UNAuthorizationStatus::Authorized {
                "已允许"
            } else if status == objc2_user_notifications::UNAuthorizationStatus::Denied {
                "已拒绝"
            } else if status == objc2_user_notifications::UNAuthorizationStatus::NotDetermined {
                "未询问"
            } else {
                "临时/其他"
            };
            eprintln!(
                "[mns-gui] 通知授权（{label}）：{status}（code {}）",
                settings.authorizationStatus().0
            );
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .getNotificationSettingsWithCompletionHandler(&handler);
    }

    pub(super) fn send(text: &str, notice_id: &str) -> bool {
        if !bundled() {
            return false;
        }
        let body = text.to_owned();
        let notice_id = notice_id.to_owned();
        // 未询问时才弹系统授权框；已允许直接送达，已拒绝不要反复打扰。
        let handler = RcBlock::new(move |granted: Bool, error: *mut NSError| {
            if granted.as_bool() {
                deliver(&body, &notice_id);
            } else if !error.is_null() {
                // 把系统给的原因打出来：没有这一行，「没弹横幅」只能靠猜。
                let message = unsafe { (*error).localizedDescription() };
                eprintln!("[mns-gui] 通知授权失败，本次不送达：{message}");
            } else {
                eprintln!("[mns-gui] 通知未获允许，请在系统设置里开启「MNS 投资助手」");
            }
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .requestAuthorizationWithOptions_completionHandler(
                UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
                &handler,
            );
        // 授权回调是异步的；这里只能承诺「已提交投递」。被系统拒绝时会打日志，
        // 审计不把那种情况写成 delivered 由宿主侧另行判断（见 `consume_pending`）。
        true
    }

    fn deliver(text: &str, notice_id: &str) {
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str("MNS · 管家"));
        content.setBody(&NSString::from_str(text));
        content.setSound(Some(&UNNotificationSound::defaultSound()));
        // 点击回调只拿得到 request 的内容，正文可能重复（同一事件重发），
        // 所以身份必须走 userInfo。
        let key = NSString::from_str("notice_id");
        let value = NSString::from_str(notice_id);
        let info: Retained<NSDictionary<NSString, NSString>> =
            NSDictionary::from_slices(&[&*key], &[&*value]);
        unsafe {
            content.setUserInfo(info.cast_unchecked::<AnyObject, AnyObject>());
        }
        // 用 notice_id 做系统侧标识：同一事件重发会替换旧横幅而不是叠两条。
        let identifier = NSString::from_str(&format!("mns-butler-{notice_id}"));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &identifier,
            &content,
            None,
        );
        let handler = RcBlock::new(|error: *mut NSError| {
            if !error.is_null() {
                eprintln!("[mns-gui] 系统通知发送失败");
            }
        });
        center.addNotificationRequest_withCompletionHandler(&request, Some(&handler));
    }
}
