//! Backend for any endpoint that speaks the OpenAI `/chat/completions` dialect.
//!
//! That is a much larger set than "OpenAI": Ollama, LM Studio, vLLM,
//! llama.cpp's server, LocalAI, OpenRouter, Together, Groq, Fireworks, DeepSeek
//! and most corporate gateways all expose it. One backend covers all of them,
//! which is why Caduceus does not ship a per-vendor integration for each.
//!
//! Chat only. Computer use is Anthropic-specific in Caduceus today — see
//! `docs/PLUGIN_GUIDE.md` if you want to add a tool-use loop for another
//! provider.

use async_trait::async_trait;
use futures::StreamExt;
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

    async fn chat(
        &self,
        messages: Vec<Message>,
        config: &BackendConfig,
    ) -> AgentResult<AgentResponse> {
        validate_config(config)?;
        // Non-streaming callers (palette one-shots, tools) still use the
        // buffered path. The chat UI goes through [`stream_chat`] so tokens
        // appear as they are generated.
        chat_once(&messages, config).await
    }
}

fn validate_config(config: &BackendConfig) -> AgentResult<()> {
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
    Ok(())
}

async fn chat_once(messages: &[Message], config: &BackendConfig) -> AgentResult<AgentResponse> {
    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));

    // Endpoints disagree about two fields and advertise neither: newer OpenAI
    // reasoning models demand `max_completion_tokens` over `max_tokens`, and
    // anything that is not a reasoning model rejects `reasoning_effort`
    // outright. Rather than making the user guess which dialect their endpoint
    // speaks, correct whichever one the 400 names and try again.
    //
    // Bounded at two corrections because there are two correctable fields; a
    // third failure is a real error and is returned as one.
    let mut dialect = Dialect::initial();
    let body = loop {
        let payload = build_payload(messages, config, dialect, false);
        match post(&url, config, &payload).await? {
            Ok(body) => break body,
            Err(AgentError::Api { status, body, .. }) if status == 400 => {
                match dialect.adjusted_for(&body) {
                    Some(next) => {
                        log::debug!("endpoint refused a field; retrying with a corrected body");
                        dialect = next;
                    }
                    None => {
                        return Err(AgentError::Api {
                            status,
                            body,
                            provider: config.display_name.clone(),
                        })
                    }
                }
            }
            Err(e) => return Err(e),
        }
    };

    parse_response(&body, config)
}

/// Stream a completion, calling `on_delta` for every content token.
///
/// Falls back to a one-shot request if the endpoint rejects `stream: true`
/// (some gateways still do). The UI still gets a single late delta in that
/// case — better than failing the turn.
pub async fn stream_chat<F>(
    messages: Vec<Message>,
    config: &BackendConfig,
    mut on_delta: F,
) -> AgentResult<AgentResponse>
where
    F: FnMut(&str) + Send,
{
    validate_config(config)?;

    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
    let dialect = Dialect::initial();
    let payload = build_payload(&messages, config, dialect, true);

    match post_stream(&url, config, &payload, &mut on_delta).await {
        Ok(response) => Ok(response),
        Err(AgentError::Api { status, body, .. }) if status == 400 => {
            // A body field the endpoint does not accept: correct it and try
            // streaming once more before giving up on streaming altogether.
            if let Some(next) = dialect.adjusted_for(&body) {
                let retry = build_payload(&messages, config, next, true);
                if let Ok(response) = post_stream(&url, config, &retry, &mut on_delta).await {
                    return Ok(response);
                }
            } else if !body.to_lowercase().contains("stream") {
                // Not a field this knows how to correct, and not a complaint
                // about streaming — falling back to one-shot would only produce
                // the same 400 more slowly.
                return Err(AgentError::Api {
                    status,
                    body,
                    provider: config.display_name.clone(),
                });
            }
            // `chat_once` runs the full correction ladder of its own, so this
            // also covers an endpoint that refuses streaming *and* a field.
            log::debug!("endpoint refused streaming; falling back to one-shot");
            let response = chat_once(&messages, config).await?;
            if !response.text.is_empty() {
                on_delta(&response.text);
            }
            Ok(response)
        }
        Err(e) => Err(e),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenParam {
    MaxTokens,
    MaxCompletionTokens,
}

/// The two ways endpoints disagree about the request body, tracked together so
/// one retry can correct either.
///
/// Both are the same shape of problem: a field that some servers require, some
/// reject, and none advertise. Guessing from the base URL would be wrong for
/// every gateway and proxy in front of a real provider, so the only reliable
/// signal is the 400 itself.
#[derive(Clone, Copy)]
struct Dialect {
    token_param: TokenParam,
    /// Whether to send `reasoning_effort`. Dropped on retry when the endpoint
    /// says it does not know the field — a non-reasoning model behind an
    /// OpenAI-compatible endpoint typically 400s on it rather than ignoring it,
    /// and losing the whole request over an optimisation hint would be a bad
    /// trade. See `BackendConfig::reasoning_effort`.
    reasoning_effort: bool,
}

impl Dialect {
    fn initial() -> Self {
        Self {
            token_param: TokenParam::MaxTokens,
            reasoning_effort: true,
        }
    }

    /// What to change after a 400, or `None` when the body does not name
    /// something this knows how to correct.
    fn adjusted_for(&self, body: &str) -> Option<Self> {
        let mut next = *self;
        let mut changed = false;

        if body.contains("max_completion_tokens") && self.token_param == TokenParam::MaxTokens {
            next.token_param = TokenParam::MaxCompletionTokens;
            changed = true;
        }
        if body.contains("reasoning_effort") && self.reasoning_effort {
            next.reasoning_effort = false;
            changed = true;
        }
        changed.then_some(next)
    }
}

fn build_payload(
    messages: &[Message],
    config: &BackendConfig,
    dialect: Dialect,
    stream: bool,
) -> Value {
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
        "stream": stream,
    });

    let obj = payload.as_object_mut().expect("payload is an object");
    match dialect.token_param {
        TokenParam::MaxTokens => obj.insert("max_tokens".into(), json!(config.max_tokens)),
        TokenParam::MaxCompletionTokens => {
            obj.insert("max_completion_tokens".into(), json!(config.max_tokens))
        }
    };
    if let Some(t) = config.temperature {
        obj.insert("temperature".into(), json!(t));
    }
    // Only when explicitly set, and only until an endpoint tells us it does not
    // know the field: a server that does not recognise this is likelier to
    // reject the whole request than to ignore it, so an unset value means "do
    // not send it" rather than "send a default", and a rejection means "never
    // mind" rather than "fail the turn".
    if dialect.reasoning_effort {
        if let Some(effort) = config.reasoning_effort.as_deref().filter(|e| !e.is_empty()) {
            obj.insert("reasoning_effort".into(), json!(effort));
        }
    }
    // OpenAI (and most compatible gateways) omit usage from SSE unless asked.
    // Harmless on servers that ignore unknown fields; Ollama still reports
    // counts on the final chunk either way.
    if stream {
        obj.insert("stream_options".into(), json!({ "include_usage": true }));
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
    let response = send(url, config, payload).await?;
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

async fn send(
    url: &str,
    config: &BackendConfig,
    payload: &Value,
) -> AgentResult<reqwest::Response> {
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

    req.send().await.map_err(|e| AgentError::Transport {
        endpoint: url.to_string(),
        source: e,
    })
}

async fn post_stream<F>(
    url: &str,
    config: &BackendConfig,
    payload: &Value,
    on_delta: &mut F,
) -> AgentResult<AgentResponse>
where
    F: FnMut(&str) + Send,
{
    let response = send(url, config, payload).await?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(AgentError::Api {
            provider: PROVIDER.into(),
            status: status.as_u16(),
            body: http::extract_error_message(&body),
        });
    }

    let mut full = String::new();
    let mut usage: Option<Usage> = None;
    let mut model = config.model.clone();
    let mut buffer = String::new();

    let mut stream = response.bytes_stream();
    while let Some(item) = stream.next().await {
        let chunk = item.map_err(|e| AgentError::Transport {
            endpoint: url.to_string(),
            source: e,
        })?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(newline) = buffer.find('\n') {
            let mut line = buffer[..newline].to_string();
            buffer.drain(..=newline);
            if line.ends_with('\r') {
                line.pop();
            }
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }

            let Ok(json) = serde_json::from_str::<Value>(data) else {
                continue;
            };

            if let Some(m) = json.get("model").and_then(Value::as_str) {
                if !m.is_empty() {
                    model = m.to_string();
                }
            }

            if let Some(piece) = delta_content(&json) {
                if !piece.is_empty() {
                    full.push_str(&piece);
                    on_delta(&piece);
                }
            }

            if let Some(parsed) = parse_usage(&json) {
                usage = Some(parsed);
            }
        }
    }

    if full.is_empty() {
        return Err(AgentError::Protocol {
            provider: PROVIDER.into(),
            detail: "stream ended with no message content".into(),
        });
    }

    Ok(AgentResponse {
        text: full,
        model,
        usage,
    })
}

fn delta_content(json: &Value) -> Option<String> {
    // OpenAI / Ollama OpenAI-compat: choices[0].delta.content
    if let Some(s) = json
        .pointer("/choices/0/delta/content")
        .and_then(Value::as_str)
    {
        return Some(s.to_string());
    }
    // Rare: delta.content as a parts array
    if let Some(parts) = json
        .pointer("/choices/0/delta/content")
        .and_then(Value::as_array)
    {
        let joined: String = parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect();
        if !joined.is_empty() {
            return Some(joined);
        }
    }
    // Some local servers emit the full message shape even while streaming.
    if let Some(s) = json
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
    {
        return Some(s.to_string());
    }
    None
}

fn parse_usage(json: &Value) -> Option<Usage> {
    let u = json.get("usage")?;
    let input = u
        .get("prompt_tokens")
        .or_else(|| u.get("input_tokens"))
        .and_then(Value::as_u64)
        .map(|v| v as u32);
    let output = u
        .get("completion_tokens")
        .or_else(|| u.get("output_tokens"))
        .and_then(Value::as_u64)
        .map(|v| v as u32);
    if input.is_none() && output.is_none() {
        return None;
    }
    Some(Usage {
        input_tokens: input,
        output_tokens: output,
    })
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
        usage: parse_usage(&json),
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
        let p = build_payload(&[Message::user("hi")], &c, Dialect::initial(), false);
        let msgs = p["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "be terse");
        assert_eq!(msgs[1]["role"], "user");
    }

    #[test]
    fn payload_omits_an_empty_system_prompt() {
        let p = build_payload(&[Message::user("hi")], &cfg(), Dialect::initial(), false);
        assert_eq!(p["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn streaming_payload_asks_for_usage() {
        let p = build_payload(&[Message::user("x")], &cfg(), Dialect::initial(), true);
        assert_eq!(p["stream"], true);
        assert_eq!(p["stream_options"]["include_usage"], true);
    }

    #[test]
    fn token_parameter_can_be_switched_for_reasoning_models() {
        let a = build_payload(&[Message::user("x")], &cfg(), Dialect::initial(), false);
        assert_eq!(a["max_tokens"], 512);
        assert!(a.get("max_completion_tokens").is_none());

        let b = build_payload(
            &[Message::user("x")],
            &cfg(),
            Dialect {
                token_param: TokenParam::MaxCompletionTokens,
                reasoning_effort: true,
            },
            false,
        );
        assert_eq!(b["max_completion_tokens"], 512);
        assert!(b.get("max_tokens").is_none());
    }

    /// The correction ladder `chat_once` walks, tested on its own because the
    /// alternative is a live endpoint that rejects things on purpose.
    ///
    /// This is the guard on a real bug: `tools::promptopt` sets
    /// `reasoning_effort` on every call it makes, and most models are not
    /// reasoning models. Without the retry, pointing the prompt optimiser at a
    /// plain hosted model failed the whole request with a 400 rather than
    /// quietly doing without the hint.
    #[test]
    fn a_refused_field_is_corrected_rather_than_failing_the_turn() {
        let start = Dialect::initial();
        assert!(start.reasoning_effort);
        assert_eq!(start.token_param, TokenParam::MaxTokens);

        let no_effort = start
            .adjusted_for("Unrecognized request argument supplied: reasoning_effort")
            .expect("a named field is correctable");
        assert!(!no_effort.reasoning_effort);

        let other_token = start
            .adjusted_for("Use 'max_completion_tokens' instead")
            .expect("a named field is correctable");
        assert_eq!(other_token.token_param, TokenParam::MaxCompletionTokens);

        // Both at once, over two rounds — an endpoint may only complain about
        // one field per response.
        let both = other_token
            .adjusted_for("Unrecognized request argument supplied: reasoning_effort")
            .expect("the second field is correctable too");
        assert!(!both.reasoning_effort);
        assert_eq!(both.token_param, TokenParam::MaxCompletionTokens);

        // The ladder must terminate: nothing left to correct means the error is
        // real and gets returned, rather than retried forever.
        assert!(both
            .adjusted_for("Unrecognized request argument supplied: reasoning_effort")
            .is_none());
        assert!(start.adjusted_for("You exceeded your quota").is_none());
    }

    #[test]
    fn reasoning_effort_is_only_sent_when_set() {
        let p = build_payload(&[Message::user("x")], &cfg(), Dialect::initial(), false);
        assert!(
            p.get("reasoning_effort").is_none(),
            "an unset value must not appear at all \u{2014} servers that do not know the field \
             reject the request rather than ignoring it"
        );

        let mut c = cfg();
        c.reasoning_effort = Some("none".into());
        let p = build_payload(&[Message::user("x")], &c, Dialect::initial(), false);
        assert_eq!(p["reasoning_effort"], "none");

        // An empty string is a cleared field, not a value to send.
        c.reasoning_effort = Some(String::new());
        let p = build_payload(&[Message::user("x")], &c, Dialect::initial(), false);
        assert!(p.get("reasoning_effort").is_none());
    }

    #[test]
    fn temperature_is_only_sent_when_set() {
        let p = build_payload(&[Message::user("x")], &cfg(), Dialect::initial(), false);
        assert!(p.get("temperature").is_none());

        let mut c = cfg();
        c.temperature = Some(0.2);
        let p = build_payload(&[Message::user("x")], &c, Dialect::initial(), false);
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

    #[test]
    fn delta_content_reads_openai_chunks() {
        let chunk = serde_json::json!({"choices":[{"delta":{"content":"Hi"}}]});
        assert_eq!(delta_content(&chunk).as_deref(), Some("Hi"));
    }
}
