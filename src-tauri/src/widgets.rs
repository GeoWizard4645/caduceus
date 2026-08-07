//! Floating widgets: small, always-on-top pixel panels the user places and
//! resizes, one on top of every app and every Space — the same trick the
//! staff uses, generalised to any number of independent windows.
//!
//! Content is out of scope here. This module is the window system underneath
//! it: create/destroy/list/move/resize, persistence across restarts, and one
//! trivial `"clock"` widget (see `src/widgets/`) that proves the whole path
//! works end to end.
//!
//! # One OS window per widget
//!
//! The staff and the Command Center are each a single, statically declared
//! window (`tauri.conf.json`'s `windows` array). A widget is neither: there
//! can be any number of them, created and destroyed at runtime, each with its
//! own position and size. Always-on-top level, Space membership and
//! drag/resize are handed out per *window* by both Tauri and macOS, so the
//! only way to get N independent floating panels is N real
//! [`tauri::WebviewWindow`]s, built with [`tauri::WebviewWindowBuilder`]
//! instead of the static `windows` array the other surfaces use. Each one is
//! labelled `widget-<uuid>`.
//!
//! # Reusing the staff's overlay trick, not reinventing it
//!
//! [`crate::window::configure_staff_floating`] already does everything a
//! widget needs to float above a full-screen app and follow the user across
//! Spaces: `canJoinAllSpaces`, the pop-up-menu window level, and — the part
//! that actually matters for "does not steal focus" — an `NSPanel` that can
//! never become key, so clicking a widget never activates Caduceus and never
//! drags the user out of whatever full-screen Space they were in. That
//! function is already generic over `WebviewWindow<R>` rather than hardcoded
//! to the staff's label, so calling it here *is* the fix; nothing in this
//! file talks to AppKit directly.
//!
//! # Why there is no click-through polling for the demo widget
//!
//! The staff is a fixed 340×340 window around a small mark, so most of its
//! area is empty and has to be made click-through or it would swallow a
//! 340px square of the desktop — that is what its `CaptureRect` and the
//! cursor tracker in `window/mod.rs` are for. A widget's OS window is instead
//! sized to its own content, so by default every pixel of it *is* the widget
//! and is supposed to capture clicks — there is no invisible margin to leak
//! clicks through in the first place. The equivalent mechanism still exists
//! for content that draws itself smaller than its own window (rounded pixel
//! corners, a shape that isn't a filled rectangle): [`widgets_set_capture_rect`]
//! registers a sub-rectangle, and [`ensure_tracker`] polls the cursor against
//! it exactly the way the staff's tracker does, one widget at a time. It
//! costs nothing for a widget that never registers one, which is every
//! widget shipped so far — the poller does not even start until something
//! calls [`widgets_set_capture_rect`], and stops itself the moment nothing is
//! left to track.
//!
//! # Persistence
//!
//! Layouts live in their own store file rather than [`crate::settings`] — this
//! module never touches the shared `Settings` schema, so a widget can be
//! added or removed without a version bump there. Every move or resize
//! (native, via the frontend's drag handle and resize grip) schedules a save
//! the same way the staff's drag does in `lib.rs`: on the window's own
//! `Moved`/`Resized` events, debounced so a drag does not write the file on
//! every frame.
//!
//! # Wired into `lib.rs`
//!
//! The `#[tauri::command]`s below are in `generate_handler!` like everything
//! else. [`restore_saved_widgets`] is not a command — it is a plain function
//! `lib.rs::setup` calls once at launch, the same moment it positions and
//! shows the staff, so widgets open in a previous session come back where
//! they were left. The one thing that stays lazy on purpose is
//! [`WidgetRuntime`]: nothing calls `app.manage` for it up front, so
//! [`ensure_managed`] runs the first time something here actually needs the
//! capture-rect tracker's state — for most widgets, shipped so far, never.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, Runtime, TitleBarStyle, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_store::StoreExt;

type Res<T> = Result<T, String>;

/// The Vite entry point for a widget's webview. See `vite.config.ts` and
/// `widget.html` at the repo root — one HTML file shared by every widget
/// instance, parameterised per-window by [`spawn_widget_window`]'s init
/// script rather than by URL, so there is nothing label- or id-shaped for the
/// asset protocol to resolve.
const WIDGET_ENTRY: &str = "widget.html";

/// Filename inside the app config directory. Deliberately not
/// [`crate::settings::STORE_FILE`] — see the module docs.
const STORE_FILE: &str = "caduceus-widgets.json";
const WIDGETS_KEY: &str = "widgets";

const LABEL_PREFIX: &str = "widget-";

const DEFAULT_WIDTH: f64 = 168.0;
const DEFAULT_HEIGHT: f64 = 96.0;
/// Below this a widget stops being legible — the same floor spirit as the
/// staff's `STAFF_SIZE_MIN` and the window manager's `MIN_SIDE`.
const MIN_WIDTH: f64 = 96.0;
const MIN_HEIGHT: f64 = 64.0;
/// Offsets each new widget a little further down-right of the last one, so
/// creating several in a row does not stack them into one indistinguishable
/// pile the way `(0, 0)` for everyone would.
const CASCADE_STEP: f64 = 28.0;
/// How long to wait after a move/resize before writing it out. A drag or a
/// resize fires the underlying window event on every frame; without this a
/// two-second drag would be a few dozen file writes for one gesture the user
/// only cares about the end state of.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(400);
/// Poll rate for the capture-rect tracker. Widgets are small and few, and
/// nothing here is animated, so this can be far lazier than the staff's
/// hover tracker without ever being felt as latency.
const CAPTURE_POLL_MS: u64 = 40;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A widget's identity, content selector, and on-screen geometry — the whole
/// of what gets persisted and handed to the webview.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WidgetLayout {
    pub id: String,
    /// Which content to render — `"clock"` today, whatever future agents add
    /// after that. Rust never branches on this string; it is handed to the
    /// webview verbatim in the init script and the frontend decides what it
    /// means. Keeping Rust ignorant of the set of widget kinds is what lets
    /// new ones be added without touching this file.
    pub kind: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A rectangle in a widget window's own coordinate space, in logical pixels —
/// the same shape and the same job as `window::CaptureRect` for the staff,
/// duplicated rather than shared because that one is private to a single
/// window's tracker and this one has to serve any number of them.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl CaptureRect {
    /// Is a physical-pixel screen point inside this rect?
    fn contains(&self, cursor_x: f64, cursor_y: f64, origin_x: f64, origin_y: f64, scale: f64) -> bool {
        let left = origin_x + self.x * scale;
        let top = origin_y + self.y * scale;
        cursor_x >= left
            && cursor_x <= left + self.width * scale
            && cursor_y >= top
            && cursor_y <= top + self.height * scale
    }
}

/// Per-widget runtime state that has no business being persisted: the
/// capture rect a widget has registered, if any.
#[derive(Default)]
struct WidgetState {
    capture_rect: RwLock<Option<CaptureRect>>,
}

/// App-managed state for the capture-rect tracker. Not managed at startup —
/// see the module docs — so every entry point that touches it goes through
/// [`ensure_managed`] first.
#[derive(Default)]
pub struct WidgetRuntime {
    entries: RwLock<HashMap<String, Arc<WidgetState>>>,
    tracker_running: AtomicBool,
}

fn ensure_managed<R: Runtime>(app: &AppHandle<R>) {
    if app.try_state::<WidgetRuntime>().is_none() {
        app.manage(WidgetRuntime::default());
    }
}

fn widget_label(id: &str) -> String {
    format!("{LABEL_PREFIX}{id}")
}

fn widget_window<R: Runtime>(app: &AppHandle<R>, id: &str) -> Option<WebviewWindow<R>> {
    app.get_webview_window(&widget_label(id))
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

fn load_layouts<R: Runtime>(app: &AppHandle<R>) -> Vec<WidgetLayout> {
    let Ok(store) = app.store(STORE_FILE) else {
        return Vec::new();
    };
    store
        .get(WIDGETS_KEY)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

fn save_layouts<R: Runtime>(app: &AppHandle<R>, layouts: &[WidgetLayout]) -> Res<()> {
    let store = app
        .store(STORE_FILE)
        .map_err(|e| format!("could not open the widget store: {e}"))?;
    let value = serde_json::to_value(layouts).map_err(|e| format!("could not encode widget layout: {e}"))?;
    store.set(WIDGETS_KEY, value);
    store.save().map_err(|e| format!("could not write widget layout: {e}"))
}

fn upsert_layout<R: Runtime>(app: &AppHandle<R>, layout: &WidgetLayout) -> Res<()> {
    let mut layouts = load_layouts(app);
    match layouts.iter_mut().find(|l| l.id == layout.id) {
        Some(existing) => *existing = layout.clone(),
        None => layouts.push(layout.clone()),
    }
    save_layouts(app, &layouts)
}

fn remove_layout<R: Runtime>(app: &AppHandle<R>, id: &str) -> Res<()> {
    let mut layouts = load_layouts(app);
    layouts.retain(|l| l.id != id);
    save_layouts(app, &layouts)
}

/// Read a widget's current on-screen geometry back off its window and write
/// it to disk. The source of truth for "where is it" is always the window
/// itself, not whatever Rust last told it to be — a native drag or resize
/// moves the window directly, without going through any command here.
fn persist_current_geometry<R: Runtime>(app: &AppHandle<R>, id: &str) -> Res<()> {
    let Some(window) = widget_window(app, id) else {
        // The widget was closed between the debounce firing and now; nothing
        // to persist.
        return Ok(());
    };
    let scale = window.scale_factor().unwrap_or(1.0);
    let pos = window.outer_position().map_err(|e| e.to_string())?.to_logical::<f64>(scale);
    let size = window.outer_size().map_err(|e| e.to_string())?.to_logical::<f64>(scale);

    let mut layouts = load_layouts(app);
    let Some(entry) = layouts.iter_mut().find(|l| l.id == id) else {
        return Ok(());
    };
    entry.x = pos.x;
    entry.y = pos.y;
    entry.width = size.width;
    entry.height = size.height;
    save_layouts(app, &layouts)
}

fn schedule_persist<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let app = app.clone();
    let id = id.to_string();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(SAVE_DEBOUNCE).await;
        let _ = persist_current_geometry(&app, &id);
    });
}

/// Where a newly created widget lands when the caller does not ask for a
/// specific spot: centred on the primary monitor, staggered a little per
/// existing widget so a run of "add a widget" clicks does not pile them on
/// top of one another.
fn default_origin<R: Runtime>(app: &AppHandle<R>, cascade_index: usize) -> (f64, f64) {
    let cascade = (cascade_index % 8) as f64 * CASCADE_STEP;
    if let Ok(Some(monitor)) = app.primary_monitor() {
        let scale = monitor.scale_factor();
        let pos = monitor.position().to_logical::<f64>(scale);
        let size = monitor.size().to_logical::<f64>(scale);
        let cx = pos.x + size.width / 2.0 - DEFAULT_WIDTH / 2.0;
        let cy = pos.y + size.height / 2.0 - DEFAULT_HEIGHT / 2.0;
        return (cx + cascade, cy + cascade);
    }
    // Headless CI or no monitor info: land somewhere on screen rather than
    // erroring out.
    (160.0 + cascade, 160.0 + cascade)
}

// ---------------------------------------------------------------------------
// Window lifecycle
// ---------------------------------------------------------------------------

/// Build, float, and show the OS window for one widget. Idempotent: a widget
/// whose window already exists (a duplicate restore, a re-entrant create) is
/// handed back rather than rebuilt.
fn spawn_widget_window<R: Runtime>(app: &AppHandle<R>, layout: &WidgetLayout) -> Res<WebviewWindow<R>> {
    let label = widget_label(&layout.id);
    if let Some(existing) = app.get_webview_window(&label) {
        return Ok(existing);
    }

    // Handed to the webview as a global before any of its own scripts run,
    // rather than as a URL query string — it keeps the one HTML entry point
    // free of id/kind parsing and matches how the rest of Caduceus prefers a
    // typed payload over a hand-rolled URL format.
    let init = format!(
        "window.__CADUCEUS_WIDGET__ = {};",
        serde_json::to_string(layout).map_err(|e| e.to_string())?
    );

    let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(WIDGET_ENTRY.into()))
        .title("Caduceus Widget")
        .inner_size(layout.width.max(MIN_WIDTH), layout.height.max(MIN_HEIGHT))
        .position(layout.x, layout.y)
        .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
        .resizable(true)
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
        .initialization_script(&init)
        .build()
        .map_err(|e| e.to_string())?;

    // Configure *then* show, not the other way round: a window ordered in
    // while its collection behaviour still says "one Space" is placed in the
    // Space it was created in, and setting `canJoinAllSpaces` afterwards does
    // not reliably drag it across. Same reasoning as `open_command_center` in
    // `window/mod.rs`, same fix.
    crate::window::configure_staff_floating(&window);
    crate::window::apply_vibrancy(&window);
    window.show().map_err(|e| e.to_string())?;

    let handle = app.clone();
    let id = layout.id.clone();
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_)) {
            schedule_persist(&handle, &id);
        }
    });

    Ok(window)
}

/// Poll the cursor against every widget that has registered a capture rect,
/// toggling click-through exactly the way the staff's tracker does — see the
/// module docs for why this is opt-in rather than running for every widget
/// unconditionally. Safe to call repeatedly; only the first call (per app run,
/// or per time the tracker has wound itself down) actually spawns anything.
fn ensure_tracker<R: Runtime>(app: &AppHandle<R>) {
    ensure_managed(app);
    let rt = app.state::<WidgetRuntime>();
    if rt.tracker_running.swap(true, Ordering::SeqCst) {
        return;
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(CAPTURE_POLL_MS)).await;

            let rt = app.state::<WidgetRuntime>();
            let snapshot: Vec<(String, Arc<WidgetState>)> =
                rt.entries.read().iter().map(|(k, v)| (k.clone(), v.clone())).collect();

            // Nothing left with a registered capture rect: the ordinary case,
            // where every widget's window is exactly its own content and
            // needs no polling at all. Stop rather than spin forever.
            if snapshot.iter().all(|(_, state)| state.capture_rect.read().is_none()) {
                rt.tracker_running.store(false, Ordering::SeqCst);
                return;
            }

            for (id, state) in snapshot {
                let Some(rect) = *state.capture_rect.read() else { continue };
                let Some(window) = widget_window(&app, &id) else { continue };
                let (Ok(cursor), Ok(origin), Ok(scale)) =
                    (window.cursor_position(), window.outer_position(), window.scale_factor())
                else {
                    continue;
                };
                let inside = rect.contains(cursor.x, cursor.y, origin.x as f64, origin.y as f64, scale);
                let _ = window.set_ignore_cursor_events(!inside);
            }
        }
    });
}

/// Gap between restoring one saved widget and the next at launch.
///
/// Each widget is a full OS-level webview, and building/showing all of them
/// back-to-back on the same tick is a thundering herd that spikes launch-time
/// CPU and memory in direct proportion to how many widgets the user has —
/// competing for the same cycles the staff itself is trying to come up on.
/// Spacing them out costs nothing perceptible (a widget is not something you
/// interact with the instant it appears) and the whole set still lands within
/// a couple of seconds of launch either way.
const WIDGET_RESTORE_STAGGER: Duration = Duration::from_millis(150);

/// Recreate every widget window from its saved layout, one at a time on a
/// staggered schedule rather than all at once — see [`WIDGET_RESTORE_STAGGER`].
/// Meant to be called once at launch, after the staff and Command Center are
/// set up — the widget equivalent of `window::position_staff` /
/// `window::should_show_staff` in `lib.rs::setup`, just for a list of windows
/// instead of one, and asynchronous where those are not because the stagger
/// needs somewhere to await between them. See the module docs for why this is
/// a plain function rather than a command.
///
/// Every widget still ends up built, visible, and on screen exactly as before
/// — this only smooths out *when* within the first couple of seconds that
/// happens, not whether it does. A widget the user has saved is a widget they
/// expect to see after login; nothing here defers that indefinitely or skips
/// it. A widget that fails to rebuild — its saved geometry now off every
/// attached display, say — is logged and skipped rather than aborting the
/// rest: one bad layout is not a reason to leave every other widget the user
/// actually has un-restored.
pub fn restore_saved_widgets<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        for layout in load_layouts(&app) {
            if let Err(error) = spawn_widget_window(&app, &layout) {
                log::warn!("could not restore widget {}: {error}", layout.id);
            }
            tokio::time::sleep(WIDGET_RESTORE_STAGGER).await;
        }
    });
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------
//
// Not registered in `generate_handler!` from this file — see the crate
// owner's notes at the top of the module. Each one below is otherwise a
// complete, ordinary Tauri command.

/// Create a new widget and open its window. `x`/`y`/`width`/`height` are
/// optional; omitted ones fall back to a cascaded default position and the
/// standard demo size.
#[tauri::command]
pub fn widgets_create<R: Runtime>(
    app: AppHandle<R>,
    kind: String,
    x: Option<f64>,
    y: Option<f64>,
    width: Option<f64>,
    height: Option<f64>,
) -> Res<WidgetLayout> {
    let existing = load_layouts(&app);
    let (default_x, default_y) = default_origin(&app, existing.len());

    let layout = WidgetLayout {
        id: uuid::Uuid::new_v4().to_string(),
        kind,
        x: x.unwrap_or(default_x),
        y: y.unwrap_or(default_y),
        width: width.unwrap_or(DEFAULT_WIDTH).max(MIN_WIDTH),
        height: height.unwrap_or(DEFAULT_HEIGHT).max(MIN_HEIGHT),
    };

    spawn_widget_window(&app, &layout)?;
    upsert_layout(&app, &layout)?;
    Ok(layout)
}

/// Close a widget for good and forget its saved layout.
///
/// Uses `destroy`, not `close`: closing a window emits `WindowEvent::CloseRequested`,
/// which the shared handler in `lib.rs` turns into a `hide()` for every window
/// but the staff — exactly the wrong thing when the user has asked to remove
/// a widget rather than tuck it away.
#[tauri::command]
pub fn widgets_destroy<R: Runtime>(app: AppHandle<R>, id: String) -> Res<()> {
    if let Some(window) = widget_window(&app, &id) {
        window.destroy().map_err(|e| e.to_string())?;
    }
    remove_layout(&app, &id)?;
    if let Some(rt) = app.try_state::<WidgetRuntime>() {
        rt.entries.write().remove(&id);
    }
    Ok(())
}

/// Every saved widget layout, open or not. The frontend that owns "what
/// widgets exist" reads this rather than tracking window lifecycles itself.
#[tauri::command]
pub fn widgets_list<R: Runtime>(app: AppHandle<R>) -> Res<Vec<WidgetLayout>> {
    Ok(load_layouts(&app))
}

/// Move an open widget to an exact position and persist it. Native dragging
/// (the frontend's drag handle) already persists on its own via the window's
/// `Moved` event; this is for programmatic repositioning — a "tidy up"
/// command, a future snap-to-grid — that does not go through a drag gesture.
#[tauri::command]
pub fn widgets_move<R: Runtime>(app: AppHandle<R>, id: String, x: f64, y: f64) -> Res<()> {
    let window = widget_window(&app, &id).ok_or_else(|| "that widget is not open".to_string())?;
    window.set_position(LogicalPosition::new(x, y)).map_err(|e| e.to_string())?;
    persist_current_geometry(&app, &id)
}

/// Resize an open widget and persist it. The programmatic counterpart to
/// [`widgets_move`] — native resizing (the frontend's resize grip) persists
/// on its own.
#[tauri::command]
pub fn widgets_resize<R: Runtime>(app: AppHandle<R>, id: String, width: f64, height: f64) -> Res<()> {
    let window = widget_window(&app, &id).ok_or_else(|| "that widget is not open".to_string())?;
    window
        .set_size(LogicalSize::new(width.max(MIN_WIDTH), height.max(MIN_HEIGHT)))
        .map_err(|e| e.to_string())?;
    persist_current_geometry(&app, &id)
}

/// Force an immediate write of a widget's current geometry, rather than
/// waiting for [`SAVE_DEBOUNCE`]. Belt-and-suspenders for a caller that wants
/// a guarantee the layout is on disk right now — e.g. right before the app
/// quits.
#[tauri::command]
pub fn widgets_save_layout<R: Runtime>(app: AppHandle<R>, id: String) -> Res<()> {
    persist_current_geometry(&app, &id)
}

/// Register (or clear) the sub-rectangle of a widget's window that should
/// capture the pointer — see the module docs for when this is actually
/// needed. `rect: None` returns the widget to the default of "the whole
/// window captures."
#[tauri::command]
pub fn widgets_set_capture_rect<R: Runtime>(app: AppHandle<R>, id: String, rect: Option<CaptureRect>) -> Res<()> {
    ensure_managed(&app);
    {
        let rt = app.state::<WidgetRuntime>();
        let mut entries = rt.entries.write();
        let entry = entries.entry(id).or_insert_with(|| Arc::new(WidgetState::default()));
        *entry.capture_rect.write() = rect;
    }
    ensure_tracker(&app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> CaptureRect {
        CaptureRect { x: 10.0, y: 10.0, width: 40.0, height: 20.0 }
    }

    #[test]
    fn a_point_inside_the_rect_is_captured() {
        // Window origin at (100, 100), scale 1: the rect spans (110,110) to
        // (150,130) in screen space.
        assert!(rect().contains(120.0, 115.0, 100.0, 100.0, 1.0));
    }

    #[test]
    fn a_point_outside_the_rect_is_not_captured() {
        assert!(!rect().contains(200.0, 115.0, 100.0, 100.0, 1.0));
        assert!(!rect().contains(120.0, 5.0, 100.0, 100.0, 1.0));
    }

    #[test]
    fn the_rect_scales_with_the_display() {
        // At 2x, the same logical rect covers twice the physical pixels —
        // a point just past the logical edge is still inside at 2x scale.
        assert!(rect().contains(100.0 + 45.0 * 2.0, 100.0 + 15.0 * 2.0, 100.0, 100.0, 2.0));
        assert!(!rect().contains(100.0 + 55.0 * 2.0, 100.0 + 15.0 * 2.0, 100.0, 100.0, 2.0));
    }

    #[test]
    fn layouts_round_trip_through_json_with_camel_case_keys() {
        let layout = WidgetLayout {
            id: "abc".into(),
            kind: "clock".into(),
            x: 1.0,
            y: 2.0,
            width: 168.0,
            height: 96.0,
        };
        let value = serde_json::to_value(&layout).unwrap();
        assert_eq!(value["kind"], "clock");
        let round_tripped: WidgetLayout = serde_json::from_value(value).unwrap();
        assert_eq!(round_tripped.id, layout.id);
        assert_eq!(round_tripped.width, layout.width);
    }
}
