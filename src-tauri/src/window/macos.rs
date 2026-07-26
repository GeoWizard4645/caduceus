//! macOS window tweaks so the staff stays above full-screen apps (MacParakeet-style).

use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior, NSWindowLevel};
use tauri::{Runtime, WebviewWindow};

/// Status-bar level keeps the staff above normal windows and most full-screen spaces.
const STAFF_WINDOW_LEVEL: NSWindowLevel = 25;

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
