//! The Agent Execution Layer.
//!
//! ```text
//!  Command Center                     AgentRuntime
//!  ──────────────                     ────────────
//!   "/ explain X"  ──chat()──────────▶ primary backend ────▶ text
//!   "/c do Y"      ──start_session()─▶ computer-use backend
//!                                        │
//!                                        ├─▶ xcap  (screenshots)
//!                                        └─▶ enigo (mouse/keyboard)
//!                                        │
//!                       AgentStep events ┘ ──▶ every window
//! ```
//!
//! Backends are looked up by [`crate::settings::BackendKind`], so adding a
//! provider means writing one impl and adding one match arm. See
//! `docs/PLUGIN_GUIDE.md`.

pub mod backend;
pub mod discover;
pub mod hermes;
mod http;
pub mod null;
pub mod openai;
pub mod types;

pub use backend::{
    AgentBackend, AgentLoopContext, ApprovalAsker, ApprovalGate, CancelToken, StepSink,
};
/// Re-exported so other subsystems (e.g. speech-to-text) can render provider
/// error bodies the same way.
pub use http::extract_error_message as http_error_message;
pub use types::*;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Runtime};

use crate::settings::{BackendConfig, BackendKind, SettingsManager};

/// Event carrying [`AgentStep`]s to the frontend.
pub const AGENT_STEP_EVENT: &str = "caduceus://agent-step";

/// Resolve a [`BackendKind`] to its implementation.
///
/// New backends are registered here. The returned value is an `Arc` because a
/// session outlives the call that started it.
pub fn backend_for(kind: BackendKind) -> Arc<dyn AgentBackend> {
    match kind {
        BackendKind::Null => Arc::new(null::NullBackend),
        BackendKind::OpenAiCompatible => Arc::new(openai::OpenAiCompatibleBackend),
        BackendKind::Hermes => Arc::new(hermes::HermesBackend),
    }
}

/// Everything the UI needs to know about one live computer-use session.
struct LiveSession {
    cancel: CancelToken,
    /// Set once, when the user answers the confirmation prompt.
    approval: Mutex<Option<tokio::sync::oneshot::Sender<bool>>>,
}

/// Tracks running agent sessions so they can be stopped and approved.
#[derive(Clone, Default)]
pub struct AgentRuntime {
    sessions: Arc<Mutex<HashMap<String, Arc<LiveSession>>>>,
}

impl AgentRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ids of every session currently running.
    pub fn active_sessions(&self) -> Vec<String> {
        self.sessions.lock().keys().cloned().collect()
    }

    pub fn has_active(&self) -> bool {
        !self.sessions.lock().is_empty()
    }

    /// Ask a session to stop. It unwinds at the next step boundary, so the
    /// current in-flight action still completes — stopping mid-drag would leave
    /// the mouse button held down.
    pub fn stop(&self, session_id: &str) -> bool {
        match self.sessions.lock().get(session_id) {
            Some(s) => {
                s.cancel.cancel();
                true
            }
            None => false,
        }
    }

    pub fn stop_all(&self) {
        for s in self.sessions.lock().values() {
            s.cancel.cancel();
        }
    }

    /// Deliver the user's answer to a pending confirmation prompt.
    pub fn resolve_approval(&self, session_id: &str, approved: bool) -> bool {
        // Clone the Arc out before releasing the map lock, so the oneshot send
        // does not happen while the registry is held.
        let session = self.sessions.lock().get(session_id).cloned();
        let Some(session) = session else {
            return false;
        };
        let sender = session.approval.lock().take();
        match sender {
            Some(tx) => tx.send(approved).is_ok(),
            None => false,
        }
    }

    fn register(&self, id: String, cancel: CancelToken) -> tokio::sync::oneshot::Receiver<bool> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.sessions.lock().insert(
            id,
            Arc::new(LiveSession {
                cancel,
                approval: Mutex::new(Some(tx)),
            }),
        );
        rx
    }

    fn unregister(&self, id: &str) {
        self.sessions.lock().remove(id);
    }
}

/// Bridges [`ApprovalGate`] to the frontend's confirm dialog.
struct FrontendApproval {
    rx: Mutex<Option<tokio::sync::oneshot::Receiver<bool>>>,
}

#[async_trait]
impl ApprovalAsker for FrontendApproval {
    async fn ask(&self, session_id: &str, _summary: &str) -> bool {
        let Some(rx) = self.rx.lock().take() else {
            log::warn!("approval for {session_id} was already consumed");
            return false;
        };
        // A dropped sender (window closed, app quitting) means "no".
        rx.await.unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// Entry points used by the IPC commands
// ---------------------------------------------------------------------------

/// Pick the backend config for a role, or explain why there isn't one.
pub fn resolve_backend(
    settings: &crate::settings::Settings,
    role: BackendRole,
) -> AgentResult<BackendConfig> {
    let id = match role {
        BackendRole::Primary => settings.agents.primary_backend_id.clone(),
        BackendRole::ComputerUse => settings.agents.computer_use_backend_id.clone(),
    };

    let Some(id) = id else {
        return Err(AgentError::NotConfigured(match role {
            BackendRole::Primary => null::NOT_CONFIGURED_MESSAGE.to_string(),
            BackendRole::ComputerUse => COMPUTER_USE_NOT_CONFIGURED.to_string(),
        }));
    };

    settings
        .agents
        .backends
        .iter()
        .find(|b| b.id == id)
        .cloned()
        .ok_or_else(|| {
            AgentError::NotConfigured(format!(
                "The selected backend (\u{201c}{id}\u{201d}) no longer exists. \
                 Pick another one in Settings \u{2192} Agent Backends."
            ))
        })
}

pub const COMPUTER_USE_NOT_CONFIGURED: &str = "\
Screen control is not set up yet.\n\n\
It runs through Hermes Agent's computer_use toolset. Open Settings \u{2192} AI, \
make sure Hermes is installed and has a model, then pick it as the screen-control \
backend.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendRole {
    Primary,
    ComputerUse,
}

/// One-shot chat against the primary backend.
pub async fn chat(settings: &SettingsManager, prompt: &str) -> AgentResult<AgentResponse> {
    chat_with_history(settings, vec![Message::user(prompt)]).await
}

/// Ask the primary backend with a conversation behind the question.
///
/// [`chat`] is this with a one-message history. Kept separate because the
/// caller owns the thread: `chat::ask` loads it from disk, trims it, and
/// persists both sides.
///
/// Which backend actually answers is decided by [`resolve_chat_backend`],
/// which consults [`crate::tools::routing`] — see that function's doc for the
/// override / auto-routing / disabled precedence.
pub async fn chat_with_history(
    settings: &SettingsManager,
    messages: Vec<Message>,
) -> AgentResult<AgentResponse> {
    let snapshot = settings.get();
    // The classifier only ever sees the newest user turn, not the whole
    // thread: it is asking "how much work is *this* message", and folding in
    // stale history would let an old complex question keep a whole thread
    // pinned to the strong backend forever, or vice versa.
    let prompt = latest_user_content(&messages);
    let config = resolve_chat_backend(&snapshot, prompt)?;
    backend_for(config.kind).chat(messages, &config).await
}

/// Like [`chat_with_history`], but feeds `on_delta` as tokens arrive when the
/// resolved backend can stream (OpenAI-compatible / Ollama). Hermes and Null
/// still answer in one shot — `on_delta` then fires once with the full text.
pub async fn chat_with_history_streaming<F>(
    settings: &SettingsManager,
    messages: Vec<Message>,
    mut on_delta: F,
) -> AgentResult<AgentResponse>
where
    F: FnMut(&str) + Send,
{
    let snapshot = settings.get();
    let prompt = latest_user_content(&messages);
    let config = resolve_chat_backend(&snapshot, prompt)?;

    match config.kind {
        BackendKind::OpenAiCompatible => openai::stream_chat(messages, &config, on_delta).await,
        other => {
            let response = backend_for(other).chat(messages, &config).await?;
            if !response.text.is_empty() {
                on_delta(&response.text);
            }
            Ok(response)
        }
    }
}

/// The text of the most recent [`Role::User`] message, or `""` if there is
/// none (e.g. a malformed all-system/assistant history) — classification on
/// an empty string is well-defined (see `routing::classify`'s tests) and
/// simply falls out as [`crate::tools::routing::TaskClass::Micro`], so this
/// never needs to be an error.
fn latest_user_content(messages: &[Message]) -> &str {
    messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .map(|m| m.content.as_str())
        .unwrap_or("")
}

/// Resolve which backend should answer one primary-chat turn, consulting
/// smart routing when it applies.
///
/// Precedence, in order:
///
/// 1. **`routing_override_backend_id` is set.** The pinned backend is used
///    directly, whether or not auto-routing itself is enabled — a user who
///    hand-picked a backend has already answered the question routing would
///    otherwise be guessing at. If the pinned id no longer matches a
///    configured backend (it was deleted), this falls through to the next
///    step rather than failing, mirroring [`tools::routing::route`]'s own
///    "a stale setting points nowhere" handling.
/// 2. **`auto_routing_enabled` is `true`** (the default) **and no override
///    won above.** [`tools::routing::route`] classifies the newest user
///    message and picks a backend — the same policy `routing_preview`
///    previews.
/// 3. **Otherwise** (`auto_routing_enabled` is `false`). This resolves to
///    exactly what [`resolve_backend`] alone would have returned — i.e.
///    `primary_backend_id` — which is the pre-routing behaviour verbatim.
///
/// Any error surfaced here is the same [`AgentError::NotConfigured`]
/// `resolve_backend` already produces for a missing or dangling
/// `primary_backend_id`; routing never introduces a new failure mode, it can
/// only redirect a request that was already going to succeed.
fn resolve_chat_backend(
    settings: &crate::settings::Settings,
    prompt: &str,
) -> AgentResult<BackendConfig> {
    // Validate the primary backend first — this is the one call that can
    // fail, and it fails with exactly the messages `resolve_backend` always
    // has, regardless of anything routing decides below.
    let primary_config = resolve_backend(settings, BackendRole::Primary)?;
    let agents = &settings.agents;

    if let Some(id) = agents.routing_override_backend_id.as_deref() {
        if let Some(cfg) = agents.backends.iter().find(|b| b.id == id) {
            return Ok(cfg.clone());
        }
        // Dangling override: fall through to auto-routing/primary below
        // instead of failing on a stale setting.
    }

    if !agents.auto_routing_enabled {
        return Ok(primary_config);
    }

    let ctx = crate::tools::routing::RoutingContext {
        backends: &agents.backends,
        primary_backend_id: agents.primary_backend_id.as_deref(),
        // The override was already handled above; routing does not need to
        // re-check it.
        override_backend_id: None,
        auto_routing_enabled: true,
    };

    let decision =
        crate::tools::routing::route(prompt, &ctx, crate::tools::routing::latency_tracker());

    match decision {
        Some(decision) => Ok(agents
            .backends
            .iter()
            .find(|b| b.id == decision.backend_id)
            .cloned()
            .unwrap_or(primary_config)),
        // route() can only return None here if primary_backend_id itself is
        // missing/dangling, which resolve_backend above would already have
        // caught — kept as a defensive fallback rather than an unreachable!().
        None => Ok(primary_config),
    }
}

/// Start a computer-use session.
///
/// Returns immediately with the session id; progress arrives as
/// [`AGENT_STEP_EVENT`] events. The session runs to completion on the async
/// runtime even if every window closes.
pub fn start_session<R: Runtime>(
    app: AppHandle<R>,
    runtime: AgentRuntime,
    settings: SettingsManager,
    task: String,
) -> AgentResult<String> {
    let snapshot = settings.get();
    let config = resolve_backend(&snapshot, BackendRole::ComputerUse)?;
    let backend = backend_for(config.kind);

    if !backend.supports_computer_use() || !config.supports_computer_use {
        return Err(AgentError::ComputerUseUnsupported);
    }

    let session_id = uuid::Uuid::new_v4().to_string();
    let cancel = CancelToken::default();
    let approval_rx = runtime.register(session_id.clone(), cancel.clone());

    let approval = if snapshot.agents.confirm_before_first_action {
        ApprovalGate::Ask(Arc::new(FrontendApproval {
            rx: Mutex::new(Some(approval_rx)),
        }))
    } else {
        ApprovalGate::AutoApprove
    };

    let emit_app = app.clone();
    let on_step: StepSink = Arc::new(move |step: AgentStep| {
        if let Err(e) = emit_app.emit(AGENT_STEP_EVENT, &step) {
            log::warn!("could not emit agent step: {e}");
        }
    });

    let ctx = AgentLoopContext {
        session_id: session_id.clone(),
        on_step: on_step.clone(),
        cancel,
        approval,
    };

    let id_for_task = session_id.clone();
    tauri::async_runtime::spawn(async move {
        let result = backend.run_agent_loop(&task, &config, ctx).await;
        if let Err(e) = result {
            log::error!("agent session {id_for_task} failed: {e}");
            on_step(AgentStep::Error {
                message: e.user_message(),
            });
            on_step(AgentStep::Finished {
                outcome: AgentOutcome {
                    session_id: id_for_task.clone(),
                    completed: false,
                    steps: 0,
                    final_message: e.user_message(),
                    stop_reason: StopReason::Error,
                    usage: None,
                },
            });
        }
        runtime.unregister(&id_for_task);
    });

    Ok(session_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    #[test]
    fn every_backend_kind_resolves_to_an_implementation() {
        for kind in [
            BackendKind::Null,
            BackendKind::OpenAiCompatible,
            BackendKind::Hermes,
        ] {
            let b = backend_for(kind);
            assert!(!b.id().is_empty());
            assert!(!b.display_name().is_empty());
        }
        assert!(backend_for(BackendKind::Hermes).supports_computer_use());
        assert!(!backend_for(BackendKind::OpenAiCompatible).supports_computer_use());
        assert!(!backend_for(BackendKind::Null).supports_computer_use());
    }

    #[test]
    fn a_fresh_install_resolves_both_roles_to_hermes() {
        // Hermes is the out-of-the-box default for chat *and* screen control.
        // Whether it is actually installed is a runtime question the backend
        // answers with an actionable message, not a config question.
        let s = Settings::default();
        assert_eq!(
            resolve_backend(&s, BackendRole::Primary).unwrap().kind,
            BackendKind::Hermes
        );
        assert_eq!(
            resolve_backend(&s, BackendRole::ComputerUse).unwrap().kind,
            BackendKind::Hermes
        );
    }

    #[test]
    fn clearing_the_screen_control_backend_gives_an_actionable_message() {
        let mut s = Settings::default();
        s.agents.computer_use_backend_id = None;
        let err = resolve_backend(&s, BackendRole::ComputerUse).unwrap_err();
        assert!(err.to_string().contains("Hermes"));
    }

    #[test]
    fn the_default_backend_list_still_offers_an_explicit_none() {
        // Deleting Hermes must leave something selectable rather than an empty
        // dropdown, so every AI code path still resolves.
        let s = Settings::default();
        assert!(s
            .agents
            .backends
            .iter()
            .any(|b| b.kind == BackendKind::Null));
    }

    #[test]
    fn a_dangling_backend_id_is_reported_rather_than_panicking() {
        let mut s = Settings::default();
        s.agents.primary_backend_id = Some("deleted-backend".into());
        let err = resolve_backend(&s, BackendRole::Primary).unwrap_err();
        assert!(err.to_string().contains("no longer exists"));
    }

    #[test]
    fn stopping_an_unknown_session_is_not_an_error() {
        let rt = AgentRuntime::new();
        assert!(!rt.stop("nope"));
        assert!(!rt.resolve_approval("nope", true));
        assert!(!rt.has_active());
    }

    #[test]
    fn registered_sessions_can_be_stopped_and_approved_once() {
        let rt = AgentRuntime::new();
        let cancel = CancelToken::default();
        let _rx = rt.register("s1".into(), cancel.clone());
        assert!(rt.has_active());
        assert_eq!(rt.active_sessions(), vec!["s1".to_string()]);

        assert!(rt.resolve_approval("s1", true));
        // The channel is single-use; a second answer is ignored.
        assert!(!rt.resolve_approval("s1", true));

        assert!(rt.stop("s1"));
        assert!(cancel.is_cancelled());

        rt.unregister("s1");
        assert!(!rt.has_active());
    }

    // -----------------------------------------------------------------
    // resolve_chat_backend / latest_user_content
    //
    // chat_with_history itself is not unit-tested here because it makes a
    // real network/subprocess call via `backend_for(config.kind).chat(...)`
    // — exactly the reason the module doc on `tools::routing` gives for
    // keeping `classify`/`route` synchronous and model-free. Everything
    // chat_with_history decides *before* that call — which backend answers —
    // lives in `resolve_chat_backend`, which is plain sync logic over
    // `Settings` and is fully testable in isolation.
    // -----------------------------------------------------------------

    fn local_backend(id: &str, port: u16) -> BackendConfig {
        BackendConfig {
            id: id.into(),
            display_name: format!("Local ({id})"),
            kind: BackendKind::OpenAiCompatible,
            base_url: format!("http://localhost:{port}/v1"),
            ..Default::default()
        }
    }

    fn cloud_backend(id: &str) -> BackendConfig {
        BackendConfig {
            id: id.into(),
            display_name: format!("Cloud ({id})"),
            kind: BackendKind::OpenAiCompatible,
            base_url: "https://api.openai.com/v1".into(),
            ..Default::default()
        }
    }

    /// `Settings` with two real backends (one local, one cloud) and the
    /// primary pointed at the cloud one — the shape every test below starts
    /// from before flipping one routing knob at a time.
    fn settings_with_local_and_cloud() -> Settings {
        let mut s = Settings::default();
        s.agents.backends = vec![local_backend("local", 11434), cloud_backend("cloud")];
        s.agents.primary_backend_id = Some("cloud".into());
        s.agents.computer_use_backend_id = None;
        s
    }

    #[test]
    fn latest_user_content_picks_the_newest_user_turn_not_the_last_message() {
        let messages = vec![
            Message::user("first question"),
            Message::assistant("first answer"),
            Message::user("second question"),
        ];
        assert_eq!(latest_user_content(&messages), "second question");
    }

    #[test]
    fn latest_user_content_is_empty_rather_than_panicking_with_no_user_message() {
        let messages = vec![Message::system("system only")];
        assert_eq!(latest_user_content(&messages), "");
    }

    #[test]
    fn auto_routing_enabled_consults_route_and_can_pick_a_non_primary_backend() {
        // A short, mechanical prompt with a local backend configured: with
        // auto-routing on, tools::routing::route should send this to the
        // local backend instead of the configured primary, exactly like
        // routing.rs's own `micro_prompt_routes_to_the_only_local_backend`.
        let mut s = settings_with_local_and_cloud();
        s.agents.auto_routing_enabled = true;
        s.agents.routing_override_backend_id = None;

        let config = resolve_chat_backend(&s, "fix this typo: teh").unwrap();
        assert_eq!(config.id, "local");
    }

    #[test]
    fn auto_routing_enabled_still_sends_complex_prompts_to_primary() {
        let mut s = settings_with_local_and_cloud();
        s.agents.auto_routing_enabled = true;
        s.agents.routing_override_backend_id = None;

        let config = resolve_chat_backend(
            &s,
            "Design a fault-tolerant architecture for our payments pipeline and analyze the trade-offs.",
        )
        .unwrap();
        assert_eq!(config.id, "cloud");
    }

    #[test]
    fn override_wins_over_auto_routing_even_for_a_micro_prompt() {
        let mut s = settings_with_local_and_cloud();
        s.agents.auto_routing_enabled = true;
        // The classifier would otherwise send this to "local" (see the test
        // above); the pin must win regardless.
        s.agents.routing_override_backend_id = Some("cloud".into());

        let config = resolve_chat_backend(&s, "fix this typo: teh").unwrap();
        assert_eq!(config.id, "cloud");
    }

    #[test]
    fn override_wins_even_when_auto_routing_is_disabled() {
        // An explicit pin is a stronger signal than the on/off switch: a user
        // who picked a specific backend by hand should get it even if they
        // separately turned auto-routing off.
        let mut s = settings_with_local_and_cloud();
        s.agents.auto_routing_enabled = false;
        s.agents.routing_override_backend_id = Some("local".into());

        let config = resolve_chat_backend(&s, "fix this typo: teh").unwrap();
        assert_eq!(config.id, "local");
    }

    #[test]
    fn auto_routing_disabled_falls_back_to_primary_backend_id_unchanged() {
        // This is the pre-routing behaviour, verbatim: no override, routing
        // off, always the configured primary — regardless of what the
        // classifier would have said about the prompt.
        let mut s = settings_with_local_and_cloud();
        s.agents.auto_routing_enabled = false;
        s.agents.routing_override_backend_id = None;

        let config = resolve_chat_backend(&s, "fix this typo: teh").unwrap();
        assert_eq!(config.id, "cloud");

        // Same result no matter how the prompt would classify.
        let config = resolve_chat_backend(
            &s,
            "Design a fault-tolerant architecture and analyze every trade-off in depth.",
        )
        .unwrap();
        assert_eq!(config.id, "cloud");
    }

    #[test]
    fn a_dangling_override_falls_through_to_auto_routing_instead_of_failing() {
        let mut s = settings_with_local_and_cloud();
        s.agents.auto_routing_enabled = true;
        s.agents.routing_override_backend_id = Some("deleted-backend".into());

        // Falls through to normal routing, which sends this micro prompt to
        // the local backend — the same "stale setting points nowhere, don't
        // fail" contract tools::routing::route already gives its own override.
        let config = resolve_chat_backend(&s, "fix this typo: teh").unwrap();
        assert_eq!(config.id, "local");
    }

    #[test]
    fn a_dangling_primary_backend_id_still_errors_with_auto_routing_on() {
        // Routing must never mask the existing "your primary backend was
        // deleted" failure — resolve_backend's validation runs first and its
        // error is exactly what a caller before this change would have seen.
        let mut s = settings_with_local_and_cloud();
        s.agents.auto_routing_enabled = true;
        s.agents.primary_backend_id = Some("deleted-backend".into());

        let err = resolve_chat_backend(&s, "fix this typo: teh").unwrap_err();
        assert!(err.to_string().contains("no longer exists"));
    }

    #[test]
    fn a_fresh_install_with_only_hermes_still_resolves_with_auto_routing_on() {
        // Auto-routing defaults to true (see `AgentSettings::default`), so
        // this is the actual out-of-the-box path: one backend, nothing to
        // route between. Must behave exactly like resolve_backend alone did
        // before this change — no regression for the zero-config case.
        let s = Settings::default();
        assert!(s.agents.auto_routing_enabled);
        let config = resolve_chat_backend(&s, "fix this typo: teh").unwrap();
        assert_eq!(config.kind, BackendKind::Hermes);
    }
}
