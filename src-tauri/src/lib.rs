//! Caduceus — a fast, local-first AI command center.
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
//! | [`window`]    | staff placement, cursor tracking, window show/hide           |
//! | [`commands`]  | every `#[tauri::command]` the webview can call             |
//!
//! # Startup order
//!
//! Settings load first because everything else reads them; the clipboard store
//! opens next so the watcher has somewhere to write; hotkeys and the tray come
//! last because they can fail without preventing the app from running.

pub mod agent;
pub mod apps;
pub mod autostart;
pub mod calc;
pub mod capture;
pub mod chat;
pub mod clipboard;
pub mod commands;
pub mod extensions;
pub mod fn_keys;
pub mod hotkeys;
pub mod notes;
pub mod palette;
pub mod settings;
pub mod shortcuts;
pub mod sysmon;
pub mod tools;
pub mod staff_mark;
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
            // Started at login with no windows shown: the tray and the staff are
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
            commands::list_browsers,
            commands::test_command,
            commands::open_external_url,
            commands::open_system_settings,
            // command center + windows
            commands::parse_input,
            commands::dispatch_input,
            commands::open_command_center,
            commands::hide_command_center,
            commands::open_settings_window,
            commands::set_staff_interactive,
            commands::set_staff_capture_rect,
            commands::chat_ask,
            commands::chat_conversations,
            commands::chat_messages,
            commands::chat_new_conversation,
            commands::chat_delete_conversation,
            commands::chat_clear,
            commands::open_chat_window,
            commands::add_to_notes,
            commands::change_case,
            commands::case_options,
            commands::copy_latest_download,
            commands::open_latest_download,
            commands::copy_finder_path,
            commands::eject_disks,
            commands::stay_awake,
            commands::stay_awake_state,
            commands::search_files,
            commands::define_word,
            commands::convert_image,
            commands::inspect_extension,
            commands::install_extension,
            commands::list_extensions,
            commands::remove_extension,
            commands::open_extensions_folder,
            commands::extension_permissions,
            commands::toggle_staff,
            commands::save_staff_position,
            commands::collapse_staff_popout,
            commands::resolve_shortcut_icon,
            commands::import_shortcut_icon,
            commands::import_staff_mark,
            commands::clear_staff_mark,
            commands::resolve_staff_mark,
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
            commands::hermes_status,
            commands::detect_local_ai,
            // system monitor
            commands::system_snapshot,
            commands::system_kill,
            commands::open_hermes_installer,
            // launcher + calculator
            commands::list_installed_apps,
            commands::launch_app,
            commands::calculate,
            // voice
            commands::voice_start,
            commands::voice_stop,
            commands::voice_cancel,
            commands::voice_is_recording,
            commands::toggle_dictation,
            commands::capture_screenshot,
            commands::capture_record_start,
            commands::capture_record_stop,
            commands::capture_recording_state,
            // window management
            commands::window_action,
            commands::window_permission,
            commands::selected_text,
            // developer toolbox
            commands::run_tool,
            // system controls
            commands::system_action,
            commands::system_permissions,
            commands::machine_summary,
            commands::wifi_summary,
            commands::media_action,
            // vision + audio devices
            commands::ocr_screen,
            commands::ocr_image,
            commands::audio_devices,
            commands::set_audio_device,
            // developer environment
            commands::listening_ports,
            commands::free_port,
            commands::git_repos,
            commands::git_status,
            commands::ssh_hosts,
            commands::ssh_connect,
            commands::docker_containers,
            commands::docker_action,
            // files
            commands::compress_selection,
            commands::expand_selection,
            commands::trash_selection,
            commands::quick_look_selection,
            commands::open_selection_in_terminal,
            commands::largest_files,
            commands::app_leftovers,
            commands::trash_paths,
            commands::reveal_path,
            commands::open_path_in_terminal,
            // network
            commands::network_summary,
            commands::public_address,
            commands::dns_lookup,
            commands::ping_host,
            // misc
            commands::quit_app,
        ])
        .setup(setup)
        .on_window_event(handle_window_event)
        .build(tauri::generate_context!())
        .expect("failed to build Caduceus")
        .run(|app, event| {
            if let RunEvent::ExitRequested { .. } = event {
                shutdown(app);
            }
        });
}

/// Remove a legacy marker older installers dropped on every reinstall. The
/// walkthrough is gated only by [`Settings::general::onboarding_done`].
fn discard_legacy_onboarding_marker<R: tauri::Runtime>(handle: &tauri::AppHandle<R>) {
    let Ok(dir) = handle.path().app_data_dir() else {
        return;
    };
    let marker = dir.join(".run-onboarding");
    if marker.exists() {
        let _ = std::fs::remove_file(marker);
    }
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();

    // macOS: no Dock icon, no app-switcher entry. Caduceus is a menu-bar utility;
    // the Settings window temporarily switches this back (see window::open_settings).
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);

    // --- settings ---------------------------------------------------------
    discard_legacy_onboarding_marker(&handle);
    let loaded = settings::load(&handle);

    let manager = SettingsManager::new(loaded.clone());
    app.manage(manager.clone());

    // --- clipboard --------------------------------------------------------
    let data_dir = handle
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("caduceus"));
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

    // --- saved conversations ----------------------------------------------
    match chat::ChatStore::open(data_dir.join(chat::DB_FILE)) {
        Ok(store) => {
            // A backend error can leave a thread that was opened and never used.
            let _ = store.prune_empty();
            app.manage(store);
        }
        Err(e) => {
            // `/` still answers without history rather than failing outright.
            log::error!("chat history is unavailable: {e}");
        }
    }

    // --- agents and voice -------------------------------------------------
    app.manage(agent::AgentRuntime::new());
    app.manage(apps::AppIndex::new());
    app.manage(voice::VoiceRuntime::new());
    app.manage(capture::CaptureRuntime::new());
    app.manage(sysmon::SysMonitor::new());

    // --- windows ----------------------------------------------------------
    if let Some(staff) = window::staff(&handle) {
        let _ = staff.set_ignore_cursor_events(true);
        let _ = window::position_staff(&handle, &manager);
        if loaded.general.staff_visible {
            let _ = staff.show();
        }
        window::configure_staff_floating(&staff);
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
        let _ = handle.emit("caduceus://hotkey-problems", &problems);
    }

    if let Err(e) = tray::build(&handle) {
        log::error!("could not create the tray icon: {e}");
    }

    autostart::sync_with_settings(&handle, loaded.general.launch_at_login);

    log::info!(
        "Caduceus {} started on {} ({} shortcuts, clipboard {})",
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
        // webview on every open would make the palette feel slow, and Caduceus is
        // only ever really quit from the tray.
        WindowEvent::CloseRequested { api, .. } => {
            if window.label() != window::STAFF_WINDOW {
                api.prevent_close();
                let _ = window.hide();
                #[cfg(target_os = "macos")]
                if window.label() == window::SETTINGS_WINDOW || window.label() == window::CHAT_WINDOW
                {
                    window::on_dock_window_closed(window.app_handle(), window.label());
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

        // Remember where the staff was dragged to.
        WindowEvent::Moved(_) => {
            if window.label() == window::STAFF_WINDOW {
                let app = window.app_handle().clone();
                if let Some(settings) = app.try_state::<SettingsManager>() {
                    let settings = settings.inner().clone();
                    // Debounced through the async runtime so a drag does not
                    // write the settings file on every frame.
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                        let _ = window::persist_staff_position(&app, &settings);
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
