//! macOS window tweaks so the staff stays above full-screen apps (MacParakeet-style).

use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior, NSWindowLevel};
use tauri::{Runtime, WebviewWindow};

/// Where the staff sits in the window stack.
///
/// `NSPopUpMenuWindowLevel`. The obvious choice is `NSStatusWindowLevel` (25),
/// which is what a menu-bar utility normally wants, and that is what this was —
/// but it is *below* the menu bar layer, and when another app takes over a
/// display full-screen the staff went with it and vanished. Pop-up menu level
/// sits above that layer, which is the point: menus have to draw over a
/// full-screen app, and so does this.
///
/// Deliberately not `NSScreenSaverWindowLevel` (1000). That would also cover
/// system alerts and the screen saver itself, and a decorative floating staff
/// has no business above a password prompt.
const STAFF_WINDOW_LEVEL: NSWindowLevel = 101;

pub fn configure_staff_window<R: Runtime>(window: &WebviewWindow<R>) {
    let _ = window.set_visible_on_all_workspaces(true);
    let _ = window.set_always_on_top(true);

    let Ok(raw) = window.ns_window() else {
        return;
    };

    unsafe {
        let ns_window: &NSWindow = &*raw.cast();
        let behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary;
        ns_window.setCollectionBehavior(behavior);
        ns_window.setLevel(STAFF_WINDOW_LEVEL);
    }
}
