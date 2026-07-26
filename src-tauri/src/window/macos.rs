//! macOS window tweaks so Caduceus stays usable over another app's full-screen space.
//!
//! Two things are required together, and either one alone does nothing:
//!
//! 1. **`canJoinAllSpaces`.** A full-screen app gets its own Space. Without
//!    this the window simply stays behind in the Space it was created in — it is
//!    not hidden or occluded, it is somewhere else entirely.
//! 2. **A window level above the menu bar layer.** `NSStatusWindowLevel` (25)
//!    is the natural pick for a menu-bar utility, but it sits below the layer a
//!    full-screen app occupies, so the window follows into the Space and is then
//!    drawn under it.

use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior, NSWindowLevel};
use tauri::{Runtime, WebviewWindow};

/// `NSPopUpMenuWindowLevel`.
///
/// Menus must draw over a full-screen app, which is exactly the requirement
/// here. Deliberately not `NSScreenSaverWindowLevel` (1000): that would also
/// cover system alerts and the login/screen-saver shield, and nothing here has
/// any business above a password prompt.
const OVERLAY_WINDOW_LEVEL: NSWindowLevel = 101;

/// Make a window follow the user into full-screen Spaces and draw above them.
///
/// The `NSWindow` calls are marshalled onto the main thread. AppKit permits
/// window mutation from nowhere else, and every caller here arrives on a Tauri
/// command handler, which runs on a worker thread — `setLevel:` from there trips
/// an assertion inside `WindowManagement` and takes the process down with
/// `EXC_BREAKPOINT`. Tauri's own `set_always_on_top` and
/// `set_visible_on_all_workspaces` already hop threads internally, which is why
/// only the raw calls need this.
fn configure_overlay<R: Runtime>(window: &WebviewWindow<R>, level: NSWindowLevel) {
    let _ = window.set_visible_on_all_workspaces(true);
    let _ = window.set_always_on_top(true);

    let handle = window.clone();
    let _ = window.run_on_main_thread(move || {
        let Ok(raw) = handle.ns_window() else {
            return;
        };
        unsafe {
            let ns_window: &NSWindow = &*raw.cast();
            // `Stationary` keeps it put during Exposé instead of being swept
            // aside with ordinary windows. `IgnoresCycle` keeps it out of Cmd-`
            // rotation, which it has no business appearing in.
            let behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle;
            ns_window.setCollectionBehavior(behavior);
            ns_window.setLevel(level);
        }
    });
}

pub fn configure_staff_window<R: Runtime>(window: &WebviewWindow<R>) {
    configure_overlay(window, OVERLAY_WINDOW_LEVEL);
}

/// The Command Center needs the same treatment as the staff.
///
/// It had neither half: no collection behavior at all, and `set_always_on_top`
/// alone puts it at `NSFloatingWindowLevel` (3) — below the menu bar layer. So
/// the one thing you actually reach for while another app is full-screen was
/// the one window that could not appear there.
pub fn configure_command_center_window<R: Runtime>(window: &WebviewWindow<R>) {
    configure_overlay(window, OVERLAY_WINDOW_LEVEL);
}
