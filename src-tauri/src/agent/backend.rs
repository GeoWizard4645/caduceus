//! The `AgentBackend` trait — Orbit's AI plug point.
//!
//! Implement this and Orbit can use your provider for chat (`/`), for voice
//! routing, and — if you set `supports_computer_use` — for driving the screen
//! (`/c`). See `docs/PLUGIN_GUIDE.md` for a worked example.

use async_trait::async_trait;

use super::computer::ComputerController;
use super::types::{AgentOutcome, AgentResult, AgentStep, AgentResponse, Message};
use crate::settings::BackendConfig;

/// Where step events go while an agent runs.
///
/// A boxed closure rather than the generic `impl Fn(AgentStep)` you might
/// expect: Orbit keeps backends in a `Vec<Arc<dyn AgentBackend>>` registry, and
/// a generic method parameter would make the trait non-object-safe. The
/// callback is invoked from the async runtime and must not block.
pub type StepSink = std::sync::Arc<dyn Fn(AgentStep) + Send + Sync>;

/// Everything a computer-use loop needs beyond the task text.
pub struct AgentLoopContext {
    pub session_id: String,
    pub on_step: StepSink,
    /// Screen capture and input simulation.
    pub computer: ComputerController,
    /// Polled between steps; when it returns true the loop unwinds and reports
    /// [`StopReason::UserStopped`](super::types::StopReason::UserStopped).
    pub cancel: CancelToken,
    /// Awaited before the *first* action of a session. Resolves to `false` if
    /// the user declines.
    pub approval: ApprovalGate,
    pub max_steps: u32,
    /// Pause after each mutating action so the screen settles before the next
    /// screenshot.
    pub settle: std::time::Duration,
}

/// Cheap, clonable cancellation flag shared with the UI's Stop button.
#[derive(Clone, Default)]
pub struct CancelToken(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Gate that a computer-use session passes through before its first action.
///
/// Nothing on the user's machine is touched until this resolves, which is what
/// makes "an agent that controls your mouse" acceptable to install.
#[derive(Clone)]
pub enum ApprovalGate {
    /// Confirmation is disabled in Settings.
    AutoApprove,
    /// Ask the UI and wait. The `String` is a summary of the first action.
    Ask(std::sync::Arc<dyn ApprovalAsker>),
}

#[async_trait]
pub trait ApprovalAsker: Send + Sync {
    /// Returns true if the user approved. Implementations must not block the
    /// runtime while waiting.
    async fn ask(&self, session_id: &str, summary: &str) -> bool;
}

impl ApprovalGate {
    pub async fn request(&self, session_id: &str, summary: &str) -> bool {
        match self {
            ApprovalGate::AutoApprove => true,
            ApprovalGate::Ask(asker) => asker.ask(session_id, summary).await,
        }
    }
}

/// A provider Orbit can talk to.
#[async_trait]
pub trait AgentBackend: Send + Sync {
    /// Stable identifier matching [`BackendConfig::kind`]'s serialised form.
    fn id(&self) -> &str;

    /// Name shown in Settings.
    fn display_name(&self) -> &str;

    /// Whether this backend can drive the screen. Backends that return `false`
    /// are never offered for the `/c` route.
    fn supports_computer_use(&self) -> bool;

    /// One-shot chat completion.
    async fn chat(&self, messages: Vec<Message>, config: &BackendConfig) -> AgentResult<AgentResponse>;

    /// Run an agentic loop against the user's screen until the model stops
    /// requesting tools, the step limit is reached, or the user stops it.
    ///
    /// The default implementation refuses, so chat-only backends get correct
    /// behaviour without writing any code.
    async fn run_agent_loop(
        &self,
        task: &str,
        config: &BackendConfig,
        ctx: AgentLoopContext,
    ) -> AgentResult<AgentOutcome> {
        let _ = (task, config, ctx);
        Err(super::types::AgentError::ComputerUseUnsupported)
    }

    /// Verify credentials and reachability. Backs the "Test connection" button.
    /// Returns a short success message, e.g. the model that answered.
    async fn test_connection(&self, config: &BackendConfig) -> AgentResult<String> {
        let response = self
            .chat(
                vec![Message::user("Reply with exactly: ok")],
                config,
            )
            .await?;
        Ok(format!(
            "Connected to {} ({}).",
            self.display_name(),
            if response.model.is_empty() {
                config.model.clone()
            } else {
                response.model
            }
        ))
    }
}
