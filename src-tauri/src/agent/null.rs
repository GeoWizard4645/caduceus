//! The backend that does nothing, on purpose.
//!
//! Orbit ships this as the default so that a fresh install has a valid,
//! resolvable AI backend without an API key. Every AI code path therefore has
//! something to call, and the "you have not set this up yet" message lives in
//! one place instead of being scattered as `Option` handling across the app.

use async_trait::async_trait;

use super::backend::AgentBackend;
use super::types::{AgentError, AgentResponse, AgentResult, Message};
use crate::settings::BackendConfig;

pub struct NullBackend;

pub const NOT_CONFIGURED_MESSAGE: &str = "No AI backend is set up yet.\n\n\
     Open Settings \u{2192} Agent Backends and add one:\n\
     \u{2022} \u{201c}Local model\u{201d} works with Ollama or LM Studio and needs no API key.\n\
     \u{2022} \u{201c}Claude\u{201d} adds computer use, and needs an Anthropic API key.\n\n\
     Everything else in Orbit \u{2014} shortcuts, clipboard history and web search \u{2014} \
     works without this.";

#[async_trait]
impl AgentBackend for NullBackend {
    fn id(&self) -> &str {
        "null"
    }

    fn display_name(&self) -> &str {
        "Not configured"
    }

    fn supports_computer_use(&self) -> bool {
        false
    }

    async fn chat(&self, _messages: Vec<Message>, _config: &BackendConfig) -> AgentResult<AgentResponse> {
        Err(AgentError::NotConfigured(NOT_CONFIGURED_MESSAGE.into()))
    }

    async fn test_connection(&self, _config: &BackendConfig) -> AgentResult<String> {
        Err(AgentError::NotConfigured(
            "There is nothing to test until you add a real backend.".into(),
        ))
    }
}
