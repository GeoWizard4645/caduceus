//! Global hotkey registration.
//!
//! Three bindings, all rebindable, all optional (an empty string means "no
//! binding"):
//!
//! | binding              | default                     | behaviour            |
//! |----------------------|-----------------------------|----------------------|
//! | toggle the staff       | `F12`                       | on key-down          |
//! | Command Center       | `Alt+Space`                 | on key-down          |
//! | push-to-talk         | `CommandOrControl+Shift+Space` | hold to record    |
//!
//! # Why push-to-talk is not bound to `Fn`
//!
//! On macOS the `Fn`/globe key is intercepted in firmware and by the window
//! server; it never reaches an application as an ordinary key event, so it
//! cannot be registered as a global shortcut. Modifier-only bindings (plain
//! Right Option) are not expressible either — every global shortcut needs a
//! non-modifier key. The default is therefore a normal combination; `F13`–`F20`
//! are good single-key alternatives on keyboards that have them.

use std::str::FromStr;

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::settings::SettingsManager;
use crate::{voice, window};

/// Register every configured hotkey, replacing anything previously registered.
///
/// Called at startup and again whenever settings change, so rebinding takes
/// effect immediately.
pub fn register_all<R: Runtime>(app: &AppHandle<R>, settings: &SettingsManager) -> Vec<String> {
    let mut problems = Vec::new();

    if let Err(e) = app.global_shortcut().unregister_all() {
        log::warn!("could not clear old hotkeys: {e}");
    }

    let cfg = settings.get();
    for (label, accelerator) in [
        ("Toggle staff", cfg.general.toggle_orb_hotkey.as_str()),
        ("Command Center", cfg.general.command_center_hotkey.as_str()),
        (
            "Push to talk",
            if cfg.voice.enabled {
                cfg.voice.push_to_talk_hotkey.as_str()
            } else {
                ""
            },
        ),
    ] {
        let accelerator = accelerator.trim();
        if accelerator.is_empty() {
            continue;
        }

        match Shortcut::from_str(accelerator) {
            Ok(shortcut) => {
                if let Err(e) = app.global_shortcut().register(shortcut) {
                    // Almost always "another app already owns this combination".
                    problems.push(format!(
                        "\u{201c}{accelerator}\u{201d} could not be registered for {label} \
                         \u{2014} another app is probably using it. ({e})"
                    ));
                }
            }
            Err(e) => problems.push(format!(
                "\u{201c}{accelerator}\u{201d} is not a valid shortcut for {label}: {e}"
            )),
        }
    }

    for p in &problems {
        log::warn!("{p}");
    }
    problems
}

/// The single handler for every global hotkey.
///
/// Registered once via the plugin builder; it dispatches by comparing the fired
/// shortcut against the current settings, so rebinding needs no re-plumbing.
pub fn handle<R: Runtime>(app: &AppHandle<R>, shortcut: &Shortcut, event_state: ShortcutState) {
    let Some(settings) = app.try_state::<SettingsManager>() else {
        return;
    };
    let settings = settings.inner().clone();
    let cfg = settings.get();

    let matches = |accelerator: &str| {
        !accelerator.trim().is_empty()
            && Shortcut::from_str(accelerator.trim())
                .map(|s| &s == shortcut)
                .unwrap_or(false)
    };

    // --- press-only bindings ------------------------------------------------
    if event_state == ShortcutState::Pressed {
        if matches(&cfg.general.toggle_orb_hotkey) {
            if let Err(e) = window::toggle_staff(app, &settings) {
                log::error!("hotkey could not toggle the staff: {e}");
            }
            crate::tray::refresh(app);
            return;
        }

        if matches(&cfg.general.command_center_hotkey) {
            if let Err(e) = window::toggle_command_center(app) {
                log::error!("hotkey could not open the Command Center: {e}");
            }
            return;
        }
    }

    // --- push-to-talk (hold) ------------------------------------------------
    if cfg.voice.enabled && matches(&cfg.voice.push_to_talk_hotkey) {
        match event_state {
            ShortcutState::Pressed => start_recording(app, &settings),
            ShortcutState::Released => stop_and_transcribe(app, &settings),
        }
    }
}

fn start_recording<R: Runtime>(app: &AppHandle<R>, settings: &SettingsManager) {
    use tauri::Emitter;

    let Some(runtime) = app.try_state::<voice::VoiceRuntime>() else {
        return;
    };
    // Repeat key-down events while the key is held are ignored by
    // `VoiceRuntime::start`.
    match runtime.start(settings) {
        Ok(()) => {
            // Show the palette so there is somewhere for the transcript to land
            // and something on screen confirming the mic is open.
            let _ = window::open_command_center(app, Default::default());
            let _ = app.emit(voice::VOICE_STATE_EVENT, voice::VoiceState::Recording);
        }
        Err(e) => {
            log::error!("could not start recording: {e}");
            let _ = app.emit(voice::VOICE_RESULT_EVENT, VoiceOutcome::error(e));
        }
    }
}

fn stop_and_transcribe<R: Runtime>(app: &AppHandle<R>, settings: &SettingsManager) {
    use tauri::Emitter;

    let Some(runtime) = app.try_state::<voice::VoiceRuntime>() else {
        return;
    };
    let Some(result) = runtime.stop() else {
        return;
    };

    let app = app.clone();
    let settings = settings.clone();
    tauri::async_runtime::spawn(async move {
        let _ = app.emit(voice::VOICE_STATE_EVENT, voice::VoiceState::Transcribing);

        let outcome = match result {
            Err(e) => VoiceOutcome::error(e),
            Ok(wav) => match voice::transcribe_and_route(wav, &settings).await {
                Ok(routed) => VoiceOutcome::ok(routed, settings.with(|s| s.voice.auto_submit)),
                Err(e) => VoiceOutcome::error(e),
            },
        };

        let _ = app.emit(voice::VOICE_STATE_EVENT, voice::VoiceState::Idle);
        let _ = app.emit(voice::VOICE_RESULT_EVENT, outcome);
    });
}

/// What the frontend receives when a push-to-talk cycle finishes.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceOutcome {
    pub ok: bool,
    pub error: Option<String>,
    pub routed: Option<voice::RoutedText>,
    /// Whether the frontend should act on the routed text straight away, or
    /// just fill the input and wait for Enter.
    pub auto_submit: bool,
}

impl VoiceOutcome {
    fn ok(routed: voice::RoutedText, auto_submit: bool) -> Self {
        Self {
            ok: true,
            error: None,
            routed: Some(routed),
            auto_submit,
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(message.into()),
            routed: None,
            auto_submit: false,
        }
    }
}

/// Validate an accelerator string without registering it, for the Settings UI.
pub fn validate(accelerator: &str) -> Result<String, String> {
    let trimmed = accelerator.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    Shortcut::from_str(trimmed)
        .map(|_| trimmed.to_string())
        .map_err(|e| format!("\u{201c}{trimmed}\u{201d} is not a valid shortcut: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_shipped_defaults() {
        for a in ["F12", "Alt+Space", "CommandOrControl+Shift+Space", "F13"] {
            assert!(validate(a).is_ok(), "{a} should be valid");
        }
    }

    #[test]
    fn an_empty_binding_means_unbound_not_invalid() {
        assert_eq!(validate("").unwrap(), "");
        assert_eq!(validate("   ").unwrap(), "");
    }

    #[test]
    fn rejects_nonsense() {
        assert!(validate("NotAKey").is_err());
        // Modifier-only bindings are not expressible as global shortcuts.
        assert!(validate("Shift").is_err());
    }
}
