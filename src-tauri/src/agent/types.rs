//! Provider-neutral types shared by every [`AgentBackend`](super::AgentBackend).
//!
//! Nothing here mentions a specific vendor. A new backend implements the trait
//! against these types and every part of Orbit that consumes AI — the `/`
//! prefix, `/c`, voice routing — works with it immediately.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Conversation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentResponse {
    pub text: String,
    pub model: String,
    pub usage: Option<Usage>,
}

// ---------------------------------------------------------------------------
// Agent loop
// ---------------------------------------------------------------------------

/// One event in a running agent session, streamed to the UI as it happens.
///
/// The user watching a computer-use session needs to see what is about to
/// happen *before* the mouse moves, which is why actions are emitted as
/// `Action` before execution and `ActionResult` after.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AgentStep {
    Started {
        session_id: String,
        task: String,
        backend: String,
        model: String,
    },
    /// The model's prose between tool calls.
    Thinking {
        text: String,
    },
    /// A screenshot was taken. `image` is a `data:` URL for the step feed.
    Screenshot {
        image: String,
        width: u32,
        height: u32,
    },
    /// About to perform an action on the user's machine.
    Action {
        index: u32,
        summary: String,
        raw: serde_json::Value,
    },
    ActionResult {
        index: u32,
        ok: bool,
        detail: String,
    },
    /// Waiting for the user to approve the first action of the session.
    AwaitingApproval {
        session_id: String,
        summary: String,
    },
    Finished {
        outcome: AgentOutcome,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model stopped asking for tools — the task is done.
    Completed,
    /// Hit the configured step ceiling.
    MaxSteps,
    /// The user pressed Stop.
    UserStopped,
    /// The user declined the confirmation prompt.
    Declined,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentOutcome {
    pub session_id: String,
    pub completed: bool,
    pub steps: u32,
    /// The model's closing message, if it produced one.
    pub final_message: String,
    pub stop_reason: StopReason,
    pub usage: Option<Usage>,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// No backend is configured — the zero-config state, not a failure.
    #[error("{0}")]
    NotConfigured(String),
    #[error("this backend cannot do computer use")]
    ComputerUseUnsupported,
    #[error("could not reach {endpoint}: {source}")]
    Transport {
        endpoint: String,
        #[source]
        source: reqwest::Error,
    },
    /// The provider replied with a non-2xx status.
    #[error("{provider} returned {status}: {body}")]
    Api {
        provider: String,
        status: u16,
        body: String,
    },
    #[error("could not understand the response from {provider}: {detail}")]
    Protocol { provider: String, detail: String },
    #[error(transparent)]
    Computer(#[from] super::computer::ComputerError),
    #[error("stopped")]
    Cancelled,
    #[error("{0}")]
    Other(String),
}

impl AgentError {
    /// A message safe and useful to show in the UI, with the next step spelled
    /// out where there is an obvious one.
    pub fn user_message(&self) -> String {
        match self {
            AgentError::Api { status: 401, .. } => {
                "The API key was rejected. Check it in Settings \u{2192} Agent Backends.".into()
            }
            AgentError::Api { status: 429, .. } => {
                "Rate limited by the provider. Wait a moment and try again.".into()
            }
            AgentError::Api {
                status, body, provider, ..
            } if *status >= 500 => {
                format!("{provider} is having trouble ({status}). {}", truncate(body, 160))
            }
            AgentError::Transport { endpoint, .. } => format!(
                "Could not reach {endpoint}. If this is a local model, check that the server is running."
            ),
            other => other.to_string(),
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}\u{2026}", s.chars().take(max).collect::<String>())
    }
}

pub type AgentResult<T> = Result<T, AgentError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_failures_tell_the_user_where_to_fix_it() {
        let e = AgentError::Api {
            provider: "Anthropic".into(),
            status: 401,
            body: "invalid x-api-key".into(),
        };
        assert!(e.user_message().contains("Settings"));
    }

    #[test]
    fn transport_failures_mention_local_servers() {
        // A local model server being down is the single most common cause.
        let e = AgentError::NotConfigured("no backend".into());
        assert_eq!(e.user_message(), "no backend");
    }

    #[test]
    fn steps_serialize_with_a_discriminator() {
        let json = serde_json::to_value(AgentStep::Thinking {
            text: "hm".into(),
        })
        .unwrap();
        assert_eq!(json["type"], "thinking");
        assert_eq!(json["text"], "hm");
    }
}
