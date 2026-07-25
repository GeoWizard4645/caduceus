//! Shared HTTP plumbing for provider backends.

use std::time::Duration;

use super::types::{AgentError, AgentResult};

/// Build a client for one request.
///
/// Clients are cheap to create and each backend can have its own timeout, so
/// there is no shared pool. `reqwest` keeps its own connection pool per client;
/// for Caduceus's request volume (a handful per minute at most) that is irrelevant.
pub fn client(timeout_secs: u64) -> AgentResult<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs.clamp(5, 900)))
        // Local model servers on a slow first token still need the connection
        // itself to come up fast, so failure is reported quickly.
        .connect_timeout(Duration::from_secs(10))
        .user_agent(concat!("Caduceus/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| AgentError::Other(format!("could not create an HTTP client: {e}")))
}

/// Pull a human-readable message out of a provider error body.
///
/// Both dialects Caduceus talks to nest the useful text differently
/// (`{"error":{"message":…}}` vs `{"error":"…"}`), and some local servers just
/// return a bare string. Falls back to the truncated raw body.
pub fn extract_error_message(body: &str) -> String {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        for pointer in ["/error/message", "/message", "/detail"] {
            if let Some(s) = json.pointer(pointer).and_then(|v| v.as_str()) {
                return s.to_string();
            }
        }
        if let Some(s) = json.get("error").and_then(|v| v.as_str()) {
            return s.to_string();
        }
    }
    truncate(body.trim(), 400)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}\u{2026}", s.chars().take(max).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_nested_provider_messages() {
        assert_eq!(
            extract_error_message(r#"{"error":{"type":"invalid_request_error","message":"bad model"}}"#),
            "bad model"
        );
        assert_eq!(extract_error_message(r#"{"error":"model not found"}"#), "model not found");
        assert_eq!(extract_error_message(r#"{"detail":"unauthorized"}"#), "unauthorized");
    }

    #[test]
    fn falls_back_to_the_raw_body() {
        assert_eq!(extract_error_message("  plain text  "), "plain text");
        let long = "x".repeat(500);
        assert!(extract_error_message(&long).ends_with('\u{2026}'));
    }
}
