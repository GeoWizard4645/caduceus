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

/// Combinations macOS keeps for itself, whatever an app asks for.
///
/// These are the reason a hotkey can look perfectly configured and simply never
/// fire. `RegisterEventHotKey` **succeeds** for `Cmd+Space` — the OS accepts the
/// registration and then routes the key to Spotlight anyway, because the system
/// binding wins. Nothing in the registration result says so, so the only way to
/// avoid shipping a dead shortcut is to know the list.
///
/// Normalised for comparison: modifiers sorted, `CommandOrControl` resolved to
/// `Command` (this is the macOS-only path).
#[cfg(target_os = "macos")]
const SYSTEM_RESERVED: &[&str] = &[
    "command+space",     // Spotlight
    "command+alt+space", // Finder search window
    "command+tab",       // application switcher
    "command+shift+tab", // application switcher, backwards
    "command+`",         // cycle windows in the active app
    "control+up",        // Mission Control
    "control+down",      // application windows
    "control+left",      // move a space left
    "control+right",     // move a space right
    "command+h",         // hide the active app
    "command+q",         // quit the active app
    "command+shift+3",   // screenshot
    "command+shift+4",   // screenshot selection
    "command+shift+5",   // screenshot toolbar
    "command+control+q", // lock screen
    "command+alt+esc",   // force quit
    "fn+f",              // not expressible, but rejected clearly if tried
];

/// Whether macOS will swallow this accelerator before Caduceus ever sees it.
#[cfg(target_os = "macos")]
fn is_system_reserved(accelerator: &str) -> bool {
    let mut parts: Vec<String> = accelerator
        .split('+')
        .map(|part| match part.trim().to_lowercase().as_str() {
            // Tauri's cross-platform alias resolves to Command on macOS.
            "commandorcontrol" | "cmdorctrl" | "cmd" | "super" | "meta" => "command".to_string(),
            "option" | "opt" => "alt".to_string(),
            "ctrl" => "control".to_string(),
            "escape" => "esc".to_string(),
            other => other.to_string(),
        })
        .collect();

    // The key is whatever is not a modifier; sort the modifiers so "Shift+Cmd+3"
    // and "Cmd+Shift+3" compare equal.
    let key = parts.pop().unwrap_or_default();
    parts.sort();
    parts.push(key);
    let normalised = parts.join("+");

    SYSTEM_RESERVED
        .iter()
        .any(|reserved| *reserved == normalised)
}

#[cfg(not(target_os = "macos"))]
fn is_system_reserved(_accelerator: &str) -> bool {
    false
}

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

const TOGGLE_STAFF_FALLBACKS: &[&str] =
    &["CommandOrControl+Alt+S", "Control+Shift+S", "Alt+Shift+S"];

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

    // Attempt one accelerator. `Ok(())` means the key will actually reach us.
    //
    // Registration succeeding is not the same as the key working: macOS accepts
    // a registration for Cmd+Space and then hands the key to Spotlight anyway.
    // A combination the system has claimed is therefore refused *here*, so it
    // falls through to a working alternative instead of shipping a shortcut
    // that silently does nothing.
    let try_register = |accelerator: &str| -> Result<(), String> {
        let accelerator = accelerator.trim();
        if accelerator.is_empty() {
            return Err("empty accelerator".into());
        }
        if is_system_reserved(accelerator) {
            return Err(format!("{accelerator} is reserved by macOS"));
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
            if let Err(e) = window::toggle_command_center(app, "hotkey") {
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
            ShortcutState::Pressed => {
                crate::fn_keys::dispatch_press(app, &settings, binding.action, &binding.shortcut_id)
            }
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
///
/// # Everything here happens off the main thread, and that is the whole point
///
/// Starting a live session is a handshake with a helper process that can
/// legitimately take fifteen seconds — or three minutes on the first run, while
/// macOS holds up its microphone and speech-recognition sheets and waits for a
/// human. Both callers of this function (the global-shortcut handler and the
/// function-key tap) are invoked by Tauri **on the main thread**.
///
/// So the old version, which did that handshake inline, froze the entire
/// application for as long as the helper took. The visible symptom was a
/// spinning beachball with no way out: the staff would not respond, the palette
/// would not open, and the key that would have stopped the recording could not
/// be processed either, because processing it needed the same thread.
///
/// The recording indicator therefore goes up immediately and the handshake runs
/// on a blocking worker. If it fails, the indicator comes down and says why.
pub fn start_push_to_talk<R: Runtime>(app: &AppHandle<R>, settings: &SettingsManager) {
    use tauri::Emitter;

    if app.try_state::<voice::VoiceRuntime>().is_none() {
        return;
    }

    // Optimistic, and correct: `VoiceRuntime::start` claims the slot with an
    // atomic before it blocks, so `is_recording()` is already true and a second
    // press will stop rather than start a second session.
    //
    // The HUD goes up *before* the handshake for the same reason. A microphone
    // that might be live must be visible immediately, and its Stop button is
    // the only thing that reliably ends a session whose helper is misbehaving.
    window::recorder::show(app);
    let _ = app.emit(voice::VOICE_STATE_EVENT, voice::VoiceState::Recording);

    let app = app.clone();
    let settings = settings.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(runtime) = app.try_state::<voice::VoiceRuntime>() else {
            return;
        };
        let app_partial = app.clone();
        let started = runtime.start(&settings, move |text| {
            let _ = app_partial.emit(voice::VOICE_PARTIAL_EVENT, text);
        });

        match started {
            Ok(()) => {}
            Err(e) => {
                log::error!("could not start recording: {e}");
                // Leave the HUD up so the failure is readable — hiding it the
                // instant the handshake fails is how "dictation does nothing"
                // felt. Emit Idle + the error; Recorder.tsx shows the message
                // and Discard closes the HUD. Also open the Command Center so
                // Repair on the microphone permission page is one click away.
                let _ = app.emit(voice::VOICE_STATE_EVENT, voice::VoiceState::Idle);
                let _ = app.emit(voice::VOICE_RESULT_EVENT, VoiceOutcome::error(&e));
                let _ = window::open_command_center(
                    &app,
                    window::CommandCenterOpenPayload {
                        source: "dictation".into(),
                        ..Default::default()
                    },
                );
            }
        }
    });
}

/// Stop capture and transcribe. Also non-blocking, for the same reason.
///
/// `VoiceRuntime::stop` waits on the helper to flush its final transcript, and
/// a helper that has wedged used to hold the main thread for two minutes.
pub fn stop_push_to_talk<R: Runtime>(app: &AppHandle<R>, settings: &SettingsManager) {
    use tauri::Emitter;

    if app.try_state::<voice::VoiceRuntime>().is_none() {
        return;
    }

    let _ = app.emit(voice::VOICE_STATE_EVENT, voice::VoiceState::Transcribing);

    let app = app.clone();
    let settings = settings.clone();
    tauri::async_runtime::spawn(async move {
        let stopped = {
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || {
                app.try_state::<voice::VoiceRuntime>()
                    .and_then(|r| r.stop())
            })
            .await
        };

        let Ok(Some(outcome)) = stopped else {
            // Nothing was running, or the worker itself failed. Either way the
            // indicator must not be left up.
            window::recorder::hide(&app);
            let _ = app.emit(voice::VOICE_STATE_EVENT, voice::VoiceState::Idle);
            return;
        };

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

        window::recorder::hide(&app);
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
    if is_system_reserved(trimmed) {
        return Err(format!(
            "\u{201c}{trimmed}\u{201d} belongs to macOS \u{2014} it would register without error \
             and then never reach Caduceus. Pick something else."
        ));
    }
    Shortcut::from_str(trimmed)
        .map(|_| trimmed.to_string())
        .map_err(|e| format!("\u{201c}{trimmed}\u{201d} is not a valid shortcut: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn combinations_macos_keeps_for_itself_are_recognised() {
        // Every one of these registers successfully and then never fires,
        // which is the whole reason the list exists.
        for reserved in [
            "CommandOrControl+Space",
            "Command+Space",
            "Cmd+Space",
            "Command+Tab",
            "Command+Shift+3",
            "Control+Left",
            "CommandOrControl+Q",
        ] {
            assert!(
                is_system_reserved(reserved),
                "{reserved} should be reserved"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn modifier_order_and_spelling_do_not_hide_a_reserved_combination() {
        // All four spell the same key press.
        for spelling in [
            "Command+Shift+3",
            "Shift+Command+3",
            "Shift+Cmd+3",
            "shift+commandorcontrol+3",
        ] {
            assert!(
                is_system_reserved(spelling),
                "{spelling} should be reserved"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn ordinary_combinations_are_left_alone() {
        for free in [
            "Alt+Space",
            "Control+Space",
            "Alt+Shift+V",
            "F12",
            "F17",
            "CommandOrControl+Alt+Space",
            "Control+Shift+Space",
        ] {
            assert!(!is_system_reserved(free), "{free} should be usable");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn every_command_center_fallback_is_actually_usable() {
        // A fallback list containing a reserved combination would move a broken
        // shortcut to a differently broken one.
        for candidate in COMMAND_CENTER_FALLBACKS
            .iter()
            .chain(PUSH_TO_TALK_FALLBACKS)
            .chain(TOGGLE_STAFF_FALLBACKS)
        {
            assert!(
                !is_system_reserved(candidate),
                "{candidate} is reserved by macOS"
            );
        }
    }

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
