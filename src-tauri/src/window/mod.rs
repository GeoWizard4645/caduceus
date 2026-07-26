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

use serde::Serialize;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Runtime, WebviewWindow};

use crate::settings::{Settings, SettingsManager, StaffEdge, Point};

#[cfg(target_os = "macos")]
mod macos;

pub const STAFF_WINDOW: &str = "staff";
pub const COMMAND_CENTER_WINDOW: &str = "command-center";
pub const SETTINGS_WINDOW: &str = "settings";

/// Emitted to the staff window as the pointer moves in and out.
pub const STAFF_HOVER_EVENT: &str = "caduceus://staff-hover";
/// Asks the Command Center to open in a particular mode.
pub const COMMAND_CENTER_OPEN_EVENT: &str = "caduceus://command-center-open";

/// Side length of the staff window. Grows with staff size and pop-out reach so
/// icons are never clipped; clamped to a sane range.
pub fn staff_window_side(settings: &Settings) -> f64 {
    let a = &settings.appearance;
    let mark = a.staff_size as f64;
    let reach = a.popout_radius as f64 + a.popout_icon_size as f64 / 2.0 + 24.0;
    (reach * 2.0).max(mark * 2.2).clamp(280.0, 480.0)
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

pub fn sync_staff_window<R: Runtime>(app: &AppHandle<R>, settings: &SettingsManager) -> tauri::Result<()> {
    if let Some(window) = staff(app) {
        configure_staff_floating(&window);
        position_staff(app, settings)?;
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
}

impl Default for CommandCenterOpenPayload {
    fn default() -> Self {
        Self {
            prefill: String::new(),
            mode: "default".into(),
            select_all: true,
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

pub fn settings_window<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    app.get_webview_window(SETTINGS_WINDOW)
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
pub fn position_staff<R: Runtime>(app: &AppHandle<R>, settings: &SettingsManager) -> tauri::Result<()> {
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

pub fn toggle_staff<R: Runtime>(app: &AppHandle<R>, settings: &SettingsManager) -> Result<bool, String> {
    let visible = staff(app).and_then(|w| w.is_visible().ok()).unwrap_or(false);
    set_staff_visible(app, settings, !visible)?;
    Ok(!visible)
}

/// Record the staff's position after a drag.
pub fn persist_staff_position<R: Runtime>(
    app: &AppHandle<R>,
    settings: &SettingsManager,
) -> Result<(), String> {
    let Some(window) = staff(app) else { return Ok(()) };
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
const FAR_POLL_MS: u64 = 400;
/// How many multiples of the pop-out radius count as "approaching".
const APPROACH_BANDS: f64 = 3.0;

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

/// Handle for stopping the tracker at shutdown.
#[derive(Clone, Default)]
pub struct CursorTracker {
    stop: Arc<AtomicBool>,
    /// Set by the staff webview after a pop-out click so the ring collapses
    /// immediately instead of waiting for the idle timer.
    collapse_now: Arc<AtomicBool>,
}

impl CursorTracker {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn request_collapse(&self) {
        self.collapse_now.store(true, Ordering::Relaxed);
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

        let emit_state = |window: &WebviewWindow<R>, state: &StaffHoverState, last: &mut StaffHoverState| {
            if *state != *last {
                *last = *state;
                let _ = window.emit(STAFF_HOVER_EVENT, state);
            }
        };

        // Adaptive poll interval. Polling at 30Hz forever costs about 1% of a
        // CPU on an always-on app, which is real battery for something that is
        // idle almost all the time. Instead the rate scales with how far the
        // pointer is from the staff: full speed when it is close enough to
        // matter, and progressively lazier as it gets further away. Approaching
        // the staff crosses the intermediate bands first, so by the time the
        // pointer arrives the loop is already running fast and hover still
        // feels instant.
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
                continue;
            };
            if !window.is_visible().unwrap_or(false) {
                // Nothing to hover: check back rarely.
                delay_ms = HIDDEN_POLL_MS;
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
                continue;
            };

            // Everything below is in physical pixels: mixing in logical values
            // silently breaks hit-testing on a scaled display.
            let centre_x = origin.x as f64 + size.width as f64 / 2.0;
            let centre_y = origin.y as f64 + size.height as f64 / 2.0;
            let distance = ((cursor.x - centre_x).powi(2) + (cursor.y - centre_y).powi(2)).sqrt();

            let orb_radius = (appearance.staff_size as f64 / 2.0 + 8.0) * scale;
            let popout_radius =
                (appearance.popout_radius as f64 + appearance.popout_icon_size as f64 / 2.0 + 12.0)
                    * scale;

            // Looser thresholds while expanded so icons do not flicker on the edge.
            let hover_slack = if state.expanded || state.hovering {
                14.0 * scale
            } else {
                0.0
            };
            let over_orb = distance <= orb_radius + hover_slack;
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
                    if left.elapsed() >= std::time::Duration::from_millis(general.collapse_idle_ms) {
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
            let should_capture = inside;
            if should_capture == click_through {
                let _ = window.set_ignore_cursor_events(!should_capture);
                click_through = !should_capture;
            }

            // --- pick the next poll interval ------------------------------
            delay_ms = next_delay(
                distance,
                popout_radius,
                general.cursor_poll_ms,
                inside || state.expanded,
            );
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

    window.show().map_err(|e| e.to_string())?;
    let _ = window.set_always_on_top(true);
    window.set_focus().map_err(|e| e.to_string())?;
    window
        .emit(COMMAND_CENTER_OPEN_EVENT, payload)
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn hide_command_center<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    if let Some(window) = command_center(app) {
        window.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn toggle_command_center<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let visible = command_center(app)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if visible {
        hide_command_center(app)
    } else {
        open_command_center(app, CommandCenterOpenPayload::default())
    }
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// Show the Settings window, optionally jumping to a tab.
pub fn open_settings<R: Runtime>(app: &AppHandle<R>, tab: Option<&str>) -> Result<(), String> {
    let Some(window) = settings_window(app) else {
        return Err("the Settings window is missing".into());
    };
    window.show().map_err(|e| e.to_string())?;
    window.unminimize().ok();
    window.set_focus().map_err(|e| e.to_string())?;
    if let Some(tab) = tab {
        let _ = window.emit("caduceus://settings-tab", tab);
    }

    // Settings is the one window that should look like a normal app window, so
    // the Dock icon comes back while it is open (macOS accessory apps have no
    // Dock presence otherwise, which makes the window hard to get back to).
    #[cfg(target_os = "macos")]
    {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    }
    Ok(())
}

/// Called when Settings closes: drop back to being a menu-bar-only app.
#[cfg(target_os = "macos")]
pub fn on_settings_closed<R: Runtime>(app: &AppHandle<R>) {
    let others_visible = [COMMAND_CENTER_WINDOW]
        .iter()
        .any(|label| {
            app.get_webview_window(label)
                .and_then(|w| w.is_visible().ok())
                .unwrap_or(false)
        });
    if !others_visible {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
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
            assert!((33..=FAR_POLL_MS).contains(&delay), "{distance} gave {delay}");
        }
    }

    #[test]
    fn a_slow_configured_rate_is_never_sped_up() {
        // If the user asks for 500ms polling, honour it rather than "optimising"
        // them back down to our own ceiling.
        assert_eq!(next_delay(9999.0, 100.0, 500, false), 500);
    }

    #[test]
    fn degenerate_radius_does_not_divide_by_zero() {
        // A zero radius would be a NaN ramp and an unusable delay.
        let delay = next_delay(10.0, 0.0, 33, false);
        assert!((33..=FAR_POLL_MS).contains(&delay), "got {delay}");
    }
}
