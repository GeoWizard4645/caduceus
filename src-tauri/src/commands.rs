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
use crate::capture;
use crate::chat;
use crate::clipboard::{self, ClipboardEntry, ClipboardStore, TransitionReport};
use crate::extensions;
use crate::notes;
use crate::palette::{self, DispatchOutcome};
use crate::settings::{self, secrets, BackendConfig, Settings, SettingsManager};
use crate::shortcuts::{self, BrowserInstall, ExecOutcome};
use crate::tools;
use crate::voice;
use crate::window;

type Res<T> = Result<T, String>;

/// Run a tool on a blocking thread and hand back its outcome.
///
/// Tauri only moves a command off the calling thread when it is `async`, and on
/// macOS the calling thread is the one drawing every window. Anything that
/// shells out — AppleScript, `docker`, `lsof`, `dig` — therefore has to go
/// through here, or one wedged subprocess beachballs the whole app.
async fn blocking_outcome<F>(work: F) -> tools::ToolOutcome
where
    F: FnOnce() -> tools::ToolOutcome + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .unwrap_or_else(|e| tools::ToolOutcome::err(format!("It could not be run: {e}")))
}

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
    if previous.general.staff_edge != next.general.staff_edge
        && next.general.staff_position.is_none()
    {
        let _ = window::position_staff(&app, &settings);
    }
    if previous.general.staff_visible != next.general.staff_visible {
        window::set_staff_visible(&app, &settings, next.general.staff_visible)?;
    }
    if previous.appearance != next.appearance {
        let _ = window::sync_staff_window(&app, &settings);
    }
    if previous.general.onboarding_done != next.general.onboarding_done {
        let _ = window::position_staff(&app, &settings);
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
    pub tts_backends: Vec<voice::TtsAvailability>,
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
        tts_backends: voice::tts::all_availability(&cfg.voice),
        browsers: shortcuts::detect_browsers(),
        clipboard_entries: store.as_ref().and_then(|s| s.count().ok()).unwrap_or(0),
        clipboard_bytes: store
            .as_ref()
            .and_then(|s| s.total_bytes().ok())
            .unwrap_or(0),
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

#[tauri::command]
pub async fn check_for_update() -> crate::update::UpdateCheck {
    crate::update::check().await
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

#[tauri::command]
pub fn set_tts_api_key(key: String) -> Res<bool> {
    secrets::set_tts_api_key(key.trim()).map_err(|e| e.to_string())?;
    Ok(secrets::has_tts_api_key())
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
/// What bounds this is that it runs a command the user just typed into that
/// field themselves — not the window it is called from. Tauri's ACL scopes
/// plugin permissions per window; an app's own `#[tauri::command]`s are
/// reachable from every window on the `windows` list in
/// `capabilities/default.json`, and there is no per-route scoping at all.
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
///
/// Marks a permission flow as active first: bringing System Settings forward
/// takes focus away from the Command Center exactly like switching to any other
/// app, and without the mark the palette's blur handler would hide it on the
/// spot — see [`window::PermissionFlowActive`].
#[tauri::command]
pub async fn open_system_settings<R: Runtime>(app: AppHandle<R>, pane: String) -> ExecOutcome {
    if let Some(state) = app.try_state::<window::PermissionFlowActive>() {
        state.mark_active();
    }
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
pub fn toggle_command_center<R: Runtime>(app: AppHandle<R>, source: Option<String>) -> Res<()> {
    window::toggle_command_center(&app, source.unwrap_or_else(|| "other".into()))
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
pub fn toggle_staff<R: Runtime>(
    app: AppHandle<R>,
    settings: tauri::State<'_, SettingsManager>,
) -> Res<bool> {
    let visible = window::toggle_staff(&app, &settings)?;
    crate::tray::refresh(&app);
    Ok(visible)
}

/// Quit and reopen Caduceus — needed after some macOS privacy grants (Screen Recording).
#[tauri::command]
pub fn relaunch_app<R: Runtime>(app: AppHandle<R>) {
    let _ = window::relaunch::schedule_relaunch();
    app.exit(0);
}

/// Gracefully restart the running process, preserving Tauri's normal teardown.
#[tauri::command]
pub fn restart_app<R: Runtime>(app: AppHandle<R>) {
    app.request_restart();
}

/// Called by the staff after a drag ends, to remember where it was left.
#[tauri::command]
pub fn save_staff_position<R: Runtime>(
    app: AppHandle<R>,
    settings: tauri::State<'_, SettingsManager>,
) -> Res<()> {
    window::persist_staff_position(&app, &settings)
}

#[tauri::command]
pub fn refresh_staff_layout<R: Runtime>(
    app: AppHandle<R>,
    settings: tauri::State<'_, SettingsManager>,
) -> Res<()> {
    window::refresh_staff_layout(&app, &settings)
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
    use tauri::Emitter;
    let token = crate::staff_mark::import_mark(&app, std::path::Path::new(&source_path))?;
    let _ = app.emit(crate::staff_mark::STAFF_MARK_CHANGED_EVENT, ());
    Ok(token)
}

#[tauri::command]
pub fn clear_staff_mark<R: Runtime>(app: AppHandle<R>) -> Res<()> {
    use tauri::Emitter;
    crate::staff_mark::clear_mark(&app)?;
    let _ = app.emit(crate::staff_mark::STAFF_MARK_CHANGED_EVENT, ());
    Ok(())
}

/// Where the Command Center's background image is on disk, if there is one.
#[tauri::command]
pub fn resolve_backdrop<R: Runtime>(app: AppHandle<R>, token: String) -> Res<Option<String>> {
    Ok(crate::backdrop::resolve_path(&app, &token).map(|p| p.to_string_lossy().into_owned()))
}

#[tauri::command]
pub fn import_backdrop<R: Runtime>(app: AppHandle<R>, source_path: String) -> Res<String> {
    crate::backdrop::import(&app, std::path::Path::new(&source_path))
}

#[tauri::command]
pub fn clear_backdrop<R: Runtime>(app: AppHandle<R>) -> Res<()> {
    crate::backdrop::clear(&app)
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
    let Some((kind, bytes)) = store
        .get_content(id, key.as_ref())
        .map_err(|e| e.to_string())?
    else {
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

/// Start an agent session that can call MCP tools.
///
/// Distinct from [`agent_start_session`], which hands the task to a
/// computer-use harness. This one runs Caduceus's own tool-calling loop
/// against the primary backend, so the tools it can reach are whatever the
/// MCP host currently has connected. Stopping and approving both go through
/// `agent_stop_session` / the existing approval command — the session is
/// registered with the same [`AgentRuntime`].
#[tauri::command]
pub fn agent_start_tool_session<R: Runtime>(
    app: AppHandle<R>,
    runtime: tauri::State<'_, AgentRuntime>,
    settings: tauri::State<'_, SettingsManager>,
    task: String,
) -> Res<String> {
    agent::start_tool_session(
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

/// Does this backend's model have enough context for tool calling? See
/// `agent::context`'s module doc for what "enough" means and why it takes a
/// live probe rather than a hard-coded table to answer.
#[tauri::command]
pub async fn agent_context_check<R: Runtime>(
    app: AppHandle<R>,
    settings: tauri::State<'_, SettingsManager>,
    backend_id: String,
) -> Res<agent::context::ContextCheck> {
    let backend_config = resolve_backend_config(&settings, &backend_id)?;
    Ok(agent::context::check(&app, &backend_config.model, &backend_config.base_url).await)
}

/// Fix a backend whose model was reported [`agent::context::ContextCheck::Insufficient`]
/// or [`agent::context::ContextCheck::Unknown`] by `agent_context_check` — see
/// `agent::context::remediate`'s doc for the mechanism (an Ollama model
/// variant with a raised context window, reused if a suitable one already
/// exists).
#[tauri::command]
pub async fn agent_context_remediate<R: Runtime>(
    app: AppHandle<R>,
    settings: tauri::State<'_, SettingsManager>,
    backend_id: String,
) -> Res<agent::context::RemediationOutcome> {
    let backend_config = resolve_backend_config(&settings, &backend_id)?;
    agent::context::remediate(&app, &backend_config.model, &backend_config.base_url).await
}

/// Shared by every `agent_*` command that takes a `backend_id` rather than a
/// full config — `agent_test_backend` and `agent_list_models` above each had
/// their own copy of this lookup; pulled out here rather than adding a third
/// (and now fourth) copy for the two context commands.
fn resolve_backend_config(settings: &SettingsManager, backend_id: &str) -> Res<BackendConfig> {
    settings
        .get()
        .agents
        .backends
        .iter()
        .find(|b| b.id == backend_id)
        .cloned()
        .ok_or_else(|| format!("no backend with id \u{201c}{backend_id}\u{201d}"))
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
///
/// Routed through the same non-blocking path the hotkey uses, so the recording
/// indicator is up before the helper handshake begins rather than after it.
#[tauri::command]
pub fn voice_start<R: Runtime>(
    app: AppHandle<R>,
    settings: tauri::State<'_, SettingsManager>,
) -> Res<()> {
    crate::hotkeys::start_push_to_talk(&app, &settings);
    Ok(())
}

/// Hold the recording, or let it go again. Returns the new paused state.
#[tauri::command]
pub fn voice_pause<R: Runtime>(
    app: AppHandle<R>,
    runtime: tauri::State<'_, voice::VoiceRuntime>,
    paused: bool,
) -> Res<bool> {
    let now = runtime.set_paused(paused)?;
    let _ = app.emit(
        voice::VOICE_STATE_EVENT,
        if now {
            voice::VoiceState::Paused
        } else {
            voice::VoiceState::Recording
        },
    );
    Ok(now)
}

/// End the recording and transcribe, from the recording HUD.
///
/// Deliberately not the same as [`voice_stop`]: this returns immediately and
/// lets the result arrive on `VOICE_RESULT_EVENT`, so the HUD's Stop button can
/// never be the thing that is waiting on transcription.
#[tauri::command]
pub fn voice_finish<R: Runtime>(
    app: AppHandle<R>,
    settings: tauri::State<'_, SettingsManager>,
) -> Res<()> {
    crate::hotkeys::stop_push_to_talk(&app, &settings);
    Ok(())
}

#[tauri::command]
pub async fn voice_stop(
    runtime: tauri::State<'_, voice::VoiceRuntime>,
    settings: tauri::State<'_, SettingsManager>,
) -> Res<Option<voice::RoutedText>> {
    // `stop` waits on the helper to flush its last transcript, which a wedged
    // one never does — the same reason `stop_push_to_talk` steps off the thread.
    let runtime = (*runtime).clone();
    let Some(outcome) = tauri::async_runtime::spawn_blocking(move || runtime.stop())
        .await
        .map_err(|e| format!("The recording could not be stopped: {e}"))?
    else {
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
pub fn voice_cancel<R: Runtime>(app: AppHandle<R>, runtime: tauri::State<'_, voice::VoiceRuntime>) {
    runtime.cancel();
    window::recorder::hide(&app);
}

#[tauri::command]
pub fn voice_is_recording(runtime: tauri::State<'_, voice::VoiceRuntime>) -> bool {
    runtime.is_recording()
}

// ---------------------------------------------------------------------------
// Text-to-speech
// ---------------------------------------------------------------------------

/// Speak `text` aloud with the configured backend.
///
/// Resolves once playback finishes or is cut off by a barge-in (see
/// [`voice::TtsRuntime::speak`]) — the frontend should not block anything
/// user-visible on this promise. It exists so a caller that *does* want to
/// know when Caduceus has gone quiet again, to re-enable a "stop talking"
/// button say, can await it, not because anything here requires waiting.
#[tauri::command]
pub async fn speak<R: Runtime>(
    app: AppHandle<R>,
    settings: tauri::State<'_, SettingsManager>,
    tts: tauri::State<'_, voice::TtsRuntime>,
    text: String,
) -> Res<()> {
    let voice_settings = settings.with(|s| s.voice.clone());
    let _ = app.emit(voice::TTS_STATE_EVENT, voice::TtsState::Speaking);
    let result = tts.speak(&text, &voice_settings).await;
    let _ = app.emit(voice::TTS_STATE_EVENT, voice::TtsState::Idle);
    result.map_err(|e| e.to_string())
}

/// Cut off whatever is currently being spoken, from a manual "stop talking"
/// control rather than a barge-in. Safe to call when nothing is speaking.
#[tauri::command]
pub fn stop_speaking(tts: tauri::State<'_, voice::TtsRuntime>) {
    tts.stop();
}

/// List installed system voices, for the Settings voice picker. Empty on
/// non-macOS or if `say` could not be run.
#[tauri::command]
pub async fn list_speech_voices() -> Vec<String> {
    voice::tts::list_say_voices().await
}

/// Type text into whatever app currently has keyboard focus.
///
/// The voice-typing page's "type it where my cursor is". The Command Center is
/// a non-activating panel, so the app behind it never lost keyboard focus and
/// System Events' `keystroke` lands exactly where the caret already is. Same
/// mechanism as the text expander, for the same reason it uses it: simulated
/// typing is the only insertion that behaves identically in every app.
#[tauri::command]
pub async fn type_text(text: String) -> Res<()> {
    if text.trim().is_empty() {
        return Err("There is nothing to type yet.".into());
    }
    // `insert_expansion` shells out to osascript and waits; not on this thread.
    tauri::async_runtime::spawn_blocking(move || {
        crate::tools::expander::insert_expansion(&crate::tools::expander::ExpansionOutcome {
            text,
            cursor_offset: None,
        })
    })
    .await
    .map_err(|e| format!("Typing failed: {e}"))?
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
pub async fn capture_screenshot(save_to_downloads: Option<bool>) -> Res<capture::ScreenshotResult> {
    // `screencapture` is a separate process that can wait on a TCC prompt, so it
    // does not belong on the async runtime's threads.
    tauri::async_runtime::spawn_blocking(move || {
        capture::screenshot_full(save_to_downloads.unwrap_or(true))
    })
    .await
    .map_err(|e| format!("The screenshot could not be taken: {e}"))?
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
    sort_by_name: Option<bool>,
) -> crate::sysmon::SystemSnapshot {
    monitor.snapshot(
        limit.unwrap_or(40).clamp(1, 500),
        sort_by_memory.unwrap_or(false),
        sort_by_name.unwrap_or(false),
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

// ---------------------------------------------------------------------------
// Chat
// ---------------------------------------------------------------------------

/// Ask the primary backend inside a conversation, persisting both turns.
///
/// `conversation_id` of `None` continues the most recent thread, or starts one
/// if there is none — which is what a bare `/` in the palette does.
///
/// Tokens are emitted on [`chat::CHAT_CHUNK_EVENT`] as they arrive so the chat
/// UI can type live; the returned [`chat::ChatReply`] is the finished turn.
#[tauri::command]
pub async fn chat_ask<R: Runtime>(
    app: AppHandle<R>,
    conversation_id: Option<i64>,
    prompt: String,
) -> Res<chat::ChatReply> {
    let store = app
        .try_state::<chat::ChatStore>()
        .ok_or("Chat history is unavailable.")?
        .inner()
        .clone();
    let settings = app
        .try_state::<SettingsManager>()
        .ok_or("Settings are unavailable.")?
        .inner()
        .clone();

    let id = match conversation_id {
        Some(id) => id,
        None => chat::active_conversation(&store).map_err(|e| e.user_message())?,
    };

    let emit_app = app.clone();
    let reply = chat::ask_streaming(&store, &settings, id, &prompt, move |chunk| {
        if let Err(e) = emit_app.emit(chat::CHAT_CHUNK_EVENT, &chunk) {
            log::warn!("could not emit chat chunk: {e}");
        }
    })
    .await
    .map_err(|e| e.user_message())?;

    let _ = app.emit(chat::CHAT_CHANGED_EVENT, id);
    Ok(reply)
}

#[tauri::command]
pub fn chat_conversations<R: Runtime>(app: AppHandle<R>) -> Res<Vec<chat::Conversation>> {
    let store = app
        .try_state::<chat::ChatStore>()
        .ok_or("Chat history is unavailable.")?;
    store.conversations(200).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn chat_messages<R: Runtime>(
    app: AppHandle<R>,
    conversation_id: i64,
) -> Res<Vec<chat::ChatMessage>> {
    let store = app
        .try_state::<chat::ChatStore>()
        .ok_or("Chat history is unavailable.")?;
    store.messages(conversation_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn chat_new_conversation<R: Runtime>(app: AppHandle<R>) -> Res<i64> {
    let store = app
        .try_state::<chat::ChatStore>()
        .ok_or("Chat history is unavailable.")?;
    // Opening a second empty thread before using the first would leave a blank
    // row in the list for every stray click on "New chat".
    let _ = store.prune_empty();
    store.create_conversation().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn chat_delete_conversation<R: Runtime>(app: AppHandle<R>, conversation_id: i64) -> Res<()> {
    let store = app
        .try_state::<chat::ChatStore>()
        .ok_or("Chat history is unavailable.")?;
    store
        .delete_conversation(conversation_id)
        .map_err(|e| e.to_string())?;
    let _ = app.emit(chat::CHAT_CHANGED_EVENT, conversation_id);
    Ok(())
}

#[tauri::command]
pub fn chat_clear<R: Runtime>(app: AppHandle<R>) -> Res<()> {
    let store = app
        .try_state::<chat::ChatStore>()
        .ok_or("Chat history is unavailable.")?;
    store.clear().map_err(|e| e.to_string())?;
    let _ = app.emit(chat::CHAT_CHANGED_EVENT, 0i64);
    Ok(())
}

/// Open the full chat window, optionally on a specific thread.
#[tauri::command]
pub fn open_chat_window<R: Runtime>(app: AppHandle<R>, conversation_id: Option<i64>) -> Res<()> {
    window::open_chat(&app, conversation_id)
}

// ---------------------------------------------------------------------------
// Notes
// ---------------------------------------------------------------------------

/// Append text to Apple Notes as a new note.
#[tauri::command]
pub async fn add_to_notes(title: Option<String>, body: String) -> Res<ExecOutcome> {
    if body.trim().is_empty() {
        return Err("There is nothing to save.".into());
    }
    let title = title.unwrap_or_default();
    // osascript blocks, and the first call waits on a permission sheet.
    let made = tauri::async_runtime::spawn_blocking(move || notes::add(&title, &body))
        .await
        .map_err(|e| format!("Could not reach Notes: {e}"))??;

    Ok(ExecOutcome {
        ok: true,
        message: format!("Saved “{made}” to Notes."),
        frontend_action: None,
        output: None,
    })
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

/// Convert text between cases. Returns the converted text for the UI to copy.
#[tauri::command]
pub fn change_case(text: String, case: tools::text::Case) -> String {
    tools::text::convert(&text, case)
}

/// The cases the UI can offer, with labels, so the list lives in one place.
#[tauri::command]
pub fn case_options() -> Vec<(tools::text::Case, String)> {
    tools::text::Case::all()
        .iter()
        .map(|c| (*c, c.label().to_string()))
        .collect()
}

#[tauri::command]
pub fn copy_latest_download() -> tools::ToolOutcome {
    tools::copy_latest_download()
}

#[tauri::command]
pub fn open_latest_download() -> tools::ToolOutcome {
    tools::open_latest_download()
}

#[tauri::command]
pub fn copy_finder_path() -> tools::ToolOutcome {
    tools::copy_finder_path()
}

#[tauri::command]
pub fn eject_disks() -> tools::ToolOutcome {
    tools::eject_all_disks()
}

#[tauri::command]
pub fn stay_awake(
    awake: tauri::State<'_, tools::awake::AwakeRuntime>,
    on: bool,
) -> tools::ToolOutcome {
    // Routed through the session runtime rather than tools::set_awake, so the
    // quick toggle and the Manage window's sessions are one state, not two
    // caffeinate processes fighting over who is keeping the machine up.
    if on {
        awake.start(None, false)
    } else {
        awake.stop()
    }
}

#[tauri::command]
pub fn stay_awake_state(awake: tauri::State<'_, tools::awake::AwakeRuntime>) -> bool {
    awake.status().active
}

/// Spotlight (`mdfind`) for the palette's file rows.
///
/// Async + `spawn_blocking` on purpose: the sync form ran on the macOS UI
/// thread, and every ≥2-character keystroke paid for a full `mdfind` wait —
/// which is exactly the rainbow wheel. Same rule as `list_installed_apps` and
/// the browser/menu providers.
#[tauri::command]
pub async fn search_files(query: String, limit: Option<usize>) -> Vec<tools::FileHit> {
    let limit = limit.unwrap_or(40);
    tauri::async_runtime::spawn_blocking(move || tools::search_files(&query, limit))
        .await
        .unwrap_or_default()
}

/// Which backend a prompt would go to, and why.
///
/// A preview rather than a side effect: routing that happens invisibly is
/// routing nobody trusts, and "why did that take eight seconds" has to be a
/// question with an answer. Classification is pure and local, so this costs
/// nothing and never reaches a model.
#[tauri::command]
pub fn routing_preview(
    settings: tauri::State<'_, SettingsManager>,
    prompt: String,
) -> Res<tools::routing::RoutingDecision> {
    let agents = settings.get().agents;
    let ctx = tools::routing::RoutingContext {
        backends: &agents.backends,
        primary_backend_id: agents.primary_backend_id.as_deref(),
        override_backend_id: agents.routing_override_backend_id.as_deref(),
        auto_routing_enabled: agents.auto_routing_enabled,
    };
    tools::routing::route(&prompt, &ctx, tools::routing::latency_tracker())
        .ok_or_else(|| "No backend is configured yet — add one in Settings → AI.".to_string())
}

// ---------------------------------------------------------------------------
// Semantic search
// ---------------------------------------------------------------------------
//
// The index and its cancel flag are process-wide, held behind a `OnceLock`
// rather than Tauri managed state. Two reasons: opening the SQLite index is
// fallible and doing it in `setup()` would mean a corrupt index stops the whole
// app from starting; and `semantic_index_cancel` has to reach a sync that is
// *already running*, which means both calls need the same flag instance rather
// than one handed in per invocation.

use std::sync::OnceLock;

struct SemanticState {
    index: tools::semantic::SemanticIndex,
    cancel: tools::semantic::CancelFlag,
}

fn semantic_state<R: Runtime>(app: &AppHandle<R>) -> Res<&'static SemanticState> {
    static STATE: OnceLock<Result<SemanticState, String>> = OnceLock::new();
    STATE
        .get_or_init(|| {
            let dir = app_data(app)?;
            std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create {dir:?}: {e}"))?;
            let index = tools::semantic::SemanticIndex::open(dir.join("semantic-index.sqlite"))?;
            Ok(SemanticState {
                index,
                cancel: tools::semantic::CancelFlag::new(),
            })
        })
        .as_ref()
        .map_err(|e| e.clone())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticIndexSnapshot {
    document_count: usize,
    roots: Vec<String>,
}

#[tauri::command]
pub fn semantic_index_stats<R: Runtime>(app: AppHandle<R>) -> Res<SemanticIndexSnapshot> {
    let state = semantic_state(&app)?;
    Ok(SemanticIndexSnapshot {
        document_count: state.index.document_count()?,
        roots: tools::semantic::default_roots()
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
    })
}

/// Index one bounded chunk. The caller loops while `truncated` is set, which is
/// what keeps a first run over a large home directory interruptible and
/// answerable rather than one opaque multi-minute call.
#[tauri::command]
pub async fn semantic_index_sync<R: Runtime>(
    app: AppHandle<R>,
) -> Res<tools::semantic::IndexStats> {
    let state = semantic_state(&app)?;
    state.cancel.reset();
    state
        .index
        .sync(
            &tools::semantic::IndexConfig::default(),
            state.cancel.clone(),
        )
        .await
}

#[tauri::command]
pub fn semantic_index_cancel<R: Runtime>(app: AppHandle<R>) -> Res<()> {
    semantic_state(&app)?.cancel.cancel();
    Ok(())
}

#[tauri::command]
pub async fn semantic_search<R: Runtime>(
    app: AppHandle<R>,
    query: String,
    limit: Option<usize>,
) -> Res<Vec<tools::semantic::SearchHit>> {
    let state = semantic_state(&app)?;
    state.index.search(&query, limit.unwrap_or(30)).await
}

// ---------------------------------------------------------------------------
// Window presets, menus, contacts, fonts, recent files
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn window_preset_save<R: Runtime>(
    app: AppHandle<R>,
    name: String,
) -> Res<tools::knowledge::WindowPreset> {
    tauri::async_runtime::spawn_blocking(move || tools::knowledge::window_preset_save(&app, name))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn window_preset_restore<R: Runtime>(
    app: AppHandle<R>,
    name: String,
) -> Res<tools::knowledge::PresetRestoreOutcome> {
    tauri::async_runtime::spawn_blocking(move || {
        tools::knowledge::window_preset_restore(&app, name)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn window_preset_list<R: Runtime>(app: AppHandle<R>) -> Vec<tools::knowledge::WindowPreset> {
    tools::knowledge::window_preset_list(&app)
}

#[tauri::command]
pub fn window_preset_delete<R: Runtime>(app: AppHandle<R>, name: String) -> Res<()> {
    tools::knowledge::window_preset_delete(&app, name)
}

/// Every menu item in the frontmost app, so "Export as PDF" is searchable
/// rather than three levels into a menu you have to remember the shape of.
#[tauri::command]
pub async fn menu_bar_items() -> Res<Vec<tools::knowledge::MenuItem>> {
    tauri::async_runtime::spawn_blocking(tools::knowledge::frontmost_menu_items)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn menu_bar_invoke(path: Vec<String>) -> Res<()> {
    tauri::async_runtime::spawn_blocking(move || tools::knowledge::invoke_frontmost_menu_item(path))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn contacts_search(query: String) -> Res<Vec<tools::knowledge::ContactHit>> {
    tauri::async_runtime::spawn_blocking(move || tools::knowledge::search_contacts(&query))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn contacts_copy(value: String) -> tools::ToolOutcome {
    tools::knowledge::contacts_copy(value)
}

#[tauri::command]
pub fn list_fonts() -> Vec<tools::knowledge::FontInfo> {
    tools::knowledge::list_installed_fonts()
}

#[tauri::command]
pub async fn recent_files(
    days: Option<u32>,
    limit: Option<usize>,
) -> Res<Vec<tools::knowledge::RecentFile>> {
    tauri::async_runtime::spawn_blocking(move || tools::knowledge::recent_files(days, limit))
        .await
        .map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------------------
// Images
// ---------------------------------------------------------------------------
//
// Async, every one, unlike the older `convert_image` above. That one is a sync
// `#[tauri::command]` that shells out to `sips` on the calling thread — which
// on macOS is the thread drawing every window, so a large photo beachballs the
// app. This file's own header says as much; these are written the way that one
// should have been.

#[tauri::command]
pub async fn compress_image(
    path: String,
    format: Option<String>,
    quality: Option<u8>,
    max_dimension: Option<u32>,
) -> tools::ToolOutcome {
    blocking_outcome(move || {
        tools::images::compress_or_convert(&path, format.as_deref(), quality, max_dimension)
    })
    .await
}

#[tauri::command]
pub async fn resize_image_to_preset(
    path: String,
    preset: tools::images::ImagePreset,
) -> tools::ToolOutcome {
    blocking_outcome(move || tools::images::resize_to_preset(&path, preset)).await
}

/// Strip EXIF, including GPS, before sharing a photo.
///
/// Decode-and-re-encode rather than asking `sips` to delete the properties:
/// `sips --deleteProperty all` refuses outright, and a format round-trip
/// through it *carries GPS through* rather than dropping it. A metadata
/// cleaner that quietly leaves the coordinates in is worse than none at all.
#[tauri::command]
pub async fn strip_image_metadata(path: String) -> tools::ToolOutcome {
    blocking_outcome(move || tools::images::strip_metadata(&path)).await
}

#[tauri::command]
pub async fn find_duplicate_images(
    dir: String,
    max_distance: Option<u32>,
) -> Res<Vec<tools::images::DuplicateGroup>> {
    tauri::async_runtime::spawn_blocking(move || {
        tools::images::find_duplicate_images(&dir, max_distance)
    })
    .await
    .map_err(|e| format!("Could not scan that folder: {e}"))?
}

/// Whether background removal is available at all, so the UI can grey it out
/// rather than offering a button that always fails.
#[tauri::command]
pub fn background_removal_available() -> bool {
    tools::images::background_removal_available()
}

// ---------------------------------------------------------------------------
// Screen perception
// ---------------------------------------------------------------------------
//
// OCR runs on-device through Apple Vision and only the extracted *text*
// reaches a model — never the screenshot. That ordering is the privacy
// property, not an implementation detail: "Caduceus can read your screen"
// and "Caduceus uploads your screen" are very different sentences, and only
// the first one is true here.

#[tauri::command]
pub async fn vision_describe_region(
    settings: tauri::State<'_, SettingsManager>,
    question: String,
) -> Res<tools::vision::VisionAnswer> {
    // `to_string()` and not a rewrite: the Screen Recording sentence has to
    // reach the webview intact, because `permissionFromMessage` matches on it
    // to open the grant walkthrough. Reword it here and the error becomes a
    // dead end instead of a guided fix.
    tools::vision::describe_region(&settings, &question)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn vision_describe_active_window(
    settings: tauri::State<'_, SettingsManager>,
    question: String,
) -> Res<tools::vision::VisionAnswer> {
    tools::vision::describe_active_window(&settings, &question)
        .await
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Documents: PDFs, articles, video
// ---------------------------------------------------------------------------
//
// Every one of these ends in a model call, so every one is `async` and every
// one reports "no backend configured" rather than failing quietly — these are
// the features most likely to be the first thing a new user tries.

#[tauri::command]
pub async fn pdf_summary(settings: tauri::State<'_, SettingsManager>, path: String) -> Res<String> {
    tools::documents::pdf_summary(&settings, &path).await
}

#[tauri::command]
pub async fn pdf_ask(
    settings: tauri::State<'_, SettingsManager>,
    path: String,
    question: String,
) -> Res<String> {
    tools::documents::pdf_ask(&settings, &path, &question).await
}

#[tauri::command]
pub async fn article_summary(
    settings: tauri::State<'_, SettingsManager>,
    url: String,
) -> Res<String> {
    tools::documents::article_summary(&settings, &url).await
}

#[tauri::command]
pub async fn youtube_summary(
    settings: tauri::State<'_, SettingsManager>,
    url: String,
) -> Res<String> {
    tools::documents::youtube_summary(&settings, &url).await
}

// ---------------------------------------------------------------------------
// The second tool bench
// ---------------------------------------------------------------------------
//
// `run_extra_tool` mirrors `run_tool` exactly rather than extending `ToolId`.
// Adding forty-odd variants to one enum was already at the point where the
// enum was the hardest thing in the file to read, and these are a separate
// bench of tools rather than more of the same.

#[tauri::command]
pub fn run_extra_tool(id: tools::devextra::ExtraToolId, input: String) -> tools::dev::ToolResult {
    tools::devextra::run(id, &input)
}

#[tauri::command]
pub async fn run_curl(command: String) -> tools::devextra::HttpPlaygroundResult {
    tools::devextra::execute(&command).await
}

/// Read-only: reports the repo's state and drafts a message. Never stages,
/// never commits — the point is to hand you a message, not to act for you.
#[tauri::command]
pub async fn git_commit_assist(
    settings: tauri::State<'_, SettingsManager>,
    repo_path: String,
) -> Res<tools::devextra::GitCommitAssist> {
    Ok(tools::devextra::git_commit_assist(&settings, &repo_path).await)
}

#[tauri::command]
pub fn inspect_dependencies(manifest_path: String) -> Res<tools::devextra::DependencyReport> {
    tools::devextra::inspect_dependencies(&manifest_path)
}

// ---------------------------------------------------------------------------
// Calendar and reminders
// ---------------------------------------------------------------------------
//
// All four block on `osascript`, so all four go through `spawn_blocking` — on
// macOS the calling thread is the one drawing every window, and an Apple Event
// to an app showing a modal dialog does not return.

/// Create an Apple Calendar event from natural language ("next Tuesday at 1pm").
#[tauri::command]
pub async fn create_calendar_event(
    title: String,
    when: String,
    duration_minutes: Option<i64>,
    location: Option<String>,
    notes: Option<String>,
) -> Res<tools::calendar::CreatedEvent> {
    tauri::async_runtime::spawn_blocking(move || {
        tools::calendar::create_event(
            &title,
            &when,
            duration_minutes,
            location.as_deref(),
            notes.as_deref(),
        )
    })
    .await
    .map_err(|e| format!("Could not reach Calendar: {e}"))?
}

#[tauri::command]
pub async fn calendar_events_today() -> Res<Vec<tools::calendar::CalendarEvent>> {
    tauri::async_runtime::spawn_blocking(tools::calendar::events_today)
        .await
        .map_err(|e| format!("Could not reach Calendar: {e}"))?
}

/// `start` and `end` are `%Y-%m-%dT%H:%M`, so the webview never has to agree
/// with Rust about what a locale-formatted date means.
#[tauri::command]
pub async fn calendar_events_between(
    start: String,
    end: String,
) -> Res<Vec<tools::calendar::CalendarEvent>> {
    let parse = |raw: &str| {
        chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M")
            .map_err(|_| format!("“{raw}” is not a date Caduceus can read."))
    };
    let start = parse(&start)?;
    let end = parse(&end)?;
    tauri::async_runtime::spawn_blocking(move || tools::calendar::events_between(start, end))
        .await
        .map_err(|e| format!("Could not reach Calendar: {e}"))?
}

#[tauri::command]
pub async fn create_reminder(
    text: String,
    due: Option<String>,
) -> Res<tools::calendar::CreatedReminder> {
    tauri::async_runtime::spawn_blocking(move || {
        tools::calendar::create_reminder(&text, due.as_deref())
    })
    .await
    .map_err(|e| format!("Could not reach Reminders: {e}"))?
}

/// "Highlight & Act": run one transformation over a piece of text.
///
/// The webview names an *action*, never a prompt. Every prompt lives in
/// `tools::textai` where it is unit-tested, and where a compromised webview
/// cannot rewrite it into an arbitrary question asked with the user's own API
/// key. That is the whole reason this takes an enum rather than a string.
#[tauri::command]
pub async fn text_ai_run(
    settings: tauri::State<'_, SettingsManager>,
    action: tools::textai::TextAiAction,
    text: String,
    target_language: Option<String>,
) -> Res<String> {
    tools::textai::run(&settings, action, &text, target_language.as_deref())
        .await
        .map_err(|e| e.to_string())
}

/// Optimise a prompt for one target model.
///
/// Takes the same shape as `text_ai_run` and for the same reason: the webview
/// names a *target* and a *level*, both closed enums, never a prompt. Every
/// instruction the optimiser sends to a model lives in `tools::promptopt`,
/// where it is unit-tested and where a compromised webview cannot rewrite it
/// into an arbitrary question asked with the user's own key.
///
/// Long-running by nature — the model passes are several bounded round trips
/// to a local server — so this is `async` and the UI shows a spinner rather
/// than blocking a keystroke on it. The instant half is `prompt_estimate`.
#[tauri::command]
pub async fn prompt_optimize(
    settings: tauri::State<'_, SettingsManager>,
    raw: String,
    target: tools::promptopt::TargetModel,
    level: tools::promptopt::OptimizeLevel,
    use_model: bool,
    output_cap_words: Option<u32>,
) -> Res<tools::promptopt::OptimizedPrompt> {
    tools::promptopt::optimize(&settings, &raw, target, level, use_model, output_cap_words)
        .await
        .map_err(|e| e.user_message())
}

/// Count what a prompt costs on one target, with no model involved.
///
/// Separate from `prompt_optimize` because the Command Center calls this on
/// every keystroke to show a live token count, and a keystroke may never wait
/// on a network round trip. Pure arithmetic, microseconds, no I/O.
#[tauri::command]
pub fn prompt_estimate(
    raw: String,
    target: tools::promptopt::TargetModel,
) -> tools::promptopt::TokenEstimate {
    tools::promptopt::estimate(&raw, target)
}

/// Which model the optimiser's judgement passes would use, if switched on.
///
/// `None` means nothing usable is configured, and the toggle says so rather
/// than offering a switch that silently does nothing. Reads settings only — no
/// network, so the page can call it on open.
#[tauri::command]
pub fn prompt_optimizer_model(
    settings: tauri::State<'_, SettingsManager>,
) -> Option<tools::promptopt::OptimizerBackend> {
    tools::promptopt::optimizer_model(&settings)
}

/// Encode text as an SVG QR code.
/// Update in place by running the website's installer in Terminal.
///
/// Returns as soon as Terminal has been handed the script — the update itself
/// quits this process, so there is nothing here to wait for.
#[tauri::command]
pub fn run_installer_update() -> Res<()> {
    crate::update::run_installer()
}

/// The command the updater will run, so the UI can show it before it does.
#[tauri::command]
pub fn install_command() -> String {
    crate::update::INSTALL_COMMAND.to_string()
}

#[tauri::command]
pub fn generate_qr(text: String, ecc: Option<String>) -> Res<String> {
    tools::qr::svg(&text, ecc.as_deref().unwrap_or("medium"))
}

/// The URL of the frontmost browser's active tab, for "QR of this tab".
///
/// `None` when the frontmost app is not a browser Caduceus knows how to ask,
/// which is an ordinary answer rather than an error.
#[tauri::command]
pub async fn front_tab_url() -> Option<String> {
    tauri::async_runtime::spawn_blocking(tools::qr::front_tab_url)
        .await
        .unwrap_or(None)
}

#[tauri::command]
pub fn define_word(word: String) -> tools::ToolOutcome {
    tools::define_word(&word)
}

#[tauri::command]
pub fn convert_image(
    path: String,
    width: Option<u32>,
    format: Option<String>,
) -> tools::ToolOutcome {
    tools::convert_image(&path, width, format.as_deref())
}

// ---------------------------------------------------------------------------
// Extensions
// ---------------------------------------------------------------------------

fn app_data<R: Runtime>(app: &AppHandle<R>) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|e| format!("Could not find the app data directory: {e}"))
}

/// Describe a candidate file without installing or running it.
#[tauri::command]
pub fn inspect_extension<R: Runtime>(
    _app: AppHandle<R>,
    path: String,
) -> Res<extensions::Extension> {
    extensions::inspect(std::path::Path::new(&path))
}

/// Install a dropped `.js` file.
#[tauri::command]
pub fn install_extension<R: Runtime>(
    app: AppHandle<R>,
    path: String,
) -> Res<extensions::InstallReport> {
    let dir = app_data(&app)?;
    match extensions::install(std::path::Path::new(&path), &dir) {
        Ok(ext) => Ok(extensions::InstallReport {
            ok: true,
            message: format!("Installed “{}”.", ext.name),
            extension: Some(ext),
        }),
        Err(e) => Ok(extensions::InstallReport {
            ok: false,
            message: e,
            extension: None,
        }),
    }
}

#[tauri::command]
pub fn list_extensions<R: Runtime>(app: AppHandle<R>) -> Res<Vec<extensions::Extension>> {
    Ok(extensions::list(&app_data(&app)?))
}

#[tauri::command]
pub fn remove_extension<R: Runtime>(app: AppHandle<R>, id: String) -> Res<()> {
    extensions::remove(&id, &app_data(&app)?)
}

/// What can be removed and what is currently installed.
#[tauri::command]
pub fn uninstall_snapshot<R: Runtime>(
    app: AppHandle<R>,
) -> Res<crate::uninstall::UninstallSnapshot> {
    crate::uninstall::snapshot(&app)
}

/// Remove selected extensions and/or AI stack components.
#[tauri::command]
pub fn run_uninstall<R: Runtime>(
    app: AppHandle<R>,
    request: crate::uninstall::UninstallRequest,
) -> Res<crate::uninstall::UninstallResult> {
    crate::uninstall::run(&app, request)
}

/// Reveal the extensions folder in Finder.
#[tauri::command]
pub fn open_extensions_folder<R: Runtime>(app: AppHandle<R>) -> Res<()> {
    let dir = extensions::extensions_dir(&app_data(&app)?);
    let _ = std::fs::create_dir_all(&dir);
    std::process::Command::new("open")
        .arg(&dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open the folder: {e}"))
}

/// The permissions an extension is allowed to ask for, for the UI to show.
#[tauri::command]
pub fn extension_permissions() -> Vec<String> {
    extensions::PERMISSIONS
        .iter()
        .map(|s| s.to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// What an extension is allowed to do
// ---------------------------------------------------------------------------
//
// The extension itself runs in a Web Worker in the Command Center webview: no
// DOM, no Tauri IPC, and a CSP that permits `connect-src 'self'` and nothing
// else. Everything below is therefore the complete list of what an installed
// extension can reach, and each one is `ctx.<something>` in the sandbox.
//
// Every command that acts on the world takes the extension's id and calls
// `extensions::require` first, which re-reads the header off disk and refuses
// if it does not claim that permission. The sandbox refuses too, but the
// sandbox is JavaScript sitting beside the extension's own JavaScript; this is
// the check that decides.
//
// Note what is *not* here: no `run_apple_script`, no shell, no filesystem, no
// window control. Those commands exist on this same IPC surface, and an
// extension has no way to name them — the bridge in `extensionRuntime.ts` maps
// a fixed set of operation names onto the functions below and nothing else.

/// The source of an installed extension, for the sandbox to run.
#[tauri::command]
pub fn extension_source<R: Runtime>(app: AppHandle<R>, id: String) -> Res<String> {
    extensions::source(&id, &app_data(&app)?)
}

/// `ctx.clipboard.read()`
#[tauri::command]
pub async fn extension_clipboard_read<R: Runtime>(app: AppHandle<R>, id: String) -> Res<String> {
    let dir = app_data(&app)?;
    extensions::require(&id, &dir, "clipboard")?;
    tauri::async_runtime::spawn_blocking(|| {
        arboard::Clipboard::new()
            .and_then(|mut c| c.get_text())
            .map_err(|_| "There is no text on the clipboard.".to_string())
    })
    .await
    .map_err(|e| format!("Could not read the clipboard: {e}"))?
}

/// `ctx.clipboard.write(text)`
#[tauri::command]
pub async fn extension_clipboard_write<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    text: String,
) -> Res<()> {
    let dir = app_data(&app)?;
    extensions::require(&id, &dir, "clipboard")?;
    tauri::async_runtime::spawn_blocking(move || {
        arboard::Clipboard::new()
            .and_then(|mut c| c.set_text(text))
            .map_err(|e| format!("Could not write to the clipboard: {e}"))
    })
    .await
    .map_err(|e| format!("Could not write to the clipboard: {e}"))?
}

/// `ctx.fetch(url, init)`
#[tauri::command]
pub async fn extension_fetch<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    request: extensions::net::FetchRequest,
) -> Res<extensions::net::FetchResponse> {
    let dir = app_data(&app)?;
    extensions::require(&id, &dir, "network")?;
    extensions::net::fetch(request).await
}

/// `ctx.selection()`
#[tauri::command]
pub async fn extension_selection<R: Runtime>(app: AppHandle<R>, id: String) -> Res<Vec<String>> {
    let dir = app_data(&app)?;
    extensions::require(&id, &dir, "selection")?;
    tauri::async_runtime::spawn_blocking(|| {
        tools::files::finder_selection()
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    })
    .await
    .map_err(|e| format!("Could not ask Finder what is selected: {e}"))
}

/// `ctx.notify(text)`
///
/// The extension's name is the notification's title, so a banner you did not
/// expect names the thing that sent it rather than just saying "Caduceus".
#[tauri::command]
pub async fn extension_notify<R: Runtime>(app: AppHandle<R>, id: String, text: String) -> Res<()> {
    let dir = app_data(&app)?;
    let ext = extensions::require(&id, &dir, "notifications")?;

    // Both halves are escaped before they reach osascript. `text` is written by
    // the extension and `name` comes out of its header, so both are strings
    // Caduceus did not write, and an unescaped quote in either one turns the
    // rest of the line into script.
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        shortcuts::escape_applescript(&truncate(&text, 400)),
        shortcuts::escape_applescript(&truncate(&ext.name, 100)),
    );
    tauri::async_runtime::spawn_blocking(move || tools::apple::run_script(&script))
        .await
        .map_err(|e| format!("Could not show that notification: {e}"))?
        .map(|_| ())
}

/// `ctx.open(url)` — hand a link to the browser.
///
/// No permission gates this because it cannot act silently: opening a URL puts
/// a window in front of you. It is still restricted to `http(s)` by
/// `shortcuts::exec::open_url`, so it cannot be used to launch an application
/// through a custom scheme.
#[tauri::command]
pub async fn extension_open<R: Runtime>(
    app: AppHandle<R>,
    settings: tauri::State<'_, SettingsManager>,
    id: String,
    url: String,
) -> Res<()> {
    let dir = app_data(&app)?;
    extensions::load(&id, &dir)?;
    let browser = settings.get().command_center.browser.clone();
    let outcome = shortcuts::exec::open_url(&url, &browser).await;
    if outcome.ok {
        Ok(())
    } else {
        Err(outcome.message)
    }
}

/// `ctx.storage.get(key)`
#[tauri::command]
pub fn extension_storage_get<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    key: String,
) -> Res<Option<serde_json::Value>> {
    let dir = app_data(&app)?;
    extensions::load(&id, &dir)?;
    extensions::storage::get(&id, &dir, &key)
}

/// `ctx.storage.set(key, value)` — a `null` value removes the key.
#[tauri::command]
pub fn extension_storage_set<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    key: String,
    value: Option<serde_json::Value>,
) -> Res<()> {
    let dir = app_data(&app)?;
    extensions::load(&id, &dir)?;
    extensions::storage::set(&id, &dir, &key, value.filter(|v| !v.is_null()))
}

/// `ctx.shell.run(command, input?)`
#[tauri::command]
pub async fn extension_shell_run<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    command: String,
    input: Option<String>,
    timeout_secs: Option<u64>,
) -> Res<ExecOutcome> {
    let dir = app_data(&app)?;
    extensions::require(&id, &dir, "shell")?;
    let timeout = timeout_secs.unwrap_or(60).min(120);
    Ok(
        shortcuts::exec::run_command_capture(&command, input.as_deref().unwrap_or(""), timeout)
            .await,
    )
}

/// `ctx.automation.runAppleScript(script)`
#[tauri::command]
pub async fn extension_automation_script<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    script: String,
) -> Res<String> {
    let dir = app_data(&app)?;
    extensions::require(&id, &dir, "automation")?;
    tauri::async_runtime::spawn_blocking(move || tools::apple::run_script(&script))
        .await
        .map_err(|e| format!("Could not run the script: {e}"))?
}

/// `ctx.automation.runShortcut(name, input?)`
#[tauri::command]
pub async fn extension_automation_shortcut<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    name: String,
    input: Option<String>,
) -> Res<String> {
    let dir = app_data(&app)?;
    extensions::require(&id, &dir, "automation")?;
    let text = input.unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || tools::apple::run_shortcut(&name, &text))
        .await
        .map_err(|e| format!("Could not run the shortcut: {e}"))?
}

/// `ctx.files.read(path)` — under ~ or app data.
#[tauri::command]
pub fn extension_files_read<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    path: String,
) -> Res<String> {
    let dir = app_data(&app)?;
    extensions::require(&id, &dir, "files")?;
    extensions::files::read(&dir, &path)
}

/// `ctx.files.write(path, content)`
#[tauri::command]
pub fn extension_files_write<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    path: String,
    content: String,
) -> Res<()> {
    let dir = app_data(&app)?;
    extensions::require(&id, &dir, "files")?;
    extensions::files::write(&dir, &path, &content)
}

/// `ctx.settings.get()`
#[tauri::command]
pub fn extension_settings_get<R: Runtime>(
    app: AppHandle<R>,
    settings: tauri::State<'_, SettingsManager>,
    id: String,
) -> Res<Settings> {
    let dir = app_data(&app)?;
    extensions::require(&id, &dir, "settings")?;
    Ok(settings.get())
}

/// `ctx.settings.set(fullSettings)` — same shape as Settings in the app.
#[tauri::command]
pub async fn extension_settings_set<R: Runtime>(
    app: AppHandle<R>,
    settings: tauri::State<'_, SettingsManager>,
    id: String,
    next: Settings,
) -> Res<()> {
    let dir = app_data(&app)?;
    extensions::require(&id, &dir, "settings")?;
    let previous = settings.get();
    settings::save(&app, &next)?;
    let _ = crate::hotkeys::register_all(&app, &settings);
    if previous.general.staff_edge != next.general.staff_edge
        && next.general.staff_position.is_none()
    {
        let _ = window::position_staff(&app, &settings);
    }
    if previous.general.staff_visible != next.general.staff_visible {
        let _ = window::set_staff_visible(&app, &settings, next.general.staff_visible);
    }
    if previous.appearance != next.appearance {
        let _ = window::sync_staff_window(&app, &settings);
    }
    crate::tray::refresh(&app);
    Ok(())
}

/// `ctx.commands.dispatch(input)` — palette routing (`/`, `/c`, prefixes, etc.).
#[tauri::command]
pub async fn extension_commands_dispatch<R: Runtime>(
    app: AppHandle<R>,
    settings: tauri::State<'_, SettingsManager>,
    id: String,
    input: String,
) -> Res<DispatchOutcome> {
    let dir = app_data(&app)?;
    extensions::require(&id, &dir, "commands")?;
    Ok(palette::dispatch(&app, &settings, &input).await)
}

/// `ctx.commands.runTool(toolId, input)`
#[tauri::command]
pub fn extension_commands_run_tool<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    tool_id: String,
    input: String,
) -> Res<tools::dev::ToolResult> {
    let dir = app_data(&app)?;
    extensions::require(&id, &dir, "commands")?;
    let tool: tools::dev::ToolId =
        serde_json::from_value(serde_json::Value::String(tool_id.clone())).map_err(|_| {
            format!("Unknown tool id “{tool_id}”. Use snake_case names like sha256, json_format.")
        })?;
    Ok(tools::dev::run(tool, &input))
}

/// `ctx.ai.ask(prompt)` — one message to the primary AI backend.
#[tauri::command]
pub async fn extension_ai_ask<R: Runtime>(
    app: AppHandle<R>,
    settings: tauri::State<'_, SettingsManager>,
    id: String,
    prompt: String,
) -> Res<String> {
    let dir = app_data(&app)?;
    extensions::require(&id, &dir, "ai")?;
    use crate::agent::Message;
    let response = agent::chat_with_history(&settings, vec![Message::user(prompt)])
        .await
        .map_err(|e| e.to_string())?;
    Ok(response.text)
}

/// `ctx.shortcuts.run(shortcutId, query?)`
#[tauri::command]
pub async fn extension_shortcuts_run<R: Runtime>(
    app: AppHandle<R>,
    settings: tauri::State<'_, SettingsManager>,
    id: String,
    shortcut_id: String,
    query: Option<String>,
) -> Res<ExecOutcome> {
    let dir = app_data(&app)?;
    extensions::require(&id, &dir, "shortcuts")?;
    let cfg = settings.get();
    let shortcut = cfg
        .shortcuts
        .iter()
        .find(|s| s.id == shortcut_id)
        .ok_or_else(|| format!("No shortcut with id “{shortcut_id}”."))?;
    Ok(shortcuts::execute_shortcut(
        shortcut,
        query.as_deref().unwrap_or_default(),
        &cfg.command_center.browser,
    )
    .await)
}

/// Clip a string to a character count, not a byte count.
fn truncate(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        return text.to_string();
    }
    text.chars().take(chars).collect::<String>() + "…"
}

// ---------------------------------------------------------------------------
// Window management
// ---------------------------------------------------------------------------
//
// Moving another application's window needs the Accessibility permission, which
// macOS grants per code signature. That is why these run in Caduceus itself
// rather than in a helper: one grant, with Caduceus's name on it, that survives
// an update.

/// Snap, move or resize the frontmost window of the frontmost application.
#[tauri::command]
pub fn window_action<R: Runtime>(
    app: AppHandle<R>,
    verb: window::manage::Verb,
) -> window::manage::WindowOutcome {
    window::manage::apply(&app, verb)
}

/// Whether window management is usable right now. Never prompts.
#[tauri::command]
pub fn window_permission() -> bool {
    window::manage::permission_granted()
}

/// Clear a stale privacy grant and ask macOS for it again.
///
/// The fix for "it is ticked in System Settings and the app says it is not".
/// See [`window::grants`] for why that state exists at all.
///
/// Off the main thread like every other command here that shells out: `tccutil`
/// is given eight seconds to answer, and a sync command spends them frozen.
#[tauri::command]
pub async fn repair_permission(grant: window::grants::Grant) -> Res<window::grants::RepairOutcome> {
    tauri::async_runtime::spawn_blocking(move || window::grants::repair(grant))
        .await
        .map_err(|e| format!("The repair could not be run: {e}"))
}

/// Ask macOS for a privacy grant without clearing an existing TCC entry.
///
/// Marks a permission flow as active first, for the same reason as
/// [`open_system_settings`]: the TCC sheet this triggers steals focus from the
/// Command Center just as effectively as switching to another app would, and
/// the blur handler cannot otherwise tell the two apart. See
/// [`window::PermissionFlowActive`].
#[tauri::command]
pub fn request_permission<R: Runtime>(app: AppHandle<R>, grant: window::grants::Grant) -> bool {
    if let Some(state) = app.try_state::<window::PermissionFlowActive>() {
        state.mark_active();
    }
    window::grants::request(grant)
}

/// The text selected in the frontmost app, or `null` if there is none.
#[tauri::command]
pub fn selected_text() -> Option<String> {
    window::manage::selected_text()
}

// ---------------------------------------------------------------------------
// Developer toolbox
// ---------------------------------------------------------------------------

/// Run one of the developer tools.
///
/// A single entry point with a closed `ToolId` enum, rather than sixty commands:
/// the webview can name a tool that exists and nothing else.
#[tauri::command]
pub fn run_tool(id: tools::dev::ToolId, input: String) -> tools::dev::ToolResult {
    tools::dev::run(id, &input)
}

// ---------------------------------------------------------------------------
// System controls
// ---------------------------------------------------------------------------

/// Every one of these shells out — `osascript`, `pmset`, `networksetup` — and a
/// sync command runs inline on the main thread, so "shut down" with one app
/// showing a "Save changes?" sheet would freeze the whole of Caduceus behind it.
#[tauri::command]
pub async fn system_action(action: tools::system::SystemAction) -> tools::ToolOutcome {
    blocking_outcome(move || tools::system::run(action)).await
}

#[tauri::command]
pub fn system_permissions() -> tools::system::PermissionReport {
    tools::system::permissions()
}

#[tauri::command]
pub async fn machine_summary() -> tools::ToolOutcome {
    blocking_outcome(tools::system::machine_summary).await
}

#[tauri::command]
pub async fn wifi_summary() -> tools::ToolOutcome {
    blocking_outcome(tools::system::wifi_summary).await
}

#[tauri::command]
pub async fn media_action(action: tools::media::MediaAction) -> tools::ToolOutcome {
    blocking_outcome(move || tools::media::run(action)).await
}

// ---------------------------------------------------------------------------
// Vision and audio (the native helper)
// ---------------------------------------------------------------------------

/// Drag a region of the screen and copy the text inside it.
///
/// `screencapture -i` blocks for as long as the user takes to drag the
/// rectangle, so this waits on a blocking thread the way the colour picker does
/// — running it inline would freeze the app for the whole of the selection.
#[tauri::command]
pub async fn ocr_screen() -> tools::ToolOutcome {
    blocking_outcome(tools::native::ocr_screen_selection).await
}

// ---------------------------------------------------------------------------
// Other applications
// ---------------------------------------------------------------------------

/// Run AppleScript and return what it printed.
///
/// # What this is and is not
///
/// This executes an arbitrary script, which is worth being precise about. The
/// scripts come from Caduceus's own command registry — a compiled-in table in
/// `shared/commands.ts` — and the webview that calls this loads nothing from the
/// network (see the CSP in `tauri.conf.json`). So the trust boundary is the
/// bundle itself, which is the same boundary as the rest of the app.
///
/// **If the extension system ever runs third-party JavaScript in this webview,
/// this command must be gated behind a permission before that ships.** Running
/// AppleScript is equivalent to driving every app on the Mac.
///
/// Automation failures are translated, because "-1743" is not an explanation.
#[tauri::command]
pub async fn run_apple_script(script: String) -> Res<String> {
    tauri::async_runtime::spawn_blocking(move || tools::apple::run_script(&script))
        .await
        .map_err(|e| format!("Could not run the script: {e}"))?
}

/// Run a shortcut from the Shortcuts app, optionally with text as its input.
#[tauri::command]
pub async fn run_apple_shortcut(name: String, input: String) -> Res<String> {
    tauri::async_runtime::spawn_blocking(move || tools::apple::run_shortcut(&name, &input))
        .await
        .map_err(|e| format!("Could not run the shortcut: {e}"))?
}

/// Every shortcut the Shortcuts app knows about.
#[tauri::command]
pub async fn list_apple_shortcuts() -> Res<Vec<String>> {
    tauri::async_runtime::spawn_blocking(tools::apple::list_shortcuts)
        .await
        .map_err(|e| format!("Could not ask the Shortcuts app: {e}"))?
}

// ---------------------------------------------------------------------------
// Storage and cleaning
// ---------------------------------------------------------------------------

/// Measure everything reclaimable. Reads only.
///
/// Blocking, and genuinely slow on a full disk — it walks the trees rather than
/// estimating, because a cleaner whose numbers are guesses is asking you to
/// delete things on the strength of a guess.
#[tauri::command]
pub async fn scan_junk() -> Res<Vec<tools::cleaner::JunkGroup>> {
    tauri::async_runtime::spawn_blocking(tools::cleaner::scan)
        .await
        .map_err(|e| format!("The scan could not be run: {e}"))
}

/// Reclaim the space in the chosen categories.
///
/// Takes kinds rather than paths so the Trash can be *emptied* rather than
/// moved to the Trash, and so the removal operates on a fresh scan rather than
/// on a list the UI has been holding since before you made a cup of tea.
#[tauri::command]
pub async fn clean_junk(kinds: Vec<tools::cleaner::JunkKind>) -> Res<tools::ToolOutcome> {
    tauri::async_runtime::spawn_blocking(move || tools::cleaner::remove(&kinds))
        .await
        .map_err(|e| format!("The cleanup could not be run: {e}"))
}

/// Every installed application, with its real size and when it was last opened.
#[tauri::command]
pub async fn list_installed_app_sizes() -> Res<Vec<tools::cleaner::InstalledApp>> {
    tauri::async_runtime::spawn_blocking(tools::cleaner::installed_apps)
        .await
        .map_err(|e| format!("The scan could not be run: {e}"))
}

// ---------------------------------------------------------------------------
// Folder sorting
// ---------------------------------------------------------------------------

/// Work out where everything in a folder would go. Changes nothing.
#[tauri::command]
pub fn sort_plan(
    session: tauri::State<'_, tools::sorter::Session>,
    directory: String,
    sort_by: tools::sorter::SortBy,
) -> Res<tools::sorter::SortPlan> {
    let plan = tools::sorter::plan(&directory, sort_by)?;
    session.remember(&plan);
    Ok(plan)
}

/// Carry out a plan the user has looked at.
#[tauri::command]
pub fn sort_apply(
    session: tauri::State<'_, tools::sorter::Session>,
    moves: Vec<serde_json::Value>,
) -> Res<tools::sorter::SortResult> {
    let planned = session.planned(&parse_moves(moves)?)?;
    let result = tools::sorter::apply(&planned);
    session.record_applied(&result.moved);
    Ok(result)
}

/// Put everything back.
#[tauri::command]
pub fn sort_revert(
    session: tauri::State<'_, tools::sorter::Session>,
    moves: Vec<serde_json::Value>,
) -> Res<tools::sorter::SortResult> {
    let applied = session.applied(&parse_moves(moves)?)?;
    let result = tools::sorter::revert(&applied);
    session.record_reverted();
    Ok(result)
}

/// Which rows of the plan the webview means, as `(from, to)` pairs.
///
/// Only the pair is read, and even that is looked up in the plan the backend
/// still holds before anything is renamed. The webview naming two paths is not
/// enough to move a file: an unrecognised pair is an error, not an instruction.
fn parse_moves(raw: Vec<serde_json::Value>) -> Res<Vec<(String, String)>> {
    raw.into_iter()
        .map(|value| {
            let get = |key: &str| {
                value
                    .get(key)
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .ok_or_else(|| format!("a move is missing its {key}"))
            };
            Ok((get("from")?, get("to")?))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Desktop icon shapes
// ---------------------------------------------------------------------------
//
// All three run on a blocking thread: reading the display layout hops to the
// main thread and waits for it, and driving Finder is a round trip per icon.

/// Where every Desktop icon would go. Moves nothing.
#[tauri::command]
pub async fn desktop_shape_plan<R: Runtime>(
    app: AppHandle<R>,
    shape: tools::shapes::Shape,
) -> Res<tools::shapes::ShapePlan> {
    tauri::async_runtime::spawn_blocking(move || {
        let area = tools::shapes::desktop_area(&app)?;
        tools::shapes::plan(shape, area)
    })
    .await
    .map_err(|e| format!("Could not read your Desktop: {e}"))?
}

/// Arrange the Desktop into a shape.
///
/// Takes the shape rather than the planned positions: the plan may have been on
/// screen for a while, and the icons that exist now are the ones to place. The
/// result carries every icon's previous position, for `desktop_shape_revert`.
#[tauri::command]
pub async fn desktop_shape_apply<R: Runtime>(
    app: AppHandle<R>,
    shape: tools::shapes::Shape,
) -> Res<tools::shapes::ShapeResult> {
    tauri::async_runtime::spawn_blocking(move || {
        let area = tools::shapes::desktop_area(&app)?;
        tools::shapes::apply(shape, area)
    })
    .await
    .map_err(|e| format!("Could not arrange your Desktop: {e}"))?
}

/// Put every icon back where it was.
#[tauri::command]
pub async fn desktop_shape_revert(
    previous: Vec<tools::shapes::Spot>,
) -> Res<tools::shapes::ShapeResult> {
    tauri::async_runtime::spawn_blocking(move || tools::shapes::revert(&previous))
        .await
        .map_err(|e| format!("Could not put the icons back: {e}"))
}

// ---------------------------------------------------------------------------
// Citations
// ---------------------------------------------------------------------------

/// The page in the frontmost browser.
#[tauri::command]
pub async fn current_page() -> Res<tools::citation::Source> {
    tauri::async_runtime::spawn_blocking(tools::citation::current_page)
        .await
        .map_err(|e| format!("Could not ask the browser: {e}"))?
}

/// Fill in author and date by fetching the page. Only ever on request.
#[tauri::command]
pub async fn enrich_citation(source: tools::citation::Source) -> Res<tools::citation::Source> {
    Ok(tools::citation::enrich(source).await)
}

/// One source, in every style.
#[tauri::command]
pub fn format_citations(
    source: tools::citation::Source,
    accessed: String,
) -> Vec<tools::citation::Citation> {
    tools::citation::format_all(&source, &accessed)
}

// ---------------------------------------------------------------------------
// Recording
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn recording_start<R: Runtime>(
    app: AppHandle<R>,
    runtime: tauri::State<'_, capture::recorder::RecorderRuntime>,
    mode: capture::recorder::RecordMode,
    microphone: bool,
) -> Res<String> {
    let partial_app = app.clone();
    let path = runtime.start(mode, microphone, move |text| {
        let _ = partial_app.emit(crate::meeting::MEETING_SYSTEM_PARTIAL_EVENT, text);
    })?;
    let _ = app.emit(RECORDING_EVENT, runtime.status());
    Ok(path)
}

#[tauri::command]
pub fn recording_pause<R: Runtime>(
    app: AppHandle<R>,
    runtime: tauri::State<'_, capture::recorder::RecorderRuntime>,
    paused: bool,
) -> Res<bool> {
    let now = runtime.set_paused(paused)?;
    let _ = app.emit(RECORDING_EVENT, runtime.status());
    Ok(now)
}

#[tauri::command]
pub async fn recording_stop<R: Runtime>(
    app: AppHandle<R>,
    runtime: tauri::State<'_, capture::recorder::RecorderRuntime>,
) -> Res<String> {
    // Finalising an hour of video takes real seconds, and this must not be one
    // of the things that can freeze the interface.
    let runtime = runtime.inner().clone();
    let finished = tauri::async_runtime::spawn_blocking(move || {
        let result = runtime.stop();
        (result, runtime.status())
    })
    .await
    .map_err(|e| format!("The recorder could not be stopped: {e}"))?;

    let _ = app.emit(RECORDING_EVENT, finished.1);
    finished.0
}

#[tauri::command]
pub fn recording_status(
    runtime: tauri::State<'_, capture::recorder::RecorderRuntime>,
) -> capture::recorder::RecordingStatus {
    runtime.status()
}

/// Emitted whenever the recording state changes, so every window agrees.
pub const RECORDING_EVENT: &str = "caduceus://recording";

/// Today's exchange rates for a base currency.
///
/// The one thing in Caduceus that needs the internet, which is why it is its
/// own command rather than part of the converter: everything else in that tool
/// works on a plane, and this must not be able to break it.
#[tauri::command]
pub async fn exchange_rates(
    cache: tauri::State<'_, tools::rates::RateCache>,
    base: String,
) -> Res<tools::rates::RateTable> {
    tools::rates::fetch(&cache, &base).await
}

/// Pick a colour from anywhere on screen.
///
/// Caduceus's own windows are hidden for the duration, because the whole point
/// is to sample what is *behind* them — and because a loupe magnifying the
/// colour picker you opened it from is a joke that stops being funny quickly.
/// They come back however this ends, including when it is cancelled.
///
/// `null` means the user pressed Escape.
#[tauri::command]
pub async fn pick_screen_color<R: Runtime>(app: AppHandle<R>) -> Res<Option<String>> {
    let hidden: Vec<_> = [window::COMMAND_CENTER_WINDOW, window::STAFF_WINDOW]
        .iter()
        .filter_map(|label| app.get_webview_window(label))
        .filter(|w| w.is_visible().unwrap_or(false))
        .collect();
    for window in &hidden {
        let _ = window.hide();
    }

    // The sampler is a whole separate process with its own run loop, so this
    // has to be off the async runtime's shoulders.
    let picked = tauri::async_runtime::spawn_blocking(tools::native::pick_screen_color)
        .await
        .map_err(|e| format!("The colour picker could not be started: {e}"))?;

    for window in &hidden {
        let _ = window.show();
        window::configure_staff_floating(window);
    }

    picked
}

/// Read the text out of an image file.
///
/// Off the async runtime for the same reason as the colour picker: the helper is
/// a separate process, and waiting on it must not occupy a worker thread the
/// rest of the app's IPC is sharing.
#[tauri::command]
pub async fn ocr_image(path: String) -> tools::ToolOutcome {
    let recognised =
        tauri::async_runtime::spawn_blocking(move || tools::native::ocr_image(&path)).await;
    match recognised {
        Ok(Ok(text)) if !text.trim().is_empty() => {
            tools::ToolOutcome::copied(text, "Copied the recognised text")
        }
        Ok(Ok(_)) => tools::ToolOutcome::err("No text was found in that image."),
        Ok(Err(e)) => tools::ToolOutcome::err(e),
        Err(e) => tools::ToolOutcome::err(format!("Text recognition could not be run: {e}")),
    }
}

#[tauri::command]
pub async fn audio_devices() -> Res<Vec<tools::native::AudioDevice>> {
    tauri::async_runtime::spawn_blocking(tools::native::audio_devices)
        .await
        .map_err(|e| format!("The device list could not be read: {e}"))?
}

#[tauri::command]
pub async fn set_audio_device(uid: String, input: bool) -> tools::ToolOutcome {
    match tauri::async_runtime::spawn_blocking(move || tools::native::set_audio_device(&uid, input))
        .await
    {
        Ok(outcome) => outcome,
        Err(e) => tools::ToolOutcome::err(format!("The device could not be changed: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Developer environment
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn listening_ports(port: Option<u16>) -> Vec<tools::devenv::PortUser> {
    tauri::async_runtime::spawn_blocking(move || tools::devenv::listening_ports(port))
        .await
        .unwrap_or_default()
}

#[tauri::command]
pub async fn free_port(port: u16) -> tools::ToolOutcome {
    blocking_outcome(move || tools::devenv::free_port(port)).await
}

#[tauri::command]
pub async fn git_repos(limit: Option<usize>) -> Vec<tools::devenv::GitRepo> {
    tauri::async_runtime::spawn_blocking(move || tools::devenv::git_repos(limit.unwrap_or(60)))
        .await
        .unwrap_or_default()
}

#[tauri::command]
pub async fn git_status(path: String) -> Option<usize> {
    tauri::async_runtime::spawn_blocking(move || tools::devenv::git_status(&path))
        .await
        .unwrap_or_default()
}

#[tauri::command]
pub async fn ssh_hosts() -> Vec<tools::devenv::SshHost> {
    tauri::async_runtime::spawn_blocking(tools::devenv::ssh_hosts)
        .await
        .unwrap_or_default()
}

#[tauri::command]
pub async fn docker_containers() -> Res<Vec<tools::devenv::Container>> {
    tauri::async_runtime::spawn_blocking(tools::devenv::containers)
        .await
        .map_err(|e| format!("Docker could not be asked: {e}"))?
}

#[tauri::command]
pub async fn docker_action(id: String, action: String) -> tools::ToolOutcome {
    blocking_outcome(move || tools::devenv::container_action(&id, &action)).await
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn compress_selection() -> tools::ToolOutcome {
    blocking_outcome(tools::files::compress_selection).await
}

#[tauri::command]
pub async fn expand_selection() -> tools::ToolOutcome {
    blocking_outcome(tools::files::expand_selection).await
}

#[tauri::command]
pub async fn trash_selection() -> tools::ToolOutcome {
    blocking_outcome(tools::files::trash_selection).await
}

#[tauri::command]
pub async fn quick_look_selection() -> tools::ToolOutcome {
    blocking_outcome(tools::files::quick_look_selection).await
}

#[tauri::command]
pub async fn open_selection_in_terminal() -> tools::ToolOutcome {
    blocking_outcome(tools::files::open_selection_in_terminal).await
}

#[tauri::command]
pub async fn largest_files(
    directory: Option<String>,
    limit: Option<usize>,
) -> Vec<tools::files::BigFile> {
    tauri::async_runtime::spawn_blocking(move || {
        tools::files::largest_files(&directory.unwrap_or_default(), limit.unwrap_or(40))
    })
    .await
    .unwrap_or_default()
}

/// Support files an application has left behind. Reports only; removing is
/// `trash_paths`, called with the list the user was shown.
#[tauri::command]
pub async fn app_leftovers(app_path: String) -> Vec<tools::files::Leftover> {
    tauri::async_runtime::spawn_blocking(move || match tools::files::bundle_id(&app_path) {
        Some(id) => tools::files::app_leftovers(&id),
        None => Vec::new(),
    })
    .await
    .unwrap_or_default()
}

#[tauri::command]
pub async fn trash_paths(paths: Vec<String>) -> tools::ToolOutcome {
    blocking_outcome(move || tools::files::trash_paths(&paths)).await
}

// ---------------------------------------------------------------------------
// Network
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn network_summary() -> tools::ToolOutcome {
    blocking_outcome(tools::net::local_summary).await
}

#[tauri::command]
pub async fn public_address() -> tools::ToolOutcome {
    tools::net::public_address().await
}

#[tauri::command]
pub async fn dns_lookup(host: String) -> tools::ToolOutcome {
    blocking_outcome(move || tools::net::dns_lookup(&host)).await
}

#[tauri::command]
pub async fn ping_host(host: String) -> tools::ToolOutcome {
    blocking_outcome(move || tools::net::ping(&host)).await
}

/// Reveal a path in Finder. Only ever called with a path Caduceus itself listed.
#[tauri::command]
pub async fn reveal_path(path: String) -> tools::ToolOutcome {
    blocking_outcome(move || tools::files::reveal(&path)).await
}

/// Open a folder in Terminal.
#[tauri::command]
pub async fn open_path_in_terminal(path: String) -> tools::ToolOutcome {
    blocking_outcome(move || tools::files::open_in_terminal(&path)).await
}

/// Connect to a host from `~/.ssh/config`.
#[tauri::command]
pub async fn ssh_connect(alias: String) -> tools::ToolOutcome {
    blocking_outcome(move || tools::devenv::ssh_connect(&alias)).await
}

// ---------------------------------------------------------------------------
// Usage ranking
// ---------------------------------------------------------------------------
//
// The palette ranks by how often *you* run something. These three commands are
// the whole of it: read the counts, add one, throw them away. Nothing is sent
// anywhere — see the module docs in `usage.rs`.

/// Every recorded id and how often it has been run.
#[tauri::command]
pub fn usage_counts(
    usage: tauri::State<'_, crate::usage::UsageStore>,
) -> std::collections::HashMap<String, crate::usage::UsageEntry> {
    usage.snapshot()
}

/// Count one use of a palette row.
#[tauri::command]
pub fn record_usage(
    usage: tauri::State<'_, crate::usage::UsageStore>,
    id: String,
) -> crate::usage::UsageEntry {
    usage.record(&id, crate::usage::now_ms())
}

/// Give several commands a starting use count (onboarding favorites).
#[tauri::command]
pub fn seed_usage(usage: tauri::State<'_, crate::usage::UsageStore>, ids: Vec<String>, count: u32) {
    let now = crate::usage::now_ms();
    for id in ids {
        usage.seed(&id, count, now);
    }
}

/// Forget every recorded count, putting the palette back to its shipped order.
#[tauri::command]
pub fn clear_usage(usage: tauri::State<'_, crate::usage::UsageStore>) {
    usage.clear();
}

// ---------------------------------------------------------------------------
// Keep-awake sessions (the Manage window's Keep Awake page)
// ---------------------------------------------------------------------------

/// Start a keep-awake session. `minutes` of `None` means until turned off.
#[tauri::command]
pub fn awake_start(
    awake: tauri::State<'_, tools::awake::AwakeRuntime>,
    minutes: Option<u64>,
    display_may_sleep: Option<bool>,
) -> tools::ToolOutcome {
    let duration = match minutes {
        None => None,
        // Rejecting rather than clamping: a UI that asked for 0 minutes has a
        // bug, and silently holding the machine awake would hide it.
        Some(0) => return tools::ToolOutcome::err("A session needs at least one minute."),
        Some(m) => Some(std::time::Duration::from_secs(m.min(7 * 24 * 60) * 60)),
    };
    awake.start(duration, display_may_sleep.unwrap_or(false))
}

#[tauri::command]
pub fn awake_stop(awake: tauri::State<'_, tools::awake::AwakeRuntime>) -> tools::ToolOutcome {
    awake.stop()
}

#[tauri::command]
pub fn awake_status(
    awake: tauri::State<'_, tools::awake::AwakeRuntime>,
) -> tools::awake::AwakeStatus {
    awake.status()
}

// ---------------------------------------------------------------------------
// Time management (world clock, converter, timers, stopwatch, pomodoro)
// ---------------------------------------------------------------------------
// See `tools::timekeeping` for why every one of these lives in Rust rather
// than in the React tree — the short version is that the Command Center
// window is hidden more often than shown, and a timer that only counts down
// while its webview happens to be visible is not a timer worth shipping.

/// Every catalogued zone with its current offset — the world clock's rows and
/// the data behind its searchable picker.
#[tauri::command]
pub fn time_list_zones() -> Vec<tools::timekeeping::ZoneClock> {
    tools::timekeeping::world_clock(chrono::Utc::now())
}

/// Read a time in one zone and show it in a set of others.
#[tauri::command]
pub fn time_convert(
    request: tools::timekeeping::ConvertRequest,
    targets: Vec<String>,
) -> Res<Vec<tools::timekeeping::ConvertedTime>> {
    tools::timekeeping::convert(&request, &targets)
}

#[tauri::command]
pub fn time_start_timer(
    runtime: tauri::State<'_, tools::timekeeping::TimekeepingRuntime>,
    name: String,
    seconds: u64,
) -> Res<tools::timekeeping::TimerSnapshot> {
    runtime.start_timer(name, seconds)
}

#[tauri::command]
pub fn time_list_timers(
    runtime: tauri::State<'_, tools::timekeeping::TimekeepingRuntime>,
) -> Vec<tools::timekeeping::TimerSnapshot> {
    runtime.list_timers()
}

#[tauri::command]
pub fn time_dismiss_timer(
    runtime: tauri::State<'_, tools::timekeeping::TimekeepingRuntime>,
    id: u64,
) {
    runtime.dismiss_timer(id);
}

#[tauri::command]
pub fn time_stopwatch_start(
    runtime: tauri::State<'_, tools::timekeeping::TimekeepingRuntime>,
) -> tools::timekeeping::StopwatchStatus {
    runtime.stopwatch_start()
}

#[tauri::command]
pub fn time_stopwatch_stop(
    runtime: tauri::State<'_, tools::timekeeping::TimekeepingRuntime>,
) -> tools::timekeeping::StopwatchStatus {
    runtime.stopwatch_stop()
}

#[tauri::command]
pub fn time_stopwatch_lap(
    runtime: tauri::State<'_, tools::timekeeping::TimekeepingRuntime>,
) -> tools::timekeeping::StopwatchStatus {
    runtime.stopwatch_lap()
}

#[tauri::command]
pub fn time_stopwatch_reset(
    runtime: tauri::State<'_, tools::timekeeping::TimekeepingRuntime>,
) -> tools::timekeeping::StopwatchStatus {
    runtime.stopwatch_reset()
}

#[tauri::command]
pub fn time_stopwatch_status(
    runtime: tauri::State<'_, tools::timekeeping::TimekeepingRuntime>,
) -> tools::timekeeping::StopwatchStatus {
    runtime.stopwatch_status()
}

#[tauri::command]
pub fn time_pomodoro_start(
    runtime: tauri::State<'_, tools::timekeeping::TimekeepingRuntime>,
    config: tools::timekeeping::PomodoroConfig,
) -> Res<tools::timekeeping::PomodoroStatus> {
    runtime.pomodoro_start(config)
}

#[tauri::command]
pub fn time_pomodoro_stop(
    runtime: tauri::State<'_, tools::timekeeping::TimekeepingRuntime>,
) -> tools::timekeeping::PomodoroStatus {
    runtime.pomodoro_stop()
}

#[tauri::command]
pub fn time_pomodoro_status(
    runtime: tauri::State<'_, tools::timekeeping::TimekeepingRuntime>,
) -> tools::timekeeping::PomodoroStatus {
    runtime.pomodoro_status()
}

/// Open the Manage window, optionally on a named page.
#[tauri::command]
pub fn open_manage_window<R: Runtime>(app: AppHandle<R>, page: Option<String>) -> Res<()> {
    window::open_manage(&app, page.as_deref())
}

/// Tell Rust whether the Command Center is currently just the palette.
///
/// The shell calls this whenever the tab set changes. Rust cannot work it out
/// on its own — "is there anything open worth keeping" is a question about tab
/// state, which lives in the webview.
#[tauri::command]
pub fn set_palette_floating<R: Runtime>(app: AppHandle<R>, floating: bool) -> Res<()> {
    window::set_palette_floating(&app, floating)
}

/// Dim one of Caduceus's own windows (the staff or the Command Center) —
/// scoped to windows this process owns; see `window::OpacityTarget`. macOS
/// only.
#[tauri::command]
pub fn window_set_opacity<R: Runtime>(
    app: AppHandle<R>,
    target: window::OpacityTarget,
    opacity: f32,
) -> Res<()> {
    window::set_window_opacity(&app, target, opacity)
}

// ---------------------------------------------------------------------------
// Regex tester
// ---------------------------------------------------------------------------

/// Run a pattern against sample text and report every match with its capture
/// groups. `flags` is any combination of `i`, `m`, `s`, `x` — unrecognised
/// letters are ignored rather than rejected.
#[tauri::command]
pub fn regex_test(
    pattern: String,
    flags: String,
    text: String,
) -> Res<Vec<tools::regex_tool::RegexMatch>> {
    tools::regex_tool::test(&pattern, &flags, &text)
}

/// A plain-English, token-by-token explanation of a pattern.
#[tauri::command]
pub fn regex_explain(pattern: String) -> Res<Vec<tools::regex_tool::ExplainToken>> {
    tools::regex_tool::explain(&pattern)
}

// ---------------------------------------------------------------------------
// Cron parser
// ---------------------------------------------------------------------------

/// Parse a 5-field cron expression, describe it in English, and list its next
/// occurrences in this Mac's local time zone — cron expressions carry no time
/// zone of their own, so "local" is the only reading that means anything here.
#[tauri::command]
pub fn parse_cron(expression: String, count: Option<usize>) -> Res<tools::cron::CronAnalysis> {
    tools::cron::analyze(
        &expression,
        chrono::Local::now().naive_local(),
        count.unwrap_or(10).clamp(1, 50),
    )
}
