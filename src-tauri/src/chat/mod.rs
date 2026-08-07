//! Saved `/` conversations.
//!
//! Before this, `/` was one-shot: every question built a fresh
//! `vec![Message::user(prompt)]`, so the model had no idea what you had just
//! asked it, and nothing was kept once the palette closed. This module adds the
//! thread — history sent with each turn, and the whole exchange on disk.

pub mod store;

pub use store::{ChatMessage, ChatStore, Conversation, Role, DB_FILE};

use serde::Serialize;

use crate::agent::{self, AgentError, AgentResult, Message, Usage};
use crate::settings::SettingsManager;

/// Emitted app-wide when a thread gains a turn or is deleted, so the palette
/// and the chat window stay in step without polling.
pub const CHAT_CHANGED_EVENT: &str = "caduceus://chat-changed";

/// Live tokens for the chat UI while a reply is being generated.
pub const CHAT_CHUNK_EVENT: &str = "caduceus://chat-chunk";

/// What `chat_ask` hands back: the reply plus the thread it landed in, since
/// the caller may not have known which thread it was continuing.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatReply {
    pub conversation_id: i64,
    pub text: String,
    pub model: String,
    pub usage: Option<Usage>,
    pub elapsed_ms: u64,
}

/// One event while a reply is streaming into the chat UI.
///
/// `rename_all_fields` is load-bearing, not decoration: `rename_all` on an
/// enum renames variants only, so without it `Started { conversation_id }`
/// went out as `conversation_id` while `Chat.tsx` read `chunk.conversationId`
/// and got `undefined` — a new conversation's id never reached the UI. See
/// [`crate::agent::types::AgentStep`] for the same trap and its test.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ChatChunk {
    /// The request has been accepted; the timer can start.
    Started { conversation_id: i64 },
    /// Fresh assistant text (append).
    Delta { conversation_id: i64, text: String },
    /// The turn finished successfully.
    Done {
        conversation_id: i64,
        text: String,
        model: String,
        usage: Option<Usage>,
        elapsed_ms: u64,
    },
    Error {
        conversation_id: i64,
        message: String,
    },
}

/// How many past turns to send with a new question.
///
/// Every turn is re-sent on each request, so an unbounded thread grows the
/// prompt without limit — slower and more expensive each time, until it is
/// refused for exceeding the context window. Twenty turns is far more than any
/// palette exchange needs and keeps the tail bounded.
const HISTORY_TURNS: usize = 20;

/// Ask the primary backend a question inside a conversation.
///
/// Both sides of the exchange are persisted: the question before the request,
/// so a reply that never arrives still leaves a record of what was asked, and
/// the answer after. `on_chunk` receives started / delta / done (or error)
/// events so the UI can type as tokens arrive.
pub async fn ask_streaming<F>(
    store: &ChatStore,
    settings: &SettingsManager,
    conversation_id: i64,
    prompt: &str,
    mut on_chunk: F,
) -> AgentResult<ChatReply>
where
    F: FnMut(ChatChunk) + Send,
{
    let _ = store.append(conversation_id, Role::User, prompt);
    on_chunk(ChatChunk::Started { conversation_id });

    let history = store
        .messages(conversation_id)
        .unwrap_or_default()
        .into_iter()
        .rev()
        .take(HISTORY_TURNS)
        .rev()
        .map(|m| match m.role {
            Role::User => Message::user(&m.text),
            Role::Assistant => Message::assistant(&m.text),
        })
        .collect::<Vec<_>>();

    let started = std::time::Instant::now();
    let result = agent::chat_with_history_streaming(settings, history, |delta| {
        on_chunk(ChatChunk::Delta {
            conversation_id,
            text: delta.to_string(),
        });
    })
    .await;

    let elapsed_ms = started.elapsed().as_millis() as u64;

    match result {
        Ok(response) => {
            let _ = store.append(conversation_id, Role::Assistant, &response.text);
            let reply = ChatReply {
                conversation_id,
                text: response.text.clone(),
                model: response.model.clone(),
                usage: response.usage.clone(),
                elapsed_ms,
            };
            on_chunk(ChatChunk::Done {
                conversation_id,
                text: response.text,
                model: response.model,
                usage: response.usage,
                elapsed_ms,
            });
            Ok(reply)
        }
        Err(error) => {
            on_chunk(ChatChunk::Error {
                conversation_id,
                message: error.user_message(),
            });
            Err(error)
        }
    }
}

/// Non-streaming wrapper for callers that only need the final string
/// (e.g. the palette's one-shot `/` path when it does not listen for chunks).
pub async fn ask(
    store: &ChatStore,
    settings: &SettingsManager,
    conversation_id: i64,
    prompt: &str,
) -> AgentResult<String> {
    let reply = ask_streaming(store, settings, conversation_id, prompt, |_| {}).await?;
    Ok(reply.text)
}

/// The thread a bare `/` should continue: the most recent one, or a new one.
///
/// Continuing beats always-new. Someone who asks a follow-up expects to be
/// understood, and the alternative — a fresh thread per question — is what made
/// `/` feel like it had no memory.
pub fn active_conversation(store: &ChatStore) -> Result<i64, AgentError> {
    let existing = store
        .conversations(1)
        .map_err(|e| AgentError::Other(e.to_string()))?;

    match existing.first() {
        Some(c) => Ok(c.id),
        None => store
            .create_conversation()
            .map_err(|e| AgentError::Other(e.to_string())),
    }
}
