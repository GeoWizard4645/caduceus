//! Global hotkey registration.
//!
//! Three bindings, all rebindable, all optional (an empty string means "no
//! binding"):
//!
//! | binding              | default                     | behaviour            |
//! |----------------------|-----------------------------|----------------------|
//! | toggle the staff       | `F12`                       | on key-down          |
//! | Command Center       | `Alt+Space`                 | on key-down          |
//! | push-to-talk         | `Alt+Shift+V`               | hold to record    |
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

use crate::settings::{FunctionKeyAction, SettingsManager};
use crate::{voice, window};

/// Accelerators tried, in order, when the configured one cannot be registered.
///
/// Every entry is deliberately something no stock macOS install claims. The
/// point is that a fresh user whose preferred key is taken by another app still
/// ends up with a *working* Caduceus rather than a silently dead shortcut.
const COMMAND_CENTER_FALLBACKS: &[&str] = &[
    "Control+Space",
    "Alt+Space",
    "Control+Shift+Space",
    "Alt+Shift+Space",
    "CommandOrControl+Alt+Space",
    "F17",
];

const PUSH_TO_TALK_FALLBACKS: &[&str] = &[
    "Alt+Shift+V",
    "Control+Shift+V",
    "CommandOrControl+Alt+V",
    "F18",
];

const TOGGLE_STAFF_FALLBACKS: &[&str] = &[
    "CommandOrControl+Alt+S",
    "Control+Shift+S",
    "Alt+Shift+S",
];

/// What happened to one binding Caduceus had to move.
#[derive(Debug, Clone)]
pub struct Rebound {
    pub label: String,
    pub wanted: String,
    pub used: String,
}

/// Register every configured hotkey, replacing anything previously registered.
///
/// Called at startup and again whenever settings change, so rebinding takes
/// effect immediately.
///
/// # Never silently losing a binding
///
/// If the configured accelerator is taken by another application, Caduceus
/// moves that action to the first free fallback and **persists** the change,
/// rather than leaving a shortcut that does nothing. A key you chose is only
/// ever replaced when the OS refuses it — the alternative is an app whose
/// documented shortcut does not work and gives no reason why.
pub fn register_all<R: Runtime>(app: &AppHandle<R>, settings: &SettingsManager) -> Vec<String> {
    let mut problems = Vec::new();
    let mut rebound: Vec<Rebound> = Vec::new();

    if let Err(e) = app.global_shortcut().unregister_all() {
        log::warn!("could not clear old hotkeys: {e}");
    }

    let mut cfg = settings.get();
    let mut claimed: Vec<String> = Vec::new();

    // Attempt one accelerator. `Ok(())` means the OS accepted it.
    let try_register = |accelerator: &str| -> Result<(), String> {
        let accelerator = accelerator.trim();
        if accelerator.is_empty() {
            return Err("empty accelerator".into());
        }
        let shortcut = Shortcut::from_str(accelerator).map_err(|e| e.to_string())?;
        app.global_shortcut()
            .register(shortcut)
            .map_err(|e| e.to_string())
    };

    /// Register `wanted`, falling back through `fallbacks` if the OS refuses.
    /// Returns the accelerator that actually took effect.
    macro_rules! bind {
        ($label:expr, $wanted:expr, $fallbacks:expr) => {{
            let wanted = $wanted.trim().to_string();
            if wanted.is_empty() {
                None
            } else if claimed.iter().any(|c| c.eq_ignore_ascii_case(&wanted)) {
                // Two Caduceus actions on one key: the second cannot have it.
                problems.push(format!(
                    "\u{201c}{wanted}\u{201d} is set for more than one Caduceus action; {} was left unbound.",
                    $label
                ));
                None
            } else if try_register(&wanted).is_ok() {
                claimed.push(wanted.clone());
                Some(wanted)
            } else {
                let replacement = $fallbacks.iter().find(|candidate| {
                    !candidate.eq_ignore_ascii_case(&wanted)
                        && !claimed.iter().any(|c| c.eq_ignore_ascii_case(candidate))
                        && try_register(candidate).is_ok()
                });
                match replacement {
                    Some(found) => {
                        claimed.push((*found).to_string());
                        rebound.push(Rebound {
                            label: $label.to_string(),
                            wanted: wanted.clone(),
                            used: (*found).to_string(),
                        });
                        Some((*found).to_string())
                    }
                    None => {
                        problems.push(format!(
                            "\u{201c}{wanted}\u{201d} could not be registered for {}, and no \
                             alternative was free either. Pick a different combination in \
                             Settings \u{2192} General.",
                            $label
                        ));
                        None
                    }
                }
            }
        }};
    }

    if let Some(used) = bind!(
        "Toggle staff",
        cfg.general.toggle_orb_hotkey.clone(),
        TOGGLE_STAFF_FALLBACKS
    ) {
        cfg.general.toggle_orb_hotkey = used;
    }

    if let Some(used) = bind!(
        "Command Center",
        cfg.general.command_center_hotkey.clone(),
        COMMAND_CENTER_FALLBACKS
    ) {
        cfg.general.command_center_hotkey = used;
    }

    if cfg.voice.enabled {
        if let Some(used) = bind!(
            "Push to talk",
            cfg.voice.push_to_talk_hotkey.clone(),
            PUSH_TO_TALK_FALLBACKS
        ) {
            cfg.voice.push_to_talk_hotkey = used;
        }
    }

    // --- function keys ------------------------------------------------------
    //
    // These are positional, so there is no sensible "fallback combination" —
    // the fallback is another *free row* in the same table.
    let free_rows: Vec<String> = cfg
        .general
        .function_keys
        .iter()
        .filter(|b| b.action == FunctionKeyAction::None)
        .map(|b| b.key.clone())
        .collect();
    let mut free_rows = free_rows.into_iter();

    for index in 0..cfg.general.function_keys.len() {
        let binding = &cfg.general.function_keys[index];
        if binding.action == FunctionKeyAction::None {
            continue;
        }
        if binding.action == FunctionKeyAction::PushToTalk && !cfg.voice.enabled {
            continue;
        }
        let key = binding.key.trim().to_string();
        if key.is_empty() {
            continue;
        }
        if claimed.iter().any(|c| c.eq_ignore_ascii_case(&key)) {
            problems.push(format!(
                "\u{201c}{key}\u{201d} is also a dedicated Caduceus hotkey \u{2014} clear it \
                 there, or move this action to another function key."
            ));
            continue;
        }

        if try_register(&key).is_ok() {
            claimed.push(key);
            continue;
        }

        // Taken by another app. Move the action to the first free row that the
        // OS will accept, rather than dropping it.
        let mut moved = None;
        for candidate in free_rows.by_ref() {
            if claimed.iter().any(|c| c.eq_ignore_ascii_case(&candidate)) {
                continue;
            }
            if try_register(&candidate).is_ok() {
                moved = Some(candidate);
                break;
            }
        }

        match moved {
            Some(new_key) => {
                let action = cfg.general.function_keys[index].action;
                let shortcut_id = cfg.general.function_keys[index].shortcut_id.clone();
                cfg.general.function_keys[index].action = FunctionKeyAction::None;
                cfg.general.function_keys[index].shortcut_id = String::new();
                if let Some(row) = cfg
                    .general
                    .function_keys
                    .iter_mut()
                    .find(|b| b.key == new_key)
                {
                    row.action = action;
                    row.shortcut_id = shortcut_id;
                }
                claimed.push(new_key.clone());
                rebound.push(Rebound {
                    label: format!("Function key {key}"),
                    wanted: key,
                    used: new_key,
                });
            }
            None => problems.push(format!(
                "\u{201c}{key}\u{201d} is held by another app and no other function key was \
                 free. Pick one in Settings \u{2192} General."
            )),
        }
    }

    // Persist anything we had to move, so the UI and the next launch agree with
    // what is actually registered.
    if !rebound.is_empty() {
        for r in &rebound {
            problems.push(format!(
                "\u{201c}{}\u{201d} was unavailable, so {} now uses \u{201c}{}\u{201d}.",
                r.wanted, r.label, r.used
            ));
        }
        if let Err(e) = crate::settings::save(app, &cfg) {
            log::error!("could not persist rebound hotkeys: {e}");
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

    // Function-key bindings take precedence over the dedicated PTT hotkey when
    // they share the same accelerator (registration should prevent that).
    if let Some(binding) = cfg
        .general
        .function_keys
        .iter()
        .find(|b| b.action != FunctionKeyAction::None && matches(&b.key))
    {
        if binding.action == FunctionKeyAction::PushToTalk && !cfg.voice.enabled {
            return;
        }
        match event_state {
            ShortcutState::Pressed => crate::fn_keys::dispatch_press(
                app,
                &settings,
                binding.action,
                &binding.shortcut_id,
            ),
            ShortcutState::Released => {
                crate::fn_keys::dispatch_release(app, &settings, binding.action)
            }
        }
        return;
    }

    // --- push-to-talk (hold) ------------------------------------------------
    if cfg.voice.enabled && matches(&cfg.voice.push_to_talk_hotkey) {
        match event_state {
            ShortcutState::Pressed => start_push_to_talk(app, &settings),
            ShortcutState::Released => stop_push_to_talk(app, &settings),
        }
        return;
    }
}

/// Tap-to-toggle dictation: F1, double-click the staff, etc. Hold-to-talk uses
/// [`start_push_to_talk`] / [`stop_push_to_talk`] instead.
pub fn toggle_dictation<R: Runtime>(app: &AppHandle<R>, settings: &SettingsManager) {
    if !settings.with(|s| s.voice.enabled) {
        use tauri::Emitter;
        let _ = app.emit(
            voice::VOICE_RESULT_EVENT,
            VoiceOutcome::error("Voice is off. Turn it on in Settings → Voice."),
        );
        return;
    }

    let Some(runtime) = app.try_state::<voice::VoiceRuntime>() else {
        return;
    };
    if runtime.is_recording() {
        stop_push_to_talk(app, settings);
    } else {
        start_push_to_talk(app, settings);
    }
}

/// Start push-to-talk / dictation capture (shared by the PTT hotkey and function keys).
pub fn start_push_to_talk<R: Runtime>(app: &AppHandle<R>, settings: &SettingsManager) {
    use tauri::Emitter;

    let Some(runtime) = app.try_state::<voice::VoiceRuntime>() else {
        return;
    };
    let app_partial = app.clone();
    match runtime.start(settings, move |text| {
        let _ = app_partial.emit(voice::VOICE_PARTIAL_EVENT, text);
    }) {
        Ok(()) => {
            let _ = window::open_command_center(app, Default::default());
            let _ = app.emit(voice::VOICE_STATE_EVENT, voice::VoiceState::Recording);
        }
        Err(e) => {
            log::error!("could not start recording: {e}");
            let _ = app.emit(voice::VOICE_RESULT_EVENT, VoiceOutcome::error(e));
        }
    }
}

pub fn stop_push_to_talk<R: Runtime>(app: &AppHandle<R>, settings: &SettingsManager) {
    use tauri::Emitter;

    let Some(runtime) = app.try_state::<voice::VoiceRuntime>() else {
        return;
    };
    let Some(outcome) = runtime.stop() else {
        return;
    };

    let app = app.clone();
    let settings = settings.clone();
    tauri::async_runtime::spawn(async move {
        let _ = app.emit(voice::VOICE_STATE_EVENT, voice::VoiceState::Transcribing);

        let result = match outcome {
            voice::StopOutcome::Batch(Ok(wav)) => {
                match voice::transcribe_and_route(wav, &settings).await {
                    Ok(routed) => VoiceOutcome::ok(routed, settings.with(|s| s.voice.auto_submit)),
                    Err(e) => VoiceOutcome::error(e),
                }
            }
            voice::StopOutcome::Batch(Err(e)) => VoiceOutcome::error(e),
            voice::StopOutcome::Live(Ok((text, _))) => {
                let routed = voice::route_transcript(&text, &settings);
                VoiceOutcome::ok(routed, settings.with(|s| s.voice.auto_submit))
            }
            voice::StopOutcome::Live(Err(e)) => VoiceOutcome::error(e),
        };

        let _ = app.emit(voice::VOICE_STATE_EVENT, voice::VoiceState::Idle);
        let _ = app.emit(voice::VOICE_RESULT_EVENT, result);
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
        for a in ["F12", "Alt+Space", "Alt+Shift+V", "F13"] {
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
