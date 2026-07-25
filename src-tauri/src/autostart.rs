//! Launch-at-login.
//!
//! A thin wrapper over `tauri-plugin-autostart`, which registers a macOS
//! LaunchAgent, a Windows `Run` registry entry, or an XDG `.desktop` autostart
//! file as appropriate.
//!
//! Everything here is best-effort and reported rather than fatal: on a locked-
//! down machine (managed Windows, immutable Linux image) registration can fail,
//! and that should surface as a message in Settings rather than stopping the
//! app from running.

use tauri::{AppHandle, Runtime};
use tauri_plugin_autostart::ManagerExt;

pub fn set_enabled<R: Runtime>(app: &AppHandle<R>, enabled: bool) -> Result<(), String> {
    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    result.map_err(|e| {
        format!(
            "Could not {} launch at login: {e}",
            if enabled { "enable" } else { "disable" }
        )
    })
}

/// Whether the OS currently has Caduceus registered to launch at login.
///
/// Read from the OS rather than from settings, so a login item removed by hand
/// is reflected accurately.
pub fn is_enabled<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

/// Reconcile the OS state with the saved preference at startup.
pub fn sync_with_settings<R: Runtime>(app: &AppHandle<R>, wanted: bool) {
    if is_enabled(app) != wanted {
        if let Err(e) = set_enabled(app, wanted) {
            log::warn!("{e}");
        }
    }
}
