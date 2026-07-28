//! Thin `#[tauri::command]` wrappers around [`super::security`].
//!
//! `security.rs`'s own header comment already spells out the wrapper list
//! this file exists to satisfy — see "Wrappers another agent needs to
//! register" there. This file is that other agent's half: nothing here
//! contains logic, it only adapts `tools::security`'s plain functions to the
//! shapes `tauri::generate_handler!` expects, and moves the blocking ones off
//! the thread that draws every window.
//!
//! # Why some of these are `async` and some are not
//!
//! Per `commands.rs`'s own note on `blocking_outcome`: Tauri only moves a
//! command off the calling thread when it is declared `async`, and on macOS
//! that calling thread is the one drawing every window — so anything that can
//! block has to be `async` + `spawn_blocking`, or one slow call beachballs
//! the whole app. Sorted by what each wrapped function actually does:
//!
//! * **Shells out** (`osascript` for the mic, `/usr/bin/log` for the
//!   activity log, `socketfilterfw`/`open` for the firewall) — always
//!   `async`.
//! * **Runs Argon2id** (the file vault) — deliberately slow, hundreds of
//!   milliseconds by design (see `security.rs`'s `key_from_passphrase` doc
//!   comment) — always `async`.
//! * **Pure/in-process** (the passphrase generator, arming or cancelling the
//!   clipboard timer, the TouchID availability check) — stays synchronous.
//!   `arm_clipboard_auto_clear` does touch the live clipboard via `arboard`,
//!   but that is a fast local read, not a subprocess, and it already spawns
//!   its own timer thread internally rather than blocking on one.
//!
//! # Why `blocking_outcome` is duplicated instead of imported
//!
//! `commands.rs::blocking_outcome` is exactly the three lines this file
//! needs, but it is private and `commands.rs` is not this file's to edit (see
//! the task brief this file was built under). Copying three lines is a
//! smaller cost than widening that function's visibility for one caller
//! outside its module — the same tradeoff `security.rs` itself makes for
//! `random_bytes`/`random_index` rather than reaching into `dev.rs`.

use tauri::async_runtime::spawn_blocking;

use super::security;
use super::ToolOutcome;

/// Mirror of `commands.rs`'s private `blocking_outcome` — see the module doc
/// comment for why this is a copy rather than an import.
async fn blocking_outcome<F>(work: F) -> ToolOutcome
where
    F: FnOnce() -> ToolOutcome + Send + 'static,
{
    spawn_blocking(work)
        .await
        .unwrap_or_else(|e| ToolOutcome::err(format!("It could not be run: {e}")))
}

// ---------------------------------------------------------------------------
// 1. Passphrase generator
// ---------------------------------------------------------------------------

/// `words` is optional — omitting it uses `security`'s own default (6 words,
/// ~66 bits), the same "sensible default, not a required field" shape every
/// other generator in this app follows.
#[tauri::command]
pub fn security_generate_passphrase(words: Option<usize>) -> ToolOutcome {
    security::passphrase_outcome(words)
}

// ---------------------------------------------------------------------------
// 2. Clipboard auto-clear
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn security_clipboard_auto_clear(seconds: u64) -> ToolOutcome {
    security::arm_clipboard_auto_clear(seconds)
}

#[tauri::command]
pub fn security_cancel_auto_clear() -> ToolOutcome {
    security::cancel_clipboard_auto_clear()
}

// ---------------------------------------------------------------------------
// 3. Microphone mute
// ---------------------------------------------------------------------------

/// Reads the mic's current mute state. `async` because it shells out to
/// `osascript` under the hood (`security::mic_input_volume`).
#[tauri::command]
pub async fn security_mic_muted() -> Result<bool, String> {
    spawn_blocking(security::mic_muted)
        .await
        .map_err(|e| format!("Could not check the microphone: {e}"))?
}

#[tauri::command]
pub async fn security_set_mic_muted(mute: bool) -> ToolOutcome {
    blocking_outcome(move || security::set_mic_muted(mute)).await
}

// ---------------------------------------------------------------------------
// 4. Camera & microphone activity log
// ---------------------------------------------------------------------------

/// `minutes` is clamped to 1-1440 inside `security::recent_camera_mic_activity`
/// itself, so an out-of-range value here is not a separate error — it is
/// quietly brought back in range, same as the function it wraps.
#[tauri::command]
pub async fn security_activity_log(minutes: u32) -> Result<Vec<security::ActivityEvent>, String> {
    spawn_blocking(move || security::recent_camera_mic_activity(minutes))
        .await
        .map_err(|e| format!("Could not read the activity log: {e}"))?
}

// ---------------------------------------------------------------------------
// 5. Firewall
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn security_firewall_state() -> Result<security::FirewallState, String> {
    spawn_blocking(security::firewall_state)
        .await
        .map_err(|e| format!("Could not check the firewall: {e}"))?
}

/// See `security::open_firewall_settings`'s doc comment for why this opens
/// System Settings instead of flipping the switch itself: changing the
/// firewall needs an admin password, and that password should only ever be
/// typed into Apple's own dialog.
#[tauri::command]
pub async fn security_open_firewall_settings() -> ToolOutcome {
    blocking_outcome(security::open_firewall_settings).await
}

// ---------------------------------------------------------------------------
// 6. TouchID app lock — documented gap
// ---------------------------------------------------------------------------

/// Always `false` today — see `security::touch_id_available`'s doc comment
/// for exactly why. Wrapped anyway (rather than the frontend just assuming
/// "false") so the reason a real answer is unreachable stays in one place,
/// and so a future build that adds the LocalAuthentication binding only has
/// to change `security.rs`, not the frontend.
#[tauri::command]
pub fn security_touch_id_available() -> bool {
    security::touch_id_available()
}

// ---------------------------------------------------------------------------
// 7. File vault
// ---------------------------------------------------------------------------
//
// Both of these run Argon2id (hundreds of milliseconds by design, see
// `security.rs`) plus real file I/O, so both are `async` — a lock/unlock
// that ran on the main thread would beachball the app for the exact duration
// the KDF is supposed to take.

#[tauri::command]
pub async fn security_lock_file(path: String, passphrase: String, delete_original: bool) -> ToolOutcome {
    blocking_outcome(move || security::lock_file_outcome(&path, &passphrase, delete_original)).await
}

#[tauri::command]
pub async fn security_unlock_file(path: String, passphrase: String) -> ToolOutcome {
    blocking_outcome(move || security::unlock_file_outcome(&path, &passphrase)).await
}
