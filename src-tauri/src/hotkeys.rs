//! Global hotkey registration.
//!
//! Three bindings, all rebindable, all optional (an empty string means "no
//! binding"):
//!
//! | binding           | default                | behaviour         |
//! |-------------------|-------------------------|-------------------|
//! | toggle the staff  | empty (see below)      | on key-down       |
//! | Command Center    | `Control+Space`         | on key-down       |
//! | push-to-talk      | `Alt+Shift+V`           | hold to record    |
//!
//! `toggle_orb_hotkey` itself defaults to empty, not `F12`: the staff already
//! toggles on `F12` out of the box via the function-key table in
//! `settings::model` (`default_function_key_bindings`), which is the one place
//! F1–F20 are configured. This field is for a *second*, non-F-key accelerator
//! layered on top of that — ⌥⇧S, say — not a replacement for it.
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
use std::sync::Arc;

use parking_lot::RwLock;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::settings::{FunctionKeyAction, FunctionKeyBinding, SettingsManager};
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
/// Every entry must already be in the normalised form `is_system_reserved`
/// produces — **modifiers sorted alphabetically, key last** — or it silently
/// never matches. `alt` sorts before `command`, which sorts before `control`,
/// which sorts before `shift`. `"command+alt+space"` sat here for a long time
/// doing nothing for exactly that reason; it only started matching when it was
/// written `"alt+command+space"`. The test below walks this table through the
/// normaliser so a future entry cannot go stale the same way.
#[cfg(target_os = "macos")]
const SYSTEM_RESERVED: &[&str] = &[
    "command+space",     // Spotlight
    "alt+command+space", // Finder search window
    // Input-source switching. Enabled on a stock install and *not* something
    // the user has to have set up: macOS ships these bound, and they win over
    // an application's registration exactly the way Spotlight wins Cmd+Space.
    // This is not hypothetical — Control+Space was this app's own default
    // Command Center accelerator, and on any Mac with more than one input
    // source (four, on the machine this was found) it could never once have
    // worked. `RegisterEventHotKey` accepted it and the system ate the key.
    "control+space",     // select the previous input source
    "alt+control+space", // select the next input source
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
    "alt+command+esc",   // force quit
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
/// Two entries were removed here once `SYSTEM_RESERVED` learned about
/// input-source switching and its own sorting bug: `Control+Space` (macOS's
/// previous-input-source key) and `CommandOrControl+Alt+Space`, which resolves
/// to `Alt+Command+Space` on macOS — the Finder search window. Both had been
/// sitting in a list whose entire purpose is "combinations that definitely
/// work", and `every_command_center_fallback_is_actually_usable` now proves
/// the rest do.
const COMMAND_CENTER_FALLBACKS: &[&str] = &[
    "Alt+Space",
    "Control+Shift+Space",
    "Alt+Shift+Space",
    "CommandOrControl+Shift+Space",
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

/// Plain-English explanation of one rebind, naming both the key that was
/// wanted and the key actually in effect.
///
/// Pulled out to its own function so the wording is covered by a test
/// independent of registering a real hotkey. Deliberately says outright that
/// Settings did not change: the old behaviour silently persisted the
/// fallback, so "session only" is the one fact this message must never leave
/// implied.
fn rebind_message(r: &Rebound) -> String {
    format!(
        "\u{201c}{}\u{201d} is unavailable right now, so {} is using \u{201c}{}\u{201d} for this \
         session instead. Nothing was changed in Settings, so \u{201c}{}\u{201d} will be used \
         again once it is free.",
        r.wanted, r.label, r.used, r.wanted
    )
}

/// One binding's health: what `Settings` has saved versus what the OS is
/// actually holding right now.
///
/// The two only ever disagree for one reason: the configured accelerator lost
/// a race to something else, and [`register_all`] fell back to a free key
/// *for this run* rather than leaving the binding dead. `configured` is what
/// will be tried again next launch; `active` is what a key press actually
/// does right now.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyHealth {
    /// Human-readable action name, e.g. "Command Center" or "Function key F5".
    pub label: String,
    /// Saved in Settings. Empty means the binding is unset.
    pub configured: String,
    /// Actually registered with the OS this session. `None` covers both
    /// "unset" and "configured, but nothing could be registered for it" —
    /// `configured` is what tells those two apart.
    pub active: Option<String>,
    /// `true` when `active` is a fallback rather than `configured` itself.
    pub rebound: bool,
}

/// What is actually registered with the OS for each binding right now.
///
/// Deliberately not part of `Settings`. Its only reason to exist is to hold
/// the one piece of state a fallback must never reach: the accelerator
/// [`register_all`] actually managed to register this session, for whichever
/// bindings that differs from what the user configured. [`handle`] matches
/// against this instead of `Settings` so a fallback actually works, and
/// [`hotkey_health`] reads it to show the gap, if any, in Settings.
///
/// `Clone` around an inner `Arc<RwLock<_>>`, the same shape as
/// `SettingsManager`, for the same reason: command handlers and the
/// global-shortcut callback each need their own handle to the one piece of
/// state, and none of them holds `&mut App`.
#[derive(Clone, Default)]
pub struct HotkeyRuntime {
    inner: Arc<RwLock<Active>>,
}

/// The part of [`HotkeyRuntime`] that actually changes, split out so a
/// snapshot can be cloned and read without holding the lock.
#[derive(Debug, Clone, Default)]
struct Active {
    toggle_staff: Option<String>,
    command_center: Option<String>,
    push_to_talk: Option<String>,
    /// Same shape as `Settings.general.function_keys`, but with any row
    /// `register_all` had to move to a free key already reflecting where it
    /// actually landed — `handle` reads this table exactly as it would read
    /// the configured one.
    function_keys: Vec<FunctionKeyBinding>,
    health: Vec<HotkeyHealth>,
}

impl HotkeyRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    fn set(&self, active: Active) {
        *self.inner.write() = active;
    }

    fn snapshot(&self) -> Active {
        self.inner.read().clone()
    }

    /// The accelerator the Command Center is *actually* reachable on right
    /// now, or `None` if it has no working binding at all.
    ///
    /// Exists for the tray menu. The menu used to render
    /// `Settings.general.command_center_hotkey`, which is the key the user
    /// asked for — and since [`register_all`] stopped writing fallbacks back
    /// into settings, that is no longer necessarily the key that works. A menu
    /// item advertising a shortcut that does nothing is worse than one
    /// advertising none, so the tray reads this instead.
    pub fn active_command_center(&self) -> Option<String> {
        self.inner.read().command_center.clone()
    }
}

/// Current hotkey health, for a status line in Settings: what each binding is
/// configured to use, and what it is actually running on right now.
///
/// Just a read of [`HotkeyRuntime`], which [`register_all`] refreshes on
/// every registration attempt — startup, and again whenever settings are
/// saved — so this is always as fresh as the last save, not a fresh
/// registration attempt of its own.
#[tauri::command]
pub fn hotkey_health(runtime: tauri::State<'_, HotkeyRuntime>) -> Vec<HotkeyHealth> {
    runtime.snapshot().health
}

/// Register every configured hotkey, replacing anything previously registered.
///
/// Called at startup and again whenever settings change, so rebinding takes
/// effect immediately.
///
/// # Falling back without rewriting what you asked for
///
/// If the configured accelerator is taken by another application, Caduceus
/// moves that action to the first free fallback **for this run**, rather than
/// leaving a shortcut that silently does nothing. What it no longer does is
/// write that fallback back into Settings — doing so is exactly what once
/// turned a user's `Control+Space` into something else with nothing on screen
/// to explain it, and the "something else" holding the key was very often a
/// second copy of Caduceus itself (see `tauri_plugin_single_instance` in
/// `lib.rs::run`). A key the user chose is only ever *tried* against
/// something else, never overwritten by it: every substitution is reported in
/// the returned `problems`, naming both the key that was wanted and the key
/// actually in effect (see [`rebind_message`]), and recorded in
/// [`HotkeyRuntime`] so [`handle`] can still act on it and [`hotkey_health`]
/// can show it.
pub fn register_all<R: Runtime>(app: &AppHandle<R>, settings: &SettingsManager) -> Vec<String> {
    let mut problems = Vec::new();
    let mut rebound: Vec<Rebound> = Vec::new();
    let mut health: Vec<HotkeyHealth> = Vec::new();

    if let Err(e) = app.global_shortcut().unregister_all() {
        log::warn!("could not clear old hotkeys: {e}");
    }

    // Read-only from here on: nothing below writes back through `settings` —
    // see the doc comment above for why a fallback must not outlive this
    // process.
    let cfg = settings.get();
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
    /// Returns the accelerator that actually took effect this session — never
    /// written back to `Settings`; `health` is what tells the UI about it.
    macro_rules! bind {
        ($label:expr, $wanted:expr, $fallbacks:expr) => {{
            let label = $label.to_string();
            let wanted = $wanted.trim().to_string();
            if wanted.is_empty() {
                health.push(HotkeyHealth {
                    label,
                    configured: String::new(),
                    active: None,
                    rebound: false,
                });
                None
            } else if claimed.iter().any(|c| c.eq_ignore_ascii_case(&wanted)) {
                // Two Caduceus actions on one key: the second cannot have it.
                problems.push(format!(
                    "\u{201c}{wanted}\u{201d} is set for more than one Caduceus action; {label} was left unbound."
                ));
                health.push(HotkeyHealth {
                    label,
                    configured: wanted,
                    active: None,
                    rebound: false,
                });
                None
            } else if try_register(&wanted).is_ok() {
                claimed.push(wanted.clone());
                health.push(HotkeyHealth {
                    label,
                    configured: wanted.clone(),
                    active: Some(wanted.clone()),
                    rebound: false,
                });
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
                            label: label.clone(),
                            wanted: wanted.clone(),
                            used: (*found).to_string(),
                        });
                        health.push(HotkeyHealth {
                            label,
                            configured: wanted,
                            active: Some((*found).to_string()),
                            rebound: true,
                        });
                        Some((*found).to_string())
                    }
                    None => {
                        problems.push(format!(
                            "\u{201c}{wanted}\u{201d} could not be registered for {label}, and no \
                             alternative was free either. Pick a different combination in \
                             Settings \u{2192} General."
                        ));
                        health.push(HotkeyHealth {
                            label,
                            configured: wanted,
                            active: None,
                            rebound: false,
                        });
                        None
                    }
                }
            }
        }};
    }

    let toggle_staff_active = bind!(
        "Toggle staff",
        cfg.general.toggle_orb_hotkey.clone(),
        TOGGLE_STAFF_FALLBACKS
    );

    let command_center_active = bind!(
        "Command Center",
        cfg.general.command_center_hotkey.clone(),
        COMMAND_CENTER_FALLBACKS
    );

    let push_to_talk_active = if cfg.voice.enabled {
        bind!(
            "Push to talk",
            cfg.voice.push_to_talk_hotkey.clone(),
            PUSH_TO_TALK_FALLBACKS
        )
    } else {
        None
    };

    // --- function keys ------------------------------------------------------
    //
    // These are positional, so there is no sensible "fallback combination" —
    // the fallback is another *free row* in the same table. As above, a move
    // only ever changes `effective_function_keys`, a scratch copy that ends up
    // in `HotkeyRuntime` — never the configured table itself.
    let mut effective_function_keys = cfg.general.function_keys.clone();
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
            health.push(HotkeyHealth {
                label: format!("Function key {key}"),
                configured: key,
                active: None,
                rebound: false,
            });
            continue;
        }

        if try_register(&key).is_ok() {
            claimed.push(key.clone());
            health.push(HotkeyHealth {
                label: format!("Function key {key}"),
                configured: key.clone(),
                active: Some(key),
                rebound: false,
            });
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
                // Move the action in the *effective* table only — Settings
                // keeps the row where the user put it; only this session's
                // registration (and dispatch table) moves. See the module
                // docs on `register_all`.
                effective_function_keys[index].action = FunctionKeyAction::None;
                effective_function_keys[index].shortcut_id = String::new();
                if let Some(row) = effective_function_keys
                    .iter_mut()
                    .find(|b| b.key == new_key)
                {
                    row.action = action;
                    row.shortcut_id = shortcut_id;
                }
                claimed.push(new_key.clone());
                rebound.push(Rebound {
                    label: format!("Function key {key}"),
                    wanted: key.clone(),
                    used: new_key.clone(),
                });
                health.push(HotkeyHealth {
                    label: format!("Function key {key}"),
                    configured: key,
                    active: Some(new_key),
                    rebound: true,
                });
            }
            None => {
                problems.push(format!(
                    "\u{201c}{key}\u{201d} is held by another app and no other function key was \
                     free. Pick one in Settings \u{2192} General."
                ));
                health.push(HotkeyHealth {
                    label: format!("Function key {key}"),
                    configured: key,
                    active: None,
                    rebound: false,
                });
            }
        }
    }

    // Explain anything moved to a fallback in plain language, naming both the
    // key that was wanted and the key actually in effect. Nothing here calls
    // `settings::save` — the whole point of a fallback is that it does not
    // become the new truth just because it was needed once.
    for r in &rebound {
        problems.push(rebind_message(r));
    }

    if let Some(runtime) = app.try_state::<HotkeyRuntime>() {
        runtime.set(Active {
            toggle_staff: toggle_staff_active,
            command_center: command_center_active,
            push_to_talk: push_to_talk_active,
            function_keys: effective_function_keys,
            health,
        });
    }

    for p in &problems {
        log::warn!("{p}");
    }
    problems
}

/// The single handler for every global hotkey.
///
/// Registered once via the plugin builder; it dispatches by comparing the
/// fired shortcut against what [`register_all`] actually registered, so
/// rebinding needs no re-plumbing.
///
/// Matches against [`HotkeyRuntime`] rather than raw settings on purpose: the
/// two differ exactly when the configured accelerator lost a race to another
/// app, and it is the fallback the OS is delivering here, not necessarily
/// whatever `Settings` still has written down. See `register_all`'s doc
/// comment for why that gap is deliberate.
pub fn handle<R: Runtime>(app: &AppHandle<R>, shortcut: &Shortcut, event_state: ShortcutState) {
    let Some(settings) = app.try_state::<SettingsManager>() else {
        return;
    };
    let settings = settings.inner().clone();
    let cfg = settings.get();

    let Some(runtime) = app.try_state::<HotkeyRuntime>() else {
        return;
    };
    let active = runtime.snapshot();

    let matches = |accelerator: &str| {
        !accelerator.trim().is_empty()
            && Shortcut::from_str(accelerator.trim())
                .map(|s| &s == shortcut)
                .unwrap_or(false)
    };

    // --- press-only bindings ------------------------------------------------
    if event_state == ShortcutState::Pressed {
        if matches(active.toggle_staff.as_deref().unwrap_or_default()) {
            if let Err(e) = window::toggle_staff(app, &settings) {
                log::error!("hotkey could not toggle the staff: {e}");
            }
            crate::tray::refresh(app);
            return;
        }

        if matches(active.command_center.as_deref().unwrap_or_default()) {
            if let Err(e) = window::toggle_command_center(app, "hotkey") {
                log::error!("hotkey could not open the Command Center: {e}");
            }
            return;
        }
    }

    // Function-key bindings take precedence over the dedicated PTT hotkey when
    // they share the same accelerator (registration should prevent that).
    if let Some(binding) = active
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
    if cfg.voice.enabled && matches(active.push_to_talk.as_deref().unwrap_or_default()) {
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
        // `Control+Space` and `CommandOrControl+Alt+Space` used to be asserted
        // here as free. Neither is: the first is input-source switching, the
        // second resolves to Alt+Command+Space (Finder search) on macOS. They
        // are covered by `input_source_switching_shortcuts_are_treated_as_reserved`
        // and `combinations_macos_keeps_for_itself_are_recognised` instead.
        for free in [
            "Alt+Space",
            "Alt+Shift+V",
            "F12",
            "F17",
            "Control+Shift+Space",
            "CommandOrControl+Shift+Space",
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

    // -- system-reserved accelerators ------------------------------------

    /// Every entry has to survive its own normaliser.
    ///
    /// An entry whose modifiers are not already sorted matches nothing and
    /// fails completely silently — the accelerator is simply never recognised
    /// as reserved, and the app ships a shortcut the OS eats. That is not a
    /// hypothetical: `"command+alt+space"` sat in this table doing nothing
    /// until it was rewritten as `"alt+command+space"`.
    #[cfg(target_os = "macos")]
    #[test]
    fn every_reserved_entry_is_written_in_its_own_normalised_form() {
        for entry in SYSTEM_RESERVED {
            assert!(
                is_system_reserved(entry),
                "{entry:?} does not normalise to itself, so it can never match \
                 — sort the modifiers alphabetically, key last"
            );
        }
    }

    /// The bug that made the shipped default unusable.
    ///
    /// `Ctrl+Space` is macOS's *select the previous input source* shortcut and
    /// is enabled out of the box, so on any Mac with more than one input
    /// source the system swallows it — while `RegisterEventHotKey` still
    /// reports success. The old default was exactly this combination.
    #[cfg(target_os = "macos")]
    #[test]
    fn input_source_switching_shortcuts_are_treated_as_reserved() {
        for spelling in ["Control+Space", "Ctrl+Space", "control+space"] {
            assert!(is_system_reserved(spelling), "{spelling} must be reserved");
        }
        // The "next source" variant, in the spellings a user might type.
        for spelling in ["Control+Option+Space", "Ctrl+Alt+Space", "Option+Control+Space"] {
            assert!(is_system_reserved(spelling), "{spelling} must be reserved");
        }
    }

    /// A default the OS eats is worse than no default: the app looks configured
    /// and its only entry point does nothing.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_shipped_default_accelerator_is_not_one_macos_claims() {
        assert!(
            !is_system_reserved(crate::settings::DEFAULT_COMMAND_CENTER_HOTKEY),
            "the default Command Center hotkey ({}) is swallowed by macOS",
            crate::settings::DEFAULT_COMMAND_CENTER_HOTKEY
        );
    }

    // -- rebind_message -------------------------------------------------

    /// The exact requirement a silent settings rewrite used to violate: name
    /// both keys, and say plainly that Settings did not change. A regression
    /// here is a regression of the bug this file exists to fix.
    #[test]
    fn a_rebind_message_names_both_keys_and_says_settings_did_not_change() {
        let message = rebind_message(&Rebound {
            label: "Command Center".into(),
            wanted: "Control+Space".into(),
            used: "Alt+Space".into(),
        });

        assert!(
            message.contains("Control+Space"),
            "must name the key the user wanted: {message}"
        );
        assert!(
            message.contains("Alt+Space"),
            "must name the key actually in effect: {message}"
        );
        assert!(
            message.contains("Command Center"),
            "must name which action moved: {message}"
        );
        assert!(
            message.contains("Nothing was changed in Settings"),
            "must not read like the old silent rewrite: {message}"
        );
    }

    #[test]
    fn a_function_key_rebind_message_reads_the_same_way() {
        let message = rebind_message(&Rebound {
            label: "Function key F5".into(),
            wanted: "F5".into(),
            used: "F9".into(),
        });
        assert!(message.contains("F5"));
        assert!(message.contains("F9"));
        assert!(message.contains("Nothing was changed in Settings"));
    }

    // -- HotkeyRuntime ----------------------------------------------------

    /// `register_all` never has a real `AppHandle` to test against here, but
    /// the state it hands off to `handle` and `hotkey_health` is a plain
    /// `Arc<RwLock<_>>` underneath, and that plumbing — a fresh runtime starts
    /// empty, `set` replaces the whole snapshot, `snapshot` reads the latest
    /// one back — is exactly what would silently break `handle` if it ever
    /// stopped matching fallbacks correctly.
    #[test]
    fn hotkey_runtime_hands_back_whatever_was_last_set() {
        let runtime = HotkeyRuntime::new();
        let empty = runtime.snapshot();
        assert!(empty.toggle_staff.is_none());
        assert!(empty.health.is_empty());

        runtime.set(Active {
            toggle_staff: None,
            command_center: Some("Alt+Space".into()),
            push_to_talk: None,
            function_keys: Vec::new(),
            health: vec![HotkeyHealth {
                label: "Command Center".into(),
                configured: "Control+Space".into(),
                active: Some("Alt+Space".into()),
                rebound: true,
            }],
        });

        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.command_center.as_deref(), Some("Alt+Space"));
        assert_eq!(snapshot.health.len(), 1);
        assert!(snapshot.health[0].rebound);
        assert_eq!(snapshot.health[0].configured, "Control+Space");
    }
}
