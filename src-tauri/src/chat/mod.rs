//! Saved `/` conversations.
//!
//! Before this, `/` was one-shot: every question built a fresh
//! `vec![Message::user(prompt)]`, so the model had no idea what you had just
//! asked it, and nothing was kept once the palette closed. This module adds the
//! thread — history sent with each turn, and the whole exchange on disk.

pub mod store;

pub use store::{ChatMessage, ChatStore, Conversation, Role, DB_FILE};

use serde::Serialize;

use crate::agent::{self, AgentError, AgentResult, Message};
use crate::settings::SettingsManager;

/// Emitted app-wide when a thread gains a turn or is deleted, so the palette
/// and the chat window stay in step without polling.
pub const CHAT_CHANGED_EVENT: &str = "caduceus://chat-changed";

/// What `chat_ask` hands back: the reply plus the thread it landed in, since
/// the caller may not have known which thread it was continuing.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatReply {
    pub conversation_id: i64,
    pub text: String,
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
/// the answer after.
pub async fn ask(
    store: &ChatStore,
    settings: &SettingsManager,
    conversation_id: i64,
    prompt: &str,
) -> AgentResult<String> {
    let _ = store.append(conversation_id, Role::User, prompt);

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

    let response = agent::chat_with_history(settings, history).await?;
    let _ = store.append(conversation_id, Role::Assistant, &response.text);
    Ok(response.text)
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
