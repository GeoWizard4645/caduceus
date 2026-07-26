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

pub use backend::{AgentBackend, AgentLoopContext, ApprovalAsker, ApprovalGate, CancelToken, StepSink};
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
    let snapshot = settings.get();
    let config = resolve_backend(&snapshot, BackendRole::Primary)?;
    backend_for(config.kind)
        .chat(vec![Message::user(prompt)], &config)
        .await
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
        for kind in [BackendKind::Null, BackendKind::OpenAiCompatible, BackendKind::Hermes] {
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
        assert!(s.agents.backends.iter().any(|b| b.kind == BackendKind::Null));
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
}
