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
pub mod backdrop;
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
pub mod uninstall;
pub mod usage;
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
            commands::awake_start,
            commands::awake_stop,
            commands::awake_status,
            commands::open_manage_window,
            commands::set_palette_floating,
            commands::search_files,
            commands::define_word,
            commands::convert_image,
            commands::inspect_extension,
            commands::install_extension,
            commands::list_extensions,
            commands::remove_extension,
            commands::uninstall_snapshot,
            commands::run_uninstall,
            commands::open_extensions_folder,
            commands::extension_permissions,
            commands::extension_source,
            commands::extension_clipboard_read,
            commands::extension_clipboard_write,
            commands::extension_fetch,
            commands::extension_selection,
            commands::extension_notify,
            commands::extension_open,
            commands::extension_storage_get,
            commands::extension_storage_set,
            commands::extension_shell_run,
            commands::extension_automation_script,
            commands::extension_automation_shortcut,
            commands::extension_files_read,
            commands::extension_files_write,
            commands::extension_settings_get,
            commands::extension_settings_set,
            commands::extension_commands_dispatch,
            commands::extension_commands_run_tool,
            commands::extension_ai_ask,
            commands::extension_shortcuts_run,
            commands::toggle_staff,
            commands::save_staff_position,
            commands::refresh_staff_layout,
            commands::collapse_staff_popout,
            commands::resolve_shortcut_icon,
            commands::import_shortcut_icon,
            commands::import_staff_mark,
            commands::clear_staff_mark,
            commands::resolve_backdrop,
            commands::import_backdrop,
            commands::clear_backdrop,
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
            commands::voice_pause,
            commands::voice_finish,
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
            commands::repair_permission,
            commands::request_permission,
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
            commands::pick_screen_color,
            commands::exchange_rates,
            // other applications
            commands::run_apple_script,
            commands::run_apple_shortcut,
            commands::list_apple_shortcuts,
            // storage, sorting, citations, recording
            commands::scan_junk,
            commands::clean_junk,
            commands::list_installed_app_sizes,
            commands::sort_plan,
            commands::sort_apply,
            commands::sort_revert,
            commands::desktop_shape_plan,
            commands::desktop_shape_apply,
            commands::desktop_shape_revert,
            commands::current_page,
            commands::enrich_citation,
            commands::format_citations,
            commands::recording_start,
            commands::recording_pause,
            commands::recording_stop,
            commands::recording_status,
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
            // usage ranking
            commands::usage_counts,
            commands::record_usage,
            commands::seed_usage,
            commands::clear_usage,
            // misc
            commands::quit_app,
        ])
        .setup(setup)
        .on_window_event(handle_window_event)
        .build(tauri::generate_context!())
        .expect("failed to build Caduceus")
        .run(|app, event| {
            if let RunEvent::ExitRequested { api, .. } = event {
                // Cancelled while a recording or a dictation session finishes.
                // `begin_shutdown` calls `exit` itself once the teardown is
                // done — see the comment there for why this cannot simply
                // block.
                if matches!(begin_shutdown(app), ShutdownDecision::Wait) {
                    api.prevent_exit();
                }
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
    let mut loaded = settings::load(&handle);

    // Until the walkthrough finishes, keep the staff on screen even if a prior
    // session hid it — otherwise a reinstall with old settings looks broken.
    let mut dirty = false;
    if !loaded.general.onboarding_done && !loaded.general.staff_visible {
        loaded.general.staff_visible = true;
        dirty = true;
    }

    // First launch of this build: show the staff, whatever a previous version
    // was told. Settings outlive the bundle, so "I installed it and nothing
    // appeared" is what a months-old `staff_visible: false` looks like — and
    // the staff is the only part of Caduceus you can see at all.
    let version = env!("CARGO_PKG_VERSION");
    if loaded.general.last_launched_version.as_deref() != Some(version) {
        loaded.general.last_launched_version = Some(version.to_string());
        loaded.general.staff_visible = true;
        dirty = true;
    }

    if dirty {
        let _ = settings::save(&handle, &loaded);
    }

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

    // --- usage ranking -----------------------------------------------------
    // Loaded before the windows so the palette's first render already has it.
    app.manage(usage::UsageStore::open(data_dir.join(usage::USAGE_FILE)));
    app.manage(tools::awake::AwakeRuntime::new());
    app.manage(tools::rates::RateCache::new());
    app.manage(tools::sorter::Session::new());
    app.manage(window::PaletteFloating::default());

    // --- agents and voice -------------------------------------------------
    app.manage(agent::AgentRuntime::new());
    app.manage(apps::AppIndex::new());
    app.manage(voice::VoiceRuntime::new());
    app.manage(capture::CaptureRuntime::new());
    app.manage(capture::recorder::RecorderRuntime::new());
    app.manage(sysmon::SysMonitor::new());

    // --- windows ----------------------------------------------------------
    if let Some(staff) = window::staff(&handle) {
        let _ = staff.set_ignore_cursor_events(true);
        let _ = window::position_staff(&handle, &manager);
        if window::should_show_staff(&loaded) {
            let _ = staff.show();
            window::configure_staff_floating(&staff);
        }
    }
    if let Some(w) = handle.get_webview_window(window::COMMAND_CENTER_WINDOW) {
        window::apply_vibrancy(&w);
        // Set up the panel now rather than on the first hotkey press. It is the
        // same work either way, and doing it while the window is still hidden
        // means the very first open is already allowed into whatever Space the
        // user happens to be in.
        window::configure_command_center_floating(&w);
    }

    // Ask again a beat later. `show()` above runs while the event loop has not
    // started, and on the first launch after an install the window server is
    // busy enough — Gatekeeper, the quarantine check, the login-item
    // registration — that it does not always stick. The symptom is an app that
    // starts and visibly does nothing, which is the worst possible first
    // impression and exactly what one cheap re-assert prevents.
    {
        let handle = handle.clone();
        let manager = manager.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(900)).await;
            if !window::should_show_staff(&manager.get()) {
                return;
            }
            let Some(staff) = window::staff(&handle) else { return };
            if !staff.is_visible().unwrap_or(false) {
                let _ = window::position_staff(&handle, &manager);
                let _ = staff.show();
            }
            window::configure_staff_floating(&staff);
        });
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
                // Closing puts Caduceus back in the menu bar where it lives.
                // The tabs are still there on the next open — they are written
                // down as well as held in memory, so even a restart reopens on
                // what you left.
                let _ = window.hide();
            }
        }

        // The palette is modal-feeling: clicking away dismisses it, the way
        // Spotlight and Raycast behave.
        // Click-away dismissal is Spotlight behaviour, and it is right for a
        // palette. It is wrong the moment the window is holding tabs you are
        // working in — nobody wants Settings to vanish because they checked
        // something in another app. `PaletteFloating` is the difference.
        WindowEvent::Focused(false) => {
            if window.label() == window::COMMAND_CENTER_WINDOW {
                let app = window.app_handle();
                let state = app.try_state::<window::PaletteFloating>();
                // A non-activating panel can resign key for a moment as the
                // window server hands it over. Closing on that would make the
                // palette flash open and vanish.
                if state.as_ref().is_some_and(|s| s.just_shown()) {
                    return;
                }
                let floating = state.map(|state| state.get()).unwrap_or(true);
                let dismisses = app
                    .try_state::<SettingsManager>()
                    .map(|s| s.with(|s| s.command_center.close_on_action))
                    .unwrap_or(true);
                if floating && dismisses {
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

/// Set once teardown has begun, so quitting twice does not run it twice.
///
/// The Quit menu item and the event loop's `ExitRequested` can both fire for a
/// single quit, and a second teardown would try to stop helpers the first one
/// is already waiting on.
static SHUTTING_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether anything is running that needs a moment to finish.
fn has_work_to_finish<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    let recording = app
        .try_state::<capture::recorder::RecorderRuntime>()
        .is_some_and(|r| r.status().active);
    let dictating = app
        .try_state::<voice::VoiceRuntime>()
        .is_some_and(|v| v.is_recording());
    recording || dictating
}

/// Quit, finishing what is in flight without freezing while it happens.
///
/// # Why this is not just `shutdown(); exit(0)`
///
/// Because both of the things worth waiting for are slow. Finalising a
/// screen recording is bounded at 25 seconds and closing a live dictation
/// session at 6, and `ExitRequested` is delivered **on the main thread** — so
/// quitting mid-recording used to beachball the entire app for up to half a
/// minute before it went away. That is the same failure the dictation hotkey
/// had, in the one code path nobody tests twice.
///
/// The fix is not to shorten the waits: an unfinalised MP4 is a corrupt file,
/// and throwing away somebody's recording to save them fifteen seconds is the
/// wrong trade. It is to stop *blocking* for them. The exit is cancelled, the
/// teardown runs on a worker, and the app exits when it is genuinely done —
/// with the event loop still turning throughout, so the window server never
/// sees an unresponsive application.
fn begin_shutdown<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> ShutdownDecision {
    use std::sync::atomic::Ordering;

    if SHUTTING_DOWN.swap(true, Ordering::SeqCst) {
        // Already under way — let this exit through rather than starting again.
        return ShutdownDecision::ExitNow;
    }

    // Nothing slow is running, so the whole teardown is a handful of atomic
    // flags. Doing it inline keeps the common quit instant.
    if !has_work_to_finish(app) {
        shutdown(app);
        return ShutdownDecision::ExitNow;
    }

    log::info!("finishing a recording or dictation before quitting");
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        shutdown(&handle);
        handle.exit(0);
    });
    ShutdownDecision::Wait
}

enum ShutdownDecision {
    ExitNow,
    /// Teardown is running on a worker; it will call `exit` when it is done.
    Wait,
}

/// Stop background workers cleanly. Safe to call more than once.
///
/// **Blocking, and sometimes for tens of seconds.** Call it through
/// [`begin_shutdown`] rather than directly, unless you are already off the
/// main thread.
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
    // A recorder left running would keep writing to a file nobody can stop.
    if let Some(recorder) = app.try_state::<capture::recorder::RecorderRuntime>() {
        recorder.shutdown();
    }
    if let Some(tracker) = app.try_state::<CursorTracker>() {
        tracker.stop();
    }
}
