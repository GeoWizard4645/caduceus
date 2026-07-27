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

use std::sync::mpsc;
use std::time::Duration;

use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior, NSWindowLevel};
use tauri::{Runtime, WebviewWindow};

/// `NSPopUpMenuWindowLevel`.
///
/// Menus must draw over a full-screen app, which is exactly the requirement
/// here. Deliberately not `NSScreenSaverWindowLevel` (1000): that would also
/// cover system alerts and the login/screen-saver shield, and nothing here has
/// any business above a password prompt.
const OVERLAY_WINDOW_LEVEL: NSWindowLevel = 101;

/// How long to wait for the main thread to finish window configuration.
const MAIN_THREAD_TIMEOUT: Duration = Duration::from_secs(5);

/// Make a window follow the user into full-screen Spaces and draw above them.
///
/// Every AppKit and Tauri window call here runs on the main thread. Tauri
/// command handlers run on Tokio worker threads; on recent macOS, touching
/// window level from a worker trips `Must only be used from the main thread`
/// inside WindowManagement and kills the process with `EXC_BREAKPOINT`.
fn configure_overlay<R: Runtime>(window: &WebviewWindow<R>, level: NSWindowLevel) {
    configure_overlay_inner(window, level, false)
}

/// As [`configure_overlay`], and additionally take the foreground.
pub fn configure_and_activate<R: Runtime>(window: &WebviewWindow<R>, level: NSWindowLevel) {
    configure_overlay_inner(window, level, true)
}

fn configure_overlay_inner<R: Runtime>(
    window: &WebviewWindow<R>,
    level: NSWindowLevel,
    activate: bool,
) {
    let handle = window.clone();
    let (done_tx, done_rx) = mpsc::sync_channel(1);

    let scheduled = window.run_on_main_thread(move || {
        let _ = handle.set_visible_on_all_workspaces(true);
        let _ = handle.set_always_on_top(true);

        if let Ok(raw) = handle.ns_window() {
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
        }

        // Activation last, and on this same main-thread hop: doing it before
        // the level is set lets macOS raise the window at its *old* level, and
        // doing it from another thread races the window server.
        if activate {
            activate_app();
            if let Ok(raw) = handle.ns_window() {
                unsafe {
                    let ns_window: &NSWindow = &*raw.cast();
                    ns_window.makeKeyAndOrderFront(None);
                }
            }
        }

        let _ = done_tx.send(());
    });

    if scheduled.is_err() {
        log::warn!("could not schedule overlay window configuration on the main thread");
        return;
    }

    if done_rx.recv_timeout(MAIN_THREAD_TIMEOUT).is_err() {
        log::warn!("timed out waiting for overlay window configuration on the main thread");
    }
}

pub fn configure_staff_window<R: Runtime>(window: &WebviewWindow<R>) {
    configure_overlay(window, OVERLAY_WINDOW_LEVEL);
}

/// Bring Caduceus to the front, across Spaces.
///
/// `set_focus` alone is `makeKeyAndOrderFront:`, which orders a window within
/// *this* application. Caduceus runs as an `Accessory` app — it has no Dock icon
/// and is never the active application — so ordering a window forward while
/// another app owns the foreground does nothing you can see. Over a full-screen
/// app, where the foreground application also owns the entire Space, the result
/// is a palette that is technically visible, focus-less, and behind everything.
///
/// `activateIgnoringOtherApps:` is what actually takes the foreground. It is the
/// same call every launcher makes, and it is deliberately confined to the moment
/// the user asked for the palette.
pub fn activate_app() {
    let Some(mtm) = objc2::MainThreadMarker::new() else {
        // Called off the main thread; the caller schedules it instead.
        log::warn!("activate_app called off the main thread");
        return;
    };
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    #[allow(deprecated)]
    // The replacement (`activate()`) is macOS 14+, and Caduceus supports 11+.
    // On 14+ this still does the right thing.
    app.activateIgnoringOtherApps(true);
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

/// Show the Command Center over whatever is in front, including a full-screen app.
pub fn present_command_center<R: Runtime>(window: &WebviewWindow<R>) {
    configure_and_activate(window, OVERLAY_WINDOW_LEVEL);
}

/// Return the window to ordinary behaviour: normal level, its own Space.
///
/// Used once the palette is holding tabs, when it stops being an overlay and
/// starts being a window you work in. An always-on-top Settings page floating
/// over every other application is nobody's idea of correct.
pub fn configure_normal_window<R: Runtime>(window: &WebviewWindow<R>) {
    let handle = window.clone();
    let (done_tx, done_rx) = mpsc::sync_channel(1);

    let scheduled = window.run_on_main_thread(move || {
        let _ = handle.set_visible_on_all_workspaces(false);
        let _ = handle.set_always_on_top(false);
        if let Ok(raw) = handle.ns_window() {
            unsafe {
                let ns_window: &NSWindow = &*raw.cast();
                ns_window.setCollectionBehavior(NSWindowCollectionBehavior::Default);
                ns_window.setLevel(0);
            }
        }
        let _ = done_tx.send(());
    });

    if scheduled.is_err() {
        return;
    }
    let _ = done_rx.recv_timeout(MAIN_THREAD_TIMEOUT);
}
