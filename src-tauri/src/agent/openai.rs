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

use std::collections::BTreeMap;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Value};

use super::backend::AgentBackend;
use super::context;
use super::http;
use super::types::{AgentError, AgentResponse, AgentResult, Message, Role, ToolCall, Usage};
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

// `pub(crate)` rather than private: `agent::toolloop` validates a config
// itself before starting a session, the same way `stream_chat` does here,
// rather than discovering "no base URL" only after already having emitted an
// `AgentStep::Started` for a session that was never going to work.
pub(crate) fn validate_config(config: &BackendConfig) -> AgentResult<()> {
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

// ---------------------------------------------------------------------------
// Tool calling
// ---------------------------------------------------------------------------
//
// Everything below this point is additive: none of it runs unless a caller
// passes a non-empty `tools` slice, which today only `agent::toolloop` does.
// `chat_once` / `stream_chat` above continue to send exactly the request they
// always have.

/// A tool offered to the model on one turn, already reduced to OpenAI's
/// function-calling shape: a wire-safe name, a description, and a JSON
/// Schema `parameters` object.
///
/// Building this from Caduceus's MCP registry — sanitizing the id, and
/// remembering how to map a wire name back to it — is `agent::toolloop`'s
/// job; this module only needs to know how to put one on the wire and how to
/// read a call back off it, the same separation `Message` keeps from the
/// dialect that serialises it.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// One model turn from a tool-enabled request.
///
/// Kept separate from [`AgentResponse`] rather than adding fields to it:
/// `AgentResponse` is "the answer", handed all the way up through
/// `AgentBackend::chat` to chat history, the palette, and half a dozen
/// `tools::*` callers that have no notion of a tool call and should never
/// need to grow one. A turn that only asked for a tool has no answer yet —
/// modelling that as an `AgentResponse` would mean every one of those callers
/// gains a `tool_calls` field they can never act on.
#[derive(Debug, Clone)]
pub(crate) struct ChatTurn {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub model: String,
    pub usage: Option<Usage>,
}

async fn chat_once(messages: &[Message], config: &BackendConfig) -> AgentResult<AgentResponse> {
    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
    let body = post_with_dialect_retry(&url, config, |dialect| {
        build_payload(messages, config, dialect, false, &[])
    })
    .await?;
    parse_response(&body, config)
}

/// Like [`chat_once`], but offers `tools` to the model and returns whatever
/// it asked for instead of insisting on prose — see [`ChatTurn`]. The tool
/// loop's primary request path is actually [`stream_chat_with_tools`], not
/// this; it exists as that function's one-shot fallback (mirroring
/// `stream_chat`'s own fallback to plain [`chat_once`]) and is exercised
/// directly by anything that has no reason to stream.
pub(crate) async fn chat_once_with_tools(
    messages: &[Message],
    config: &BackendConfig,
    tools: &[ToolSpec],
) -> AgentResult<ChatTurn> {
    validate_config(config)?;
    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
    let body = post_with_dialect_retry(&url, config, |dialect| {
        build_payload(messages, config, dialect, false, tools)
    })
    .await?;
    parse_response_with_tools(&body, config)
}

/// Post one request, walking the same dialect-correction ladder [`Dialect`]
/// documents: on a 400 that names a field it knows how to drop or rename,
/// rebuild the payload with `build` and try again, bounded by however many
/// fields `Dialect` can correct (two, today).
///
/// Factored out of [`chat_once`] so [`chat_once_with_tools`] gets the exact
/// same ladder rather than a second copy of it — the ladder itself has
/// nothing to do with whether the request also carries a `tools` array, it is
/// purely about which fields a given endpoint tolerates.
async fn post_with_dialect_retry(
    url: &str,
    config: &BackendConfig,
    build: impl Fn(Dialect) -> Value,
) -> AgentResult<String> {
    let mut dialect = Dialect::initial();
    loop {
        let payload = build(dialect);
        match post(url, config, &payload).await? {
            Ok(body) => return Ok(body),
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
    }
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
    let payload = build_payload(&messages, config, dialect, true, &[]);

    match post_stream(&url, config, &payload, &mut on_delta).await {
        Ok(response) => Ok(response),
        Err(AgentError::Api { status, body, .. }) if status == 400 => {
            // A body field the endpoint does not accept: correct it and try
            // streaming once more before giving up on streaming altogether.
            if let Some(next) = dialect.adjusted_for(&body) {
                let retry = build_payload(&messages, config, next, true, &[]);
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

/// Like [`stream_chat`], but offers `tools` and returns whatever the model
/// asked for — see [`ChatTurn`]. `on_delta` fires for text tokens exactly
/// like `stream_chat`'s; a caller that only cares about the fully-assembled
/// turn once the request completes (the tool loop, today) can pass a no-op.
///
/// This is the tool loop's primary request path, not [`chat_once_with_tools`]
/// — the loop asks for a streamed response for the same reason the chat
/// window does: an endpoint that is slow to finish a long turn is still
/// making visible progress rather than looking hung, and on any failure this
/// falls back to the one-shot path exactly like `stream_chat` does, so
/// nothing about the loop's reliability depends on streaming actually
/// working.
pub(crate) async fn stream_chat_with_tools<F>(
    messages: &[Message],
    config: &BackendConfig,
    tools: &[ToolSpec],
    mut on_delta: F,
) -> AgentResult<ChatTurn>
where
    F: FnMut(&str) + Send,
{
    validate_config(config)?;

    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
    let dialect = Dialect::initial();
    let payload = build_payload(messages, config, dialect, true, tools);

    match post_stream_with_tools(&url, config, &payload, &mut on_delta).await {
        Ok(turn) => Ok(turn),
        Err(AgentError::Api { status, body, .. }) if status == 400 => {
            if let Some(next) = dialect.adjusted_for(&body) {
                let retry = build_payload(messages, config, next, true, tools);
                if let Ok(turn) = post_stream_with_tools(&url, config, &retry, &mut on_delta).await {
                    return Ok(turn);
                }
            } else if !body.to_lowercase().contains("stream") {
                return Err(AgentError::Api {
                    status,
                    body,
                    provider: config.display_name.clone(),
                });
            }
            log::debug!("endpoint refused streaming; falling back to one-shot");
            let turn = chat_once_with_tools(messages, config, tools).await?;
            if !turn.text.is_empty() {
                on_delta(&turn.text);
            }
            Ok(turn)
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
    /// Whether to send Ollama's `options`/`think` extras (see `build_payload`
    /// and `looks_like_ollama`). Dropped on retry the same way
    /// `reasoning_effort` is: real Ollama does not reject these (verified
    /// live against 0.32.1 — it silently ignores them), but a stricter proxy
    /// or gateway sitting in front of something Ollama-shaped still might,
    /// and losing the whole request over a best-effort extra would be exactly
    /// the bad trade `reasoning_effort`'s own doc describes.
    ollama_extras: bool,
}

impl Dialect {
    fn initial() -> Self {
        Self {
            token_param: TokenParam::MaxTokens,
            reasoning_effort: true,
            ollama_extras: true,
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
        if self.ollama_extras
            && (body.contains("options") || body.contains("num_ctx") || body.contains("\"think\""))
        {
            next.ollama_extras = false;
            changed = true;
        }
        changed.then_some(next)
    }
}

/// Best-effort "is this endpoint Ollama" signal, from the base URL alone —
/// `BackendConfig` has no distinct Ollama kind (it is generically
/// [`crate::settings::BackendKind::OpenAiCompatible`]; Ollama is "just" an
/// OpenAI-compatible base URL, per `agent::context`'s module doc). Mirrors
/// the exact heuristic the reference implementation's own CLI banner uses
/// (`"11434" in base_url or "ollama" in base_url.lower()`) rather than
/// inventing a new one: Ollama's documented default port, or the word
/// appearing in a custom host or path.
///
/// The three fixes this gates (see `build_payload`) are only worth applying
/// to something that is actually Ollama: unlike the existing
/// `reasoning_effort` field, which every OpenAI-compatible server is
/// expected to at least recognise the *name* of, forcing every backend's
/// `max_tokens` up to [`context::OLLAMA_REQUEST_FLOOR`] regardless of what
/// was configured would be a real behaviour change for a real cloud
/// endpoint — this stays a no-op for anything that does not look like
/// Ollama, exactly like today.
fn looks_like_ollama(config: &BackendConfig) -> bool {
    let base = config.base_url.to_ascii_lowercase();
    base.contains(":11434") || base.contains("ollama")
}

fn build_payload(
    messages: &[Message],
    config: &BackendConfig,
    dialect: Dialect,
    stream: bool,
    tools: &[ToolSpec],
) -> Value {
    let mut wire: Vec<Value> = Vec::with_capacity(messages.len() + 1);

    if !config.system_prompt.trim().is_empty() {
        wire.push(json!({ "role": "system", "content": config.system_prompt }));
    }
    for m in messages {
        let mut entry = json!({
            "role": match m.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            },
            "content": m.content,
        });
        // Both of these are wire-format specifics of tool calling and absent
        // on every turn that predates it — exactly what `Message`'s own
        // `#[serde(default)]`s protect on the way in (see `agent::types`),
        // mirrored here on the way out so a tool-less conversation produces
        // the identical body it always has.
        let entry_obj = entry.as_object_mut().expect("entry is an object");
        if !m.tool_calls.is_empty() {
            let wire_calls: Vec<Value> = m
                .tool_calls
                .iter()
                .map(|tc| {
                    json!({
                        "id": tc.id,
                        "type": "function",
                        "function": { "name": tc.name, "arguments": tc.arguments },
                    })
                })
                .collect();
            entry_obj.insert("tool_calls".into(), json!(wire_calls));
        }
        if let Some(id) = &m.tool_call_id {
            entry_obj.insert("tool_call_id".into(), json!(id));
        }
        wire.push(entry);
    }

    let mut payload = json!({
        "model": config.model,
        "messages": wire,
        "stream": stream,
    });

    let obj = payload.as_object_mut().expect("payload is an object");
    let is_ollama = looks_like_ollama(config);

    // Ollama fix #1 (see `context`'s module doc): floor the output-token
    // budget for a tool-calling request specifically, rather than sending
    // whatever was configured verbatim.
    //
    // Why a floor at all: Ollama falls back to its own tiny `num_predict=128`
    // default whenever the request's budget is not generous, and a
    // thinking-capable model can burn that *entire* budget on its hidden
    // reasoning trace before emitting any real content — reproduced live
    // against `qwen3.5:0.8b`: a 100-token budget came back with empty
    // `content`, `finish_reason: "length"`, all 100 tokens spent on
    // `reasoning`. For a tool call specifically, that is not "a short reply"
    // — the loop gets no `tool_calls` back at all and reads as silently
    // broken, which is the exact trap this whole module exists to prevent.
    //
    // Why gated on `!tools.is_empty()` rather than applied to every request:
    // the reference implementation's own floor (`default_max_tokens =
    // 65536`) is a *default* — applied only when the caller never set one —
    // not a floor that overrides an explicit choice, and the two are not the
    // same thing. `BackendConfig::max_tokens` is a plain `u32` with no
    // "unset" state to tell "the user chose 512" apart from "nobody ever
    // touched this", so this cannot faithfully reproduce "default" the same
    // way. What it can do is bound *where* an override is even acceptable:
    // someone who sets a small `max_tokens` on Ollama for fast, short prose
    // replies has made a deliberate, reasonable choice for that case, and a
    // plain chat turn silently ignoring it would be exactly the undocumented
    // "we know better" behaviour this needs to avoid. A tool-calling turn has
    // no such reasonable small setting — there is no value of "I want short
    // tool calls" that trades off against "I want tool calls to work at
    // all" — so only that path is floored. Every plain-chat request, tool or
    // no tool, on every non-Ollama endpoint, keeps sending exactly the
    // configured value, byte-identical to before this existed.
    let max_tokens = if is_ollama && !tools.is_empty() {
        config.max_tokens.max(context::OLLAMA_REQUEST_FLOOR)
    } else {
        config.max_tokens
    };
    match dialect.token_param {
        TokenParam::MaxTokens => obj.insert("max_tokens".into(), json!(max_tokens)),
        TokenParam::MaxCompletionTokens => {
            obj.insert("max_completion_tokens".into(), json!(max_tokens))
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
            // Ollama fix #3: disabling reasoning needs a companion field
            // alongside this one. Confirmed live against Ollama 0.32.1 —
            // this endpoint (`/chat/completions`, the only one this backend
            // ever calls) reads `reasoning_effort` but silently ignores
            // `think=`; the *native* `/api/chat` endpoint is the reverse
            // (ollama#14820). So `think: false` alone would be a no-op on
            // every call path Caduceus actually uses — it is sent anyway,
            // matching the reference implementation's belt-and-suspenders
            // approach, in case a future Ollama version or an intermediary
            // honours it where today's `/chat/completions` does not. Free
            // either way: an endpoint that does not recognise the field just
            // ignores it (verified live — no 400).
            if is_ollama && dialect.ollama_extras && effort.eq_ignore_ascii_case("none") {
                obj.insert("think".into(), json!(false));
            }
        }
    }
    // Ollama fix #2: `num_ctx` has no OpenAI-spec equivalent, and Ollama
    // maintainers rejected adding one to this endpoint upstream
    // (ollama/ollama#6137: "this does not follow OpenAI's API spec"). Verified
    // live against 0.32.1: `/chat/completions` does not honour this in *any*
    // wire shape tried (bare top-level, nested under `options`, or nested
    // under a literal `extra_body` key) — only `/api/chat` (native, never
    // called here) reads it. Sent anyway because it is free (confirmed no
    // 400, just silently unused) and because a different server, proxy, or
    // future Ollama version in this same "OpenAI-compatible" family might
    // honour it where this one does not. The fix actually confirmed to survive
    // every call path is `agent::context::remediate`, which bakes `num_ctx`
    // into the model itself via a Modelfile so it stops being a per-request
    // override at all — see that function's doc.
    if is_ollama && dialect.ollama_extras {
        obj.insert("options".into(), json!({ "num_ctx": context::OLLAMA_REQUEST_FLOOR }));
    }
    // OpenAI (and most compatible gateways) omit usage from SSE unless asked.
    // Harmless on servers that ignore unknown fields; Ollama still reports
    // counts on the final chunk either way.
    if stream {
        obj.insert("stream_options".into(), json!({ "include_usage": true }));
    }
    // Only when there is at least one tool to offer: an empty `"tools": []`
    // is accepted by OpenAI itself but rejected outright by some
    // OpenAI-compatible servers, so the simplest thing that is correct
    // everywhere is to omit the field entirely when there is nothing in it —
    // which is also exactly the "byte-identical to today" behaviour every
    // existing caller (none of which pass tools) depends on.
    if !tools.is_empty() {
        let wire_tools: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    },
                })
            })
            .collect();
        obj.insert("tools".into(), json!(wire_tools));
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

/// Like [`post_stream`], but also assembles any `tool_calls` deltas the
/// stream carries and returns them alongside whatever text arrived — see
/// [`ToolCallAssembler`] and [`ChatTurn`].
///
/// A separate function rather than a flag on `post_stream` because the two
/// have a real behavioural difference beyond "also look for tool_calls": an
/// empty `full` is a protocol error for `post_stream`'s tool-less callers (a
/// stream that said nothing at all), but is completely ordinary here
/// whenever the model's entire turn was a tool request with no prose
/// alongside it.
async fn post_stream_with_tools<F>(
    url: &str,
    config: &BackendConfig,
    payload: &Value,
    on_delta: &mut F,
) -> AgentResult<ChatTurn>
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
    let mut tool_calls = ToolCallAssembler::default();

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

            if let Some(delta) = json.pointer("/choices/0/delta") {
                tool_calls.absorb(delta);
            }

            if let Some(parsed) = parse_usage(&json) {
                usage = Some(parsed);
            }
        }
    }

    let tool_calls = tool_calls.finish();
    if full.is_empty() && tool_calls.is_empty() {
        return Err(AgentError::Protocol {
            provider: PROVIDER.into(),
            detail: "stream ended with no message content".into(),
        });
    }

    Ok(ChatTurn {
        text: full,
        tool_calls,
        model,
        usage,
    })
}

/// Accumulates OpenAI-style streamed `tool_calls` deltas into complete
/// [`ToolCall`]s.
///
/// A streamed response never sends a tool call whole: each delta carries an
/// `index` (which of possibly several parallel calls this fragment belongs
/// to), and `id` / `function.name` typically arrive once, on that index's
/// first delta, while `function.arguments` arrives split across many —
/// concatenated in order, the pieces are not even valid JSON on their own.
/// Buffered by index rather than by array position, because nothing in the
/// format guarantees indices arrive contiguously, in order, or
/// un-interleaved with another call's deltas.
#[derive(Default)]
struct ToolCallAssembler {
    by_index: BTreeMap<u64, PartialToolCall>,
}

#[derive(Default)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

impl ToolCallAssembler {
    /// Fold one chunk's `delta.tool_calls` array, if it has one, into the
    /// buffer. Safe to call on every chunk of every stream, tool-enabled or
    /// not — a delta with nothing under `tool_calls` is simply a no-op, which
    /// is what lets [`post_stream_with_tools`] call this unconditionally
    /// rather than needing to know in advance whether tools were offered.
    fn absorb(&mut self, delta: &Value) {
        let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) else {
            return;
        };
        for call in calls {
            let Some(index) = call.get("index").and_then(Value::as_u64) else {
                continue; // a fragment naming no index cannot be placed
            };
            let entry = self.by_index.entry(index).or_default();
            if let Some(id) = call.get("id").and_then(Value::as_str) {
                entry.id = id.to_string();
            }
            if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                entry.name.push_str(name);
            }
            if let Some(args) = call.pointer("/function/arguments").and_then(Value::as_str) {
                entry.arguments.push_str(args);
            }
        }
    }

    /// Every call assembled so far, in index order. A call whose name never
    /// arrived is dropped: with no name there is nothing to resolve it back
    /// to an MCP tool with, so it could never have been executed anyway, and
    /// silently losing one malformed call is a better outcome than failing
    /// the whole turn over it.
    fn finish(self) -> Vec<ToolCall> {
        self.by_index
            .into_values()
            .filter(|c| !c.name.is_empty())
            .map(|c| ToolCall {
                id: c.id,
                name: c.name,
                arguments: c.arguments,
            })
            .collect()
    }
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

/// Like [`parse_response`], but for a request that offered tools: the model
/// may reply with no prose at all — `content` is legitimately `null` on the
/// wire whenever the whole turn was a tool request — so an absent message
/// content is not the protocol error here that it is for `parse_response`'s
/// tool-less callers. A response with *neither* content nor tool calls is
/// still an error, since that is not a shape any provider should produce.
fn parse_response_with_tools(body: &str, config: &BackendConfig) -> AgentResult<ChatTurn> {
    let json: Value = serde_json::from_str(body).map_err(|e| AgentError::Protocol {
        provider: PROVIDER.into(),
        detail: format!("response was not JSON: {e}"),
    })?;

    let text = json
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
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
        .unwrap_or_default();

    let tool_calls = parse_tool_calls(json.pointer("/choices/0/message/tool_calls"));

    if text.is_empty() && tool_calls.is_empty() {
        return Err(AgentError::Protocol {
            provider: PROVIDER.into(),
            detail: "no message content or tool calls in the response".into(),
        });
    }

    Ok(ChatTurn {
        text,
        tool_calls,
        model: json
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(&config.model)
            .to_string(),
        usage: parse_usage(&json),
    })
}

/// Read a complete (non-streamed) `tool_calls` array off the wire —
/// `[{ id, type, function: { name, arguments } }, ...]`.
///
/// Entries missing a name are dropped rather than surfaced as an error, the
/// same policy [`ToolCallAssembler::finish`] uses for the streamed case: a
/// call the loop cannot identify is not something it could resolve back to
/// an MCP tool or execute either way, and dropping just that one entry beats
/// failing an otherwise-usable list over it.
fn parse_tool_calls(value: Option<&Value>) -> Vec<ToolCall> {
    let Some(calls) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    calls
        .iter()
        .filter_map(|c| {
            let name = c.pointer("/function/name")?.as_str()?.to_string();
            let id = c
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let arguments = c
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}")
                .to_string();
            Some(ToolCall { id, name, arguments })
        })
        .collect()
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

    /// A generic, deliberately non-Ollama-looking endpoint — every existing
    /// test in this module predates `looks_like_ollama` and asserts on
    /// `build_payload`'s output as it has always been, so this must stay
    /// outside that heuristic or every one of them would start silently
    /// exercising the new Ollama-only branches instead of what their names
    /// say they test. [`ollama_cfg`] is the dedicated config for those.
    fn cfg() -> BackendConfig {
        BackendConfig {
            id: "test".into(),
            base_url: "https://api.example.com/v1".into(),
            model: "llama3.2".into(),
            max_tokens: 512,
            ..Default::default()
        }
    }

    /// Same shape as [`cfg`], but with an Ollama-shaped `base_url` so
    /// `looks_like_ollama` matches — for the tests that specifically cover
    /// the three Ollama-only fixes in `build_payload`.
    fn ollama_cfg() -> BackendConfig {
        BackendConfig {
            base_url: "http://localhost:11434/v1".into(),
            ..cfg()
        }
    }

    #[test]
    fn payload_includes_the_system_prompt_first() {
        let mut c = cfg();
        c.system_prompt = "be terse".into();
        let p = build_payload(&[Message::user("hi")], &c, Dialect::initial(), false, &[]);
        let msgs = p["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "be terse");
        assert_eq!(msgs[1]["role"], "user");
    }

    #[test]
    fn payload_omits_an_empty_system_prompt() {
        let p = build_payload(&[Message::user("hi")], &cfg(), Dialect::initial(), false, &[]);
        assert_eq!(p["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn streaming_payload_asks_for_usage() {
        let p = build_payload(&[Message::user("x")], &cfg(), Dialect::initial(), true, &[]);
        assert_eq!(p["stream"], true);
        assert_eq!(p["stream_options"]["include_usage"], true);
    }

    #[test]
    fn token_parameter_can_be_switched_for_reasoning_models() {
        let a = build_payload(&[Message::user("x")], &cfg(), Dialect::initial(), false, &[]);
        assert_eq!(a["max_tokens"], 512);
        assert!(a.get("max_completion_tokens").is_none());

        let b = build_payload(
            &[Message::user("x")],
            &cfg(),
            Dialect {
                token_param: TokenParam::MaxCompletionTokens,
                reasoning_effort: true,
                ollama_extras: true,
            },
            false,
            &[],
        );
        assert_eq!(b["max_completion_tokens"], 512);
        assert!(b.get("max_tokens").is_none());
    }

    // -----------------------------------------------------------------
    // Ollama-specific request fixes (looks_like_ollama gating + the three
    // defensive additions build_payload makes for it)
    // -----------------------------------------------------------------

    #[test]
    fn looks_like_ollama_matches_the_default_port_or_the_word_in_the_url() {
        let mut c = cfg();
        assert!(!looks_like_ollama(&c), "the plain cloud-shaped test config must not match");

        c.base_url = "http://localhost:11434/v1".into();
        assert!(looks_like_ollama(&c));

        c.base_url = "http://my-ollama-box.local:9999/v1".into();
        assert!(looks_like_ollama(&c), "the word \"ollama\" alone is also a match, any port");

        c.base_url = "http://localhost:11434/v1".to_ascii_uppercase();
        assert!(looks_like_ollama(&c), "matching is case-insensitive");
    }

    #[test]
    fn a_non_ollama_endpoint_gets_none_of_the_three_ollama_fixes() {
        let mut c = cfg();
        c.max_tokens = 512;
        c.reasoning_effort = Some("none".into());
        let p = build_payload(&[Message::user("x")], &c, Dialect::initial(), false, &[a_tool()]);
        assert_eq!(p["max_tokens"], 512, "max_tokens must pass through unfloored");
        assert!(p.get("options").is_none(), "no num_ctx nudge for a non-Ollama endpoint");
        assert!(p.get("think").is_none(), "no think:false companion for a non-Ollama endpoint");
    }

    #[test]
    fn ollama_max_tokens_is_floored_for_a_tool_calling_request_but_not_a_plain_one() {
        // See build_payload's own doc for why this is gated on `tools` rather
        // than applied unconditionally: a small `max_tokens` is a reasonable,
        // deliberate choice for fast plain-chat replies, and only a
        // tool-calling turn has no such reasonable small setting to respect.
        let mut c = ollama_cfg();
        c.max_tokens = 512;

        let plain = build_payload(&[Message::user("x")], &c, Dialect::initial(), false, &[]);
        assert_eq!(plain["max_tokens"], 512, "a deliberate small budget must survive plain chat");

        let with_tools =
            build_payload(&[Message::user("x")], &c, Dialect::initial(), false, &[a_tool()]);
        assert_eq!(with_tools["max_tokens"], context::OLLAMA_REQUEST_FLOOR);
    }

    #[test]
    fn ollama_max_tokens_floor_never_lowers_an_already_generous_value() {
        let mut c = ollama_cfg();
        c.max_tokens = context::OLLAMA_REQUEST_FLOOR * 2;
        let p = build_payload(&[Message::user("x")], &c, Dialect::initial(), false, &[a_tool()]);
        assert_eq!(p["max_tokens"], context::OLLAMA_REQUEST_FLOOR * 2);
    }

    #[test]
    fn ollama_requests_carry_a_num_ctx_nudge_regardless_of_tools() {
        // Unlike the max_tokens floor, this never overrides a value the user
        // configured -- `BackendConfig` has no num_ctx field at all yet, so
        // there is nothing to respect instead of, and every kind of request
        // benefits from more context, not just tool calls.
        let p = build_payload(&[Message::user("x")], &ollama_cfg(), Dialect::initial(), false, &[]);
        assert_eq!(p["options"]["num_ctx"], context::OLLAMA_REQUEST_FLOOR);
    }

    #[test]
    fn think_false_is_sent_only_when_ollama_and_reasoning_is_explicitly_none() {
        let mut c = ollama_cfg();

        // Not set at all -- nothing to disable.
        let p = build_payload(&[Message::user("x")], &c, Dialect::initial(), false, &[]);
        assert!(p.get("think").is_none());

        // Set to something other than "none" -- a real effort level, not a
        // disable request.
        c.reasoning_effort = Some("high".into());
        let p = build_payload(&[Message::user("x")], &c, Dialect::initial(), false, &[]);
        assert!(p.get("think").is_none());

        // Set to "none" -- this is the actual disable signal.
        c.reasoning_effort = Some("none".into());
        let p = build_payload(&[Message::user("x")], &c, Dialect::initial(), false, &[]);
        assert_eq!(p["think"], false);
        assert_eq!(p["reasoning_effort"], "none", "the top-level field must still be sent too");

        // Case-insensitive, matching how the rest of the codebase treats
        // this value (e.g. Dialect's own retry ladder is not case-sensitive
        // either -- it matches on the field name, not the value).
        c.reasoning_effort = Some("NONE".into());
        let p = build_payload(&[Message::user("x")], &c, Dialect::initial(), false, &[]);
        assert_eq!(p["think"], false);
    }

    #[test]
    fn dropping_ollama_extras_after_a_400_removes_both_options_and_think() {
        let mut c = ollama_cfg();
        c.reasoning_effort = Some("none".into());

        let dropped = Dialect::initial()
            .adjusted_for("Unrecognized request argument supplied: options")
            .expect("a named field is correctable");
        assert!(!dropped.ollama_extras);

        let p = build_payload(&[Message::user("x")], &c, dropped, false, &[]);
        assert!(p.get("options").is_none());
        assert!(p.get("think").is_none());
        // The generic reasoning_effort mechanism is untouched by this flag --
        // only the Ollama-specific extras are dropped.
        assert_eq!(p["reasoning_effort"], "none");
    }

    #[test]
    fn an_unrelated_400_does_not_touch_ollama_extras() {
        assert!(Dialect::initial().adjusted_for("You exceeded your quota").is_none());
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
        let p = build_payload(&[Message::user("x")], &cfg(), Dialect::initial(), false, &[]);
        assert!(
            p.get("reasoning_effort").is_none(),
            "an unset value must not appear at all \u{2014} servers that do not know the field \
             reject the request rather than ignoring it"
        );

        let mut c = cfg();
        c.reasoning_effort = Some("none".into());
        let p = build_payload(&[Message::user("x")], &c, Dialect::initial(), false, &[]);
        assert_eq!(p["reasoning_effort"], "none");

        // An empty string is a cleared field, not a value to send.
        c.reasoning_effort = Some(String::new());
        let p = build_payload(&[Message::user("x")], &c, Dialect::initial(), false, &[]);
        assert!(p.get("reasoning_effort").is_none());
    }

    #[test]
    fn temperature_is_only_sent_when_set() {
        let p = build_payload(&[Message::user("x")], &cfg(), Dialect::initial(), false, &[]);
        assert!(p.get("temperature").is_none());

        let mut c = cfg();
        c.temperature = Some(0.2);
        let p = build_payload(&[Message::user("x")], &c, Dialect::initial(), false, &[]);
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

    // -----------------------------------------------------------------
    // Tool calling: request side (build_payload)
    // -----------------------------------------------------------------

    fn a_tool() -> ToolSpec {
        ToolSpec {
            name: "fs__read_file".into(),
            description: "Read a file".into(),
            parameters: json!({ "type": "object", "properties": { "path": { "type": "string" } } }),
        }
    }

    #[test]
    fn no_tools_means_no_tools_field_at_all() {
        // The byte-identical-to-today guarantee: every existing caller passes
        // `&[]`, and this is what makes that safe rather than merely
        // convention.
        let p = build_payload(&[Message::user("x")], &cfg(), Dialect::initial(), false, &[]);
        assert!(p.get("tools").is_none());
    }

    #[test]
    fn tools_are_advertised_in_openai_function_shape() {
        let p = build_payload(
            &[Message::user("x")],
            &cfg(),
            Dialect::initial(),
            false,
            &[a_tool()],
        );
        let tools = p["tools"].as_array().expect("tools is an array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["function"]["name"], "fs__read_file");
        assert_eq!(tools[0]["function"]["description"], "Read a file");
        assert_eq!(tools[0]["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn an_assistant_turn_with_tool_calls_carries_them_on_the_wire() {
        let calls = vec![ToolCall {
            id: "call_1".into(),
            name: "fs__read_file".into(),
            arguments: r#"{"path":"/tmp/x"}"#.into(),
        }];
        let messages = [Message::assistant_tool_calls("", calls)];
        let p = build_payload(&messages, &cfg(), Dialect::initial(), false, &[]);
        let wire = &p["messages"].as_array().unwrap()[0];
        assert_eq!(wire["role"], "assistant");
        let wire_calls = wire["tool_calls"].as_array().expect("tool_calls is an array");
        assert_eq!(wire_calls[0]["id"], "call_1");
        assert_eq!(wire_calls[0]["type"], "function");
        assert_eq!(wire_calls[0]["function"]["name"], "fs__read_file");
        assert_eq!(wire_calls[0]["function"]["arguments"], r#"{"path":"/tmp/x"}"#);
    }

    #[test]
    fn a_tool_result_turn_sends_role_tool_and_its_call_id() {
        let messages = [Message::tool_result("call_1", "42")];
        let p = build_payload(&messages, &cfg(), Dialect::initial(), false, &[]);
        let wire = &p["messages"].as_array().unwrap()[0];
        assert_eq!(wire["role"], "tool");
        assert_eq!(wire["tool_call_id"], "call_1");
        assert_eq!(wire["content"], "42");
    }

    #[test]
    fn a_plain_turn_carries_neither_tool_calls_nor_tool_call_id() {
        // Guards the byte-identical claim at message granularity, not just
        // for the payload as a whole: a `Message::user`/`assistant` built
        // before tool calling existed must produce the exact same per-message
        // JSON it always did.
        let p = build_payload(&[Message::user("hi")], &cfg(), Dialect::initial(), false, &[]);
        let wire = &p["messages"].as_array().unwrap()[0];
        assert!(wire.get("tool_calls").is_none());
        assert!(wire.get("tool_call_id").is_none());
    }

    // -----------------------------------------------------------------
    // Tool calling: non-streaming response side (parse_response_with_tools)
    // -----------------------------------------------------------------

    #[test]
    fn a_tool_only_reply_has_null_content_and_is_not_an_error() {
        // The exact shape `parse_response` (the tool-less parser) would
        // reject as "no message content" — the whole reason this needs a
        // separate parser rather than a shared one.
        let body = r#"{"model":"gpt","choices":[{"message":{"role":"assistant","content":null,
                       "tool_calls":[{"id":"call_1","type":"function",
                       "function":{"name":"fs__read_file","arguments":"{\"path\":\"/tmp/x\"}"}}]}}]}"#;
        let turn = parse_response_with_tools(body, &cfg()).unwrap();
        assert_eq!(turn.text, "");
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].id, "call_1");
        assert_eq!(turn.tool_calls[0].name, "fs__read_file");
        assert_eq!(turn.tool_calls[0].arguments, r#"{"path":"/tmp/x"}"#);
    }

    #[test]
    fn prose_alongside_tool_calls_is_kept_too() {
        let body = r#"{"choices":[{"message":{"content":"Let me check.",
                       "tool_calls":[{"id":"call_1","type":"function",
                       "function":{"name":"t","arguments":"{}"}}]}}]}"#;
        let turn = parse_response_with_tools(body, &cfg()).unwrap();
        assert_eq!(turn.text, "Let me check.");
        assert_eq!(turn.tool_calls.len(), 1);
    }

    #[test]
    fn a_reply_with_neither_content_nor_tool_calls_is_still_an_error() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":null}}]}"#;
        assert!(matches!(
            parse_response_with_tools(body, &cfg()),
            Err(AgentError::Protocol { .. })
        ));
    }

    #[test]
    fn a_tool_call_missing_its_function_name_is_dropped_silently_when_there_is_other_content() {
        // The drop happens in `parse_tool_calls` itself, not by rejecting the
        // whole reply — proven here by pairing the malformed call with real
        // prose, which must still come through untouched.
        let body = r#"{"choices":[{"message":{"content":"hello",
                       "tool_calls":[{"id":"call_1","type":"function","function":{"arguments":"{}"}}]}}]}"#;
        let turn = parse_response_with_tools(body, &cfg()).unwrap();
        assert_eq!(turn.text, "hello");
        assert!(turn.tool_calls.is_empty());
    }

    #[test]
    fn a_reply_whose_only_tool_call_is_dropped_and_has_no_text_is_still_an_error() {
        // Downstream of the same drop: with nothing else in the reply, losing
        // the one malformed call leaves genuinely nothing to act on, and that
        // has to surface as the same protocol error an entirely empty reply
        // would — not as an empty, silently-accepted turn.
        let body = r#"{"choices":[{"message":{"content":"","tool_calls":
                       [{"id":"call_1","type":"function","function":{"arguments":"{}"}}]}}]}"#;
        assert!(matches!(
            parse_response_with_tools(body, &cfg()),
            Err(AgentError::Protocol { .. })
        ));
    }

    #[test]
    fn an_absent_arguments_field_defaults_to_an_empty_object() {
        let body = r#"{"choices":[{"message":{"content":"","tool_calls":
                       [{"id":"call_1","type":"function","function":{"name":"t"}}]}}]}"#;
        let turn = parse_response_with_tools(body, &cfg()).unwrap();
        assert_eq!(turn.tool_calls[0].arguments, "{}");
    }

    // -----------------------------------------------------------------
    // Tool calling: streaming delta assembly (ToolCallAssembler)
    // -----------------------------------------------------------------

    #[test]
    fn a_tool_call_sent_whole_in_one_delta_assembles_correctly() {
        let mut asm = ToolCallAssembler::default();
        asm.absorb(&json!({
            "tool_calls": [{
                "index": 0,
                "id": "call_1",
                "type": "function",
                "function": { "name": "fs__read_file", "arguments": r#"{"path":"/tmp/x"}"# },
            }],
        }));
        let calls = asm.finish();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "fs__read_file");
        assert_eq!(calls[0].arguments, r#"{"path":"/tmp/x"}"#);
    }

    #[test]
    fn arguments_split_across_many_deltas_are_concatenated_in_order() {
        // The realistic shape: OpenAI sends `id`/`name` once, on the first
        // delta for an index, then streams `arguments` a few characters at a
        // time across many more deltas with no `id`/`name` at all.
        let mut asm = ToolCallAssembler::default();
        asm.absorb(&json!({ "tool_calls": [{
            "index": 0, "id": "call_1", "type": "function",
            "function": { "name": "fs__read_file", "arguments": "" },
        }] }));
        for piece in ["{\"path\":", "\"/tmp/", "x\"}"] {
            asm.absorb(&json!({ "tool_calls": [{
                "index": 0, "function": { "arguments": piece },
            }] }));
        }
        let calls = asm.finish();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments, r#"{"path":"/tmp/x"}"#);
    }

    #[test]
    fn two_parallel_calls_interleaved_by_index_do_not_cross_contaminate() {
        let mut asm = ToolCallAssembler::default();
        // Arrives out of order and interleaved on purpose — nothing in the
        // format promises indices arrive contiguously or grouped.
        asm.absorb(&json!({ "tool_calls": [
            { "index": 1, "id": "call_b", "type": "function", "function": { "name": "b", "arguments": "" } },
            { "index": 0, "id": "call_a", "type": "function", "function": { "name": "a", "arguments": "" } },
        ] }));
        asm.absorb(&json!({ "tool_calls": [{ "index": 0, "function": { "arguments": "1" } }] }));
        asm.absorb(&json!({ "tool_calls": [{ "index": 1, "function": { "arguments": "2" } }] }));

        let calls = asm.finish();
        assert_eq!(calls.len(), 2);
        // `finish` yields index order, so call "a" (index 0) comes first.
        assert_eq!(calls[0].id, "call_a");
        assert_eq!(calls[0].arguments, "1");
        assert_eq!(calls[1].id, "call_b");
        assert_eq!(calls[1].arguments, "2");
    }

    #[test]
    fn a_delta_with_no_tool_calls_is_a_harmless_no_op() {
        let mut asm = ToolCallAssembler::default();
        asm.absorb(&json!({ "content": "hello" }));
        assert!(asm.finish().is_empty());
    }

    #[test]
    fn a_call_that_never_receives_a_name_is_dropped() {
        let mut asm = ToolCallAssembler::default();
        asm.absorb(&json!({ "tool_calls": [{ "index": 0, "id": "call_1", "type": "function", "function": { "arguments": "{}" } }] }));
        assert!(asm.finish().is_empty());
    }
}
