//! 系统通知：管家判定「该说」且窗口没开时送达一次。
//!
//! 这里只负责送达，不判断该不该说——开口判定在 `butler::schedule`。macOS
//! 走 `UNUserNotificationCenter`：打包后的 `MNS.app` 有 bundle id，横幅会以
//! 本应用名义出现；点击通知会通过通道唤醒 GUI 打开管家窗口。
//!
//! Windows 11 走 WinRT Toast。未打包应用必须先有一个带 `AppUserModelID`
//! 属性的开始菜单快捷方式，否则 `CreateToastNotifierWithId` 会拒绝投递；
//! 这个快捷方式由本模块在宿主启动时按当前 exe 路径创建/补齐（与安装包创建
//! 的快捷方式同名，升级覆盖后会自动补回属性）。点击走自定义 `mns:` 协议：
//! 系统按注册的命令拉起一个新的 `mns-gui`，新进程把 notice id 交给已在运行
//! 的宿主后立即退出。Windows 10 与 Linux 没有这条通道，仍以托盘提示为准。

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
    #[cfg(target_os = "windows")]
    windows::install();
    rx
}

/// 送达一条通知。正文已经过开口判定的长度与话术校验。
///
/// `notice_id` 写进 `userInfo`：系统把整条 `UNNotificationResponse` 交回来
/// 时，宿主能知道用户点的是哪一条（只靠正文匹配会在两条同文案时串号）。
///
/// 返回 `true` 表示已交给系统投递（不是「用户已看到」）；未打包的开发
/// 二进制、非 macOS / Windows 11 平台返回 `false`，调用方据此不要把审计写成
/// 「已送达」。
pub fn send(text: &str, notice_id: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::send(text, notice_id)
    }
    #[cfg(target_os = "windows")]
    {
        windows::send(text, notice_id)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (text, notice_id);
        false
    }
}

/// Windows Toast 点击后由系统按 `mns:` 协议拉起本程序，参数形如
/// `--toast "mns:butler?notice=<id>"`。其他平台没有这条路径。
#[cfg(target_os = "windows")]
pub fn toast_notice_arg() -> Option<String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let raw = args
        .iter()
        .position(|arg| arg == "--toast")
        .and_then(|index| args.get(index + 1))
        .or_else(|| args.iter().find(|arg| arg.starts_with("mns:")))?;
    launch_notice(raw)
}

/// 从 `mns:butler?notice=<id>` 里取出 notice id。
#[cfg(target_os = "windows")]
fn launch_notice(raw: &str) -> Option<String> {
    let (_, rest) = raw.split_once("notice=")?;
    let id = rest.split(['&', '#']).next().unwrap_or(rest).trim();
    (!id.is_empty()).then(|| id.to_owned())
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

/// Windows 11 的 Toast 投递与通知身份注册。
///
/// 未打包应用没有包身份，系统只认「开始菜单里带 AppUserModelID 属性的
/// 快捷方式」：没有它，`CreateToastNotifierWithId` 能创建对象但 `Show` 会被
/// 系统丢掉，也不会有任何报错。安装包建的同名快捷方式在升级时会被重写、
/// 丢掉属性，所以宿主每次启动都补一次 AUMID；已存在的快捷方式不动目标
/// （便携版/开发版不该把「开始 → MNS」改指到临时构建），只有完全缺失时
/// 才新建一条。AUMID 本身稳定不变。
///
/// 点击不做 COM 激活服务器（需要注册 CLSID 并实现
/// `INotificationActivationCallback`）：Toast 声明 `activationType="protocol"`，
/// 系统按 HKCU 里注册的 `mns:` 协议拉起一个新的 `mns-gui`，新进程把 notice
/// id 通过本地通道交给已有宿主后退出。
#[cfg(target_os = "windows")]
mod windows {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};

    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemAlloc, IBindCtx, IPersistFile,
    };
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegSetValueExW,
    };
    use windows::Win32::System::Variant::VT_LPWSTR;
    use windows::Win32::System::WinRT::{RO_INIT_SINGLETHREADED, RoInitialize};
    use windows::Win32::UI::Shell::PropertiesSystem::{
        GPS_READWRITE, IPropertyStore, SHGetPropertyStoreFromParsingName,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{HSTRING, Interface, PCWSTR, PWSTR};

    /// 稳定的应用身份。改了它等于换一个通知来源，用户的通知设置会重来一次。
    const AUMID: &str = "Sopaco.MNS.Investment";
    /// 与安装包 `[Icons]` 创建的同名快捷方式，避免开始菜单出现两个 MNS。
    const SHORTCUT_NAME: &str = "MNS 投资助手.lnk";
    /// Toast 点击回跳用的协议。命令在 HKCU 下注册，不需要管理员。
    const URI_SCHEME: &str = "mns";

    static REGISTERED: AtomicBool = AtomicBool::new(false);

    pub(super) fn install() {
        // WinRT 与 Shell COM 都要求线程初始化过 COM。GPUI 在起消息循环前
        // 已经 OleInitialize 成 STA，这里再声明一次同模式（重复初始化返回
        // S_FALSE/RPC_E_CHANGED_MODE），失败可以忽略。不能用 MTA：那会让
        // UI 线程的 STA 初始化失败，拖拽与文件对话框都会跟着坏。
        unsafe {
            let _ = RoInitialize(RO_INIT_SINGLETHREADED);
        }
        if let Err(error) = register() {
            eprintln!("[mns-gui] Windows 通知注册失败：{error}");
        }
    }

    fn register() -> Result<(), String> {
        let exe = std::env::current_exe()
            .map_err(|error| format!("定位 mns-gui.exe 失败：{error}"))?;
        ensure_shortcut(&exe)?;
        ensure_uri_scheme(&exe)?;
        REGISTERED.store(true, Ordering::Relaxed);
        Ok(())
    }

    pub(super) fn send(text: &str, notice_id: &str) -> bool {
        // 启动时注册失败（例如 APPDATA 一时不可写）时，投递前再补一次。
        if !REGISTERED.load(Ordering::Relaxed)
            && let Err(error) = register()
        {
            eprintln!("[mns-gui] Windows 通知注册失败：{error}");
            return false;
        }
        match show(text, notice_id) {
            Ok(()) => true,
            Err(error) => {
                eprintln!("[mns-gui] Windows 通知发送失败：{error}");
                false
            }
        }
    }

    fn show(text: &str, notice_id: &str) -> Result<(), String> {
        let launch = format!("{URI_SCHEME}:butler?notice={notice_id}");
        // `ToastGeneric` 是 Windows 11 的通用模板；Title 固定，正文是管家
        // 已经过开口校验的一句话。点击走协议，不依赖 COM 激活服务器。
        let xml = format!(
            "<toast activationType=\"protocol\" launch=\"{}\"><visual><binding template=\"ToastGeneric\"><text>MNS · 管家</text><text>{}</text></binding></visual><audio src=\"ms-winsoundevent:Notification.Default\"/></toast>",
            xml_escape(&launch),
            xml_escape(text)
        );
        let document = XmlDocument::new()
            .map_err(|error| format!("创建通知内容失败：{error}"))?;
        document
            .LoadXml(&HSTRING::from(xml))
            .map_err(|error| format!("通知内容无效：{error}"))?;
        let notifier =
            ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))
                .map_err(|error| format!("注册通知通道失败：{error}"))?;
        let toast = ToastNotification::CreateToastNotification(&document)
            .map_err(|error| format!("构造通知失败：{error}"))?;
        notifier
            .Show(&toast)
            .map_err(|error| format!("提交通知失败：{error}"))?;
        Ok(())
    }

    /// 确保开始菜单里有带 `PKEY_AppUserModel_ID` 的快捷方式。
    ///
    /// 已存在时只补属性，不改目标：安装包建的快捷方式指向正式安装目录，
    /// 便携版/开发版若把它改指自己，用户下次点「开始 → MNS」就会跑到临时
    /// 构建上。真正缺快捷方式（例如解压即用的便携包）才新建一条。
    fn ensure_shortcut(exe: &Path) -> Result<(), String> {
        let path = shortcut_path().ok_or("找不到 APPDATA，无法注册通知身份")?;
        if path.exists() {
            return set_aumid(&path);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("创建开始菜单目录失败：{error}"))?;
        }
        let exe_wide = HSTRING::from(exe.as_os_str().to_string_lossy().as_ref());
        let path_wide = HSTRING::from(path.as_os_str().to_string_lossy().as_ref());
        unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
                .map_err(|error| format!("创建快捷方式失败：{error}"))?;
            link.SetPath(&exe_wide)
                .map_err(|error| format!("设置快捷方式目标失败：{error}"))?;
            let _ = link.SetDescription(&HSTRING::from("MNS 投资助手"));
            let _ = link.SetIconLocation(&exe_wide, 0);
            let file: IPersistFile = link
                .cast()
                .map_err(|error| format!("快捷方式不支持持久化：{error}"))?;
            file.Save(&path_wide, true)
                .map_err(|error| format!("写入快捷方式失败：{error}"))?;
        }
        set_aumid(&path)
    }

    fn shortcut_path() -> Option<PathBuf> {
        let appdata = std::env::var_os("APPDATA")?;
        Some(
            PathBuf::from(appdata)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join(SHORTCUT_NAME),
        )
    }

    fn set_aumid(path: &Path) -> Result<(), String> {
        let path_wide = HSTRING::from(path.as_os_str().to_string_lossy().as_ref());
        let value = prop_variant_string(AUMID)?;
        unsafe {
            let store: IPropertyStore = SHGetPropertyStoreFromParsingName(
                &path_wide,
                None::<&IBindCtx>,
                GPS_READWRITE,
            )
            .map_err(|error| format!("打开快捷方式属性失败：{error}"))?;
            store
                .SetValue(&PKEY_AppUserModel_ID, &value)
                .map_err(|error| format!("写入 AppUserModelID 失败：{error}"))?;
            store
                .Commit()
                .map_err(|error| format!("保存快捷方式属性失败：{error}"))?;
        }
        // `PROPVARIANT` 的 Drop 会调用 `PropVariantClear` 释放我们分配的
        // `VT_LPWSTR`，这里不要再 `CoTaskMemFree` 一次——双重释放会直接
        // 触发 Windows 堆损坏（STATUS_HEAP_CORRUPTION）。
        Ok(())
    }

    /// 构造 `VT_LPWSTR` 的 PROPVARIANT；生命周期结束时由它的 `Drop` 释放。
    fn prop_variant_string(text: &str) -> Result<PROPVARIANT, String> {
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let raw = unsafe { CoTaskMemAlloc(wide.len() * 2) } as *mut u16;
        if raw.is_null() {
            return Err("分配通知身份字符串失败".to_owned());
        }
        unsafe {
            std::ptr::copy_nonoverlapping(wide.as_ptr(), raw, wide.len());
        }
        let mut value = PROPVARIANT::default();
        unsafe {
            let inner = &mut *value.Anonymous.Anonymous;
            inner.vt = VT_LPWSTR;
            inner.Anonymous.pwszVal = PWSTR(raw);
        }
        Ok(value)
    }

    /// 注册 `mns:` 协议：点击 Toast 时由系统拉起 `mns-gui --toast <uri>`。
    fn ensure_uri_scheme(exe: &Path) -> Result<(), String> {
        let command = format!("\"{}\" --toast \"%1\"", exe.display());
        write_reg_string(&format!(r"Software\Classes\{URI_SCHEME}"), None, "URL:MNS Protocol")?;
        write_reg_string(
            &format!(r"Software\Classes\{URI_SCHEME}"),
            Some("URL Protocol"),
            "",
        )?;
        write_reg_string(
            &format!(r"Software\Classes\{URI_SCHEME}\shell\open\command"),
            None,
            &command,
        )?;
        Ok(())
    }

    fn write_reg_string(subkey: &str, name: Option<&str>, value: &str) -> Result<(), String> {
        let subkey = HSTRING::from(subkey);
        let mut key = HKEY::default();
        let created = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                &subkey,
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut key,
                None,
            )
        };
        if created.0 != 0 {
            return Err(format!("打开注册表项失败：{}", created.0));
        }
        let name_wide = name.map(HSTRING::from);
        let name_ptr = match &name_wide {
            Some(name) => PCWSTR(name.as_ptr()),
            None => PCWSTR::null(),
        };
        // REG_SZ 需要把结尾的 NUL 一起写进去。
        let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes =
            unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };
        let written = unsafe { RegSetValueExW(key, name_ptr, None, REG_SZ, Some(bytes)) };
        let _ = unsafe { RegCloseKey(key) };
        if written.0 != 0 {
            return Err(format!("写入注册表值失败：{}", written.0));
        }
        Ok(())
    }

    fn xml_escape(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for ch in text.chars() {
            match ch {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&apos;"),
                other => out.push(other),
            }
        }
        out
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::launch_notice;

    #[test]
    fn parses_notice_id_from_protocol_launch() {
        assert_eq!(
            launch_notice("mns:butler?notice=n1763689500000-1a2b3c4d").as_deref(),
            Some("n1763689500000-1a2b3c4d")
        );
        assert_eq!(
            launch_notice("mns:butler?notice=n1-ff&from=toast").as_deref(),
            Some("n1-ff")
        );
        // 没有 notice 参数或 id 为空时不能误开一个空窗。
        assert_eq!(launch_notice("mns:butler"), None);
        assert_eq!(launch_notice("mns:butler?notice="), None);
    }
}
