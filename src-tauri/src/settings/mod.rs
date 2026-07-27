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
    // v2: launch-at-login became the default. Applied once, gated on the stored
    // version: a hotkey only works while the process is running, so an install
    // that is not a login item fails at exactly the moment it is reached for.
    // A user who turns it off after this keeps their choice — the gate never
    // fires twice.
    if s.version < 2 {
        s.general.launch_at_login = true;
    }

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

    // F1-F20 used to be configurable in two places: a dedicated
    // `toggle_orb_hotkey` field and the function-key table, which rejected
    // whatever the field held as "reserved". Fold the field into the table so
    // there is one owner, and leave non-F-key accelerators (⌥⇧S and friends)
    // exactly where they are.
    {
        let staff_key = s.general.toggle_orb_hotkey.trim().to_ascii_uppercase();
        let is_function_key = FUNCTION_KEY_LABELS
            .iter()
            .any(|label| label.eq_ignore_ascii_case(&staff_key));
        if is_function_key {
            let already_toggles = s
                .general
                .function_keys
                .iter()
                .any(|b| b.action == FunctionKeyAction::ToggleStaff);

            if !already_toggles {
                // Prefer the key they actually had. If they had since assigned
                // something else to that row, take the first *free* row instead
                // — never overwrite a binding the user chose, and never drop the
                // staff toggle on the floor either.
                let preferred = s
                    .general
                    .function_keys
                    .iter()
                    .position(|b| {
                        b.key.eq_ignore_ascii_case(&staff_key)
                            && b.action == FunctionKeyAction::None
                    })
                    .or_else(|| {
                        s.general
                            .function_keys
                            .iter()
                            .position(|b| b.action == FunctionKeyAction::None)
                    });

                if let Some(index) = preferred {
                    s.general.function_keys[index].action = FunctionKeyAction::ToggleStaff;
                }
                // Every row occupied is possible but absurd; the menu-bar icon
                // still toggles the staff, so nothing is unreachable.
            }
            s.general.toggle_orb_hotkey.clear();
        }
    }

    // A config that predates the function-key table has no rows at all, which
    // would leave the staff with no way to be toggled.
    if s.general.function_keys.is_empty() {
        s.general.function_keys = default_function_key_bindings();
    }

    // Clamp anything a hand-edited file could put out of range.
    // Floor of 0 ("fold back the moment the pointer leaves"), not 500: the
    // default is 50ms, and a floor above the default would silently rewrite it
    // on every load.
    s.general.collapse_idle_ms = s.general.collapse_idle_ms.min(60_000);
    s.general.cursor_poll_ms = s.general.cursor_poll_ms.clamp(8, 500);
    s.clipboard.poll_interval_ms = s.clipboard.poll_interval_ms.clamp(100, 10_000);
    s.clipboard.max_items = s.clipboard.max_items.clamp(10, 100_000);
    s.appearance.staff_size = s.appearance.staff_size.clamp(28, 160);
    s.appearance.popout_radius = s.appearance.popout_radius.clamp(56, 132);
    s.appearance.popout_icon_size = s.appearance.popout_icon_size.clamp(24, 52);
    s.appearance.staff_idle_opacity = s.appearance.staff_idle_opacity.clamp(0.15, 1.0);
    s.voice.max_recording_secs = s.voice.max_recording_secs.clamp(3, 600);

    // Claude should open the desktop app, not the website.
    for shortcut in &mut s.shortcuts {
        if shortcut.id == "sc-dictation"
            || shortcut.label.eq_ignore_ascii_case("dictation")
            || shortcut.label.eq_ignore_ascii_case("dictation app")
        {
            shortcut.hidden = true;
            shortcut.show_in_staff = false;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrading_to_v2_turns_launch_at_login_on_exactly_once() {
        // An existing install that never chose either way gets the new default…
        let mut old = Settings { version: 1, ..Settings::default() };
        old.general.launch_at_login = false;
        migrate(&mut old);
        assert!(old.general.launch_at_login);
        assert_eq!(old.version, Settings::CURRENT_VERSION);

        // …and turning it off afterwards is a choice migrate never overrides.
        let mut opted_out = old.clone();
        opted_out.general.launch_at_login = false;
        migrate(&mut opted_out);
        assert!(!opted_out.general.launch_at_login);
    }

    fn action_for(s: &Settings, key: &str) -> FunctionKeyAction {
        s.general
            .function_keys
            .iter()
            .find(|b| b.key == key)
            .map(|b| b.action)
            .expect("function key row should exist")
    }

    /// The exact spelling the TypeScript `BackendKind` union, the settings UI
    /// and the installer all write. Serde would otherwise derive
    /// `open_ai_compatible` from the Rust casing, and the two disagreeing meant
    /// a backend added in the UI could not be read back.
    #[test]
    fn backend_kind_uses_the_spelling_the_rest_of_the_project_writes() {
        let json = serde_json::to_string(&BackendKind::OpenAiCompatible).unwrap();
        assert_eq!(json, "\"openai_compatible\"");

        let parsed: BackendKind = serde_json::from_str("\"openai_compatible\"").unwrap();
        assert_eq!(parsed, BackendKind::OpenAiCompatible);

        let stt = serde_json::to_string(&SttBackendKind::OpenAiCompatible).unwrap();
        assert_eq!(stt, "\"openai_compatible\"");
    }

    /// Files written while Rust and the UI disagreed still contain the derived
    /// spelling. They have to keep loading, or fixing the bug would itself wipe
    /// the settings of everyone who hit it.
    #[test]
    fn the_old_derived_spelling_still_loads() {
        let parsed: BackendKind = serde_json::from_str("\"open_ai_compatible\"").unwrap();
        assert_eq!(parsed, BackendKind::OpenAiCompatible);

        let stt: SttBackendKind = serde_json::from_str("\"open_ai_compatible\"").unwrap();
        assert_eq!(stt, SttBackendKind::OpenAiCompatible);
    }

    /// A backend configured in the UI must survive a round trip through the
    /// whole document. One unreadable enum fails the entire file, so this is
    /// the case that cost a full settings profile rather than one field.
    #[test]
    fn a_ui_configured_backend_survives_a_full_round_trip() {
        let raw = serde_json::json!({
            "agents": {
                "primaryBackendId": "ollama-chat",
                "backends": [{
                    "id": "ollama-chat",
                    "displayName": "Ollama",
                    "kind": "openai_compatible",
                    "baseUrl": "http://localhost:11434/v1",
                    "model": "qwen3.5:4b",
                }],
            }
        });

        let settings: Settings =
            serde_json::from_value(raw).expect("a UI-written backend must load");
        assert_eq!(settings.agents.backends[0].kind, BackendKind::OpenAiCompatible);

        let round_tripped: Settings =
            serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
        assert_eq!(round_tripped.agents.backends[0].kind, BackendKind::OpenAiCompatible);
    }

    #[test]
    fn f12_toggles_the_staff_out_of_the_box() {
        let s = Settings::default();
        assert_eq!(action_for(&s, "F12"), FunctionKeyAction::ToggleStaff);
        assert!(
            s.general.toggle_orb_hotkey.is_empty(),
            "F12 lives in the table, so the dedicated field must not also claim it"
        );
    }

    #[test]
    fn an_old_config_keeps_its_f_key_staff_toggle() {
        // Before unification the staff toggle was its own field and the table
        // rejected that key as reserved. Migrating must not silently unbind it.
        let mut s = Settings::default();
        s.general.toggle_orb_hotkey = "F9".into();
        if let Some(b) = s.general.function_keys.iter_mut().find(|b| b.key == "F12") {
            b.action = FunctionKeyAction::None;
        }

        migrate(&mut s);

        assert_eq!(action_for(&s, "F9"), FunctionKeyAction::ToggleStaff);
        assert!(s.general.toggle_orb_hotkey.is_empty(), "field should hand over to the table");
    }

    #[test]
    fn migration_does_not_clobber_a_deliberate_binding() {
        let mut s = Settings::default();
        s.general.toggle_orb_hotkey = "F5".into();
        if let Some(b) = s.general.function_keys.iter_mut().find(|b| b.key == "F5") {
            b.action = FunctionKeyAction::Screenshot;
        }
        // Clear the default F12 toggle so the migration has to find a home.
        if let Some(b) = s.general.function_keys.iter_mut().find(|b| b.key == "F12") {
            b.action = FunctionKeyAction::None;
        }

        migrate(&mut s);

        assert_eq!(
            action_for(&s, "F5"),
            FunctionKeyAction::Screenshot,
            "a key the user already assigned must win over the migrated default"
        );
        assert!(
            s.general
                .function_keys
                .iter()
                .any(|b| b.action == FunctionKeyAction::ToggleStaff),
            "the staff toggle must land somewhere rather than being dropped"
        );
    }

    #[test]
    fn a_fully_occupied_table_does_not_panic() {
        let mut s = Settings::default();
        s.general.toggle_orb_hotkey = "F7".into();
        for b in s.general.function_keys.iter_mut() {
            b.action = FunctionKeyAction::Screenshot;
        }

        migrate(&mut s);

        // Nothing was overwritten, and the field still handed over.
        assert!(s
            .general
            .function_keys
            .iter()
            .all(|b| b.action == FunctionKeyAction::Screenshot));
        assert!(s.general.toggle_orb_hotkey.is_empty());
    }

    #[test]
    fn a_non_function_key_staff_hotkey_is_left_alone() {
        let mut s = Settings::default();
        s.general.toggle_orb_hotkey = "CommandOrControl+Shift+O".into();

        migrate(&mut s);

        assert_eq!(s.general.toggle_orb_hotkey, "CommandOrControl+Shift+O");
    }

    #[test]
    fn an_empty_function_key_table_is_repopulated() {
        // A config written before the table existed would otherwise leave the
        // staff with no key at all.
        let mut s = Settings::default();
        s.general.function_keys.clear();

        migrate(&mut s);

        assert_eq!(action_for(&s, "F12"), FunctionKeyAction::ToggleStaff);
    }
}
