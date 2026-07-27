//! macOS window tweaks so Caduceus stays usable over another app's full-screen space.
//!
//! Three things are required together, and any one alone does nothing:
//!
//! 1. **`canJoinAllSpaces`.** A full-screen app gets its own Space. Without
//!    this the window simply stays behind in the Space it was created in — it is
//!    not hidden or occluded, it is somewhere else entirely.
//! 2. **A window level above the menu bar layer.** `NSStatusWindowLevel` (25)
//!    is the natural pick for a menu-bar utility, but it sits below the layer a
//!    full-screen app occupies, so the window follows into the Space and is then
//!    drawn under it.
//! 3. **A non-activating `NSPanel`.** The first two put the window on screen;
//!    neither gives it the keyboard. See [`super::panel`] — that module is where
//!    the interesting half of this now lives.

use std::sync::mpsc;
use std::time::Duration;

use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior, NSWindowLevel};
use tauri::{Runtime, WebviewWindow};

use super::panel;

/// `NSPopUpMenuWindowLevel`.
///
/// Menus must draw over a full-screen app, which is exactly the requirement
/// here. Deliberately not `NSScreenSaverWindowLevel` (1000): that would also
/// cover system alerts and the login/screen-saver shield, and nothing here has
/// any business above a password prompt.
const OVERLAY_WINDOW_LEVEL: NSWindowLevel = 101;

/// How long to wait for the main thread to finish window configuration.
const MAIN_THREAD_TIMEOUT: Duration = Duration::from_secs(5);

/// Collection behaviour shared by both overlays.
///
/// `Stationary` keeps them put during Exposé instead of being swept aside with
/// ordinary windows. `IgnoresCycle` keeps them out of Cmd-` rotation, which they
/// have no business appearing in.
fn overlay_behavior() -> NSWindowCollectionBehavior {
    NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::FullScreenAuxiliary
        | NSWindowCollectionBehavior::Stationary
        | NSWindowCollectionBehavior::IgnoresCycle
}

/// What to do with the window once it has been configured.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Present {
    /// Configure only; leave the window where it is in the ordering.
    No,
    /// Raise it and give it the keyboard without activating Caduceus.
    WithoutActivating,
}

/// Whether this window is allowed to be typed into without Caduceus activating.
///
/// Both overlays want it. Caduceus is an Accessory app and therefore never the
/// active one, so the alternative is `activateIgnoringOtherApps:`, and doing
/// that from inside somebody's full-screen Space throws them out of it. That is
/// the behaviour this whole module exists to avoid — including for Settings,
/// which is a tab in the same window and must not drag you to the desktop just
/// because you opened it.
const NONACTIVATING: bool = true;

/// Make a window follow the user into full-screen Spaces and draw above them.
///
/// Every AppKit and Tauri window call here runs on the main thread. Tauri
/// command handlers run on Tokio worker threads; on recent macOS, touching
/// window level from a worker trips `Must only be used from the main thread`
/// inside WindowManagement and kills the process with `EXC_BREAKPOINT`.
fn configure_overlay<R: Runtime>(
    window: &WebviewWindow<R>,
    kind: panel::Kind,
    present: Present,
) {
    let handle = window.clone();
    let (done_tx, done_rx) = mpsc::sync_channel(1);

    let scheduled = window.run_on_main_thread(move || {
        let _ = handle.set_visible_on_all_workspaces(true);
        let _ = handle.set_always_on_top(true);

        if let Ok(raw) = handle.ns_window() {
            // SAFETY: `ns_window()` hands back this window's live `NSWindow`,
            // and this closure is on the main thread.
            unsafe {
                let ns_window: &NSWindow = &*raw.cast();

                // The panel conversion has to happen before anything else: the
                // style mask and the panel-only setters below are meaningless
                // on a plain window, and `Present::WithoutActivating` is only
                // honest once the non-activating flag is really set.
                let is_panel = panel::ensure_panel(ns_window, kind);
                if is_panel {
                    panel::configure_panel(ns_window, NONACTIVATING);
                }

                ns_window.setCollectionBehavior(overlay_behavior());
                ns_window.setLevel(OVERLAY_WINDOW_LEVEL);

                // Ordering last, and on this same main-thread hop: doing it
                // before the level is set lets macOS raise the window at its
                // *old* level, and doing it from another thread races the
                // window server.
                match present {
                    Present::No => {}
                    Present::WithoutActivating if is_panel => {
                        panel::present_without_activating(ns_window);
                    }
                    // No panel means no non-activating key window, so the only
                    // way to be typed into is the old one. Better a Space switch
                    // than a palette that swallows keystrokes.
                    Present::WithoutActivating => {
                        activate_app();
                        ns_window.makeKeyAndOrderFront(None);
                    }
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
    configure_overlay(window, panel::Kind::Staff, Present::No);
}

/// Make the Command Center ready to appear in whatever Space is in front.
///
/// **Call this before `show()`, not after.** A window ordered in while its
/// collection behaviour still says "one Space" is placed in the Space it was
/// created in — the desktop — and from inside a full-screen app that is simply
/// somewhere else. Setting `canJoinAllSpaces` on it afterwards does not
/// reliably drag it across, so the palette came up correctly and invisibly.
/// This was the last thing standing between the hotkey and a full-screen app.
pub fn prepare_command_center<R: Runtime>(window: &WebviewWindow<R>) {
    configure_overlay(window, panel::Kind::Command, Present::No);
}

/// Bring Caduceus to the front, across Spaces.
///
/// The fallback path only, now. `activateIgnoringOtherApps:` is what makes an
/// Accessory app's window focusable, and it is also what drops the user out of
/// whatever full-screen Space they were in — which is why the panel route above
/// is preferred wherever it is available.
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
pub fn configure_command_center_window<R: Runtime>(window: &WebviewWindow<R>) {
    configure_overlay(window, panel::Kind::Command, Present::No);
}

/// Show the Command Center over whatever is in front, including a full-screen app.
pub fn present_command_center<R: Runtime>(window: &WebviewWindow<R>) {
    configure_overlay(window, panel::Kind::Command, Present::WithoutActivating);
}

/// Keep the window where the user is, even once it is holding tabs.
///
/// This used to hand the window back to AppKit wholesale — `Default` collection
/// behaviour, level 0, `Regular` activation policy, `activateIgnoringOtherApps:`
/// — on the reasoning that a window you work in should not float over
/// everything and should have a Dock icon.
///
/// It should not, and it should. But the price was that opening Settings from
/// inside a full-screen app threw you out to the desktop, because a window that
/// belongs to one Space and an app that has just been activated leave macOS
/// nowhere else to put you. Being yanked out of what you were doing is a much
/// worse thing than a Settings window that stays in front, so the trade is off:
/// the window keeps following you, and the only thing that changes when tabs
/// appear is that clicking away no longer dismisses it.
pub fn keep_in_place<R: Runtime>(window: &WebviewWindow<R>) {
    configure_overlay(window, panel::Kind::Command, Present::No);
}
