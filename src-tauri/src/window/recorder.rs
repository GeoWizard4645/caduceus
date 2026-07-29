//! The recording HUD: a small panel at the bottom of the screen that exists
//! for exactly as long as something is being recorded.
//!
//! # Why this is its own window
//!
//! Dictation used to be invisible. The Command Center showed a "Recording…"
//! pill, which is fine right up until the palette is behind another window, on
//! another display, or was never opened — and then a live microphone is running
//! with nothing on screen saying so and no way to stop it. That is the worst
//! shape a feature like this can take, and it is how a wedged helper turned
//! into "the app is hung and I cannot even turn the recording off".
//!
//! So the indicator is a window of its own:
//!
//! * **Always visible while recording.** Bottom-centre, above everything,
//!   present in every Space including another app's full screen.
//! * **Never in the way.** A non-activating panel — clicking Pause does not
//!   take focus from whatever you are dictating into. It is click-through
//!   everywhere except the pill and the transcript.
//! * **Always an exit.** Stop is one click, and it is on screen the entire
//!   time. Nothing about ending a recording routes through the palette.
//!
//! The transcript sits directly above the controls, so what the recogniser is
//! actually hearing is visible as it is heard rather than after the fact.

use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Runtime, WebviewWindow};

pub const RECORDER_WINDOW: &str = "recorder";

/// Height of the whole HUD, transcript included.
///
/// Fixed rather than fitted: a window that resizes as the transcript grows
/// jitters at the bottom of the screen on every partial result, and the
/// transcript scrolls inside it perfectly well.
const HUD_HEIGHT: f64 = 168.0;
const HUD_WIDTH: f64 = 520.0;
/// Gap between the bottom of the HUD and the bottom of the screen.
///
/// Clear of the Dock at its default size, and clear of nothing important when
/// the Dock is hidden.
const BOTTOM_MARGIN: f64 = 96.0;

pub fn recorder<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    app.get_webview_window(RECORDER_WINDOW)
}

/// Put the HUD on screen, centred at the bottom of the display holding the
/// pointer.
pub fn show<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = recorder(app) else {
        log::warn!("the recording HUD window is missing");
        return;
    };

    position(&window);

    // Panel-ised *before* it is shown, matching the meeting pop-out. `show()`
    // on a plain window is `makeKeyAndOrderFront:`, and a HUD that becomes the
    // key window for even a frame makes the Command Center resign key — which
    // its blur handler reads as "the user clicked away" and hides the palette.
    // That was the whole "starting dictation closes the Command Center" bug:
    // the transcript then streamed into a window that was no longer on screen.
    // As a panel it is incapable of taking the keyboard at all. See
    // `window::panel`.
    super::configure_staff_floating(&window);

    if let Err(e) = window.show() {
        log::error!("could not show the recording HUD: {e}");
    }
}

pub fn hide<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = recorder(app) {
        let _ = window.hide();
    }
}

/// Centre the HUD at the bottom of whichever display the pointer is on.
fn position<R: Runtime>(window: &WebviewWindow<R>) {
    let _ = window.set_size(LogicalSize::new(HUD_WIDTH, HUD_HEIGHT));

    let (Ok(cursor), Ok(monitors)) = (window.cursor_position(), window.available_monitors()) else {
        return;
    };

    let target = monitors
        .into_iter()
        .find(|m| {
            let p = m.position();
            let s = m.size();
            cursor.x >= p.x as f64
                && cursor.x < (p.x + s.width as i32) as f64
                && cursor.y >= p.y as f64
                && cursor.y < (p.y + s.height as i32) as f64
        });

    let Some(monitor) = target else { return };
    let scale = monitor.scale_factor();
    let position = monitor.position().to_logical::<f64>(scale);
    let size = monitor.size().to_logical::<f64>(scale);

    let x = position.x + (size.width - HUD_WIDTH) / 2.0;
    let y = position.y + size.height - HUD_HEIGHT - BOTTOM_MARGIN;
    let _ = window.set_position(LogicalPosition::new(x, y));
}
