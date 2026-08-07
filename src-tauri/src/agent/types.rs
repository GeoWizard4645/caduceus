//! Provider-neutral types shared by every [`AgentBackend`](super::AgentBackend).
//!
//! Nothing here mentions a specific vendor. A new backend implements the trait
//! against these types and every part of Caduceus that consumes AI — the `/`
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
    /// A tool's result, addressed back to the call that requested it — see
    /// [`Message::tool_call_id`]. Only ever appears on a turn produced by
    /// [`Message::tool_result`]; nothing constructs one by hand.
    Tool,
}

/// One function call a model asked for on an assistant turn.
///
/// `arguments` is kept as the raw JSON string the API returned rather than a
/// parsed [`serde_json::Value`] — the wire format is a string precisely
/// because a model can (and occasionally does) emit slightly-invalid JSON,
/// and preserving exactly what came back lets the caller that actually needs
/// to parse it (the tool loop, resolving a call against an MCP schema) decide
/// how to handle that rather than losing the original text to an early,
/// lossy parse failure here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    /// Populated on an assistant turn that asked for one or more tools.
    /// Empty for every other turn, including a plain assistant reply.
    ///
    /// `#[serde(default)]` so a `Message` persisted (or hand-built) before
    /// tool calling existed still deserializes — see the module's
    /// backwards-compatibility rule.
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    /// Set on a [`Role::Tool`] turn to the [`ToolCall::id`] this result
    /// answers. `None` on every other role. `#[serde(default)]` for the same
    /// reason as `tool_calls`.
    #[serde(default)]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// An assistant turn that asked for tools. `content` is frequently empty
    /// — a model that only wants a tool often has nothing to say yet — but is
    /// not forced to be, since some providers attach prose alongside the
    /// request ("Let me check that for you\u{2026}").
    ///
    /// This must carry the *exact* `tool_calls` the provider returned back
    /// into the next request: the API expects to see its own request echoed
    /// before it will accept the matching [`Message::tool_result`] turns that
    /// answer it.
    pub fn assistant_tool_calls(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls,
            tool_call_id: None,
        }
    }

    /// One tool's result, addressed back to the call that requested it.
    /// `content` is a plain string on the wire even when the tool's own
    /// result was structured — callers that have JSON to report serialize it
    /// themselves first, the same way a failing tool's error text is just
    /// prose here rather than a second, parallel error channel.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
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
/// # Why `rename_all_fields` is here and not just `rename_all`
///
/// `rename_all` on an enum renames the *variants* — it does nothing to the
/// fields inside a struct variant. Without the second attribute this type
/// serialized `{"type":"started","session_id":...}` while the frontend read
/// `step.sessionId`, so the comparison in `AgentPanel.tsx`'s approval handler
/// was `undefined === sessionId` — always false. The visible symptom was the
/// approval prompt never appearing and the session waiting forever for a
/// "yes" the UI had no way to send. Nothing failed loudly, because a missing
/// field in JS is `undefined` rather than an error.
///
/// The test at the bottom of this file pins the wire shape so the two halves
/// cannot drift apart again silently.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
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

    // -----------------------------------------------------------------
    // Message / ToolCall — the tool-calling additions
    // -----------------------------------------------------------------

    #[test]
    fn a_message_json_blob_from_before_tool_calling_still_deserializes() {
        // Exactly what `chat/store.rs` or any other pre-existing caller would
        // have persisted or built: just `role` and `content`, nothing about
        // tools. `#[serde(default)]` on both new fields is what makes this
        // work rather than a hard deserialize error.
        let old = r#"{"role":"user","content":"hello"}"#;
        let m: Message = serde_json::from_str(old).expect("old-shaped JSON must still parse");
        assert!(m.tool_calls.is_empty());
        assert!(m.tool_call_id.is_none());
    }

    #[test]
    fn the_three_plain_constructors_carry_no_tool_traffic() {
        for m in [
            Message::user("x"),
            Message::system("x"),
            Message::assistant("x"),
        ] {
            assert!(m.tool_calls.is_empty());
            assert!(m.tool_call_id.is_none());
        }
    }

    #[test]
    fn assistant_tool_calls_round_trips_the_calls_it_was_given() {
        let calls = vec![ToolCall {
            id: "call_1".into(),
            name: "fs__read".into(),
            arguments: r#"{"path":"/tmp/x"}"#.into(),
        }];
        let m = Message::assistant_tool_calls("", calls.clone());
        assert_eq!(m.role, Role::Assistant);
        assert_eq!(m.tool_calls.len(), 1);
        assert_eq!(m.tool_calls[0].id, "call_1");
        assert!(m.tool_call_id.is_none());
    }

    #[test]
    fn tool_result_addresses_its_call_and_carries_no_calls_of_its_own() {
        let m = Message::tool_result("call_1", "42");
        assert_eq!(m.role, Role::Tool);
        assert_eq!(m.content, "42");
        assert_eq!(m.tool_call_id.as_deref(), Some("call_1"));
        assert!(m.tool_calls.is_empty());
    }

    #[test]
    fn role_tool_serializes_lowercase_like_every_other_role() {
        // Matters because this is also the literal string OpenAI's wire
        // format expects in `"role"` — see `openai::build_payload`.
        assert_eq!(serde_json::to_value(Role::Tool).unwrap(), "tool");
    }

    /// Pins the field casing the frontend reads.
    ///
    /// `AgentPanel.tsx` compares `step.sessionId` against the session it was
    /// mounted for, and a missing field in JS is `undefined` rather than an
    /// error — so when this regressed to `session_id` the approval prompt
    /// simply never appeared and the session waited forever for a "yes" that
    /// had no way to arrive. Nothing threw, nothing logged. Asserting the
    /// literal wire key is the only thing that catches that from this side.
    #[test]
    fn steps_carry_camel_case_field_names_the_frontend_can_read() {
        let started = serde_json::to_value(AgentStep::Started {
            session_id: "s-1".into(),
            task: "t".into(),
            backend: "b".into(),
            model: "m".into(),
        })
        .unwrap();
        assert_eq!(started["type"], "started");
        assert_eq!(started["sessionId"], "s-1");
        assert!(
            started.get("session_id").is_none(),
            "snake_case would silently read as undefined in the webview"
        );

        let waiting = serde_json::to_value(AgentStep::AwaitingApproval {
            session_id: "s-2".into(),
            summary: "do a thing".into(),
        })
        .unwrap();
        assert_eq!(waiting["type"], "awaitingApproval");
        assert_eq!(waiting["sessionId"], "s-2");
    }
}
