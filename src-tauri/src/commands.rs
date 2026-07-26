//! The IPC surface exposed to the webview.
//!
//! This is the **entire** attack surface between the frontend and the machine.
//! Caduceus deliberately does not enable Tauri's `shell`, `fs` or `http` plugins,
//! so the webview cannot ask to run a command, read a file, or call an
//! arbitrary URL. It can only invoke the functions below, each of which decides
//! for itself what is allowed:
//!
//! * shortcuts run **by id**, resolved against saved settings — the frontend
//!   never supplies a command string;
//! * URLs are validated to be `http(s)` before anything is opened;
//! * API keys go **into** the keychain through these commands and never come
//!   back out.

use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::agent::{self, AgentRuntime};
use crate::clipboard::{self, ClipboardEntry, ClipboardStore, TransitionReport};
use crate::palette::{self, DispatchOutcome};
use crate::settings::{self, secrets, BackendConfig, Settings, SettingsManager};
use crate::shortcuts::{self, BrowserInstall, ExecOutcome};
use crate::capture;
use crate::voice;
use crate::window;

type Res<T> = Result<T, String>;

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_settings(settings: tauri::State<'_, SettingsManager>) -> Settings {
    settings.get()
}

/// Persist a full settings tree.
///
/// The frontend always sends the whole object (it holds the canonical copy while
/// the Settings window is open), which keeps this a single idempotent write with
/// no partial-update merge logic to get wrong.
#[tauri::command]
pub async fn update_settings<R: Runtime>(
    app: AppHandle<R>,
    settings: tauri::State<'_, SettingsManager>,
    next: Settings,
) -> Res<SettingsApplyReport> {
    let previous = settings.get();
    settings::save(&app, &next)?;

    // Re-register hotkeys so a rebind takes effect without a restart.
    let hotkey_problems = crate::hotkeys::register_all(&app, &settings);

    // Reposition the staff if the edge changed and there is no manual position.
    if previous.general.staff_edge != next.general.staff_edge && next.general.staff_position.is_none() {
        let _ = window::position_staff(&app, &settings);
    }
    if previous.general.staff_visible != next.general.staff_visible {
        window::set_staff_visible(&app, &settings, next.general.staff_visible)?;
    }
    if previous.appearance != next.appearance {
        let _ = window::sync_staff_window(&app, &settings);
    }
    crate::tray::refresh(&app);

    // Launch-at-login is an OS-level registration, not just a flag.
    let mut autostart_error = None;
    if previous.general.launch_at_login != next.general.launch_at_login {
        if let Err(e) = crate::autostart::set_enabled(&app, next.general.launch_at_login) {
            autostart_error = Some(e);
        }
    }

    // Encryption changes rewrite the whole history table, so they are applied
    // here rather than silently on the next clipboard write.
    let mut encryption_report = None;
    if previous.clipboard.encrypt_at_rest != next.clipboard.encrypt_at_rest {
        if let Some(store) = app.try_state::<ClipboardStore>() {
            match clipboard::set_encryption(&store, next.clipboard.encrypt_at_rest) {
                Ok(report) => encryption_report = Some(report),
                Err(e) => return Err(e),
            }
        }
    }

    Ok(SettingsApplyReport {
        settings: settings.get(),
        hotkey_problems,
        autostart_error,
        encryption_report,
    })
}

/// What changed as a side effect of saving settings, so the UI can report it.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsApplyReport {
    pub settings: Settings,
    /// Hotkeys that could not be registered, usually because another app owns
    /// the combination.
    pub hotkey_problems: Vec<String>,
    pub autostart_error: Option<String>,
    pub encryption_report: Option<TransitionReport>,
}

#[tauri::command]
pub fn reset_settings<R: Runtime>(app: AppHandle<R>) -> Res<Settings> {
    let next = settings::reset_to_defaults(&app)?;
    if let Some(mgr) = app.try_state::<SettingsManager>() {
        let _ = crate::hotkeys::register_all(&app, &mgr);
        let _ = window::position_staff(&app, &mgr);
    }
    crate::tray::refresh(&app);
    Ok(next)
}

/// Everything the UI needs that is not a user preference: platform facts,
/// capability probes, and warnings.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInfo {
    pub version: String,
    pub platform: String,
    pub arch: String,
    /// False on systems with no usable secret storage (headless Linux).
    pub keychain_available: bool,
    pub stt_backends: Vec<voice::SttAvailability>,
    pub browsers: Vec<BrowserInstall>,
    pub clipboard_entries: i64,
    pub clipboard_bytes: i64,
    /// Which backends currently have a key in the keychain, by backend id.
    pub backends_with_keys: Vec<String>,
    pub computer_use_note: String,
    /// Whether Hermes Agent is installed and configured, for Settings.
    pub hermes: crate::agent::hermes::HermesStatus,
}

#[tauri::command]
pub async fn get_runtime_info<R: Runtime>(
    app: AppHandle<R>,
    settings: tauri::State<'_, SettingsManager>,
) -> Result<RuntimeInfo, String> {
    let cfg = settings.get();
    let hermes_status = crate::agent::hermes::status().await;
    let store = app.try_state::<ClipboardStore>();

    Ok(RuntimeInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        platform: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        keychain_available: secrets::keychain_available(),
        stt_backends: voice::stt::all_availability(&cfg.voice),
        browsers: shortcuts::detect_browsers(),
        clipboard_entries: store.as_ref().and_then(|s| s.count().ok()).unwrap_or(0),
        clipboard_bytes: store.as_ref().and_then(|s| s.total_bytes().ok()).unwrap_or(0),
        backends_with_keys: cfg
            .agents
            .backends
            .iter()
            .filter(|b| secrets::has_backend_api_key(&b.id))
            .map(|b| b.id.clone())
            .collect(),
        computer_use_note: platform_computer_use_note().into(),
        hermes: hermes_status,
    })
}

fn platform_computer_use_note() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "macOS will ask for Screen Recording and Accessibility permission the first time an \
         agent runs. Caduceus never requests them at launch."
    }
    #[cfg(target_os = "windows")]
    {
        "Computer use works without extra permissions, but cannot interact with windows \
         running as administrator unless Caduceus is also elevated."
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "Under Wayland, input simulation is blocked by the compositor and computer use will \
         not work. X11 sessions are fine. See docs/PLATFORM_SUPPORT.md."
    }
}

// ---------------------------------------------------------------------------
// Secrets
// ---------------------------------------------------------------------------

/// Store an API key in the OS keychain.
///
/// One-way by design: there is no command to read a key back out, so a
/// compromised webview cannot exfiltrate one. The UI shows "key saved", never
/// the key.
#[tauri::command]
pub fn set_backend_api_key(backend_id: String, key: String) -> Res<bool> {
    secrets::set_backend_api_key(&backend_id, key.trim()).map_err(|e| e.to_string())?;
    Ok(secrets::has_backend_api_key(&backend_id))
}

#[tauri::command]
pub fn delete_backend_api_key(backend_id: String) -> Res<()> {
    secrets::delete_backend_api_key(&backend_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_stt_api_key(key: String) -> Res<bool> {
    secrets::set_stt_api_key(key.trim()).map_err(|e| e.to_string())?;
    Ok(secrets::has_stt_api_key())
}

// ---------------------------------------------------------------------------
// Shortcuts
// ---------------------------------------------------------------------------

/// Run a shortcut by id. The frontend never supplies the target.
#[tauri::command]
pub async fn run_shortcut(
    settings: tauri::State<'_, SettingsManager>,
    id: String,
    query: Option<String>,
) -> Res<ExecOutcome> {
    let cfg = settings.get();
    let shortcut = cfg
        .shortcuts
        .iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("no shortcut with id \u{201c}{id}\u{201d}"))?;

    Ok(shortcuts::execute_shortcut(
        shortcut,
        query.as_deref().unwrap_or_default(),
        &cfg.command_center.browser,
    )
    .await)
}

#[tauri::command]
pub fn list_browsers() -> Vec<BrowserInstall> {
    shortcuts::detect_browsers()
}

/// Run a shell command and return its output, for the Settings "Test" button.
///
/// Reachable only from the Settings window, and only for a command the user
/// just typed there themselves.
#[tauri::command]
pub async fn test_command(command: String) -> ExecOutcome {
    shortcuts::exec::run_command_capture(&command, "", 20).await
}

#[tauri::command]
pub async fn open_external_url(
    settings: tauri::State<'_, SettingsManager>,
    url: String,
) -> Res<ExecOutcome> {
    let cfg = settings.get();
    Ok(shortcuts::exec::open_url(&url, &cfg.command_center.browser).await)
}

/// Send the user to a named System Settings pane. See
/// [`shortcuts::exec::open_settings_pane`] for why `pane` is a key and not a URL.
#[tauri::command]
pub async fn open_system_settings(pane: String) -> ExecOutcome {
    shortcuts::exec::open_settings_pane(&pane).await
}

// ---------------------------------------------------------------------------
// Command Center
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn parse_input(
    settings: tauri::State<'_, SettingsManager>,
    input: String,
) -> palette::ParsedInput {
    palette::parse(&input, &settings.get().command_center)
}

#[tauri::command]
pub async fn dispatch_input<R: Runtime>(
    app: AppHandle<R>,
    settings: tauri::State<'_, SettingsManager>,
    input: String,
) -> Res<DispatchOutcome> {
    Ok(palette::dispatch(&app, &settings, &input).await)
}

#[tauri::command]
pub fn hide_command_center<R: Runtime>(app: AppHandle<R>) -> Res<()> {
    window::hide_command_center(&app)
}

#[tauri::command]
pub fn open_command_center<R: Runtime>(
    app: AppHandle<R>,
    mode: Option<String>,
    prefill: Option<String>,
    source: Option<String>,
) -> Res<()> {
    window::open_command_center(
        &app,
        window::CommandCenterOpenPayload {
            mode: mode.unwrap_or_else(|| "default".into()),
            prefill: prefill.unwrap_or_default(),
            select_all: true,
            source: source.unwrap_or_else(|| "other".into()),
        },
    )
}

/// Keep the staff window clickable while the first-run walkthrough is showing.
///
/// Without this the walkthrough's own buttons fall through to whatever is
/// behind the staff, because the window is click-through everywhere except the
/// staff itself.
#[tauri::command]
pub fn set_staff_interactive<R: Runtime>(app: AppHandle<R>, interactive: bool) {
    if let Some(tracker) = app.try_state::<window::CursorTracker>() {
        tracker.set_force_interactive(interactive);
    }
}

/// Register a region of the staff window that should capture the pointer, or
/// pass `None` to clear it.
///
/// For overlays that live in the staff window and need clicks without taking
/// the whole window with them — the first-run walkthrough card. Coordinates are
/// logical pixels relative to the window's top-left, i.e. straight out of
/// `getBoundingClientRect()`.
#[tauri::command]
pub fn set_staff_capture_rect<R: Runtime>(app: AppHandle<R>, rect: Option<window::CaptureRect>) {
    if let Some(tracker) = app.try_state::<window::CursorTracker>() {
        tracker.set_capture_rect(rect);
    }
}

#[tauri::command]
pub fn open_settings_window<R: Runtime>(app: AppHandle<R>, tab: Option<String>) -> Res<()> {
    window::open_settings(&app, tab.as_deref())
}

// ---------------------------------------------------------------------------
// Staff
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn toggle_staff<R: Runtime>(app: AppHandle<R>, settings: tauri::State<'_, SettingsManager>) -> Res<bool> {
    let visible = window::toggle_staff(&app, &settings)?;
    crate::tray::refresh(&app);
    Ok(visible)
}

/// Called by the staff after a drag ends, to remember where it was left.
#[tauri::command]
pub fn save_staff_position<R: Runtime>(
    app: AppHandle<R>,
    settings: tauri::State<'_, SettingsManager>,
) -> Res<()> {
    window::persist_staff_position(&app, &settings)
}

/// Collapse the staff pop-out immediately (e.g. after a shortcut click).
#[tauri::command]
pub fn collapse_staff_popout<R: Runtime>(app: AppHandle<R>) -> Res<()> {
    if let Some(tracker) = app.try_state::<window::CursorTracker>() {
        tracker.request_collapse();
    }
    Ok(())
}

/// Resolve an `image:…` icon token to an absolute path for display in the webview.
#[tauri::command]
pub fn resolve_shortcut_icon<R: Runtime>(app: AppHandle<R>, icon: String) -> Res<Option<String>> {
    Ok(shortcuts::icons::resolve_path(&app, &icon).map(|p| p.to_string_lossy().into_owned()))
}

/// Import a user-picked image as this shortcut's icon.
#[tauri::command]
pub fn import_shortcut_icon<R: Runtime>(
    app: AppHandle<R>,
    shortcut_id: String,
    source_path: String,
) -> Res<String> {
    shortcuts::icons::import_icon(&app, &shortcut_id, std::path::Path::new(&source_path))
}

#[tauri::command]
pub fn resolve_staff_mark<R: Runtime>(app: AppHandle<R>, icon: String) -> Res<Option<String>> {
    Ok(crate::staff_mark::resolve_path(&app, &icon).map(|p| p.to_string_lossy().into_owned()))
}

#[tauri::command]
pub fn import_staff_mark<R: Runtime>(app: AppHandle<R>, source_path: String) -> Res<String> {
    crate::staff_mark::import_mark(&app, std::path::Path::new(&source_path))
}

#[tauri::command]
pub fn clear_staff_mark<R: Runtime>(app: AppHandle<R>) -> Res<()> {
    crate::staff_mark::clear_mark(&app)
}

// ---------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn clipboard_list(
    store: tauri::State<'_, ClipboardStore>,
    settings: tauri::State<'_, SettingsManager>,
    query: Option<String>,
    limit: Option<usize>,
    pinned_only: Option<bool>,
) -> Res<Vec<ClipboardEntry>> {
    let cfg = settings.with(|s| s.clipboard.clone());
    let key = clipboard::active_key(&cfg)?;
    store
        .list(
            query.as_deref().unwrap_or_default(),
            limit.unwrap_or(60).min(500),
            pinned_only.unwrap_or(false),
            key.as_ref(),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clipboard_copy(
    store: tauri::State<'_, ClipboardStore>,
    settings: tauri::State<'_, SettingsManager>,
    id: i64,
) -> Res<()> {
    let cfg = settings.with(|s| s.clipboard.clone());
    clipboard::copy_entry_to_clipboard(&store, id, &cfg)
}

/// Full image bytes for one entry, as a data URL. Kept out of `clipboard_list`
/// so scrolling history does not shuttle megabytes across the IPC bridge.
#[tauri::command]
pub fn clipboard_image(
    store: tauri::State<'_, ClipboardStore>,
    settings: tauri::State<'_, SettingsManager>,
    id: i64,
) -> Res<Option<String>> {
    use base64::Engine as _;
    let cfg = settings.with(|s| s.clipboard.clone());
    let key = clipboard::active_key(&cfg)?;
    let Some((kind, bytes)) = store.get_content(id, key.as_ref()).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    if kind != clipboard::EntryKind::Image {
        return Ok(None);
    }
    Ok(Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )))
}

#[tauri::command]
pub fn clipboard_pin(store: tauri::State<'_, ClipboardStore>, id: i64, pinned: bool) -> Res<()> {
    store.set_pinned(id, pinned).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clipboard_delete(store: tauri::State<'_, ClipboardStore>, id: i64) -> Res<()> {
    store.delete(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clipboard_clear(store: tauri::State<'_, ClipboardStore>, keep_pinned: bool) -> Res<usize> {
    store.clear(keep_pinned).map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardStats {
    pub entries: i64,
    pub bytes: i64,
    pub encrypted: bool,
}

#[tauri::command]
pub fn clipboard_stats(
    store: tauri::State<'_, ClipboardStore>,
    settings: tauri::State<'_, SettingsManager>,
) -> Res<ClipboardStats> {
    Ok(ClipboardStats {
        entries: store.count().map_err(|e| e.to_string())?,
        bytes: store.total_bytes().map_err(|e| e.to_string())?,
        encrypted: settings.with(|s| s.clipboard.encrypt_at_rest),
    })
}

// ---------------------------------------------------------------------------
// Agents
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn agent_chat(
    settings: tauri::State<'_, SettingsManager>,
    prompt: String,
) -> Res<agent::AgentResponse> {
    agent::chat(&settings, &prompt)
        .await
        .map_err(|e| e.user_message())
}

#[tauri::command]
pub fn agent_start_session<R: Runtime>(
    app: AppHandle<R>,
    runtime: tauri::State<'_, AgentRuntime>,
    settings: tauri::State<'_, SettingsManager>,
    task: String,
) -> Res<String> {
    agent::start_session(
        app.clone(),
        runtime.inner().clone(),
        settings.inner().clone(),
        task,
    )
    .map_err(|e| e.user_message())
}

#[tauri::command]
pub fn agent_stop_session(runtime: tauri::State<'_, AgentRuntime>, session_id: String) -> bool {
    runtime.stop(&session_id)
}

#[tauri::command]
pub fn agent_stop_all(runtime: tauri::State<'_, AgentRuntime>) {
    runtime.stop_all();
}

/// Answer the "let this agent control your machine?" prompt.
#[tauri::command]
pub fn agent_approve(
    runtime: tauri::State<'_, AgentRuntime>,
    session_id: String,
    approved: bool,
) -> bool {
    runtime.resolve_approval(&session_id, approved)
}

#[tauri::command]
pub fn agent_active_sessions(runtime: tauri::State<'_, AgentRuntime>) -> Vec<String> {
    runtime.active_sessions()
}

#[tauri::command]
pub async fn agent_test_backend(
    settings: tauri::State<'_, SettingsManager>,
    backend_id: String,
) -> Res<String> {
    let cfg = settings.get();
    let backend_config = cfg
        .agents
        .backends
        .iter()
        .find(|b| b.id == backend_id)
        .cloned()
        .ok_or_else(|| format!("no backend with id \u{201c}{backend_id}\u{201d}"))?;

    agent::backend_for(backend_config.kind)
        .test_connection(&backend_config)
        .await
        .map_err(|e| e.user_message())
}

/// Ask an OpenAI-compatible endpoint what models it serves.
#[tauri::command]
pub async fn agent_list_models(
    settings: tauri::State<'_, SettingsManager>,
    backend_id: String,
) -> Res<Vec<String>> {
    let cfg = settings.get();
    let backend_config = cfg
        .agents
        .backends
        .iter()
        .find(|b| b.id == backend_id)
        .cloned()
        .ok_or_else(|| format!("no backend with id \u{201c}{backend_id}\u{201d}"))?;

    match backend_config.kind {
        settings::BackendKind::OpenAiCompatible => agent::openai::list_models(&backend_config)
            .await
            .map_err(|e| e.user_message()),
        // Hermes owns its own model list; `hermes model` is where you change it.
        settings::BackendKind::Hermes | settings::BackendKind::Null => Ok(Vec::new()),
    }
}

/// Pre-filled configurations offered by the "Add backend" flow.
#[tauri::command]
pub fn agent_backend_templates() -> Vec<BackendConfig> {
    vec![
        settings::hermes_template(uuid::Uuid::new_v4().to_string()),
        settings::openai_compatible_template(uuid::Uuid::new_v4().to_string()),
    ]
}

// ---------------------------------------------------------------------------
// Voice
// ---------------------------------------------------------------------------

/// Start recording from a UI button rather than the hotkey.
#[tauri::command]
pub fn voice_start<R: Runtime>(
    app: AppHandle<R>,
    runtime: tauri::State<'_, voice::VoiceRuntime>,
    settings: tauri::State<'_, SettingsManager>,
) -> Res<()> {
    let emit = app.clone();
    runtime
        .start(&settings, move |text| {
            let _ = emit.emit(voice::VOICE_PARTIAL_EVENT, text);
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn voice_stop(
    runtime: tauri::State<'_, voice::VoiceRuntime>,
    settings: tauri::State<'_, SettingsManager>,
) -> Res<Option<voice::RoutedText>> {
    let Some(outcome) = runtime.stop() else {
        return Ok(None);
    };
    match outcome {
        voice::StopOutcome::Batch(Ok(wav)) => {
            voice::transcribe_and_route(wav, &settings).await.map(Some)
        }
        voice::StopOutcome::Batch(Err(e)) => Err(e),
        voice::StopOutcome::Live(Ok((text, _))) => {
            Ok(Some(voice::route_transcript(&text, &settings)))
        }
        voice::StopOutcome::Live(Err(e)) => Err(e),
    }
}

#[tauri::command]
pub fn voice_cancel(runtime: tauri::State<'_, voice::VoiceRuntime>) {
    runtime.cancel();
}

#[tauri::command]
pub fn voice_is_recording(runtime: tauri::State<'_, voice::VoiceRuntime>) -> bool {
    runtime.is_recording()
}

#[tauri::command]
pub fn toggle_dictation<R: Runtime>(
    app: AppHandle<R>,
    settings: tauri::State<'_, SettingsManager>,
) -> Res<()> {
    crate::hotkeys::toggle_dictation(&app, &settings);
    Ok(())
}

// ---------------------------------------------------------------------------
// Screen capture
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn capture_screenshot(save_to_downloads: Option<bool>) -> Res<capture::ScreenshotResult> {
    capture::screenshot_full(save_to_downloads.unwrap_or(true))
}

#[tauri::command]
pub fn capture_record_start<R: Runtime>(
    app: AppHandle<R>,
    mic: Option<bool>,
    system_audio: Option<bool>,
) -> Res<capture::RecordingState> {
    capture::start_recording(&app, mic.unwrap_or(true), system_audio.unwrap_or(false))
}

#[tauri::command]
pub fn capture_record_stop<R: Runtime>(app: AppHandle<R>) -> Res<capture::RecordingState> {
    capture::stop_recording(&app)
}

#[tauri::command]
pub fn capture_recording_state<R: Runtime>(app: AppHandle<R>) -> capture::RecordingState {
    capture::recording_state(&app)
}

// ---------------------------------------------------------------------------
// Misc
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn validate_hotkey(accelerator: String) -> Res<String> {
    crate::hotkeys::validate(&accelerator)
}

/// Quit from the UI, giving background workers a chance to stop first.
#[tauri::command]
pub fn quit_app<R: Runtime>(app: AppHandle<R>) {
    crate::shutdown(&app);
    app.exit(0);
}

// ---------------------------------------------------------------------------
// Launcher + calculator
// ---------------------------------------------------------------------------

/// Every installed application, for the palette's launcher provider.
///
/// Cached in Rust (see [`crate::apps::AppIndex`]) so this is cheap to call on
/// every palette open, but never on every keystroke — the frontend fetches once
/// and filters locally.
#[tauri::command]
pub async fn list_installed_apps(
    index: tauri::State<'_, crate::apps::AppIndex>,
) -> Res<Vec<crate::apps::InstalledApp>> {
    let index = index.inner().clone();
    tokio::task::spawn_blocking(move || index.all())
        .await
        .map_err(|e| format!("could not list applications: {e}"))
}

/// Launch an application by bundle path.
#[tauri::command]
pub async fn launch_app(path: String) -> ExecOutcome {
    shortcuts::exec::open_app(&path, &[]).await
}

/// Evaluate an arithmetic expression, or `None` if the input is not maths.
#[tauri::command]
pub fn calculate(input: String) -> Option<CalcResult> {
    crate::calc::evaluate(&input).map(|c| CalcResult {
        expression: c.expression,
        display: c.display,
        value: c.value,
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalcResult {
    pub expression: String,
    pub display: String,
    pub value: f64,
}

/// Current state of the Hermes Agent installation, for Settings.
#[tauri::command]
pub async fn hermes_status() -> crate::agent::hermes::HermesStatus {
    crate::agent::hermes::status().await
}

// ---------------------------------------------------------------------------
// System monitor
// ---------------------------------------------------------------------------

/// One refreshed snapshot of the machine: load, memory, disks, network, and the
/// `limit` heaviest processes.
#[tauri::command]
pub fn system_snapshot(
    monitor: tauri::State<'_, crate::sysmon::SysMonitor>,
    limit: Option<usize>,
    sort_by_memory: Option<bool>,
) -> crate::sysmon::SystemSnapshot {
    monitor.snapshot(
        limit.unwrap_or(40).clamp(1, 500),
        sort_by_memory.unwrap_or(false),
    )
}

/// Ask a process to quit. `force` sends SIGKILL rather than SIGTERM.
///
/// Separate from [`system_snapshot`] on purpose: reading the process list and
/// terminating something out of it should never be the same call.
#[tauri::command]
pub fn system_kill(
    monitor: tauri::State<'_, crate::sysmon::SysMonitor>,
    pid: u32,
    force: Option<bool>,
) -> Res<()> {
    monitor.kill(pid, force.unwrap_or(false))
}

/// Probe this machine for AI runtimes that are already installed and serving.
///
/// Read-only: it reports what it found and never edits settings. Connecting a
/// result is a separate, explicit step in the UI, because adding a backend and
/// repointing the `/` prefix is a change someone should choose rather than
/// have happen as a side effect of looking.
#[tauri::command]
pub async fn detect_local_ai() -> crate::agent::discover::LocalAiScan {
    crate::agent::discover::scan().await
}

/// Open Terminal with the Hermes install command pre-typed.
///
/// Deliberately does *not* run it: piping a remote script into a shell is the
/// user's decision to make, and they should see the command before it runs.
#[tauri::command]
pub async fn open_hermes_installer() -> Res<ExecOutcome> {
    let command = settings::HERMES_INSTALL_COMMAND;
    let script = format!(
        r#"tell application "Terminal"
    activate
    do script "{command}"
end tell"#
    );
    match shortcuts::exec::run_applescript(&script).await {
        Ok(_) => Ok(ExecOutcome {
            ok: true,
            message: "Opened Terminal with the install command.".into(),
            frontend_action: None,
            output: None,
        }),
        Err(e) => Err(format!("Could not open Terminal: {e}")),
    }
}
