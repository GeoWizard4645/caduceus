//! Highlight & Act — the "PopBar": highlight text in any app, press a
//! hotkey, and a small floating bar of one-click AI actions appears next to
//! the cursor. Picking an action puts the transformed text on the clipboard,
//! ready to paste back wherever it came from. No prompt is ever typed.
//!
//! # This is a thin shell over two things that already exist
//!
//! Everything that actually understands text lives in
//! [`crate::tools::textai`] — eleven actions, prompt-injection defended,
//! provider-neutral, already tested. And everything that knows how to read
//! "whatever is selected in the frontmost app" lives in
//! [`crate::window::manage::selected_text`]. This module's entire job is
//! putting a small window in front of the user at the right moment and
//! wiring its six buttons to those two things — it does not add a twelfth
//! transformation, and it does not add a second way to read a selection.
//!
//! # Reusing the staff's overlay, not reinventing it
//!
//! The PopBar is built and floated exactly the way [`crate::widgets`] floats
//! a widget: a [`tauri::WebviewWindowBuilder`] window (there is no static
//! entry for it in `tauri.conf.json`, on purpose — see the widgets module
//! docs for why a dynamically created window is the only way to get one that
//! is not part of the state webviews array), configured with
//! [`crate::window::configure_staff_floating`] so it sits above a full-screen
//! app, follows the user across Spaces, and — the important part —
//! **can never become the key window**. Clicking a PopBar button must not
//! activate Caduceus or defocus whatever app the text was highlighted in;
//! a panel that cannot become key is what guarantees that.
//!
//! That guarantee has a price, and this module pays it in two places rather
//! than pretending it does not exist:
//!
//! 1. **No text input.** A submenu of preset choices stands in for typed
//!    input everywhere one might otherwise reach for a text field — target
//!    language for Translate, style for Rewrite. See `src/popbar/PopbarApp.tsx`.
//! 2. **No keyboard focus, so no `keydown` for Escape.** A window that can
//!    never be key never receives character events, full stop — so "dismiss
//!    on Escape" cannot be a JS listener the way it would be in an ordinary
//!    window. It is instead a *second*, temporary global hotkey
//!    ([`DISMISS_KEY`]) registered only while the PopBar is on screen and
//!    unregistered the moment it closes — the same tool this module's own
//!    trigger hotkey uses, aimed at Escape for as long as it is relevant and
//!    not a moment longer.
//!
//! Click-away dismissal has the same root problem — a non-key window gets no
//! blur event to hang a "clicked elsewhere" handler off. [`start_click_away_watch`]
//! solves it the same way [`crate::window::CursorTracker`] solves "is the
//! pointer over the staff": by polling, here for a fresh left-click
//! ([`objc2_app_kit::NSEvent::pressedMouseButtons`]) outside the PopBar's own
//! bounds, rather than waiting for an AppKit notification that will never
//! arrive.
//!
//! # Wired into `lib.rs`
//!
//! [`register_hotkey`] is called once from `setup()`, and [`handle_shortcut`]
//! is tried first in the global-shortcut plugin's one `.with_handler(...)`
//! closure, ahead of `hotkeys::handle` — see the doc comment on
//! [`handle_shortcut`] for the exact shape of that wiring. The
//! `#[tauri::command]`s below are in `generate_handler!` alongside everything
//! else.

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Emitter, LogicalPosition, Manager, Runtime, TitleBarStyle, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::settings::SettingsManager;
use crate::tools::textai::{self, TextAiAction};

type Res<T> = Result<T, String>;

// ---------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------

pub const POPBAR_WINDOW: &str = "popbar";

/// The Vite entry point for the PopBar's webview. See `vite.config.ts` and
/// `popbar.html` at the repo root — same one-HTML-file-per-surface layout as
/// `widget.html` and `recorder.html`.
const POPBAR_ENTRY: &str = "popbar.html";

/// Fixed frame for every state the bar can be in: the top-level menu, either
/// submenu, the progress view, the done confirmation, and an inline error.
///
/// Deliberately not resized per state the way a widget resizes to its
/// content. A window that jumps size every time the user picks "Translate"
/// reads as a glitch next to the cursor, and the tallest state (the four-item
/// submenu) comfortably fits everything shorter above it once the content is
/// vertically centred — see `PopbarApp.tsx`. The frontend's own
/// `overflow-y-auto` is the fallback for anything that still does not fit
/// (an unusually long backend error message, say), so nothing is ever
/// silently clipped with no way to read it.
const BAR_WIDTH: f64 = 248.0;
const BAR_HEIGHT: f64 = 220.0;

/// How far below-right of the cursor the bar opens. Straight under the
/// pointer would sit the bar under the hand that is about to click it.
const CURSOR_OFFSET_X: f64 = 12.0;
const CURSOR_OFFSET_Y: f64 = 16.0;

fn popbar_window<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    app.get_webview_window(POPBAR_WINDOW)
}

/// Build the PopBar's window the first time it is needed, or hand back the
/// existing one. Idempotent, the same way `widgets.rs::spawn_widget_window`
/// is — the window is built once and reused for every future press of the
/// hotkey, shown and hidden rather than destroyed and rebuilt, which is both
/// cheaper and avoids re-doing the panel dance in [`crate::window::configure_staff_floating`]
/// on every single invocation.
fn ensure_window<R: Runtime>(app: &AppHandle<R>) -> Res<WebviewWindow<R>> {
    if let Some(existing) = popbar_window(app) {
        return Ok(existing);
    }

    WebviewWindowBuilder::new(app, POPBAR_WINDOW, WebviewUrl::App(POPBAR_ENTRY.into()))
        .title("Caduceus")
        .inner_size(BAR_WIDTH, BAR_HEIGHT)
        .min_inner_size(BAR_WIDTH, BAR_HEIGHT)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .visible(false)
        .accept_first_mouse(true)
        .hidden_title(true)
        .title_bar_style(TitleBarStyle::Transparent)
        .build()
        .map_err(|e| e.to_string())
}

/// Move the (already-sized) PopBar window to just below-right of the pointer,
/// clamped to the monitor the pointer is actually on so it never opens
/// half off-screen on a display in the corner of a multi-monitor layout.
///
/// Same "which monitor is the cursor on" test `window::open_command_center`
/// and `window::recorder::position` both use, in physical pixels — cheaper
/// than converting every monitor to logical space just to compare against a
/// physical cursor point.
fn position_near_cursor<R: Runtime>(window: &WebviewWindow<R>) {
    let (Ok(cursor), Ok(monitors)) = (window.cursor_position(), window.available_monitors())
    else {
        return;
    };

    let target = monitors.into_iter().find(|m| {
        let p = m.position();
        let s = m.size();
        cursor.x >= p.x as f64
            && cursor.x < (p.x + s.width as i32) as f64
            && cursor.y >= p.y as f64
            && cursor.y < (p.y + s.height as i32) as f64
    });
    let Some(monitor) = target else { return };

    let scale = monitor.scale_factor();
    let cursor_logical = cursor.to_logical::<f64>(scale);
    let monitor_pos = monitor.position().to_logical::<f64>(scale);
    let monitor_size = monitor.size().to_logical::<f64>(scale);

    let (x, y) = clamp_to_monitor(
        cursor_logical.x + CURSOR_OFFSET_X,
        cursor_logical.y + CURSOR_OFFSET_Y,
        BAR_WIDTH,
        BAR_HEIGHT,
        monitor_pos.x,
        monitor_pos.y,
        monitor_size.width,
        monitor_size.height,
    );
    let _ = window.set_position(LogicalPosition::new(x, y));
}

/// Pure placement math, pulled out of [`position_near_cursor`] so it can be
/// tested without a running window server. Keeps the bar's whole frame on
/// the monitor it opened on, preferring to slide it back on-screen over
/// letting any edge hang off — the same trade [`crate::window::position_staff`]
/// makes for the staff.
fn clamp_to_monitor(
    x: f64,
    y: f64,
    bar_w: f64,
    bar_h: f64,
    mon_x: f64,
    mon_y: f64,
    mon_w: f64,
    mon_h: f64,
) -> (f64, f64) {
    // `.max(mon_x)` guards a monitor narrower than the bar (a tiny virtual
    // display in CI, say): better to pin the bar to the monitor's origin
    // than to hand `clamp` a max below its min and panic.
    let max_x = (mon_x + mon_w - bar_w).max(mon_x);
    let max_y = (mon_y + mon_h - bar_h).max(mon_y);
    (x.clamp(mon_x, max_x), y.clamp(mon_y, max_y))
}

// ---------------------------------------------------------------------------
// Runtime state
// ---------------------------------------------------------------------------

/// Everything about the PopBar that has no business being persisted:
/// whatever selection the most recent hotkey press captured, and whether the
/// click-away watcher is currently running.
///
/// Managed lazily via [`ensure_managed`] rather than at `setup()` time, the
/// same trick `widgets.rs::WidgetRuntime` uses — this module adds nothing to
/// `lib.rs` beyond the one `pub mod` line, so it cannot rely on anything
/// having called `app.manage(PopbarRuntime::default())` for it.
#[derive(Default)]
pub struct PopbarRuntime {
    latest: RwLock<Option<PopbarShowPayload>>,
    watching: AtomicBool,
}

fn ensure_managed<R: Runtime>(app: &AppHandle<R>) {
    if app.try_state::<PopbarRuntime>().is_none() {
        app.manage(PopbarRuntime::default());
    }
}

/// Emitted to the PopBar window every time the hotkey opens it.
pub const POPBAR_SHOW_EVENT: &str = "caduceus://popbar-show";

/// What the frontend needs to know about a single "hotkey was pressed"
/// moment: the selection at that instant, and whether reading a selection at
/// all is even possible right now.
///
/// `request_id` exists so the frontend can tell two different opens apart
/// even when both happen to carry identical text — pressing the hotkey twice
/// on the same selection must still reset the bar back to its top-level menu
/// the second time, not silently no-op because "nothing changed".
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PopbarShowPayload {
    pub request_id: String,
    /// `None` when nothing is selected, or Caduceus does not have the
    /// Accessibility permission `selected_text` needs — the frontend tells
    /// those two apart using `permission_granted` rather than guessing from
    /// the absence of text alone.
    pub text: Option<String>,
    pub permission_granted: bool,
}

// ---------------------------------------------------------------------------
// Show / hide
// ---------------------------------------------------------------------------

/// Open the PopBar: capture the current selection, position a window next to
/// the cursor, and tell the frontend what it is working with.
///
/// # Why the selection is read *before* anything about the window
///
/// `window::manage::selected_text` reads the Accessibility focus of whatever
/// app is currently frontmost. Building or showing the PopBar's own window
/// does not change that — Caduceus never activates, by design — but reading
/// the selection first rather than after is still the right order to leave
/// it in: it means the very first thing this function does is the one thing
/// that has to reflect the exact instant the hotkey was pressed, before any
/// window-server round trip has a chance to add latency in front of it.
pub fn show<R: Runtime>(app: &AppHandle<R>) {
    let text = crate::window::manage::selected_text();
    let permission_granted = crate::window::manage::permission_granted();
    let payload = PopbarShowPayload {
        request_id: uuid::Uuid::new_v4().to_string(),
        text,
        permission_granted,
    };

    ensure_managed(app);
    if let Some(rt) = app.try_state::<PopbarRuntime>() {
        *rt.latest.write() = Some(payload.clone());
    }

    let window = match ensure_window(app) {
        Ok(w) => w,
        Err(e) => {
            log::error!("could not create the PopBar window: {e}");
            return;
        }
    };

    position_near_cursor(&window);
    // Same overlay treatment as every other Caduceus HUD — see the module
    // docs for why `configure_staff_floating` specifically, not the
    // Command Center's key-capable variant.
    crate::window::configure_staff_floating(&window);
    crate::window::apply_vibrancy(&window);

    if let Err(e) = window.show() {
        log::error!("could not show the PopBar: {e}");
        return;
    }
    // Window-scoped, like `COMMAND_CENTER_OPEN_EVENT` — nothing else needs to
    // hear that the PopBar opened.
    let _ = window.emit(POPBAR_SHOW_EVENT, &payload);

    register_dismiss_key(app);
    start_click_away_watch(app);
}

/// Close the PopBar and release the two things [`show`] only needs while it
/// is open: the temporary Escape binding, and (implicitly, via the watcher
/// noticing the window is hidden on its next tick) the click-away poll.
pub fn hide<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = popbar_window(app) {
        let _ = window.hide();
    }
    unregister_dismiss_key(app);
}

// ---------------------------------------------------------------------------
// Hotkeys
// ---------------------------------------------------------------------------

/// Default trigger for the PopBar.
///
/// Checked against every binding already claimed in `hotkeys.rs` — `F12`
/// (toggle the staff), `Alt+Space` and its fallback chain (Command Center),
/// `Alt+Shift+V` and its fallback chain (push-to-talk) — and against
/// `hotkeys::SYSTEM_RESERVED`, which macOS would otherwise silently keep for
/// itself. `Control+Shift+H` ("Highlight") appears in none of those lists
/// and is not a stock macOS shortcut, so it registers cleanly and actually
/// fires.
pub const POPBAR_HOTKEY_DEFAULT: &str = "Control+Shift+H";

/// The temporary dismiss binding — see the module docs for why Escape has to
/// be a hotkey here rather than a `keydown` listener. A bare key with no
/// modifier is a valid accelerator (`hotkeys.rs` documents the same for
/// `F13`–`F20`); parsed the same way `"Escape"`/`"Esc"` resolve for
/// `hotkeys::is_system_reserved`.
const DISMISS_KEY: &str = "Escape";

/// Register the PopBar's trigger hotkey. Meant to be called once from
/// `lib.rs::setup`, after the global-shortcut plugin is installed — see
/// [`handle_shortcut`] for the rest of the wiring this needs.
///
/// Registered directly against the plugin rather than through
/// `hotkeys::register_all`: that function's fallback chains and "already
/// claimed by another Caduceus action" bookkeeping all key off of
/// `Settings`, and the PopBar hotkey is not (yet) user-configurable. Should
/// it become so, this is the function to fold into that system instead of
/// calling directly.
pub fn register_hotkey<R: Runtime>(app: &AppHandle<R>) {
    match Shortcut::from_str(POPBAR_HOTKEY_DEFAULT) {
        Ok(shortcut) => {
            if let Err(e) = app.global_shortcut().register(shortcut) {
                log::warn!(
                    "could not register the PopBar hotkey ({POPBAR_HOTKEY_DEFAULT}): {e}"
                );
            }
        }
        Err(e) => log::warn!("PopBar hotkey default does not parse: {e}"),
    }
}

/// Claim Escape for exactly as long as the PopBar is open.
///
/// Registering it globally while the bar is up means Caduceus intercepts
/// Escape everywhere, not just inside the PopBar's own (non-key, so
/// otherwise keyboard-deaf) window — an acceptable trade because the moment
/// this is active is, by construction, the moment the user's attention is on
/// the bar. [`unregister_dismiss_key`] gives the key straight back the
/// instant the bar closes, by whatever means it closed.
fn register_dismiss_key<R: Runtime>(app: &AppHandle<R>) {
    if let Ok(shortcut) = Shortcut::from_str(DISMISS_KEY) {
        // Registering an already-registered shortcut is a harmless no-op
        // from the plugin's side (it can happen if a previous PopBar session
        // never reached `hide`, e.g. the process was killed mid-open) — the
        // error is not worth surfacing.
        let _ = app.global_shortcut().register(shortcut);
    }
}

fn unregister_dismiss_key<R: Runtime>(app: &AppHandle<R>) {
    if let Ok(shortcut) = Shortcut::from_str(DISMISS_KEY) {
        let _ = app.global_shortcut().unregister(shortcut);
    }
}

/// Entry point for the global-shortcut plugin's single handler.
///
/// # How this is wired into `lib.rs`
///
/// The `tauri_plugin_global_shortcut` builder takes exactly one handler for
/// *every* shortcut anyone registers, installed once when the plugin is built
/// (`lib.rs::run`, the `.with_handler(...)` call). That closure tries this
/// function first and only falls through to `hotkeys::handle` when it returns
/// `false`, so Escape and `Control+Shift+H` never reach the general hotkey
/// dispatcher:
///
/// ```ignore
/// .with_handler(|app, shortcut, event| {
///     if popbar::handle_shortcut(app, shortcut, event.state()) {
///         return;
///     }
///     hotkeys::handle(app, shortcut, event.state());
/// })
/// ```
///
/// [`register_hotkey`] is the other half of the wiring, called once from
/// `setup()` so `Control+Shift+H` is actually registered before anything
/// could fire it.
pub fn handle_shortcut<R: Runtime>(app: &AppHandle<R>, shortcut: &Shortcut, state: ShortcutState) -> bool {
    if state != ShortcutState::Pressed {
        return false;
    }
    if Shortcut::from_str(POPBAR_HOTKEY_DEFAULT).is_ok_and(|s| &s == shortcut) {
        show(app);
        return true;
    }
    if Shortcut::from_str(DISMISS_KEY).is_ok_and(|s| &s == shortcut) {
        hide(app);
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Click-away dismissal
// ---------------------------------------------------------------------------

/// Poll interval for the click-away watcher. The PopBar is small, the watch
/// only ever runs while it is open, and a click landing within ~60ms of the
/// bar closing is imperceptible — there is no reason to spend more CPU
/// chasing a tighter bound the way the staff's hover tracker does for
/// something the user is actively looking at moving.
const CLICK_POLL_MS: u64 = 60;

/// Watch for a fresh mouse-down outside the PopBar's bounds and dismiss it
/// when one lands, for as long as the bar stays open. Stops on its own once
/// the window is hidden (by any path — Escape, an action completing, this
/// same watcher) rather than needing an explicit "stop" call to pair with
/// [`show`], which keeps [`hide`] simple: hide the window and give Escape
/// back, and whichever click-away loop happens to be running notices on its
/// next tick and winds itself down.
///
/// macOS-only: it reads global mouse-button state via AppKit, which has no
/// portable equivalent, and every other overlay trick in this codebase
/// (`window::macos`, `window::panel`) is already gated the same way.
#[cfg(target_os = "macos")]
fn start_click_away_watch<R: Runtime>(app: &AppHandle<R>) {
    ensure_managed(app);
    let rt = app.state::<PopbarRuntime>();
    if rt.watching.swap(true, Ordering::SeqCst) {
        // Already watching from an earlier `show` that has not closed yet.
        return;
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        use objc2_app_kit::NSEvent;

        // Seeded from whatever button state already exists so the very first
        // tick cannot mistake "the hotkey's own key-down is still technically
        // in flight" for a fresh click, which would close the bar the
        // instant it opened.
        let mut was_pressed = NSEvent::pressedMouseButtons() != 0;

        loop {
            tokio::time::sleep(Duration::from_millis(CLICK_POLL_MS)).await;

            let Some(window) = app.get_webview_window(POPBAR_WINDOW) else {
                break;
            };
            if !window.is_visible().unwrap_or(false) {
                break;
            }

            let pressed = NSEvent::pressedMouseButtons() != 0;
            if pressed && !was_pressed {
                let outside = match (window.cursor_position(), window.outer_position(), window.outer_size())
                {
                    (Ok(cursor), Ok(origin), Ok(size)) => {
                        cursor.x < origin.x as f64
                            || cursor.y < origin.y as f64
                            || cursor.x > origin.x as f64 + size.width as f64
                            || cursor.y > origin.y as f64 + size.height as f64
                    }
                    // Cannot tell where the bar is right now — safer to leave
                    // it open than to close it on a guess.
                    _ => false,
                };
                if outside {
                    hide(&app);
                    break;
                }
            }
            was_pressed = pressed;
        }

        if let Some(rt) = app.try_state::<PopbarRuntime>() {
            rt.watching.store(false, Ordering::SeqCst);
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn start_click_away_watch<R: Runtime>(_app: &AppHandle<R>) {}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------
//
// All three — `popbar_pending`, `popbar_run`, `popbar_dismiss` — are in
// `generate_handler!` in `lib.rs`, alongside everything else. See the module
// docs for the hotkey side of the same wiring.

/// Whatever the most recent hotkey press captured, for the frontend to read
/// on mount.
///
/// This exists *alongside* [`POPBAR_SHOW_EVENT`], not instead of it, to close
/// a real race rather than a theoretical one: the very first time the PopBar
/// opens in a session, [`show`] both builds the webview and emits the event
/// in the same call, and there is no guarantee the page has finished loading
/// React and attached its listener before that emit goes out. Every open
/// after the first is not at risk — the window and its listener already
/// exist — but the frontend cannot tell which kind of open it just had, so
/// it always does both: read whatever is pending here on mount, and listen
/// for the event for every subsequent press.
#[tauri::command]
pub fn popbar_pending<R: Runtime>(app: AppHandle<R>) -> Option<PopbarShowPayload> {
    ensure_managed(&app);
    app.state::<PopbarRuntime>().latest.read().clone()
}

/// Run one Highlight & Act transformation and put the result on the
/// clipboard — the entire point of the feature, expressed as a single round
/// trip so the frontend has nothing to orchestrate beyond calling this and
/// showing what came back.
///
/// Deliberately duplicates the two lines `commands::text_ai_run` would
/// otherwise share with this: `commands.rs` is out of this module's file
/// list (see the crate owner's split), and `tools::textai::run` is already
/// `pub`, so routing through it directly here costs nothing and asks for no
/// changes to a file this module does not own.
#[tauri::command]
pub async fn popbar_run(
    settings: tauri::State<'_, SettingsManager>,
    action: TextAiAction,
    text: String,
    target_language: Option<String>,
) -> Res<String> {
    let result = textai::run(&settings, action, &text, target_language.as_deref())
        .await
        .map_err(|e| e.to_string())?;
    write_clipboard(&result)?;
    Ok(result)
}

/// Write to the system clipboard on a scratch `arboard::Clipboard`.
///
/// Not `clipboard::copy_entry_to_clipboard` — that copies a *history entry*
/// by id back out of the SQLite store, which is a different job than "put
/// this fresh string on the clipboard", and its signature reflects that
/// (`id: i64`, not `text: &str`). This is the same one line
/// `copy_entry_to_clipboard`'s text branch boils down to, done directly
/// rather than manufacturing a history entry just to immediately copy it
/// back out.
fn write_clipboard(text: &str) -> Res<()> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
    clipboard
        .set_text(text.to_string())
        .map_err(|e| format!("could not write to the clipboard: {e}"))
}

/// Close the PopBar on request — Escape's non-hotkey sibling, for a click on
/// an explicit "close"/"back to menu" affordance or for the frontend's own
/// auto-dismiss after an action finishes.
#[tauri::command]
pub fn popbar_dismiss<R: Runtime>(app: AppHandle<R>) -> Res<()> {
    hide(&app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- hotkeys --------------------------------------------------------

    #[test]
    fn the_default_hotkey_and_the_dismiss_key_both_parse() {
        assert!(Shortcut::from_str(POPBAR_HOTKEY_DEFAULT).is_ok());
        assert!(Shortcut::from_str(DISMISS_KEY).is_ok());
    }

    #[test]
    fn the_default_hotkey_does_not_collide_with_any_binding_hotkeys_rs_already_uses() {
        // Mirrors the combinations named in `hotkeys.rs`'s own doc table and
        // fallback lists — a compile-time reminder to update this test (and
        // pick a new default) if this module's hotkey is ever changed to
        // something that clashes.
        let taken = [
            "F12",
            "Alt+Space",
            "Control+Space",
            "Control+Shift+Space",
            "Alt+Shift+Space",
            "CommandOrControl+Alt+Space",
            "F17",
            "Alt+Shift+V",
            "Control+Shift+V",
            "CommandOrControl+Alt+V",
            "F18",
            "CommandOrControl+Alt+S",
            "Control+Shift+S",
            "Alt+Shift+S",
        ];
        for other in taken {
            assert_ne!(
                POPBAR_HOTKEY_DEFAULT, other,
                "PopBar hotkey collides with an existing Caduceus binding"
            );
        }
    }

    // -- placement --------------------------------------------------------

    #[test]
    fn a_bar_that_fits_stays_exactly_where_the_cursor_puts_it() {
        let (x, y) = clamp_to_monitor(100.0, 100.0, 248.0, 220.0, 0.0, 0.0, 1512.0, 982.0);
        assert_eq!((x, y), (100.0, 100.0));
    }

    #[test]
    fn a_bar_that_would_hang_off_the_right_edge_is_pulled_back_on_screen() {
        let (x, _) = clamp_to_monitor(1500.0, 100.0, 248.0, 220.0, 0.0, 0.0, 1512.0, 982.0);
        assert!(x + 248.0 <= 1512.0);
    }

    #[test]
    fn a_bar_that_would_hang_off_the_bottom_edge_is_pulled_back_on_screen() {
        let (_, y) = clamp_to_monitor(100.0, 900.0, 248.0, 220.0, 0.0, 0.0, 1512.0, 982.0);
        assert!(y + 220.0 <= 982.0);
    }

    #[test]
    fn placement_respects_a_monitor_that_does_not_start_at_the_origin() {
        // A secondary display to the left of the primary one, in Caduceus's
        // own coordinate convention (negative x).
        let (x, y) = clamp_to_monitor(-1800.0, 50.0, 248.0, 220.0, -1920.0, 0.0, 1920.0, 1080.0);
        assert!(x >= -1920.0 && x + 248.0 <= -1920.0 + 1920.0);
        assert!(y >= 0.0 && y + 220.0 <= 1080.0);
    }

    #[test]
    fn a_monitor_narrower_than_the_bar_does_not_panic() {
        // Degenerate, but a `clamp(min, max)` with `max < min` panics, and a
        // tiny virtual display in CI is exactly the kind of input that would
        // hit it if `clamp_to_monitor` did not guard for it.
        let (x, y) = clamp_to_monitor(10.0, 10.0, 248.0, 220.0, 0.0, 0.0, 100.0, 100.0);
        assert_eq!((x, y), (0.0, 0.0));
    }

    // -- payloads --------------------------------------------------------

    #[test]
    fn show_payloads_round_trip_through_json_with_camel_case_keys() {
        let payload = PopbarShowPayload {
            request_id: "abc".into(),
            text: Some("hello".into()),
            permission_granted: true,
        };
        let value = serde_json::to_value(&payload).unwrap();
        assert_eq!(value["requestId"], "abc");
        assert_eq!(value["permissionGranted"], true);
        let round_tripped: PopbarShowPayload = serde_json::from_value(value).unwrap();
        assert_eq!(round_tripped.text.as_deref(), Some("hello"));
    }

    #[test]
    fn a_payload_with_no_selection_carries_none_rather_than_an_empty_string() {
        // `window::manage::selected_text` already collapses "nothing
        // selected" to `None` rather than `Some("")` — this just confirms
        // that distinction survives the trip through JSON, since the
        // frontend's "nothing selected" view keys off `text === null`.
        let payload = PopbarShowPayload {
            request_id: "abc".into(),
            text: None,
            permission_granted: true,
        };
        let value = serde_json::to_value(&payload).unwrap();
        assert!(value["text"].is_null());
    }
}
