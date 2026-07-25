//! Window management: the orb, the Command Center, and Settings.
//!
//! # How the orb stays out of your way
//!
//! The orb lives in a fixed 340×340 transparent, always-on-top window — big
//! enough to hold the fully expanded radial pop-out. A window that size sitting
//! on top of everything would normally swallow every click in a 340px square,
//! which would be intolerable.
//!
//! The fix is [`set_ignore_cursor_events`], driven by a background cursor
//! tracker:
//!
//! ```text
//!   cursor far away        →  window is click-through, orb is a calm dot
//!   cursor over the orb    →  window becomes interactive, pop-out expands
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

use crate::settings::{OrbEdge, Point, SettingsManager};

pub const ORB_WINDOW: &str = "orb";
pub const COMMAND_CENTER_WINDOW: &str = "command-center";
pub const SETTINGS_WINDOW: &str = "settings";

/// Emitted to the orb window as the pointer moves in and out.
pub const ORB_HOVER_EVENT: &str = "orbit://orb-hover";
/// Asks the Command Center to open in a particular mode.
pub const COMMAND_CENTER_OPEN_EVENT: &str = "orbit://command-center-open";

/// Side length of the orb window. Fixed so the radial pop-out always has room;
/// the *visible* orb inside it is what `appearance.orbSize` controls.
pub const ORB_WINDOW_SIZE: f64 = 340.0;

/// Gap between the orb and the screen edge when it snaps.
const EDGE_MARGIN: f64 = 14.0;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OrbHoverState {
    /// The pointer is over the orb or its pop-out.
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

pub fn orb<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    app.get_webview_window(ORB_WINDOW)
}

pub fn command_center<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    app.get_webview_window(COMMAND_CENTER_WINDOW)
}

pub fn settings_window<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    app.get_webview_window(SETTINGS_WINDOW)
}

// ---------------------------------------------------------------------------
// Orb
// ---------------------------------------------------------------------------

/// Place the orb window at its saved position, or snap it to the configured
/// edge on first run.
///
/// Also runs at startup for a saved position, because a monitor may have been
/// unplugged since — an orb parked at x=3000 on a since-removed display would
/// otherwise be invisible and unreachable.
pub fn position_orb<R: Runtime>(app: &AppHandle<R>, settings: &SettingsManager) -> tauri::Result<()> {
    let Some(window) = orb(app) else {
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
    let saved = cfg.general.orb_position.and_then(|p| {
        let inside = p.x >= screen_x - ORB_WINDOW_SIZE
            && p.x <= screen_x + screen_w
            && p.y >= screen_y - ORB_WINDOW_SIZE
            && p.y <= screen_y + screen_h;
        inside.then_some(p)
    });

    let position = saved.unwrap_or_else(|| {
        let y = screen_y + (screen_h - ORB_WINDOW_SIZE) / 2.0;
        let x = match cfg.general.orb_edge {
            OrbEdge::Right => screen_x + screen_w - ORB_WINDOW_SIZE + EDGE_MARGIN,
            OrbEdge::Left => screen_x - EDGE_MARGIN,
        };
        Point { x, y }
    });

    window.set_size(LogicalSize::new(ORB_WINDOW_SIZE, ORB_WINDOW_SIZE))?;
    window.set_position(LogicalPosition::new(position.x, position.y))?;
    Ok(())
}

/// Show or hide the orb and persist the choice.
pub fn set_orb_visible<R: Runtime>(
    app: &AppHandle<R>,
    settings: &SettingsManager,
    visible: bool,
) -> Result<(), String> {
    if let Some(window) = orb(app) {
        if visible {
            let _ = position_orb(app, settings);
            window.show().map_err(|e| e.to_string())?;
            // Re-assert always-on-top: macOS drops it when a window is hidden
            // while another app is in full screen.
            let _ = window.set_always_on_top(true);
        } else {
            window.hide().map_err(|e| e.to_string())?;
        }
    }

    let mut next = settings.get();
    if next.general.orb_visible != visible {
        next.general.orb_visible = visible;
        crate::settings::save(app, &next)?;
    }
    Ok(())
}

pub fn toggle_orb<R: Runtime>(app: &AppHandle<R>, settings: &SettingsManager) -> Result<bool, String> {
    let visible = orb(app).and_then(|w| w.is_visible().ok()).unwrap_or(false);
    set_orb_visible(app, settings, !visible)?;
    Ok(!visible)
}

/// Record the orb's position after a drag.
pub fn persist_orb_position<R: Runtime>(
    app: &AppHandle<R>,
    settings: &SettingsManager,
) -> Result<(), String> {
    let Some(window) = orb(app) else { return Ok(()) };
    let scale = window.scale_factor().unwrap_or(1.0);
    let Ok(pos) = window.outer_position() else {
        return Ok(());
    };
    let logical = pos.to_logical::<f64>(scale);

    let mut next = settings.get();
    next.general.orb_position = Some(Point {
        x: logical.x,
        y: logical.y,
    });
    crate::settings::save(app, &next)
}

// ---------------------------------------------------------------------------
// Cursor tracker
// ---------------------------------------------------------------------------

/// Handle for stopping the tracker at shutdown.
#[derive(Clone, Default)]
pub struct CursorTracker {
    stop: Arc<AtomicBool>,
}

impl CursorTracker {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Start the loop that drives orb hover, auto-collapse and click-through.
pub fn spawn_cursor_tracker<R: Runtime>(
    app: AppHandle<R>,
    settings: SettingsManager,
) -> CursorTracker {
    let tracker = CursorTracker::default();
    let stop = tracker.stop.clone();

    tauri::async_runtime::spawn(async move {
        let mut state = OrbHoverState {
            hovering: false,
            expanded: false,
        };
        // When the pointer first entered the orb, for the expand delay.
        let mut entered_at: Option<std::time::Instant> = None;
        // When the pointer last left, for the collapse delay.
        let mut left_at: Option<std::time::Instant> = None;
        let mut click_through = true;

        loop {
            if stop.load(Ordering::Relaxed) {
                return;
            }

            let cfg = settings.with(|s| (s.general.clone(), s.appearance.clone()));
            let (general, appearance) = cfg;
            tokio::time::sleep(std::time::Duration::from_millis(general.cursor_poll_ms)).await;

            let Some(window) = app.get_webview_window(ORB_WINDOW) else {
                continue;
            };
            if !window.is_visible().unwrap_or(false) {
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
                continue;
            };

            // Everything below is in physical pixels: mixing in logical values
            // silently breaks hit-testing on a scaled display.
            let centre_x = origin.x as f64 + size.width as f64 / 2.0;
            let centre_y = origin.y as f64 + size.height as f64 / 2.0;
            let distance = ((cursor.x - centre_x).powi(2) + (cursor.y - centre_y).powi(2)).sqrt();

            let orb_radius = (appearance.orb_size as f64 / 2.0 + 8.0) * scale;
            let popout_radius =
                (appearance.popout_radius as f64 + appearance.popout_icon_size as f64 / 2.0 + 12.0)
                    * scale;

            let over_orb = distance <= orb_radius;
            let over_popout = state.expanded && distance <= popout_radius;
            let inside = over_orb || over_popout;

            // --- expand ---------------------------------------------------
            if over_orb {
                left_at = None;
                let entry = *entered_at.get_or_insert_with(std::time::Instant::now);
                if !state.expanded
                    && entry.elapsed() >= std::time::Duration::from_millis(general.hover_expand_delay_ms)
                {
                    state = OrbHoverState {
                        hovering: true,
                        expanded: true,
                    };
                    let _ = window.emit(ORB_HOVER_EVENT, state);
                } else if !state.hovering {
                    state.hovering = true;
                    let _ = window.emit(ORB_HOVER_EVENT, state);
                }
            } else {
                entered_at = None;

                // --- collapse ---------------------------------------------
                if state.expanded && !over_popout {
                    let left = *left_at.get_or_insert_with(std::time::Instant::now);
                    if left.elapsed() >= std::time::Duration::from_millis(general.collapse_idle_ms) {
                        state = OrbHoverState {
                            hovering: false,
                            expanded: false,
                        };
                        left_at = None;
                        let _ = window.emit(ORB_HOVER_EVENT, state);
                    }
                } else if !state.expanded && state.hovering {
                    state.hovering = false;
                    let _ = window.emit(ORB_HOVER_EVENT, state);
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
        let _ = window.emit("orbit://settings-tab", tab);
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
