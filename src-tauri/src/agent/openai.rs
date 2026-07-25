//! Backend for any endpoint that speaks the OpenAI `/chat/completions` dialect.
//!
//! That is a much larger set than "OpenAI": Ollama, LM Studio, vLLM,
//! llama.cpp's server, LocalAI, OpenRouter, Together, Groq, Fireworks, DeepSeek
//! and most corporate gateways all expose it. One backend covers all of them,
//! which is why Orbit does not ship a per-vendor integration for each.
//!
//! Chat only. Computer use is Anthropic-specific in Orbit today — see
//! `docs/PLUGIN_GUIDE.md` if you want to add a tool-use loop for another
//! provider.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::backend::AgentBackend;
use super::http;
use super::types::{AgentError, AgentResponse, AgentResult, Message, Role, Usage};
use crate::settings::{secrets, BackendConfig};

pub struct OpenAiCompatibleBackend;

const PROVIDER: &str = "OpenAI-compatible endpoint";

#[async_trait]
impl AgentBackend for OpenAiCompatibleBackend {
    fn id(&self) -> &str {
        "openai_compatible"
    }

    fn display_name(&self) -> &str {
        "OpenAI-compatible"
    }

    fn supports_computer_use(&self) -> bool {
        false
    }

    async fn chat(&self, messages: Vec<Message>, config: &BackendConfig) -> AgentResult<AgentResponse> {
        if config.base_url.trim().is_empty() {
            return Err(AgentError::NotConfigured(
                "This backend has no base URL. Set one in Settings \u{2192} Agent Backends \
                 (for Ollama that is http://localhost:11434/v1)."
                    .into(),
            ));
        }
        if config.model.trim().is_empty() {
            return Err(AgentError::NotConfigured(
                "This backend has no model name set.".into(),
            ));
        }

        let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
        let payload = build_payload(&messages, config, TokenParam::MaxTokens);

        let response = post(&url, config, &payload).await?;

        // Newer OpenAI reasoning models reject `max_tokens` and demand
        // `max_completion_tokens`. Rather than making the user guess which
        // spelling their endpoint wants, detect the rejection and retry once.
        let body = match response {
            Ok(body) => body,
            Err(AgentError::Api { status, body, .. })
                if status == 400 && body.contains("max_completion_tokens") =>
            {
                log::debug!("endpoint wants max_completion_tokens; retrying");
                let retry = build_payload(&messages, config, TokenParam::MaxCompletionTokens);
                post(&url, config, &retry).await??
            }
            Err(e) => return Err(e),
        };

        parse_response(&body, config)
    }
}

enum TokenParam {
    MaxTokens,
    MaxCompletionTokens,
}

fn build_payload(messages: &[Message], config: &BackendConfig, token_param: TokenParam) -> Value {
    let mut wire: Vec<Value> = Vec::with_capacity(messages.len() + 1);

    if !config.system_prompt.trim().is_empty() {
        wire.push(json!({ "role": "system", "content": config.system_prompt }));
    }
    for m in messages {
        wire.push(json!({
            "role": match m.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
            },
            "content": m.content,
        }));
    }

    let mut payload = json!({
        "model": config.model,
        "messages": wire,
        "stream": false,
    });

    let obj = payload.as_object_mut().expect("payload is an object");
    match token_param {
        TokenParam::MaxTokens => obj.insert("max_tokens".into(), json!(config.max_tokens)),
        TokenParam::MaxCompletionTokens => {
            obj.insert("max_completion_tokens".into(), json!(config.max_tokens))
        }
    };
    if let Some(t) = config.temperature {
        obj.insert("temperature".into(), json!(t));
    }

    payload
}

/// Send the request. The outer `Result` is a transport failure; the inner one
/// carries an API-level error so the caller can inspect and retry it.
async fn post(
    url: &str,
    config: &BackendConfig,
    payload: &Value,
) -> AgentResult<AgentResult<String>> {
    let client = http::client(config.timeout_secs)?;
    let mut req = client.post(url).json(payload);

    // A blank key is normal and correct for local servers.
    if let Some(key) = secrets::get_backend_api_key_opt(&config.id) {
        req = req.bearer_auth(key);
    }
    for [name, value] in &config.extra_headers {
        if !name.is_empty() {
            req = req.header(name, value);
        }
    }

    let response = req.send().await.map_err(|e| AgentError::Transport {
        endpoint: url.to_string(),
        source: e,
    })?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();

    if status.is_success() {
        Ok(Ok(body))
    } else {
        Ok(Err(AgentError::Api {
            provider: PROVIDER.into(),
            status: status.as_u16(),
            body: http::extract_error_message(&body),
        }))
    }
}

fn parse_response(body: &str, config: &BackendConfig) -> AgentResult<AgentResponse> {
    let json: Value = serde_json::from_str(body).map_err(|e| AgentError::Protocol {
        provider: PROVIDER.into(),
        detail: format!("response was not JSON: {e}"),
    })?;

    let text = json
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        // Some servers (and reasoning models) return content as an array of
        // parts rather than a plain string.
        .map(str::to_string)
        .or_else(|| {
            json.pointer("/choices/0/message/content")
                .and_then(Value::as_array)
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|p| p.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("")
                })
        })
        .ok_or_else(|| AgentError::Protocol {
            provider: PROVIDER.into(),
            detail: "no message content in the response".into(),
        })?;

    Ok(AgentResponse {
        text,
        model: json
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(&config.model)
            .to_string(),
        usage: json.get("usage").map(|u| Usage {
            input_tokens: u.get("prompt_tokens").and_then(Value::as_u64).map(|v| v as u32),
            output_tokens: u
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .map(|v| v as u32),
        }),
    })
}

/// Ask the endpoint what models it has, for the Settings model picker.
///
/// Every server in this family implements `GET /models`; the ones that do not
/// simply produce an error the UI turns into "type the model name yourself".
pub async fn list_models(config: &BackendConfig) -> AgentResult<Vec<String>> {
    let url = format!("{}/models", config.base_url.trim_end_matches('/'));
    let client = http::client(config.timeout_secs.min(20))?;
    let mut req = client.get(&url);
    if let Some(key) = secrets::get_backend_api_key_opt(&config.id) {
        req = req.bearer_auth(key);
    }
    for [name, value] in &config.extra_headers {
        if !name.is_empty() {
            req = req.header(name, value);
        }
    }

    let response = req.send().await.map_err(|e| AgentError::Transport {
        endpoint: url.clone(),
        source: e,
    })?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(AgentError::Api {
            provider: PROVIDER.into(),
            status: status.as_u16(),
            body: http::extract_error_message(&body),
        });
    }

    let json: Value = serde_json::from_str(&body).map_err(|e| AgentError::Protocol {
        provider: PROVIDER.into(),
        detail: e.to_string(),
    })?;

    let mut names: Vec<String> = json
        .get("data")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|m| m.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    names.sort_unstable();
    names.dedup();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> BackendConfig {
        BackendConfig {
            id: "test".into(),
            base_url: "http://localhost:11434/v1".into(),
            model: "llama3.2".into(),
            max_tokens: 512,
            ..Default::default()
        }
    }

    #[test]
    fn payload_includes_the_system_prompt_first() {
        let mut c = cfg();
        c.system_prompt = "be terse".into();
        let p = build_payload(&[Message::user("hi")], &c, TokenParam::MaxTokens);
        let msgs = p["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "be terse");
        assert_eq!(msgs[1]["role"], "user");
    }

    #[test]
    fn payload_omits_an_empty_system_prompt() {
        let p = build_payload(&[Message::user("hi")], &cfg(), TokenParam::MaxTokens);
        assert_eq!(p["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn token_parameter_can_be_switched_for_reasoning_models() {
        let a = build_payload(&[Message::user("x")], &cfg(), TokenParam::MaxTokens);
        assert_eq!(a["max_tokens"], 512);
        assert!(a.get("max_completion_tokens").is_none());

        let b = build_payload(&[Message::user("x")], &cfg(), TokenParam::MaxCompletionTokens);
        assert_eq!(b["max_completion_tokens"], 512);
        assert!(b.get("max_tokens").is_none());
    }

    #[test]
    fn temperature_is_only_sent_when_set() {
        let p = build_payload(&[Message::user("x")], &cfg(), TokenParam::MaxTokens);
        assert!(p.get("temperature").is_none());

        let mut c = cfg();
        c.temperature = Some(0.2);
        let p = build_payload(&[Message::user("x")], &c, TokenParam::MaxTokens);
        // Compared with a tolerance: the config field is an f32 and widens to a
        // JSON double, so an exact match would be asserting on binary
        // representation rather than on behaviour.
        let sent = p["temperature"].as_f64().expect("temperature is a number");
        assert!((sent - 0.2).abs() < 1e-6, "got {sent}");
    }

    #[test]
    fn parses_a_standard_completion() {
        let body = r#"{"model":"llama3.2","choices":[{"message":{"content":"hello"}}],
                       "usage":{"prompt_tokens":4,"completion_tokens":2}}"#;
        let r = parse_response(body, &cfg()).unwrap();
        assert_eq!(r.text, "hello");
        assert_eq!(r.model, "llama3.2");
        assert_eq!(r.usage.unwrap().input_tokens, Some(4));
    }

    #[test]
    fn parses_array_style_content() {
        let body = r#"{"choices":[{"message":{"content":[{"type":"text","text":"a"},
                       {"type":"text","text":"b"}]}}]}"#;
        assert_eq!(parse_response(body, &cfg()).unwrap().text, "ab");
    }

    #[test]
    fn missing_content_is_a_protocol_error_not_a_panic() {
        assert!(matches!(
            parse_response(r#"{"choices":[]}"#, &cfg()),
            Err(AgentError::Protocol { .. })
        ));
        assert!(matches!(
            parse_response("not json", &cfg()),
            Err(AgentError::Protocol { .. })
        ));
    }
}
