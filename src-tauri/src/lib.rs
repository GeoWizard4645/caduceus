//! Orbit — a fast, local-first AI command center.
//!
//! # Layout
//!
//! | module        | responsibility                                            |
//! |---------------|-----------------------------------------------------------|
//! | [`settings`]  | the config schema, JSON persistence, OS-keychain secrets   |
//! | [`shortcuts`] | the `Shortcut` primitive and how each kind is executed     |
//! | [`clipboard`] | watcher, SQLite history, optional encryption at rest       |
//! | [`agent`]     | the `AgentBackend` trait, providers, and computer use      |
//! | [`voice`]     | push-to-talk capture, speech-to-text, keyword routing      |
//! | [`palette`]   | Command Center prefix parsing and dispatch                 |
//! | [`window`]    | orb placement, cursor tracking, window show/hide           |
//! | [`commands`]  | every `#[tauri::command]` the webview can call             |
//!
//! # Startup order
//!
//! Settings load first because everything else reads them; the clipboard store
//! opens next so the watcher has somewhere to write; hotkeys and the tray come
//! last because they can fail without preventing the app from running.

pub mod agent;
pub mod autostart;
pub mod clipboard;
pub mod commands;
pub mod hotkeys;
pub mod palette;
pub mod settings;
pub mod shortcuts;
pub mod tray;
pub mod voice;
pub mod window;

use tauri::{Emitter, Manager, RunEvent, WindowEvent};

use crate::clipboard::{ClipboardStore, WatcherHandle};
use crate::settings::SettingsManager;
use crate::window::CursorTracker;

/// Entry point called by `main.rs`.
pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            // Started at login with no windows shown: the tray and the orb are
            // the entire UI until the user asks for more.
            Some(vec!["--minimized"]),
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    hotkeys::handle(app, shortcut, event.state());
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            // settings
            commands::get_settings,
            commands::update_settings,
            commands::reset_settings,
            commands::get_runtime_info,
            commands::validate_hotkey,
            // secrets
            commands::set_backend_api_key,
            commands::delete_backend_api_key,
            commands::set_stt_api_key,
            // shortcuts
            commands::run_shortcut,
            commands::list_chrome_profiles,
            commands::test_command,
            commands::open_external_url,
            // command center + windows
            commands::parse_input,
            commands::dispatch_input,
            commands::open_command_center,
            commands::hide_command_center,
            commands::open_settings_window,
            commands::toggle_orb,
            commands::save_orb_position,
            // clipboard
            commands::clipboard_list,
            commands::clipboard_copy,
            commands::clipboard_image,
            commands::clipboard_pin,
            commands::clipboard_delete,
            commands::clipboard_clear,
            commands::clipboard_stats,
            // agents
            commands::agent_chat,
            commands::agent_start_session,
            commands::agent_stop_session,
            commands::agent_stop_all,
            commands::agent_approve,
            commands::agent_active_sessions,
            commands::agent_test_backend,
            commands::agent_list_models,
            commands::agent_backend_templates,
            // voice
            commands::voice_start,
            commands::voice_stop,
            commands::voice_cancel,
            commands::voice_is_recording,
            // misc
            commands::quit_app,
        ])
        .setup(setup)
        .on_window_event(handle_window_event)
        .build(tauri::generate_context!())
        .expect("failed to build Orbit")
        .run(|app, event| {
            if let RunEvent::ExitRequested { .. } = event {
                shutdown(app);
            }
        });
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();

    // macOS: no Dock icon, no app-switcher entry. Orbit is a menu-bar utility;
    // the Settings window temporarily switches this back (see window::open_settings).
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);

    // --- settings ---------------------------------------------------------
    let loaded = settings::load(&handle);
    let manager = SettingsManager::new(loaded.clone());
    app.manage(manager.clone());

    // --- clipboard --------------------------------------------------------
    let data_dir = handle
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("orbit"));
    let _ = std::fs::create_dir_all(&data_dir);

    match ClipboardStore::open(data_dir.join(clipboard::DB_FILE)) {
        Ok(store) => {
            // Apply retention on launch so a long-idle install does not keep
            // months of history it was configured to drop.
            let _ = store.prune(loaded.clipboard.max_items, loaded.clipboard.max_age_days);
            app.manage(store.clone());
            let watcher = clipboard::watcher::spawn(handle.clone(), store, manager.clone());
            app.manage(watcher);
        }
        Err(e) => {
            // History is a feature, not a prerequisite: the rest of the app
            // continues without it.
            log::error!("clipboard history is unavailable: {e}");
            app.manage(WatcherHandle::default());
        }
    }

    // --- agents and voice -------------------------------------------------
    app.manage(agent::AgentRuntime::new());
    app.manage(voice::VoiceRuntime::new());

    // --- windows ----------------------------------------------------------
    if let Some(orb) = window::orb(&handle) {
        // Start click-through; the cursor tracker turns this off when the
        // pointer actually reaches the orb.
        let _ = orb.set_ignore_cursor_events(true);
        let _ = window::position_orb(&handle, &manager);
        if loaded.general.orb_visible {
            let _ = orb.show();
            let _ = orb.set_always_on_top(true);
        }
    }
    for label in [window::COMMAND_CENTER_WINDOW, window::SETTINGS_WINDOW] {
        if let Some(w) = handle.get_webview_window(label) {
            window::apply_vibrancy(&w);
        }
    }

    let tracker = window::spawn_cursor_tracker(handle.clone(), manager.clone());
    app.manage(tracker);

    // --- hotkeys and tray -------------------------------------------------
    let problems = hotkeys::register_all(&handle, &manager);
    if !problems.is_empty() {
        // Surfaced in Settings rather than as a modal at launch: a clashing
        // hotkey is worth knowing about, not worth interrupting for.
        let _ = handle.emit("orbit://hotkey-problems", &problems);
    }

    if let Err(e) = tray::build(&handle) {
        log::error!("could not create the tray icon: {e}");
    }

    autostart::sync_with_settings(&handle, loaded.general.launch_at_login);

    log::info!(
        "Orbit {} started on {} ({} shortcuts, clipboard {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        loaded.shortcuts.len(),
        if loaded.clipboard.enabled { "on" } else { "off" },
    );
    Ok(())
}

fn handle_window_event(window: &tauri::Window, event: &WindowEvent) {
    match event {
        // Closing a window hides it instead of destroying it: recreating a
        // webview on every open would make the palette feel slow, and Orbit is
        // only ever really quit from the tray.
        WindowEvent::CloseRequested { api, .. } => {
            if window.label() != window::ORB_WINDOW {
                api.prevent_close();
                let _ = window.hide();
                #[cfg(target_os = "macos")]
                if window.label() == window::SETTINGS_WINDOW {
                    window::on_settings_closed(window.app_handle());
                }
            }
        }

        // The palette is modal-feeling: clicking away dismisses it, the way
        // Spotlight and Raycast behave.
        WindowEvent::Focused(false) => {
            if window.label() == window::COMMAND_CENTER_WINDOW {
                let hide = window
                    .app_handle()
                    .try_state::<SettingsManager>()
                    .map(|s| s.with(|s| s.command_center.close_on_action))
                    .unwrap_or(true);
                if hide {
                    let _ = window.hide();
                }
            }
        }

        // Remember where the orb was dragged to.
        WindowEvent::Moved(_) => {
            if window.label() == window::ORB_WINDOW {
                let app = window.app_handle().clone();
                if let Some(settings) = app.try_state::<SettingsManager>() {
                    let settings = settings.inner().clone();
                    // Debounced through the async runtime so a drag does not
                    // write the settings file on every frame.
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                        let _ = window::persist_orb_position(&app, &settings);
                    });
                }
            }
        }

        _ => {}
    }
}

/// Stop background workers cleanly. Safe to call more than once.
pub fn shutdown<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(runtime) = app.try_state::<agent::AgentRuntime>() {
        runtime.stop_all();
    }
    if let Some(voice) = app.try_state::<voice::VoiceRuntime>() {
        voice.cancel();
    }
    if let Some(watcher) = app.try_state::<WatcherHandle>() {
        watcher.stop();
    }
    if let Some(tracker) = app.try_state::<CursorTracker>() {
        tracker.stop();
    }
}
