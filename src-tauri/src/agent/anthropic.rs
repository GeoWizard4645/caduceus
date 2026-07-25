//! The Claude Messages API backend, including the computer-use agent loop.
//!
//! # Keeping up with the API
//!
//! Every version-sensitive string here is a **configuration field**, not a
//! constant baked into the binary:
//!
//! | what                   | config field                | shipped default          |
//! |------------------------|-----------------------------|--------------------------|
//! | beta header            | `anthropicBetaHeader`       | `computer-use-2025-11-24`|
//! | computer tool `type`   | `computerToolVersion`       | `computer_20251124`      |
//! | model                  | `model`                     | `claude-opus-5`          |
//!
//! When Anthropic ships a newer tool version, a user edits three text boxes in
//! Settings rather than waiting for an Orbit release. The defaults come from
//! <https://platform.claude.com/docs/en/agents-and-tools/tool-use/computer-use-tool>,
//! read while this backend was written.
//!
//! # The loop
//!
//! ```text
//!   task ──▶ screenshot ──▶ Claude ──▶ tool_use? ──no──▶ done
//!                ▲                        │
//!                │                       yes
//!                │                        ▼
//!                └────── screenshot ◀── execute action (enigo)
//! ```
//!
//! Bounded by `max_steps`, interruptible via the cancel token, and gated on
//! explicit user approval before the very first action.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::backend::{AgentBackend, AgentLoopContext};
use super::computer::{ComputerAction, Screenshot};
use super::http;
use super::types::{
    AgentError, AgentOutcome, AgentResponse, AgentResult, AgentStep, Message, Role, StopReason,
    Usage,
};
use crate::settings::BackendConfig;
use crate::settings::secrets;

pub struct AnthropicBackend;

const PROVIDER: &str = "Anthropic";

/// The Messages API version. Distinct from the *beta* header and far more
/// stable; still overridable through `extraHeaders` if that ever changes.
const API_VERSION: &str = "2023-06-01";

/// How many screenshots stay in the transcript.
///
/// Every step adds a full-resolution screenshot, and a 25-step session would
/// otherwise resend ~25 images on the final turn — slow and expensive for no
/// benefit, since the model reasons about the *current* screen. Older images are
/// replaced by a short placeholder; the accompanying text is kept.
const MAX_RETAINED_SCREENSHOTS: usize = 3;

/// Guidance prepended to every computer-use session.
const COMPUTER_USE_SYSTEM_PROMPT: &str = "\
You are driving a real desktop belonging to the person who asked. Work carefully.

* Take a screenshot before assuming what is on screen, and after any action whose \
result you need to verify.
* Prefer keyboard shortcuts over hunting for small targets.
* Do not open, read, or send anything the task did not ask for. Never enter \
passwords, payment details, or other credentials \u{2014} stop and say so instead.
* If the task is ambiguous or you cannot make progress, stop and explain what you \
need rather than guessing.
* When the task is complete, reply with a short plain-text summary and no further \
tool calls.";

#[async_trait]
impl AgentBackend for AnthropicBackend {
    fn id(&self) -> &str {
        "anthropic"
    }

    fn display_name(&self) -> &str {
        "Claude"
    }

    fn supports_computer_use(&self) -> bool {
        true
    }

    async fn chat(&self, messages: Vec<Message>, config: &BackendConfig) -> AgentResult<AgentResponse> {
        let api_key = require_api_key(config)?;

        // The Messages API takes the system prompt out-of-band, and only
        // user/assistant turns in `messages`.
        let mut system = config.system_prompt.trim().to_string();
        let mut wire = Vec::new();
        for m in &messages {
            match m.role {
                Role::System => {
                    if !system.is_empty() {
                        system.push_str("\n\n");
                    }
                    system.push_str(&m.content);
                }
                Role::User => wire.push(json!({"role": "user", "content": m.content})),
                Role::Assistant => wire.push(json!({"role": "assistant", "content": m.content})),
            }
        }
        if wire.is_empty() {
            return Err(AgentError::Other("nothing to send".into()));
        }

        let mut payload = json!({
            "model": config.model,
            "max_tokens": config.max_tokens,
            "messages": wire,
        });
        if !system.is_empty() {
            payload["system"] = json!(system);
        }
        if let Some(t) = config.temperature {
            payload["temperature"] = json!(t);
        }

        let body = send(config, &api_key, &payload, false).await?;
        let json: Value = parse_json(&body)?;

        Ok(AgentResponse {
            text: collect_text(&json),
            model: json
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or(&config.model)
                .to_string(),
            usage: read_usage(&json),
        })
    }

    async fn run_agent_loop(
        &self,
        task: &str,
        config: &BackendConfig,
        ctx: AgentLoopContext,
    ) -> AgentResult<AgentOutcome> {
        run_loop(task, config, ctx).await
    }

    async fn test_connection(&self, config: &BackendConfig) -> AgentResult<String> {
        let response = self
            .chat(vec![Message::user("Reply with exactly: ok")], config)
            .await?;
        Ok(format!(
            "Connected to Claude ({}).{}",
            response.model,
            if config.supports_computer_use {
                format!(
                    " Computer use is enabled with tool {} and beta header {}.",
                    config.computer_tool_version, config.anthropic_beta_header
                )
            } else {
                String::new()
            }
        ))
    }
}

// ---------------------------------------------------------------------------
// The agent loop
// ---------------------------------------------------------------------------

async fn run_loop(
    task: &str,
    config: &BackendConfig,
    ctx: AgentLoopContext,
) -> AgentResult<AgentOutcome> {
    let api_key = require_api_key(config)?;
    let emit = &ctx.on_step;

    emit(AgentStep::Started {
        session_id: ctx.session_id.clone(),
        task: task.to_string(),
        backend: PROVIDER.into(),
        model: config.model.clone(),
    });

    // The screen as it looked at the start. Re-captured after each action.
    let computer = ctx.computer.clone();
    let mut shot = tokio::task::spawn_blocking(move || computer.capture())
        .await
        .map_err(|e| AgentError::Other(format!("capture task failed: {e}")))??;

    emit(AgentStep::Screenshot {
        image: data_url(&shot.png_base64),
        width: shot.model_width,
        height: shot.model_height,
    });

    let mut messages: Vec<Value> = vec![json!({
        "role": "user",
        "content": [
            {"type": "text", "text": format!("Task: {task}")},
            image_block(&shot.png_base64),
        ]
    })];

    let mut steps: u32 = 0;
    let mut action_index: u32 = 0;
    let mut approved = matches!(ctx.approval, super::backend::ApprovalGate::AutoApprove);
    let mut final_message = String::new();
    let mut total_usage = Usage::default();

    loop {
        if ctx.cancel.is_cancelled() {
            return Ok(finish(&ctx, steps, final_message, StopReason::UserStopped, total_usage));
        }
        if steps >= ctx.max_steps {
            emit(AgentStep::Error {
                message: format!(
                    "Stopped after {} steps (the limit set in Settings \u{2192} Agent Backends).",
                    ctx.max_steps
                ),
            });
            return Ok(finish(&ctx, steps, final_message, StopReason::MaxSteps, total_usage));
        }
        steps += 1;

        trim_old_screenshots(&mut messages);

        let payload = json!({
            "model": config.model,
            "max_tokens": config.max_tokens,
            "system": system_prompt(config),
            "tools": [computer_tool(config, &shot)],
            "messages": messages,
        });

        let body = send(config, &api_key, &payload, true).await?;
        let response: Value = parse_json(&body)?;
        accumulate_usage(&mut total_usage, &response);

        let text = collect_text(&response);
        if !text.trim().is_empty() {
            emit(AgentStep::Thinking { text: text.clone() });
            final_message = text;
        }

        let content = response
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let tool_uses: Vec<&Value> = content
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
            .collect();

        // No tools requested: Claude considers the task finished.
        if tool_uses.is_empty() {
            return Ok(finish(&ctx, steps, final_message, StopReason::Completed, total_usage));
        }

        // Nothing has touched the machine yet — this is the gate.
        if !approved {
            let summary = tool_uses
                .first()
                .and_then(|t| t.get("input"))
                .and_then(|i| serde_json::from_value::<ComputerAction>(i.clone()).ok())
                .map(|a| a.describe())
                .unwrap_or_else(|| "control your mouse and keyboard".into());

            emit(AgentStep::AwaitingApproval {
                session_id: ctx.session_id.clone(),
                summary: summary.clone(),
            });

            if !ctx.approval.request(&ctx.session_id, &summary).await {
                return Ok(finish(&ctx, steps, final_message, StopReason::Declined, total_usage));
            }
            approved = true;
        }

        messages.push(json!({"role": "assistant", "content": content}));

        let mut results = Vec::with_capacity(tool_uses.len());
        for tool_use in tool_uses {
            if ctx.cancel.is_cancelled() {
                return Ok(finish(&ctx, steps, final_message, StopReason::UserStopped, total_usage));
            }

            let tool_id = tool_use
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let input = tool_use.get("input").cloned().unwrap_or(Value::Null);

            action_index += 1;
            let parsed: Result<ComputerAction, _> = serde_json::from_value(input.clone());

            let action = match parsed {
                Ok(a) => a,
                Err(e) => {
                    // Report the failure back to the model instead of aborting:
                    // it can usually correct itself on the next turn.
                    let detail = format!("Could not run that action: {e}");
                    emit(AgentStep::ActionResult {
                        index: action_index,
                        ok: false,
                        detail: detail.clone(),
                    });
                    results.push(error_result(&tool_id, &detail));
                    continue;
                }
            };

            emit(AgentStep::Action {
                index: action_index,
                summary: action.describe(),
                raw: input.clone(),
            });

            let mutating = action.is_mutating();
            let computer = ctx.computer.clone();
            let snapshot = shot.clone();
            let outcome = tokio::task::spawn_blocking(move || computer.execute(action, &snapshot))
                .await
                .map_err(|e| AgentError::Other(format!("action task failed: {e}")))?;

            match outcome {
                Ok(result) => {
                    emit(AgentStep::ActionResult {
                        index: action_index,
                        ok: true,
                        detail: result.text.clone().unwrap_or_else(|| "Done".into()),
                    });

                    if mutating {
                        tokio::time::sleep(ctx.settle).await;
                    }

                    // Every action reports back with a fresh screenshot: it is
                    // how the model verifies that what it intended actually
                    // happened.
                    let computer = ctx.computer.clone();
                    let refreshed = tokio::task::spawn_blocking(move || computer.capture())
                        .await
                        .map_err(|e| AgentError::Other(format!("capture task failed: {e}")))?;

                    let image = match refreshed {
                        Ok(fresh) => {
                            shot = fresh;
                            emit(AgentStep::Screenshot {
                                image: data_url(&shot.png_base64),
                                width: shot.model_width,
                                height: shot.model_height,
                            });
                            shot.png_base64.clone()
                        }
                        // A zoom/screenshot action already produced an image;
                        // fall back to it if re-capture failed.
                        Err(e) => {
                            log::warn!("re-capture failed: {e}");
                            result.image_base64.clone().unwrap_or_default()
                        }
                    };

                    let mut blocks = Vec::new();
                    if let Some(text) = &result.text {
                        blocks.push(json!({"type": "text", "text": text}));
                    }
                    // The zoom action's own crop is more useful than the
                    // full-screen re-capture.
                    let payload_image = result
                        .image_base64
                        .filter(|_| matches!(input.get("action").and_then(Value::as_str), Some("zoom")))
                        .unwrap_or(image);
                    if !payload_image.is_empty() {
                        blocks.push(image_block(&payload_image));
                    }
                    if blocks.is_empty() {
                        blocks.push(json!({"type": "text", "text": "Done."}));
                    }

                    results.push(json!({
                        "type": "tool_result",
                        "tool_use_id": tool_id,
                        "content": blocks,
                    }));
                }
                Err(e) => {
                    let detail = e.to_string();
                    emit(AgentStep::ActionResult {
                        index: action_index,
                        ok: false,
                        detail: detail.clone(),
                    });
                    // A permissions failure will fail identically every time;
                    // ending the session beats burning 25 steps on it.
                    if matches!(e, super::computer::ComputerError::Input(_)) {
                        emit(AgentStep::Error {
                            message: detail.clone(),
                        });
                        return Ok(finish(&ctx, steps, detail, StopReason::Error, total_usage));
                    }
                    results.push(error_result(&tool_id, &detail));
                }
            }
        }

        messages.push(json!({"role": "user", "content": results}));
    }
}

fn finish(
    ctx: &AgentLoopContext,
    steps: u32,
    final_message: String,
    stop_reason: StopReason,
    usage: Usage,
) -> AgentOutcome {
    let outcome = AgentOutcome {
        session_id: ctx.session_id.clone(),
        completed: stop_reason == StopReason::Completed,
        steps,
        final_message,
        stop_reason,
        usage: Some(usage),
    };
    (ctx.on_step)(AgentStep::Finished {
        outcome: outcome.clone(),
    });
    outcome
}

fn error_result(tool_id: &str, detail: &str) -> Value {
    json!({
        "type": "tool_result",
        "tool_use_id": tool_id,
        "is_error": true,
        "content": [{"type": "text", "text": detail}],
    })
}

// ---------------------------------------------------------------------------
// Request construction
// ---------------------------------------------------------------------------

fn system_prompt(config: &BackendConfig) -> String {
    let extra = config.system_prompt.trim();
    if extra.is_empty() {
        COMPUTER_USE_SYSTEM_PROMPT.to_string()
    } else {
        format!("{COMPUTER_USE_SYSTEM_PROMPT}\n\n{extra}")
    }
}

/// Build the `computer` tool definition.
///
/// `display_width_px` / `display_height_px` must match the screenshots we
/// actually send, not the physical display, or every coordinate the model
/// returns will be scaled wrong.
fn computer_tool(config: &BackendConfig, shot: &Screenshot) -> Value {
    let mut tool = json!({
        "type": config.computer_tool_version,
        "name": "computer",
        "display_width_px": shot.model_width,
        "display_height_px": shot.model_height,
    });
    // `enable_zoom` is only understood by computer_20251124 and later; sending
    // it to an older tool version is rejected outright.
    if config.enable_zoom && config.computer_tool_version.as_str() >= "computer_20251124" {
        tool["enable_zoom"] = json!(true);
    }
    tool
}

fn image_block(base64_png: &str) -> Value {
    json!({
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": "image/png",
            "data": base64_png,
        }
    })
}

fn data_url(base64_png: &str) -> String {
    format!("data:image/png;base64,{base64_png}")
}

/// Replace all but the newest screenshots with a placeholder.
///
/// Operates on `tool_result` blocks in place, so the conversation structure
/// (and every `tool_use_id` pairing) is preserved — dropping whole messages
/// would make the transcript invalid.
fn trim_old_screenshots(messages: &mut [Value]) {
    let mut image_positions: Vec<(usize, usize, usize)> = Vec::new();

    for (mi, message) in messages.iter().enumerate() {
        let Some(blocks) = message.get("content").and_then(Value::as_array) else {
            continue;
        };
        for (bi, block) in blocks.iter().enumerate() {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let Some(inner) = block.get("content").and_then(Value::as_array) else {
                continue;
            };
            for (ii, part) in inner.iter().enumerate() {
                if part.get("type").and_then(Value::as_str) == Some("image") {
                    image_positions.push((mi, bi, ii));
                }
            }
        }
    }

    if image_positions.len() <= MAX_RETAINED_SCREENSHOTS {
        return;
    }

    let drop_count = image_positions.len() - MAX_RETAINED_SCREENSHOTS;
    for &(mi, bi, ii) in &image_positions[..drop_count] {
        if let Some(part) = messages
            .get_mut(mi)
            .and_then(|m| m.get_mut("content"))
            .and_then(|c| c.get_mut(bi))
            .and_then(|b| b.get_mut("content"))
            .and_then(|c| c.get_mut(ii))
        {
            *part = json!({
                "type": "text",
                "text": "[earlier screenshot omitted to save context \u{2014} take a new one if you need to see the screen]"
            });
        }
    }
}

async fn send(
    config: &BackendConfig,
    api_key: &str,
    payload: &Value,
    computer_use: bool,
) -> AgentResult<String> {
    let base = if config.base_url.trim().is_empty() {
        crate::settings::DEFAULT_ANTHROPIC_BASE_URL
    } else {
        config.base_url.trim_end_matches('/')
    };
    let url = format!("{base}/v1/messages");

    let client = http::client(config.timeout_secs)?;
    let mut req = client
        .post(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", API_VERSION)
        .json(payload);

    if computer_use && !config.anthropic_beta_header.trim().is_empty() {
        req = req.header("anthropic-beta", config.anthropic_beta_header.trim());
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
    if status.is_success() {
        Ok(body)
    } else {
        Err(AgentError::Api {
            provider: PROVIDER.into(),
            status: status.as_u16(),
            body: http::extract_error_message(&body),
        })
    }
}

fn require_api_key(config: &BackendConfig) -> AgentResult<String> {
    secrets::get_backend_api_key_opt(&config.id).ok_or_else(|| {
        AgentError::NotConfigured(
            "No Anthropic API key is stored. Add one in Settings \u{2192} Agent Backends \
             (it goes into your OS keychain, never into a config file)."
                .into(),
        )
    })
}

fn parse_json(body: &str) -> AgentResult<Value> {
    serde_json::from_str(body).map_err(|e| AgentError::Protocol {
        provider: PROVIDER.into(),
        detail: format!("response was not JSON: {e}"),
    })
}

fn collect_text(response: &Value) -> String {
    response
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn read_usage(response: &Value) -> Option<Usage> {
    let u = response.get("usage")?;
    Some(Usage {
        input_tokens: u.get("input_tokens").and_then(Value::as_u64).map(|v| v as u32),
        output_tokens: u.get("output_tokens").and_then(Value::as_u64).map(|v| v as u32),
    })
}

fn accumulate_usage(total: &mut Usage, response: &Value) {
    if let Some(u) = read_usage(response) {
        total.input_tokens = Some(total.input_tokens.unwrap_or(0) + u.input_tokens.unwrap_or(0));
        total.output_tokens = Some(total.output_tokens.unwrap_or(0) + u.output_tokens.unwrap_or(0));
    }
}

/// Models Orbit suggests in the Settings picker. Free-text entry always wins —
/// this list is a convenience, never a restriction, so a model released after
/// this build is still usable.
pub const SUGGESTED_MODELS: &[&str] = &[
    "claude-opus-5",
    "claude-sonnet-5",
    "claude-opus-4-8",
    "claude-haiku-4-5",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(w: u32, h: u32) -> Screenshot {
        Screenshot {
            png_base64: "AAAA".into(),
            model_width: w,
            model_height: h,
            input_width: w * 2,
            input_height: h * 2,
            origin_x: 0,
            origin_y: 0,
        }
    }

    #[test]
    fn tool_dimensions_match_the_screenshot_we_send() {
        let cfg = BackendConfig::default();
        let tool = computer_tool(&cfg, &shot(1280, 800));
        assert_eq!(tool["display_width_px"], 1280);
        assert_eq!(tool["display_height_px"], 800);
        assert_eq!(tool["name"], "computer");
    }

    #[test]
    fn zoom_is_only_offered_to_tool_versions_that_support_it() {
        let mut cfg = BackendConfig::default();
        cfg.enable_zoom = true;

        cfg.computer_tool_version = "computer_20251124".into();
        assert_eq!(computer_tool(&cfg, &shot(100, 100))["enable_zoom"], true);

        cfg.computer_tool_version = "computer_20250124".into();
        assert!(computer_tool(&cfg, &shot(100, 100)).get("enable_zoom").is_none());

        cfg.computer_tool_version = "computer_20251124".into();
        cfg.enable_zoom = false;
        assert!(computer_tool(&cfg, &shot(100, 100)).get("enable_zoom").is_none());
    }

    #[test]
    fn tool_version_comes_from_config_not_a_constant() {
        let mut cfg = BackendConfig::default();
        cfg.computer_tool_version = "computer_29990101".into();
        assert_eq!(computer_tool(&cfg, &shot(10, 10))["type"], "computer_29990101");
    }

    #[test]
    fn collects_text_across_blocks_and_ignores_tool_use() {
        let r = json!({"content": [
            {"type": "text", "text": "first"},
            {"type": "tool_use", "id": "t1", "name": "computer", "input": {}},
            {"type": "text", "text": "second"}
        ]});
        assert_eq!(collect_text(&r), "first\nsecond");
    }

    #[test]
    fn trimming_keeps_only_the_newest_screenshots() {
        let mut messages: Vec<Value> = (0..6)
            .map(|i| {
                json!({"role": "user", "content": [{
                    "type": "tool_result",
                    "tool_use_id": format!("t{i}"),
                    "content": [image_block("PAYLOAD")]
                }]})
            })
            .collect();

        trim_old_screenshots(&mut messages);

        let remaining = messages
            .iter()
            .filter(|m| serde_json::to_string(m).unwrap().contains("PAYLOAD"))
            .count();
        assert_eq!(remaining, MAX_RETAINED_SCREENSHOTS);

        // The newest ones are the ones kept.
        assert!(serde_json::to_string(&messages[5]).unwrap().contains("PAYLOAD"));
        assert!(!serde_json::to_string(&messages[0]).unwrap().contains("PAYLOAD"));

        // Structure survives: every tool_result still has its id.
        for (i, m) in messages.iter().enumerate() {
            assert_eq!(m["content"][0]["tool_use_id"], format!("t{i}"));
            assert_eq!(m["content"][0]["type"], "tool_result");
        }
    }

    #[test]
    fn trimming_is_a_no_op_below_the_threshold() {
        let mut messages = vec![json!({"role": "user", "content": [{
            "type": "tool_result", "tool_use_id": "t0", "content": [image_block("KEEP")]
        }]})];
        trim_old_screenshots(&mut messages);
        assert!(serde_json::to_string(&messages[0]).unwrap().contains("KEEP"));
    }

    #[test]
    fn usage_accumulates_across_turns() {
        let mut total = Usage::default();
        accumulate_usage(&mut total, &json!({"usage": {"input_tokens": 10, "output_tokens": 3}}));
        accumulate_usage(&mut total, &json!({"usage": {"input_tokens": 5, "output_tokens": 2}}));
        assert_eq!(total.input_tokens, Some(15));
        assert_eq!(total.output_tokens, Some(5));
    }

    #[test]
    fn system_prompt_appends_user_instructions() {
        let mut cfg = BackendConfig::default();
        cfg.system_prompt = "Always use Safari.".into();
        let p = system_prompt(&cfg);
        assert!(p.starts_with("You are driving a real desktop"));
        assert!(p.ends_with("Always use Safari."));
    }
}
