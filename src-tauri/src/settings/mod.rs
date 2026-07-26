//! Settings persistence.
//!
//! The whole [`Settings`] tree is stored as a single JSON value under the key
//! `"settings"` in a `tauri-plugin-store` file. Keeping it as one blob (rather
//! than a key per field) means reads are atomic and the frontend can round-trip
//! the entire config in one IPC call, which is what the Settings window does.
//!
//! Secrets never enter this file — see [`secrets`].

pub mod model;
pub mod secrets;

pub use model::*;

use std::sync::Arc;

use parking_lot::RwLock;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_store::StoreExt;

/// Filename inside the app config directory.
pub const STORE_FILE: &str = "caduceus-settings.json";
const SETTINGS_KEY: &str = "settings";

/// Event emitted to every window whenever settings change, so open windows
/// re-render without polling.
pub const SETTINGS_CHANGED_EVENT: &str = "caduceus://settings-changed";

/// In-memory cache of the config, shared by every subsystem.
///
/// Background workers (clipboard watcher, cursor tracker, agent runner) read
/// this on every tick rather than holding their own copies, so a settings change
/// takes effect immediately without restarting anything.
#[derive(Clone)]
pub struct SettingsManager {
    inner: Arc<RwLock<Settings>>,
}

impl SettingsManager {
    pub fn new(initial: Settings) -> Self {
        Self {
            inner: Arc::new(RwLock::new(initial)),
        }
    }

    pub fn get(&self) -> Settings {
        self.inner.read().clone()
    }

    /// Read a projection of the settings without cloning the whole tree.
    /// Preferred inside hot loops.
    pub fn with<T>(&self, f: impl FnOnce(&Settings) -> T) -> T {
        f(&self.inner.read())
    }

    fn replace(&self, next: Settings) {
        *self.inner.write() = next;
    }
}

/// Load settings from disk, falling back to defaults on a missing or corrupt
/// file. A corrupt file is backed up rather than deleted so nothing is lost.
pub fn load<R: Runtime>(app: &AppHandle<R>) -> Settings {
    let store = match app.store(STORE_FILE) {
        Ok(s) => s,
        Err(e) => {
            log::error!("could not open settings store, using defaults: {e}");
            return Settings::default();
        }
    };

    let Some(raw) = store.get(SETTINGS_KEY) else {
        log::info!("no settings found; writing defaults");
        let defaults = Settings::default();
        store.set(SETTINGS_KEY, serde_json::to_value(&defaults).unwrap_or_default());
        let _ = store.save();
        return defaults;
    };

    match serde_json::from_value::<Settings>(raw.clone()) {
        Ok(mut s) => {
            migrate(&mut s);
            s
        }
        Err(e) => {
            log::error!("settings file is not readable ({e}); backing it up and starting fresh");
            backup_corrupt_settings(app, &raw);
            Settings::default()
        }
    }
}

/// Write settings to disk and notify every window.
pub fn save<R: Runtime>(app: &AppHandle<R>, settings: &Settings) -> Result<(), String> {
    let store = app
        .store(STORE_FILE)
        .map_err(|e| format!("could not open settings store: {e}"))?;
    let value = serde_json::to_value(settings).map_err(|e| format!("could not encode settings: {e}"))?;
    store.set(SETTINGS_KEY, value);
    store.save().map_err(|e| format!("could not write settings: {e}"))?;

    if let Some(mgr) = app.try_state::<SettingsManager>() {
        mgr.replace(settings.clone());
    }

    use tauri::Emitter;
    let _ = app.emit(SETTINGS_CHANGED_EVENT, settings);
    Ok(())
}

/// Apply forward-compatible fixups to a config loaded from an older version.
///
/// `#[serde(default)]` already handles *added* fields. This function handles
/// the cases where a default alone would leave the app in a broken state.
fn migrate(s: &mut Settings) {
    // A config that predates the Null backend, or one whose backend list was
    // emptied by hand, would leave every AI route unresolvable.
    if s.agents.backends.is_empty() {
        s.agents.backends = default_backends();
    }
    if s.agents
        .primary_backend_id
        .as_ref()
        .is_none_or(|id| !s.agents.backends.iter().any(|b| &b.id == id))
    {
        s.agents.primary_backend_id = s.agents.backends.first().map(|b| b.id.clone());
    }
    // A computer-use backend that no longer exists (or lost the capability)
    // must not stay selected.
    if let Some(id) = s.agents.computer_use_backend_id.clone() {
        let still_valid = s
            .agents
            .backends
            .iter()
            .any(|b| b.id == id && b.supports_computer_use);
        if !still_valid {
            s.agents.computer_use_backend_id = None;
        }
    }

    // Clamp anything a hand-edited file could put out of range.
    // Floor of 0 ("fold back the moment the pointer leaves"), not 500: the
    // default is 50ms, and a floor above the default would silently rewrite it
    // on every load.
    s.general.collapse_idle_ms = s.general.collapse_idle_ms.min(60_000);
    s.general.cursor_poll_ms = s.general.cursor_poll_ms.clamp(8, 500);
    s.clipboard.poll_interval_ms = s.clipboard.poll_interval_ms.clamp(100, 10_000);
    s.clipboard.max_items = s.clipboard.max_items.clamp(10, 100_000);
    s.appearance.staff_size = s.appearance.staff_size.clamp(36, 120);
    s.appearance.popout_radius = s.appearance.popout_radius.clamp(56, 132);
    s.appearance.popout_icon_size = s.appearance.popout_icon_size.clamp(24, 52);
    s.appearance.staff_idle_opacity = s.appearance.staff_idle_opacity.clamp(0.15, 1.0);
    s.voice.max_recording_secs = s.voice.max_recording_secs.clamp(3, 600);

    // Claude should open the desktop app, not the website.
    for shortcut in &mut s.shortcuts {
        if shortcut.id == "sc-claude"
            && (shortcut.target == "https://claude.ai"
                || shortcut.target.starts_with("https://claude.ai/"))
        {
            shortcut.kind = crate::shortcuts::ShortcutKind::OpenApp;
            shortcut.target = default_claude_desktop_target().into();
            shortcut.description = "Open the Claude desktop app".into();
            if shortcut.icon == "✳" || shortcut.icon.is_empty() {
                shortcut.icon = "glyph:chat".into();
            }
        }

        // `brand:*` was a set of hand-redrawn third-party logos, replaced by the
        // neutral glyph family. Rewritten wherever it appears — not just on the
        // default shortcuts — because a user could have picked one for their own.
        if let Some(brand) = shortcut.icon.strip_prefix("brand:") {
            shortcut.icon = match brand {
                "chrome" => "glyph:globe",
                "gmail" => "glyph:mail",
                "gemini" => "glyph:sparkle",
                "claude" => "glyph:chat",
                "clipboard" => "glyph:clipboard",
                _ => "glyph:star",
            }
            .into();
        }

        if shortcut.id.starts_with("sc-") && shortcut.icon.chars().count() <= 2 {
            // Upgrade the older emoji defaults to the glyph family.
            let glyph = match shortcut.id.as_str() {
                "sc-gemini" => Some("glyph:sparkle"),
                "sc-gmail" => Some("glyph:mail"),
                "sc-chrome" => Some("glyph:globe"),
                "sc-claude" => Some("glyph:chat"),
                "sc-clipboard" => Some("glyph:clipboard"),
                _ => None,
            };
            if let Some(token) = glyph {
                shortcut.icon = token.into();
            }
        }
    }

    if s.voice.enabled && s.voice.stt_backend == crate::settings::SttBackendKind::Disabled {
        s.voice.stt_backend = crate::settings::SttBackendKind::SystemNative;
    }
    if s.voice.push_to_talk_hotkey == "CommandOrControl+Shift+Space" {
        s.voice.push_to_talk_hotkey = "Alt+Shift+V".into();
    }

    if s.general.function_keys.is_empty() {
        s.general.function_keys = crate::settings::default_function_key_bindings();
    } else {
        merge_function_key_bindings(&mut s.general.function_keys);
    }

    s.version = Settings::CURRENT_VERSION;
}

/// Ensure every `F1`–`F20` row exists so the Settings UI stays stable across upgrades.
fn merge_function_key_bindings(bindings: &mut Vec<FunctionKeyBinding>) {
    for label in crate::settings::FUNCTION_KEY_LABELS {
        if !bindings.iter().any(|b| b.key.eq_ignore_ascii_case(label)) {
            bindings.push(FunctionKeyBinding {
                key: (*label).into(),
                action: FunctionKeyAction::None,
                shortcut_id: String::new(),
            });
        }
    }
    bindings.sort_by(|a, b| {
        fn order(key: &str) -> usize {
            crate::settings::FUNCTION_KEY_LABELS
                .iter()
                .position(|k| k.eq_ignore_ascii_case(key))
                .unwrap_or(999)
        }
        order(&a.key).cmp(&order(&b.key))
    });
}

fn default_claude_desktop_target() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "com.anthropic.claudefordesktop"
    }
    #[cfg(not(target_os = "macos"))]
    {
        "Claude"
    }
}

fn backup_corrupt_settings<R: Runtime>(app: &AppHandle<R>, raw: &serde_json::Value) {
    let Ok(dir) = app.path().app_config_dir() else {
        return;
    };
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let path = dir.join(format!("caduceus-settings.corrupt-{stamp}.json"));
    if let Ok(text) = serde_json::to_string_pretty(raw) {
        if let Err(e) = std::fs::write(&path, text) {
            log::warn!("could not back up corrupt settings: {e}");
        } else {
            log::warn!("previous settings backed up to {}", path.display());
        }
    }
}

/// Reset everything to factory defaults, including deleting stored secrets for
/// backends that are about to disappear.
pub fn reset_to_defaults<R: Runtime>(app: &AppHandle<R>) -> Result<Settings, String> {
    if let Some(mgr) = app.try_state::<SettingsManager>() {
        for backend in mgr.get().agents.backends {
            let _ = secrets::delete_backend_api_key(&backend.id);
        }
    }
    let defaults = Settings::default();
    save(app, &defaults)?;
    Ok(defaults)
}
