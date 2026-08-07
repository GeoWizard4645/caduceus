//! Window management: the staff, the Command Center, and Settings.
//!
//! # How the staff stays out of your way
//!
//! The staff lives in a fixed 340×340 transparent, always-on-top window — big
//! enough to hold the fully expanded radial pop-out. A window that size sitting
//! on top of everything would normally swallow every click in a 340px square,
//! which would be intolerable.
//!
//! The fix is [`set_ignore_cursor_events`], driven by a background cursor
//! tracker:
//!
//! ```text
//!   cursor far away        →  window is click-through, staff is a calm dot
//!   cursor over the staff    →  window becomes interactive, pop-out expands
//!   cursor leaves for Ns   →  pop-out collapses, window goes click-through
//! ```
//!
//! Tracking the global cursor is required anyway — "collapse after the pointer
//! has been elsewhere for N seconds" cannot be implemented from DOM events,
//! because the webview stops receiving them the moment the pointer leaves.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Runtime, WebviewWindow};

use crate::settings::{Point, Settings, SettingsManager, StaffEdge};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod panel;

#[cfg(target_os = "macos")]
pub mod accessibility;
pub mod grants;
pub mod manage;
pub mod recorder;
pub mod relaunch;

pub const STAFF_WINDOW: &str = "staff";
pub const COMMAND_CENTER_WINDOW: &str = "command-center";

/// Asks the Command Center to open (or focus) a tab.
///
/// Everything Caduceus shows lives in one window now — Settings, chat, the
/// clipboard, the management pages. The Rust side names a tab; the shell in the
/// webview decides whether that means focusing an existing one or adding a new
/// one, because "is this already open" is a question only it can answer.
pub const TAB_OPEN_EVENT: &str = "caduceus://tab-open";

/// Which tab to open, and what to open it on.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabRequest {
    /// "home" | "clipboard" | "chat" | "settings" | "system" | "awake" |
    /// "sound" | "ports" | "docker" | "machine". Validated in the shell.
    pub kind: String,
    /// For `settings`, the pane to select.
    pub section: Option<String>,
    /// For `chat`, the conversation to show.
    pub conversation_id: Option<i64>,
}

/// Whether the Command Center is currently an overlay rather than a window.
///
/// True while the Command Center is in “palette” mode (a lone Home tab). Used
/// for frontend window sizing; click-away dismissal is independent of this flag.
#[derive(Debug)]
pub struct PaletteFloating {
    floating: AtomicBool,
    /// When the Command Center was last presented.
    ///
    /// A non-activating panel takes key status while Caduceus stays in the
    /// background, and AppKit can hand it back and forth once as the window
    /// server settles. Without a grace period that momentary resign reads as
    /// "the user clicked away" and closes the palette on the frame it opened.
    shown_at: parking_lot::Mutex<std::time::Instant>,
}

/// How long after opening a lost-focus event is ignored.
const FOCUS_SETTLE: std::time::Duration = std::time::Duration::from_millis(400);

impl Default for PaletteFloating {
    fn default() -> Self {
        Self {
            floating: AtomicBool::new(true),
            shown_at: parking_lot::Mutex::new(std::time::Instant::now() - FOCUS_SETTLE * 2),
        }
    }
}

impl PaletteFloating {
    pub fn get(&self) -> bool {
        self.floating.load(Ordering::Relaxed)
    }
    pub fn set(&self, floating: bool) {
        self.floating.store(floating, Ordering::Relaxed);
    }
    pub fn mark_shown(&self) {
        *self.shown_at.lock() = std::time::Instant::now();
    }
    /// Whether the window is still inside its post-open settling window.
    pub fn just_shown(&self) -> bool {
        self.shown_at.lock().elapsed() < FOCUS_SETTLE
    }
}

/// Whether the Command Center is mid permission-flow: System Settings was just
/// sent to the front, or a TCC prompt was just triggered, on the user's behalf.
///
/// Either of those steals focus the moment it appears, which is indistinguishable
/// from "the user clicked away" to the blur handler below — without this, asking
/// for a permission was enough to make the palette vanish out from under the
/// person who asked for it, taking with it whatever they had typed and the tab
/// they were on. Unlike [`PaletteFloating::just_shown`], which only needs to
/// survive a single-frame focus hiccup, this has to survive the user actually
/// leaving the app: finding the right toggle in System Settings, or answering a
/// TCC sheet, can take anywhere from a couple of seconds to a couple of minutes.
/// A bare "ignore the next blur" flag would either fire too early and lose the
/// race, or never get cleared and permanently disable click-away dismissal — so
/// this is a timestamp, re-armed on every permission-related action and treated
/// as expired once it is stale enough that "still mid-flow" is no longer the
/// likely explanation.
#[derive(Debug)]
pub struct PermissionFlowActive {
    marked_at: parking_lot::Mutex<std::time::Instant>,
}

/// How long a permission flow is assumed to still be in progress after the last
/// time it was touched.
///
/// Generous on purpose: System Settings is not fast to navigate, and a user
/// hunting for, say, Accessibility inside Privacy & Security can easily spend a
/// minute or two on it. Too short and the palette is right back to closing
/// itself mid-flow; too long only matters if the user quietly gives up on the
/// permission and later clicks away expecting the palette to dismiss, in which
/// case they simply get one stale grace window rather than a broken feature.
const PERMISSION_FLOW_WINDOW: std::time::Duration = std::time::Duration::from_secs(180);

impl Default for PermissionFlowActive {
    fn default() -> Self {
        Self {
            // Start already expired, mirroring `PaletteFloating`'s default —
            // nothing has asked for a permission yet.
            marked_at: parking_lot::Mutex::new(
                std::time::Instant::now() - PERMISSION_FLOW_WINDOW * 2,
            ),
        }
    }
}

impl PermissionFlowActive {
    /// Record that a permission flow (System Settings, or a TCC prompt) just
    /// started or is still ongoing.
    pub fn mark_active(&self) {
        *self.marked_at.lock() = std::time::Instant::now();
    }

    /// Whether a permission flow is still assumed to be in progress.
    pub fn is_active(&self) -> bool {
        self.marked_at.lock().elapsed() < PERMISSION_FLOW_WINDOW
    }

    /// Clear the flag outright, so a deliberate dismissal (Escape, an explicit
    /// hide) is never blocked by a permission flow the user has since abandoned.
    pub fn clear(&self) {
        *self.marked_at.lock() = std::time::Instant::now() - PERMISSION_FLOW_WINDOW * 2;
    }
}

/// Emitted to the staff window as the pointer moves in and out.
pub const STAFF_HOVER_EVENT: &str = "caduceus://staff-hover";
/// Asks the Command Center to open in a particular mode.
pub const COMMAND_CENTER_OPEN_EVENT: &str = "caduceus://command-center-open";

/// Height the walkthrough card needs for its longest step, plus its own padding.
///
/// Raised from 210 when the walkthrough grew a permissions phase and became a
/// proper centred modal rather than a small strip. The card asks for 560px of
/// width and self-limits to `calc(100% - 32px)`, so a window that is too small
/// does not clip it — it quietly squeezes it instead, which is how the old
/// walkthrough ended up feeling cramped. Sizing the window from a realistic card
/// height keeps the modal at its intended width and leaves the longest step
/// (three permissions, each with its own numbered instructions) room to breathe.
const ONBOARDING_CARD_HEIGHT: f64 = 260.0;
/// Gap between the top of the window and the card, and between card and mark.
const ONBOARDING_CARD_GAP: f64 = 16.0;

/// Clearance between the outermost pop-out icon and the edge of the window.
/// Labels now occupy one fixed chip inside the ring instead of hanging from all
/// six icons, so the window only needs room for the target, its shadow and the
/// expand animation.
const POPOUT_EDGE_MARGIN: f64 = 32.0;

/// Side length of the staff window. Grows with staff size and pop-out reach so
/// icons and the hover label are never clipped; clamped to a sane range.
///
/// While the first-run walkthrough is unfinished the window also has to hold its
/// card, which is drawn in the top half so the mark at the centre stays visible
/// and clickable. At the default staff size the ordinary window is 280px, which
/// leaves the card 88px for content that needs about 210 — so it arrived
/// scrolled and cut in half. The window is click-through everywhere except the
/// mark and the card, so making it bigger for the duration costs nothing.
pub fn staff_window_side(settings: &Settings) -> f64 {
    let a = &settings.appearance;
    let mark = a.staff_size as f64;
    let reach = a.popout_radius as f64 + a.popout_icon_size as f64 / 2.0 + POPOUT_EDGE_MARGIN;
    let base = (reach * 2.0).max(mark * 2.2).clamp(280.0, 480.0);

    if settings.general.onboarding_done {
        return base;
    }

    // Half the window must fit: top gap, the card, a gap, and the mark's radius.
    let half_needed =
        ONBOARDING_CARD_GAP + ONBOARDING_CARD_HEIGHT + ONBOARDING_CARD_GAP + mark / 2.0;
    base.max((half_needed * 2.0).min(680.0))
}

/// Keep the staff above other apps, including another app's full-screen space.
pub fn configure_staff_floating<R: Runtime>(window: &WebviewWindow<R>) {
    #[cfg(target_os = "macos")]
    macos::configure_staff_window(window);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window.set_visible_on_all_workspaces(true);
        let _ = window.set_always_on_top(true);
    }
}

/// The same, for the Command Center — the window you actually reach for while
/// another app is full-screen.
pub fn configure_command_center_floating<R: Runtime>(window: &WebviewWindow<R>) {
    #[cfg(target_os = "macos")]
    macos::configure_command_center_window(window);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window.set_visible_on_all_workspaces(true);
        let _ = window.set_always_on_top(true);
    }
}

pub fn sync_staff_window<R: Runtime>(
    app: &AppHandle<R>,
    settings: &SettingsManager,
) -> tauri::Result<()> {
    if let Some(window) = staff(app) {
        configure_staff_floating(&window);
        position_staff(app, settings)?;
    }
    Ok(())
}

/// Whether the staff window should be visible at launch / after layout refresh.
pub fn should_show_staff(settings: &Settings) -> bool {
    settings.general.staff_visible || !settings.general.onboarding_done
}

/// Resize, reposition, and show the staff when the webview has loaded settings.
pub fn refresh_staff_layout<R: Runtime>(
    app: &AppHandle<R>,
    settings: &SettingsManager,
) -> Result<(), String> {
    let cfg = settings.get();
    if let Some(window) = staff(app) {
        position_staff(app, settings).map_err(|e| e.to_string())?;
        if should_show_staff(&cfg) {
            window.show().map_err(|e| e.to_string())?;
            configure_staff_floating(&window);
        }
    }
    Ok(())
}

/// Gap between the staff and the screen edge when it snaps.
const EDGE_MARGIN: f64 = 14.0;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StaffHoverState {
    /// The pointer is over the staff or its pop-out.
    pub hovering: bool,
    /// The pop-out is showing.
    pub expanded: bool,
}

/// Broadcast app-wide whenever the Command Center becomes visible.
///
/// [`COMMAND_CENTER_OPEN_EVENT`] is emitted to that window alone, so no other
/// webview can observe it. The staff needs to.
pub const COMMAND_CENTER_SHOWN_EVENT: &str = "caduceus://command-center-shown";
/// Latest GitHub release is newer than this build.
pub const UPDATE_AVAILABLE_EVENT: &str = "caduceus://update-available";

/// What the Command Center should show when it opens.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandCenterOpenPayload {
    /// Text to pre-fill the input with.
    pub prefill: String,
    /// `"clipboard"` opens straight into clipboard history; `"default"` is the
    /// normal palette.
    pub mode: String,
    /// Focus the input and select any prefilled text.
    pub select_all: bool,
    /// What opened it: `"hotkey"`, `"staff"`, `"tray"`, or `"other"`.
    ///
    /// Only the first-run walkthrough reads this — it asks you to use the
    /// keyboard shortcut specifically, and cannot tell a hotkey from a click
    /// without being told.
    pub source: String,
}

impl Default for CommandCenterOpenPayload {
    fn default() -> Self {
        Self {
            prefill: String::new(),
            mode: "default".into(),
            select_all: true,
            source: "other".into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Lookups
// ---------------------------------------------------------------------------

pub fn staff<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    app.get_webview_window(STAFF_WINDOW)
}

pub fn command_center<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    app.get_webview_window(COMMAND_CENTER_WINDOW)
}

// ---------------------------------------------------------------------------
// Staff
// ---------------------------------------------------------------------------

/// Place the staff window at its saved position, or snap it to the configured
/// edge on first run.
///
/// Also runs at startup for a saved position, because a monitor may have been
/// unplugged since — an staff parked at x=3000 on a since-removed display would
/// otherwise be invisible and unreachable.
pub fn position_staff<R: Runtime>(
    app: &AppHandle<R>,
    settings: &SettingsManager,
) -> tauri::Result<()> {
    let Some(window) = staff(app) else {
        return Ok(());
    };

    let scale = window.scale_factor().unwrap_or(1.0);
    let monitor = window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten());

    let (screen_x, screen_y, screen_w, screen_h) = match &monitor {
        Some(m) => {
            let pos = m.position().to_logical::<f64>(scale);
            let size = m.size().to_logical::<f64>(scale);
            (pos.x, pos.y, size.width, size.height)
        }
        // No monitor info (headless CI, mostly): fall back to something sane.
        None => (0.0, 0.0, 1440.0, 900.0),
    };

    let cfg = settings.get();
    let side = staff_window_side(&cfg);
    let saved = cfg.general.staff_position.and_then(|p| {
        let inside = p.x >= screen_x - side
            && p.x <= screen_x + screen_w
            && p.y >= screen_y - side
            && p.y <= screen_y + screen_h;
        inside.then_some(p)
    });

    let position = saved.unwrap_or_else(|| {
        let y = screen_y + (screen_h - side) / 2.0;
        let x = match cfg.general.staff_edge {
            StaffEdge::Right => screen_x + screen_w - side + EDGE_MARGIN,
            StaffEdge::Left => screen_x - EDGE_MARGIN,
        };
        Point { x, y }
    });

    window.set_size(LogicalSize::new(side, side))?;
    window.set_position(LogicalPosition::new(position.x, position.y))?;
    Ok(())
}

/// Show or hide the staff and persist the choice.
pub fn set_staff_visible<R: Runtime>(
    app: &AppHandle<R>,
    settings: &SettingsManager,
    visible: bool,
) -> Result<(), String> {
    if let Some(window) = staff(app) {
        if visible {
            let _ = position_staff(app, settings);
            window.show().map_err(|e| e.to_string())?;
            configure_staff_floating(&window);
        } else {
            window.hide().map_err(|e| e.to_string())?;
        }
    }

    let mut next = settings.get();
    if next.general.staff_visible != visible {
        next.general.staff_visible = visible;
        crate::settings::save(app, &next)?;
    }
    Ok(())
}

pub fn toggle_staff<R: Runtime>(
    app: &AppHandle<R>,
    settings: &SettingsManager,
) -> Result<bool, String> {
    let visible = staff(app)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    set_staff_visible(app, settings, !visible)?;
    Ok(!visible)
}

/// Record the staff's position after a drag.
pub fn persist_staff_position<R: Runtime>(
    app: &AppHandle<R>,
    settings: &SettingsManager,
) -> Result<(), String> {
    let Some(window) = staff(app) else {
        return Ok(());
    };
    let scale = window.scale_factor().unwrap_or(1.0);
    let Ok(pos) = window.outer_position() else {
        return Ok(());
    };
    let logical = pos.to_logical::<f64>(scale);

    let mut next = settings.get();
    next.general.staff_position = Some(Point {
        x: logical.x,
        y: logical.y,
    });
    crate::settings::save(app, &next)
}

// ---------------------------------------------------------------------------
// Cursor tracker
// ---------------------------------------------------------------------------

/// Poll interval used while the pointer is near the staff, before settings load.
const DEFAULT_POLL_MS: u64 = 33;
/// Interval used when the staff is hidden — there is nothing to hover.
const HIDDEN_POLL_MS: u64 = 1000;
/// Interval used when the window or cursor cannot be read at all.
const IDLE_POLL_MS: u64 = 500;
/// Slowest interval used while the staff is visible but the pointer is far away.
///
/// This is a latency floor, not just a CPU knob: the tracker cannot notice the
/// pointer arriving until the next tick, so whatever this is set to is roughly
/// the worst-case delay before the staff reacts. At the old 400ms that was a
/// visible stall on both hover and the first click.
const FAR_POLL_MS: u64 = 60;
/// How many multiples of the pop-out radius count as "approaching".
///
/// Larger keeps the fast rate over a wider area, so a pointer heading for the
/// staff is already being sampled quickly by the time it arrives.
const APPROACH_BANDS: f64 = 7.0;
/// Extra radius, in logical px, where the window still captures clicks.
///
/// Click-through is toggled from a polled sample, so between the pointer
/// entering the staff and the next tick the window is still transparent and the
/// click lands on whatever is behind it. Arming capture early closes that gap
/// without making the visible hit area bigger.
///
/// Sized against the worst case rather than the typical one: a pointer crossing
/// at a few thousand logical pixels per second covers a lot of ground in one
/// [`FAR_POLL_MS`] tick, and every pixel of that is a click that lands on the
/// app underneath and looks like the staff ignoring you. The cost of being
/// generous is a ring a couple of dozen pixels wider than the mark that
/// swallows clicks — on a window the user positioned at the edge of the screen
/// precisely so nothing else is there.
const CAPTURE_MARGIN: f64 = 28.0;
/// Slowest interval once the pointer has been sitting still somewhere that
/// cannot currently affect the staff — not hovering, not expanded, not over a
/// registered capture rect — for [`STILL_TICKS_FOR_IDLE`] consecutive ticks.
///
/// [`FAR_POLL_MS`] alone caps out a handful of pop-out radii from the staff;
/// past that the loop never gets any slower, so a pointer sitting untouched on
/// another monitor is polled exactly as often as one just outside the
/// approach band. That is the case the "1% of a CPU, forever" doc comment
/// above is actually about — not a pointer that is merely far, but one that
/// has stopped moving at all, which is what an idle desktop looks like far
/// more often than "someone is approaching the staff" does.
const FAR_IDLE_POLL_MS: u64 = 500;
/// How many consecutive stationary, inactive ticks before [`FAR_IDLE_POLL_MS`]
/// kicks in. At [`FAR_POLL_MS`] this is a little under a second — long enough
/// that a brief pause between mouse movements is not mistaken for having
/// walked away, short enough that the idle rate is what the app spends nearly
/// all of its time at.
const STILL_TICKS_FOR_IDLE: u32 = 16;

/// Poll interval for the next tick, given how far the pointer is from the staff.
///
/// Returns `fast_ms` inside the interactive region and ramps linearly up to
/// [`FAR_POLL_MS`] over [`APPROACH_BANDS`] multiples of the pop-out radius.
fn next_delay(distance: f64, popout_radius: f64, fast_ms: u64, active: bool) -> u64 {
    if active {
        return fast_ms;
    }
    let radius = popout_radius.max(1.0);
    let slack = (distance - radius).max(0.0);
    let ramp = (slack / (radius * APPROACH_BANDS)).clamp(0.0, 1.0);
    let fast = fast_ms as f64;
    let slow = FAR_POLL_MS.max(fast_ms) as f64;
    (fast + (slow - fast) * ramp).round() as u64
}

/// Whether the idle rate ([`FAR_IDLE_POLL_MS`]) should override the distance
/// ramp this tick, and the tick counter to carry into the next one.
///
/// `active` is "something time-sensitive is in flight" (`inside`,
/// `state.expanded`, or `over_capture_rect` at the call site) and `moved` is
/// whether the cursor's physical position differs from last tick's. Either
/// one resets the counter to zero, so neither an approach nor a resumed drag
/// ever inherits leftover idle latency — only a pointer that is both doing
/// nothing *and* going nowhere accumulates toward the idle rate.
fn idle_tick(still_ticks: u32, active: bool, moved: bool) -> (bool, u32) {
    let next = if active || moved {
        0
    } else {
        still_ticks.saturating_add(1)
    };
    (next >= STILL_TICKS_FOR_IDLE, next)
}

/// Handle for stopping the tracker at shutdown.
#[derive(Clone, Default)]
pub struct CursorTracker {
    stop: Arc<AtomicBool>,
    /// Set by the staff webview after a pop-out click so the ring collapses
    /// immediately instead of waiting for the idle timer.
    collapse_now: Arc<AtomicBool>,
    /// Keeps the whole staff window clickable regardless of pointer distance.
    ///
    /// The window is click-through everywhere except the staff itself, which is
    /// what stops it swallowing a 340px square of your desktop. Only for
    /// gestures that own the pointer until they end — a resize drag, where the
    /// pointer routinely leaves the mark's hit circle and losing capture
    /// mid-drag would drop the gesture.
    ///
    /// Anything merely *drawn* in this window wants [`Self::capture_rect`]
    /// instead. Forcing capture for as long as something is on screen makes a
    /// square of the desktop dead to clicks for that whole time.
    force_interactive: Arc<AtomicBool>,
    /// One extra region that captures the pointer, in logical pixels relative to
    /// the staff window's top-left.
    ///
    /// The first-run walkthrough draws a card here and needs its buttons
    /// clickable — but only the card. It used to set `force_interactive` for the
    /// entire walkthrough, which swallowed every click in the window's bounds
    /// until the tour ended: the staff could not be dragged and whatever was
    /// behind the window could not be reached.
    capture_rect: Arc<RwLock<Option<CaptureRect>>>,
}

/// A rectangle in the staff window's own coordinate space, in logical pixels.
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
    fn contains(
        &self,
        cursor_x: f64,
        cursor_y: f64,
        origin_x: f64,
        origin_y: f64,
        scale: f64,
    ) -> bool {
        let left = origin_x + self.x * scale;
        let top = origin_y + self.y * scale;
        cursor_x >= left
            && cursor_x <= left + self.width * scale
            && cursor_y >= top
            && cursor_y <= top + self.height * scale
    }
}

impl CursorTracker {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn request_collapse(&self) {
        self.collapse_now.store(true, Ordering::Relaxed);
    }

    pub fn set_force_interactive(&self, on: bool) {
        self.force_interactive.store(on, Ordering::Relaxed);
    }

    pub fn set_capture_rect(&self, rect: Option<CaptureRect>) {
        *self.capture_rect.write() = rect;
    }
}

/// Start the loop that drives staff hover, auto-collapse and click-through.
pub fn spawn_cursor_tracker<R: Runtime>(
    app: AppHandle<R>,
    settings: SettingsManager,
) -> CursorTracker {
    let tracker = CursorTracker::default();
    let stop = tracker.stop.clone();
    let collapse_now = tracker.collapse_now.clone();
    let force_interactive = tracker.force_interactive.clone();
    let capture_rect = tracker.capture_rect.clone();

    tauri::async_runtime::spawn(async move {
        let mut state = StaffHoverState {
            hovering: false,
            expanded: false,
        };
        // When the pointer first entered the staff, for the expand delay.
        let mut entered_at: Option<std::time::Instant> = None;
        // When the pointer last left, for the collapse delay.
        let mut left_at: Option<std::time::Instant> = None;
        let mut click_through = true;
        // After a pop-out click, do not re-open until the pointer leaves the staff.
        let mut block_expand = false;
        let mut last_emitted = state;
        // Last physical cursor position seen, and how many consecutive ticks
        // it has gone unchanged while nothing time-sensitive was in progress —
        // see FAR_IDLE_POLL_MS below.
        let mut last_cursor: Option<(f64, f64)> = None;
        let mut still_ticks: u32 = 0;

        let emit_state =
            |window: &WebviewWindow<R>, state: &StaffHoverState, last: &mut StaffHoverState| {
                if *state != *last {
                    *last = *state;
                    let _ = window.emit(STAFF_HOVER_EVENT, state);
                }
            };

        // Adaptive poll interval. Polling at 30Hz forever costs about 1% of a
        // CPU on an always-on app, which is real battery for something that is
        // idle almost all the time. The rate scales along two axes: how far
        // the pointer is from the staff (full speed when it is close enough to
        // matter, progressively lazier as it gets further away — approaching
        // the staff crosses the intermediate bands first, so by the time the
        // pointer arrives the loop is already running fast and hover still
        // feels instant), and, further out, whether the pointer is moving at
        // all (a pointer that has stopped moving anywhere backs off past the
        // distance ramp's own floor — see FAR_IDLE_POLL_MS).
        let mut delay_ms: u64 = DEFAULT_POLL_MS;

        loop {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;

            if stop.load(Ordering::Relaxed) {
                return;
            }

            let cfg = settings.with(|s| (s.general.clone(), s.appearance.clone()));
            let (general, appearance) = cfg;

            let Some(window) = app.get_webview_window(STAFF_WINDOW) else {
                delay_ms = IDLE_POLL_MS;
                last_cursor = None;
                continue;
            };
            if !window.is_visible().unwrap_or(false) {
                // Nothing to hover: check back rarely.
                delay_ms = HIDDEN_POLL_MS;
                last_cursor = None;
                if !click_through {
                    let _ = window.set_ignore_cursor_events(true);
                    click_through = true;
                }
                continue;
            }

            let (Ok(cursor), Ok(origin), Ok(size), Ok(scale)) = (
                window.cursor_position(),
                window.outer_position(),
                window.outer_size(),
                window.scale_factor(),
            ) else {
                delay_ms = IDLE_POLL_MS;
                last_cursor = None;
                continue;
            };

            // Everything below is in physical pixels: mixing in logical values
            // silently breaks hit-testing on a scaled display.
            let centre_x = origin.x as f64 + size.width as f64 / 2.0;
            let centre_y = origin.y as f64 + size.height as f64 / 2.0;
            let distance = ((cursor.x - centre_x).powi(2) + (cursor.y - centre_y).powi(2)).sqrt();

            let orb_radius = (appearance.staff_size as f64 / 2.0 + 8.0) * scale;
            // Corner resize knobs sit on a square around the mark — reach the
            // corner of that square plus the knob radius so they stay hittable.
            let resize_reach =
                ((appearance.staff_size as f64 * 0.5 + 8.0) * std::f64::consts::SQRT_2 + 10.0)
                    * scale;
            let popout_radius =
                (appearance.popout_radius as f64 + appearance.popout_icon_size as f64 / 2.0 + 12.0)
                    * scale;

            // Looser thresholds while expanded so icons do not flicker on the edge.
            let hover_slack = if state.expanded || state.hovering {
                14.0 * scale
            } else {
                0.0
            };
            let hit_radius = if state.hovering {
                orb_radius.max(resize_reach)
            } else {
                orb_radius
            };
            let over_orb = distance <= hit_radius + hover_slack;
            let over_popout = state.expanded && distance <= popout_radius + hover_slack;
            let inside = over_orb || over_popout;
            let near_staff = distance <= popout_radius + 28.0 * scale;

            if !near_staff {
                block_expand = false;
            }

            if collapse_now.swap(false, Ordering::Relaxed) && state.expanded {
                block_expand = true;
                state = StaffHoverState {
                    hovering: false,
                    expanded: false,
                };
                entered_at = None;
                left_at = None;
                emit_state(&window, &state, &mut last_emitted);
            }

            // --- expand ---------------------------------------------------
            if over_orb {
                left_at = None;
                if block_expand {
                    if !state.hovering {
                        state.hovering = true;
                        emit_state(&window, &state, &mut last_emitted);
                    }
                } else {
                    let entry = *entered_at.get_or_insert_with(std::time::Instant::now);
                    if !state.expanded
                        && entry.elapsed()
                            >= std::time::Duration::from_millis(general.hover_expand_delay_ms)
                    {
                        state = StaffHoverState {
                            hovering: true,
                            expanded: true,
                        };
                        emit_state(&window, &state, &mut last_emitted);
                    } else if !state.hovering {
                        state.hovering = true;
                        emit_state(&window, &state, &mut last_emitted);
                    }
                }
            } else {
                entered_at = None;

                // --- collapse ---------------------------------------------
                if state.expanded && !over_popout {
                    let left = *left_at.get_or_insert_with(std::time::Instant::now);
                    if left.elapsed() >= std::time::Duration::from_millis(general.collapse_idle_ms)
                    {
                        state = StaffHoverState {
                            hovering: false,
                            expanded: false,
                        };
                        left_at = None;
                        emit_state(&window, &state, &mut last_emitted);
                    }
                } else if !state.expanded && state.hovering && !over_popout {
                    state.hovering = false;
                    emit_state(&window, &state, &mut last_emitted);
                } else if over_popout {
                    left_at = None;
                }
            }

            // --- click-through -------------------------------------------
            // Only the circle the user can actually interact with captures the
            // pointer; the rest of the 340px square stays transparent to clicks.
            //
            // Armed a few pixels early (see CAPTURE_MARGIN): the toggle only
            // happens on a poll tick, so capturing exactly at the visible edge
            // means a fast pointer can land and click while the window is still
            // transparent, sending the click to the app underneath.
            let capture_margin = CAPTURE_MARGIN * scale;
            let capture_radius = if state.hovering {
                orb_radius.max(resize_reach)
            } else {
                orb_radius
            };
            // A registered rect (the walkthrough card) captures on its own
            // bounds only, so the rest of the window stays click-through and the
            // staff underneath keeps working while the card is up.
            let over_capture_rect = capture_rect.read().is_some_and(|r| {
                r.contains(cursor.x, cursor.y, origin.x as f64, origin.y as f64, scale)
            });

            let should_capture = force_interactive.load(Ordering::Relaxed)
                || over_capture_rect
                || distance <= capture_radius + capture_margin
                || (state.expanded && distance <= popout_radius + capture_margin);
            if should_capture == click_through {
                let _ = window.set_ignore_cursor_events(!should_capture);
                click_through = !should_capture;
            }

            // --- pick the next poll interval ------------------------------
            // The card can sit further from the mark than the pop-out ring
            // reaches, so distance alone would drop the loop to its lazy rate
            // and make its buttons feel unresponsive.
            let active = inside || state.expanded || over_capture_rect;
            let moved = last_cursor != Some((cursor.x, cursor.y));
            last_cursor = Some((cursor.x, cursor.y));

            let (is_idle, next_still_ticks) = idle_tick(still_ticks, active, moved);
            still_ticks = next_still_ticks;

            delay_ms = if is_idle {
                FAR_IDLE_POLL_MS
            } else {
                next_delay(distance, popout_radius, general.cursor_poll_ms, active)
            };
        }
    });

    tracker
}

// ---------------------------------------------------------------------------
// Command Center
// ---------------------------------------------------------------------------

/// Show the Command Center, centred on the display holding the pointer.
pub fn open_command_center<R: Runtime>(
    app: &AppHandle<R>,
    payload: CommandCenterOpenPayload,
) -> Result<(), String> {
    let Some(window) = command_center(app) else {
        return Err("the Command Center window is missing".into());
    };

    // Centre on whichever screen the user is currently looking at, not on the
    // screen the app happened to start on.
    if let (Ok(cursor), Ok(monitors)) = (window.cursor_position(), window.available_monitors()) {
        let target = monitors.into_iter().find(|m| {
            let p = m.position();
            let s = m.size();
            cursor.x >= p.x as f64
                && cursor.x < (p.x + s.width as i32) as f64
                && cursor.y >= p.y as f64
                && cursor.y < (p.y + s.height as i32) as f64
        });
        if let Some(monitor) = target {
            if let Ok(size) = window.outer_size() {
                let p = monitor.position();
                let s = monitor.size();
                let x = p.x + (s.width as i32 - size.width as i32) / 2;
                // Sitting slightly above centre reads as "floating" rather than
                // "stuck to the middle", and leaves room for the results list.
                let y = p.y + (s.height as i32 - size.height as i32) / 3;
                let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
            }
        }
    }

    if let Some(state) = app.try_state::<PaletteFloating>() {
        state.mark_shown();
    }

    // Configure, *then* show. Ordering a window in before its collection
    // behaviour allows every Space puts it in the Space it was created in — the
    // desktop — where, from inside a full-screen app, it is not hidden or
    // behind anything, it is elsewhere. Setting the flag afterwards does not
    // reliably bring it across, which is why the hotkey appeared to do nothing.
    #[cfg(target_os = "macos")]
    macos::prepare_command_center(&window);

    window.show().map_err(|e| e.to_string())?;
    window.unminimize().ok();

    // And not just the level and the Space either. Caduceus is an Accessory
    // app, so it is never the active application, and an ordinary window of an
    // inactive app cannot hold the keyboard. It is presented as a
    // non-activating panel: key — so it can be typed into immediately — without
    // dragging the user out of the full-screen Space they are in.
    #[cfg(target_os = "macos")]
    macos::present_command_center(&window);
    #[cfg(not(target_os = "macos"))]
    {
        configure_command_center_floating(&window);
        // `set_focus` is `activateIgnoringOtherApps:` underneath. Everywhere
        // but macOS that is simply how a window gets focused.
        window.set_focus().map_err(|e| e.to_string())?;
    }
    let source = payload.source.clone();
    window
        .emit(COMMAND_CENTER_OPEN_EVENT, payload)
        .map_err(|e| e.to_string())?;
    // The event above is window-scoped, so the staff never sees it. The
    // walkthrough lives in the staff window and needs to know.
    {
        use tauri::Emitter;
        let _ = app.emit(COMMAND_CENTER_SHOWN_EVENT, source);
    }
    Ok(())
}

pub fn hide_command_center<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    // This is the explicit-dismissal path — Escape, clicking the close affordance,
    // `toggle_command_center` while visible — as opposed to the automatic hide in
    // `handle_window_event`'s blur handler. A deliberate "go away" from the user
    // should always win, even mid permission-flow: otherwise someone who opens
    // System Settings, changes their mind, and Escapes back to the palette would
    // find it refuses to take the hint for up to three minutes.
    if let Some(state) = app.try_state::<PermissionFlowActive>() {
        state.clear();
    }
    if let Some(window) = command_center(app) {
        window.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn toggle_command_center<R: Runtime>(
    app: &AppHandle<R>,
    source: impl Into<String>,
) -> Result<(), String> {
    let visible = command_center(app)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if visible {
        hide_command_center(app)
    } else {
        open_command_center(
            app,
            CommandCenterOpenPayload {
                source: source.into(),
                ..Default::default()
            },
        )
    }
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// Show the Settings window, optionally jumping to a tab.
/// Open the Command Center on a particular tab.
///
/// The single entry point behind "open Settings", "open that conversation" and
/// "show me the ports" — all of which used to be their own window.
pub fn open_tab<R: Runtime>(app: &AppHandle<R>, request: TabRequest) -> Result<(), String> {
    open_command_center(
        app,
        CommandCenterOpenPayload {
            source: "tab".into(),
            ..Default::default()
        },
    )?;

    let Some(window) = command_center(app) else {
        return Err("the Command Center window is missing".into());
    };
    window
        .emit(TAB_OPEN_EVENT, request)
        .map_err(|e| e.to_string())
}

pub fn open_settings<R: Runtime>(app: &AppHandle<R>, tab: Option<&str>) -> Result<(), String> {
    open_tab(
        app,
        TabRequest {
            kind: "settings".into(),
            section: tab.map(str::to_string),
            conversation_id: None,
        },
    )
}

pub fn open_chat<R: Runtime>(
    app: &AppHandle<R>,
    conversation_id: Option<i64>,
) -> Result<(), String> {
    open_tab(
        app,
        TabRequest {
            kind: "chat".into(),
            section: None,
            conversation_id,
        },
    )
}

pub fn open_manage<R: Runtime>(app: &AppHandle<R>, page: Option<&str>) -> Result<(), String> {
    open_tab(
        app,
        TabRequest {
            kind: page.unwrap_or("awake").to_string(),
            section: None,
            conversation_id: None,
        },
    )
}

/// Switch the Command Center between overlay and ordinary-window behaviour.
pub fn set_palette_floating<R: Runtime>(app: &AppHandle<R>, floating: bool) -> Result<(), String> {
    if let Some(state) = app.try_state::<PaletteFloating>() {
        state.set(floating);
    }
    let Some(window) = command_center(app) else {
        return Ok(());
    };

    // Both personalities are the same window on macOS now, and deliberately so.
    // Switching to `Regular` and handing the window back to AppKit gave a
    // window with tabs a Dock icon and an ordinary level — and threw anyone who
    // opened Settings from a full-screen app out to the desktop to see it. The
    // only thing that changes here is whether clicking away dismisses, which is
    // read from `PaletteFloating` in the blur handler above.
    #[cfg(target_os = "macos")]
    macos::keep_in_place(&window);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window.set_always_on_top(floating);
    }
    Ok(())
}

/// Which of Caduceus's own windows [`set_window_opacity`] can dim.
///
/// Deliberately not "any window" — a foreign window's opacity is not
/// something this process is allowed to touch at all (see
/// `tools::knowledge`'s "always-on-top gap" for the same boundary on window
/// level), so the only sensible reading of "opacity control" for a launcher
/// like this one is *its own* windows: the staff and the Command Center.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpacityTarget {
    Staff,
    CommandCenter,
}

/// Set how much of the desktop shows through one of Caduceus's own windows —
/// not the webview content's CSS opacity, the whole window, chrome included.
///
/// macOS only: `NSWindow.alphaValue` is the mechanism, and there is no
/// equivalent this crate already reaches for on Windows or Linux. Asking on
/// another platform is a clear, readable error rather than a silent no-op.
pub fn set_window_opacity<R: Runtime>(
    app: &AppHandle<R>,
    target: OpacityTarget,
    opacity: f32,
) -> Result<(), String> {
    let window = match target {
        OpacityTarget::Staff => staff(app),
        OpacityTarget::CommandCenter => command_center(app),
    };
    let Some(window) = window else {
        return Err("That window is not open right now.".into());
    };

    #[cfg(target_os = "macos")]
    {
        macos::set_window_opacity(&window, opacity);
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, opacity);
        Err("Window opacity control is only available on macOS.".into())
    }
}

/// Apply the platform's native background material to a window.
///
/// CSS `backdrop-filter` blurs page content only — it cannot see the desktop
/// behind a transparent window. Real glass needs the OS compositor, which is
/// what this asks for. Failure is non-fatal: the frontend's own translucent
/// fills already look correct on their own.
pub fn apply_vibrancy<R: Runtime>(window: &WebviewWindow<R>) {
    #[cfg(target_os = "macos")]
    {
        use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};
        if let Err(e) = apply_vibrancy(
            window,
            NSVisualEffectMaterial::HudWindow,
            Some(NSVisualEffectState::Active),
            Some(16.0),
        ) {
            log::debug!("vibrancy unavailable: {e}");
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Acrylic on Windows 11; silently unavailable on 10 and earlier.
        if let Err(e) = window_vibrancy::apply_acrylic(window, Some((18, 19, 27, 190))) {
            log::debug!("acrylic unavailable: {e}");
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // No portable equivalent across GNOME/KDE/wlroots.
        let _ = window;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_flow_is_inactive_until_marked() {
        // Mirrors `PaletteFloating`'s default: nothing has happened yet, so there
        // is no flow to protect and a stray blur should hide the palette as usual.
        let flow = PermissionFlowActive::default();
        assert!(!flow.is_active());
    }

    #[test]
    fn permission_flow_stays_active_right_after_marking() {
        let flow = PermissionFlowActive::default();
        flow.mark_active();
        assert!(flow.is_active());
    }

    #[test]
    fn permission_flow_expires_on_its_own() {
        // Never latches forever — a flag that only gets set and never times out
        // would leave the palette refusing to dismiss on blur indefinitely if the
        // user simply abandoned the permission prompt.
        let flow = PermissionFlowActive::default();
        *flow.marked_at.lock() =
            std::time::Instant::now() - PERMISSION_FLOW_WINDOW - std::time::Duration::from_secs(1);
        assert!(!flow.is_active());
    }

    #[test]
    fn clearing_a_freshly_marked_flow_deactivates_it() {
        // The explicit-dismissal escape hatch: `hide_command_center` calls this
        // so Escape always wins, even seconds after a permission was requested.
        let flow = PermissionFlowActive::default();
        flow.mark_active();
        assert!(flow.is_active());
        flow.clear();
        assert!(!flow.is_active());
    }

    #[test]
    fn polls_fast_while_the_pointer_is_engaged() {
        // Inside the region, or expanded, must never be throttled — this is the
        // path where latency is felt.
        assert_eq!(next_delay(10.0, 100.0, 33, true), 33);
        assert_eq!(next_delay(9999.0, 100.0, 33, true), 33);
    }

    #[test]
    fn polls_fast_at_the_edge_of_the_region() {
        assert_eq!(next_delay(100.0, 100.0, 33, false), 33);
    }

    #[test]
    fn backs_off_as_the_pointer_moves_away() {
        let near = next_delay(200.0, 100.0, 33, false);
        let mid = next_delay(300.0, 100.0, 33, false);
        let far = next_delay(1000.0, 100.0, 33, false);

        assert!(near > 33, "should start backing off past the region");
        assert!(mid > near, "should keep slowing with distance");
        assert_eq!(far, FAR_POLL_MS, "should saturate rather than grow forever");
        assert!(near < FAR_POLL_MS);
    }

    #[test]
    fn never_polls_slower_than_the_far_rate() {
        for distance in [0.0, 50.0, 500.0, 100_000.0] {
            let delay = next_delay(distance, 100.0, 33, false);
            assert!(
                (33..=FAR_POLL_MS).contains(&delay),
                "{distance} gave {delay}"
            );
        }
    }

    #[test]
    fn a_slow_configured_rate_is_never_sped_up() {
        // If the user asks for 500ms polling, honour it rather than "optimising"
        // them back down to our own ceiling.
        assert_eq!(next_delay(9999.0, 100.0, 500, false), 500);
    }

    #[test]
    fn worst_case_latency_stays_imperceptible() {
        // The poll interval *is* the reaction time: nothing is noticed until the
        // next tick. This was 400ms and read as the staff being broken — both
        // hover and, worse, the first click, which lands on the app underneath
        // while the window is still click-through.
        assert!(
            FAR_POLL_MS <= 100,
            "a pointer arriving at the staff waits up to FAR_POLL_MS ({FAR_POLL_MS}ms) \
             before anything happens; keep it under human reaction time"
        );
    }

    #[test]
    fn approach_is_sampled_quickly_well_before_arrival() {
        // Moving toward the staff should already be at or near the fast rate a
        // couple of radii out, not only once the pointer is on top of it.
        let approaching = next_delay(250.0, 100.0, 33, false);
        assert!(
            approaching <= 50,
            "still {approaching}ms at 2.5x radius — the ramp backs off too early"
        );
    }

    #[test]
    fn degenerate_radius_does_not_divide_by_zero() {
        // A zero radius would be a NaN ramp and an unusable delay.
        let delay = next_delay(10.0, 0.0, 33, false);
        assert!((33..=FAR_POLL_MS).contains(&delay), "got {delay}");
    }

    #[test]
    fn idle_only_kicks_in_after_enough_still_inactive_ticks() {
        let mut ticks = 0;
        let mut idle;
        for _ in 0..STILL_TICKS_FOR_IDLE - 1 {
            (idle, ticks) = idle_tick(ticks, false, false);
            assert!(!idle, "went idle before the threshold");
        }
        (idle, _) = idle_tick(ticks, false, false);
        assert!(idle, "never went idle despite enough still ticks");
    }

    #[test]
    fn movement_resets_the_idle_counter() {
        // A pointer that is merely far but still being moved — working in
        // another app, say — must never be mistaken for having walked away.
        let mut ticks = STILL_TICKS_FOR_IDLE - 1;
        let (idle, next) = idle_tick(ticks, false, true);
        assert!(!idle);
        assert_eq!(next, 0);
        ticks = next;
        assert_eq!(ticks, 0);
    }

    #[test]
    fn activity_resets_the_idle_counter_even_if_the_pointer_has_not_moved() {
        // Hovering or expanded with a perfectly still pointer must stay on the
        // fast ramp — the collapse/expand timers are real-time, not
        // motion-driven, and would stall if this backed off instead.
        let (idle, next) = idle_tick(STILL_TICKS_FOR_IDLE, true, false);
        assert!(!idle);
        assert_eq!(next, 0);
    }

    #[test]
    fn the_idle_counter_never_wraps() {
        let (_, next) = idle_tick(u32::MAX, false, false);
        assert_eq!(next, u32::MAX, "saturating_add should clamp, not wrap");
    }
}
