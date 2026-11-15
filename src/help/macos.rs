//! macOS-only window/menu plumbing for the help viewer.
//!
//! Everything here exists because `mns-help` is not a standalone bundle: it
//! ships inside `MNS.app/Contents/MacOS`, so AppKit treats it as the `MNS`
//! application. Two consequences follow:
//!
//! * The application menu is labelled from `CFBundleName` ("MNS"), so the
//!   viewer has to rename it after the menu is on screen.
//! * The process has no Dock icon of its own and no default menu handling, so
//!   the viewer has to install its own menu items and key bindings.

use std::ffi::CString;

use objc2::runtime::AnyObject;
use objc2::{class, msg_send};

/// Replaces the title AppKit draws for the application menu.
///
/// Two AppKit quirks make this a late operation:
///
/// * The menu has to be installed first. `NSMenuItem::setTitle` on a menu that
///   does not exist yet is discarded when the menu bar is created.
/// * The new title must *differ* from the one the menu was installed with.
///   Re-writing the same string leaves AppKit's cached application name in
///   place, which is why the menu is installed as plain "MNS" and this
///   function is what turns it into "MNS 帮助中心".
///
/// Must run on the main thread; the GPUI update loop guarantees that.
pub fn rename_app_menu(title: &str) {
    let Ok(name) = CString::new(title) else {
        return;
    };
    // SAFETY: every pointer is checked for null, the C string is NUL-terminated
    // and outlives the calls that read it, and the selectors used here are part
    // of NSApplication/NSMenu/NSMenuItem on every supported macOS.
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        if app.is_null() {
            return;
        }
        let main_menu: *mut AnyObject = msg_send![app, mainMenu];
        if main_menu.is_null() {
            return;
        }
        let app_item: *mut AnyObject = msg_send![main_menu, itemAtIndex: 0isize];
        if app_item.is_null() {
            return;
        }
        let ns_title: *mut AnyObject =
            msg_send![class!(NSString), stringWithUTF8String: name.as_ptr()];
        if ns_title.is_null() {
            return;
        }
        let _: () = msg_send![app_item, setTitle: ns_title];
        // The submenu carries the same name; keeping the two in sync avoids a
        // stale "MNS" in the menu's own title bar/AX description.
        let submenu: *mut AnyObject = msg_send![app_item, submenu];
        if !submenu.is_null() {
            let _: () = msg_send![submenu, setTitle: ns_title];
        }
    }
}
