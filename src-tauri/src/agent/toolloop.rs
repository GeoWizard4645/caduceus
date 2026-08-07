//! The MCP tool-calling loop.
//!
//! Caduceus has two things that, until now, never met: [`crate::mcp`], a full
//! MCP client that spawns local tool servers and aggregates their tools, and
//! [`super::openai`], a backend that can now advertise tools and parse a
//! model's request to call one (see [`openai::ToolSpec`], `openai::ChatTurn`).
//! This module is the loop that connects them: it sends a conversation with
//! the MCP registry's tools attached, and whenever the model asks for one,
//! executes it through [`crate::mcp::mcp_call_tool`], feeds the result back,
//! and asks again — until the model stops asking, the iteration cap is hit,
//! or the user stops it.
//!
//! ```text
//!  run_tool_loop
//!  ──────────────
//!   messages + tools ──▶ openai::stream_chat_with_tools ──▶ ChatTurn
//!                                                               │
//!                     no tool_calls ◀────────────────────────┤── tool_calls
//!                          │                                    │
//!                    StopReason::Completed          mcp::mcp_call_tool (each)
//!                                                               │
//!                                                Message::tool_result, loop again
//! ```
//!
//! # Why this is not on `AgentBackend`
//!
//! [`super::AgentBackend::run_agent_loop`] is specifically the *computer-use*
//! loop — one per backend, driving the screen. Tool calling is orthogonal to
//! that and, today, specific to the OpenAI dialect (Hermes brings its own
//! tools and has no wire format this module could drive), so this is a plain
//! function that takes a config rather than a trait method every backend
//! would have to either implement or inherit a no-op for.
//!
//! # How to invoke it
//!
//! [`run_tool_loop`] takes exactly what [`super::start_session`] already
//! knows how to build for a computer-use session — an [`AgentLoopContext`]
//! (session id, step sink, cancel token, approval gate) — plus a
//! [`BackendConfig`] and the conversation so far. A caller that wants to
//! expose this over IPC the way `agent_start_session` exposes computer use
//! would register a session with [`super::AgentRuntime`], build the context
//! the same way `start_session` does, and `tauri::async_runtime::spawn` this
//! function; every [`AgentStep`] it emits already goes out on
//! [`super::AGENT_STEP_EVENT`], so the existing `AgentPanel.tsx` step feed
//! needs no changes to display it.
//!
//! # Safety
//!
//! Nothing calls an MCP tool until the user has approved the session's first
//! one — [`AgentLoopContext::approval`], the identical one-time gate a
//! computer-use session asks through, backed by the identical
//! `confirm_before_first_action` setting and `AgentStep::AwaitingApproval` /
//! `AgentRuntime` oneshot machinery once a caller wires the setting into the
//! `ApprovalGate` it builds, exactly as `start_session` does today. A tool
//! that fails — an unresolvable name, arguments the model got wrong, or the
//! tool's own reported error — is *never* a reason to end the session: its
//! message becomes the content of the [`Message::tool_result`] sent back, so
//! the model sees what went wrong and can try again. Only a failure talking
//! to the backend itself (the request that would have carried the next tool
//! call) aborts the loop, since there is no response in that case to hand
//! back to the model at all.

use std::collections::HashMap;

use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Runtime};

use super::backend::{AgentLoopContext, StepSink};
use super::openai::{self, ToolSpec};
use super::types::{AgentOutcome, AgentResult, AgentStep, Message, StopReason, ToolCall, Usage};
use crate::computeruse;
use crate::mcp::{self, McpTool};
use crate::settings::BackendConfig;

/// Hard ceiling on how many round trips to the model the loop will make
/// before giving up and reporting [`StopReason::MaxSteps`] — a model stuck
/// calling tools in a cycle is a bug in the model or the tool, not something
/// Caduceus should let run (and bill) forever.
pub const MAX_ITERATIONS: u32 = 25;

/// Run the loop to completion: send `messages` with every tool the MCP
/// registry currently offers, execute whatever the model asks for, and keep
/// going until it stops asking, [`MAX_ITERATIONS`] is hit, or `ctx.cancel`
/// fires. See the module doc for the shape of the loop and how a caller
/// assembles `ctx`.
pub async fn run_tool_loop<R: Runtime>(
    app: &AppHandle<R>,
    config: &BackendConfig,
    mut messages: Vec<Message>,
    ctx: AgentLoopContext,
) -> AgentResult<AgentOutcome> {
    // Fail before announcing a session that was never going to work, exactly
    // like `HermesBackend::run_agent_loop`'s `require_hermes()?` runs before
    // its own `Started` — see `openai::validate_config`'s doc for why this is
    // `pub(crate)` rather than private.
    openai::validate_config(config)?;

    // See `inject_memory_context`'s doc: MEMORY.md/USER.md are read fresh for
    // every session and spliced in as a leading system message, so this must
    // run before the conversation's first request goes out.
    inject_memory_context(app, &mut messages);

    let emit = &ctx.on_step;
    emit(AgentStep::Started {
        session_id: ctx.session_id.clone(),
        task: super::latest_user_content(&messages).to_string(),
        backend: config.display_name.clone(),
        model: config.model.clone(),
    });

    // Snapshotted once, at the start of the session, not re-fetched every
    // iteration: a server connecting or disconnecting mid-loop should not
    // change what the model believes it can call between one of its own
    // turns and the next. `mcp_list_tools` is an in-memory read (no process
    // spawned), so there is no performance reason to refetch either.
    let tools = mcp::mcp_list_tools(app.clone()).await.unwrap_or_else(|e| {
        log::warn!("could not list MCP tools; continuing with none offered: {e}");
        Vec::new()
    });
    let (tool_specs, tools_by_wire_name) = build_tool_table(&tools);

    let mut usage_total = UsageAccumulator::default();
    let mut first_action_gated = false;
    let mut action_index: u32 = 0;
    let mut steps: u32 = 0;

    loop {
        if ctx.cancel.is_cancelled() {
            return Ok(finish(
                &ctx,
                steps,
                String::new(),
                StopReason::UserStopped,
                usage_total.into_usage(),
            ));
        }
        if steps >= MAX_ITERATIONS {
            emit(AgentStep::Error {
                message: format!(
                    "Stopped after {MAX_ITERATIONS} rounds of tool calls without a final answer."
                ),
            });
            return Ok(finish(
                &ctx,
                steps,
                String::new(),
                StopReason::MaxSteps,
                usage_total.into_usage(),
            ));
        }

        // A transport/API failure has no response to hand back to the model,
        // unlike a failing tool call — so, per the module doc, this is the one
        // failure mode that propagates rather than becoming a step and
        // continuing. The eventual caller mirrors `start_session`'s handling
        // of a top-level `Err` from `run_agent_loop`: emit `Error` then
        // `Finished` itself.
        let turn = openai::stream_chat_with_tools(&messages, config, &tool_specs, |_| {}).await?;
        steps += 1;
        usage_total.add(turn.usage.as_ref());
        // `AgentOutcome` has nowhere to carry a per-turn model name (unlike
        // `AgentResponse`, which callers outside an agent loop read `.model`
        // from directly), but a server that silently resolves an alias — e.g.
        // Ollama answering a request for "llama3.2" as "llama3.2:latest" — is
        // still worth a trace for anyone debugging which model actually ran.
        if !turn.model.is_empty() && turn.model != config.model {
            log::debug!(
                "tool loop: backend answered as \"{}\" (requested \"{}\")",
                turn.model,
                config.model
            );
        }

        if turn.tool_calls.is_empty() {
            if !turn.text.trim().is_empty() {
                emit(AgentStep::Thinking { text: turn.text.clone() });
            }
            // Fire-and-forget: see `memory::nudge`'s module doc. This only
            // decides whether to spawn a detached background task and
            // returns immediately either way — it never delays this return.
            let mut reviewed = messages.clone();
            if !turn.text.trim().is_empty() {
                reviewed.push(Message::assistant(turn.text.clone()));
            }
            crate::memory::nudge::maybe_spawn_review(app, config, &reviewed);
            return Ok(finish(
                &ctx,
                steps,
                turn.text,
                StopReason::Completed,
                usage_total.into_usage(),
            ));
        }

        // Some providers attach prose alongside a tool request ("Let me check
        // that for you…"); show it exactly like a final answer's text would
        // be, since to the person watching the feed it is the same kind of
        // thing — the model talking — regardless of what follows it.
        if !turn.text.trim().is_empty() {
            emit(AgentStep::Thinking { text: turn.text.clone() });
        }

        // The API requires its own tool_calls echoed back on an assistant
        // turn before it will accept the tool_result turns that answer them.
        messages.push(Message::assistant_tool_calls(turn.text.clone(), turn.tool_calls.clone()));

        for call in &turn.tool_calls {
            if ctx.cancel.is_cancelled() {
                return Ok(finish(
                    &ctx,
                    steps,
                    String::new(),
                    StopReason::UserStopped,
                    usage_total.into_usage(),
                ));
            }

            let real_id = tools_by_wire_name.get(call.name.as_str()).cloned();
            let summary = format!("call the \"{}\" tool", real_id.as_deref().unwrap_or(&call.name));

            // The whole session is gated once, on its very first tool call —
            // not once per call — the same "ask before the first thing
            // touches anything, then proceed" contract a computer-use session
            // already uses. See `ApprovalGate::Ask`'s docs.
            if !first_action_gated {
                first_action_gated = true;
                emit(AgentStep::AwaitingApproval {
                    session_id: ctx.session_id.clone(),
                    summary: summary.clone(),
                });
                if !ctx.approval.request(&ctx.session_id, &summary).await {
                    return Ok(finish(
                        &ctx,
                        steps,
                        String::new(),
                        StopReason::Declined,
                        usage_total.into_usage(),
                    ));
                }
            }

            let result_text =
                call_one_tool(app, call, real_id.as_deref(), &summary, action_index, emit).await;
            action_index += 1;
            messages.push(Message::tool_result(call.id.clone(), result_text));
        }
    }
}

/// Splice `MEMORY.md`/`USER.md`'s current snapshot in as a leading system
/// message, so a tool-calling turn starts already knowing what the agent has
/// learned. See [`crate::memory::store::MemoryStore::snapshot_block`] for the
/// budget-bannered block this renders.
///
/// This lives here rather than on [`BackendConfig::system_prompt`] because
/// that field is a fixed per-backend setting resolved long before any
/// particular session's messages exist, while memory can change between one
/// session and the next and needs to be read fresh on every call. Inserting
/// into `messages` reaches the wire the same way: `openai::build_payload`
/// sends every `Role::System` message in `messages` as an additional
/// system-role turn, after `config.system_prompt` — see that function — so
/// this does not need `BackendConfig` itself to change per call.
///
/// A no-op when the memory feature is unavailable (`MemoryStore` was never
/// `app.manage()`-d — see `lib.rs::setup`'s "a feature, not a prerequisite"
/// handling for the same pattern on clipboard/chat) or both files are
/// currently empty.
fn inject_memory_context<R: Runtime>(app: &AppHandle<R>, messages: &mut Vec<Message>) {
    let Some(memory) = app.try_state::<crate::memory::MemoryStore>() else {
        return;
    };
    let mut blocks = Vec::new();
    if let Some(b) = memory.snapshot_block(crate::memory::Target::User) {
        blocks.push(b);
    }
    if let Some(b) = memory.snapshot_block(crate::memory::Target::Memory) {
        blocks.push(b);
    }
    if blocks.is_empty() {
        return;
    }
    messages.insert(0, Message::system(blocks.join("\n\n")));
}

/// Resolve, run, and report on one tool call, returning the text that goes
/// back to the model as this call's [`Message::tool_result`].
///
/// Never fails outright — an unresolvable name, arguments that do not parse,
/// or a tool that itself errors all become the *content* of the result
/// rather than aborting the loop. See the module doc.
async fn call_one_tool<R: Runtime>(
    app: &AppHandle<R>,
    call: &ToolCall,
    real_id: Option<&str>,
    summary: &str,
    index: u32,
    emit: &StepSink,
) -> String {
    let Some(real_id) = real_id else {
        // Most likely: the model is acting on a tool it saw earlier in a long
        // conversation whose server has since disconnected, or it simply
        // hallucinated a name. Either way there is nothing to run.
        let detail = format!(
            "\"{}\" is not a tool this session can see \u{2014} its server may have disconnected mid-run.",
            call.name
        );
        emit(AgentStep::Action {
            index,
            summary: summary.to_string(),
            raw: Value::Null,
        });
        emit(AgentStep::ActionResult {
            index,
            ok: false,
            detail: detail.clone(),
        });
        return detail;
    };

    let args = match parse_tool_arguments(&call.arguments) {
        Ok(v) => v,
        Err(detail) => {
            emit(AgentStep::Action {
                index,
                summary: summary.to_string(),
                raw: Value::Null,
            });
            emit(AgentStep::ActionResult {
                index,
                ok: false,
                detail: detail.clone(),
            });
            return detail;
        }
    };

    // The desktop-control guard, applied to the arguments that are about to go
    // on the wire rather than to the model's stated intent — a summary can say
    // one thing while the payload does another.
    //
    // # Why this sits below the approval gate rather than beside it
    //
    // The session's approval gate is asked once, before the first action. That
    // is the right shape for "do you trust this run at all", and the wrong
    // shape for "may it press this specific key combination", because a yes
    // given to open a document also covers logging out an hour later. This is
    // a ceiling on what any approval can authorise: a blocked action stays
    // blocked no matter what was approved, what an auto-approve setting says,
    // or what a tool result asked for.
    //
    // Refusals are returned as the tool's own result text rather than aborting
    // the loop, so the model reads why and can choose another route — the same
    // handling every other tool failure gets above.
    if let Some(bare) = computeruse::strip_namespace(real_id) {
        match computeruse::evaluate_action(bare, &args) {
            computeruse::GuardVerdict::Allow => {}
            computeruse::GuardVerdict::Blocked { reason } => {
                let detail = format!("Refused: {reason}");
                emit(AgentStep::Action {
                    index,
                    summary: summary.to_string(),
                    raw: args.clone(),
                });
                emit(AgentStep::ActionResult {
                    index,
                    ok: false,
                    detail: detail.clone(),
                });
                return detail;
            }
            // Fail closed. The verdict asks for a fresh, this-action-specific
            // "yes", and the session's approval channel is a single oneshot
            // that the one-time gate has already consumed — there is nowhere
            // to ask. Refusing is the safe end of that gap: what triggers this
            // verdict is a password prompt, a system security dialog or a
            // payment sheet, and an agent that clicks one of those because
            // per-action approval was unavailable is precisely the outcome the
            // guard exists to prevent. Wiring real per-action approval is a
            // follow-up; until then the honest answer is "ask the human to do
            // this part themselves", which the message says.
            computeruse::GuardVerdict::RequiresFreshApproval { reason } => {
                let detail = format!(
                    "Refused: {reason} This needs a person to do it directly \u{2014} \
                     Caduceus cannot approve it on your behalf."
                );
                emit(AgentStep::Action {
                    index,
                    summary: summary.to_string(),
                    raw: args.clone(),
                });
                emit(AgentStep::ActionResult {
                    index,
                    ok: false,
                    detail: detail.clone(),
                });
                return detail;
            }
        }
    }

    emit(AgentStep::Action {
        index,
        summary: summary.to_string(),
        raw: args.clone(),
    });

    // A built-in tool is dispatched here rather than over MCP. `real_id` for
    // one of these is the name it registered under, so a registry hit is what
    // tells the two apart — there is no separate namespace to check, and a
    // built-in cannot be shadowed by an MCP tool because `build_tool_table`
    // claims built-in names first.
    //
    // `native_tools::call` is synchronous (bounded filesystem work against a
    // small directory), so it goes through `spawn_blocking` rather than being
    // awaited inline, which would park a runtime worker on file I/O.
    if crate::native_tools::is_registered(real_id) {
        let name = real_id.to_string();
        let result = tauri::async_runtime::spawn_blocking(move || {
            crate::native_tools::call(&name, args)
        })
        .await;

        let (ok, detail) = match result {
            Ok(Ok(value)) => (true, native_result_text(&value)),
            Ok(Err(e)) => (false, e),
            // The handler panicked, or the blocking pool was torn down mid-run.
            // Neither is the model's fault and neither should end the session:
            // it reads this as a failed tool and can try something else.
            Err(e) => (false, format!("The tool did not finish: {e}")),
        };
        emit(AgentStep::ActionResult {
            index,
            ok,
            detail: detail.clone(),
        });
        return detail;
    }

    match mcp::mcp_call_tool(app.clone(), real_id.to_string(), Some(args)).await {
        Ok(outcome) => {
            let detail = if outcome.text.is_empty() {
                default_result_text(outcome.is_error)
            } else {
                outcome.text.clone()
            };
            emit(AgentStep::ActionResult {
                index,
                ok: !outcome.is_error,
                detail: detail.clone(),
            });
            detail
        }
        Err(e) => {
            // A protocol-level failure (disconnected server, malformed
            // response) rather than the tool's own `isError` — see
            // `mcp`'s module doc on why the two are kept distinct. Both end
            // up as text the model reads, but only this one is worth a
            // distinct prefix, since `outcome.is_error` above already reads
            // like a tool's own explanation without one.
            emit(AgentStep::ActionResult {
                index,
                ok: false,
                detail: e.clone(),
            });
            format!("Tool call failed: {e}")
        }
    }
}

/// Render a built-in tool's return value as the text the model reads back.
///
/// A JSON string is unwrapped rather than shown with its quotes, because most
/// of these handlers answer in prose (a skill's body, a memory write's
/// confirmation) and `"Saved."` reads like a bug next to every other tool
/// result on the transcript. Anything structured is passed through as JSON,
/// which is what a model expects from a tool that returns an object.
fn native_result_text(value: &Value) -> String {
    match value {
        Value::String(s) if !s.is_empty() => s.clone(),
        Value::Null => "(the tool completed with no output)".to_string(),
        other => other.to_string(),
    }
}

fn default_result_text(is_error: bool) -> String {
    if is_error {
        "The tool reported an error but gave no further detail.".to_string()
    } else {
        "(the tool completed with no output)".to_string()
    }
}

/// Parse a tool call's raw `arguments` string into the JSON object
/// `mcp_call_tool` expects.
///
/// Blank is treated as "no arguments" — some providers send an empty string
/// rather than `"{}"` for a tool that takes none. Anything else that fails to
/// parse is returned as `Err` whose text is meant to go straight back to the
/// model as the tool's result: a model can often correct its own malformed
/// JSON on the next turn if it is told what was wrong with it, which is more
/// useful than logging the failure and losing the call.
///
/// `pub(crate)` rather than private: `memory::nudge`'s background review
/// dispatches its own (whitelisted) tool calls the same way this loop does
/// and reuses this exact parser rather than a second copy of it.
pub(crate) fn parse_tool_arguments(raw: &str) -> Result<Value, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(trimmed)
        .map_err(|e| format!("Could not call the tool: the arguments were not valid JSON ({e})."))
}

fn finish(
    ctx: &AgentLoopContext,
    steps: u32,
    final_message: String,
    stop_reason: StopReason,
    usage: Option<Usage>,
) -> AgentOutcome {
    let outcome = AgentOutcome {
        session_id: ctx.session_id.clone(),
        completed: stop_reason == StopReason::Completed,
        steps,
        final_message,
        stop_reason,
        usage,
    };
    (ctx.on_step)(AgentStep::Finished {
        outcome: outcome.clone(),
    });
    outcome
}

// ---------------------------------------------------------------------------
// MCP schema → OpenAI schema
// ---------------------------------------------------------------------------

/// Sum of token usage across every request the loop makes, the same way a
/// multi-turn tool-calling exchange is still "one" turn from the user's point
/// of view and deserves one usage figure on [`AgentOutcome`], not one per
/// HTTP request made along the way.
#[derive(Default)]
struct UsageAccumulator {
    input: Option<u32>,
    output: Option<u32>,
}

impl UsageAccumulator {
    fn add(&mut self, usage: Option<&Usage>) {
        let Some(usage) = usage else { return };
        if let Some(v) = usage.input_tokens {
            self.input = Some(self.input.unwrap_or(0) + v);
        }
        if let Some(v) = usage.output_tokens {
            self.output = Some(self.output.unwrap_or(0) + v);
        }
    }

    /// `None` only when not one single request along the way reported usage
    /// — matching [`Usage`]'s own all-or-nothing-per-request shape rather
    /// than manufacturing a `Some(0)` no request actually claimed.
    fn into_usage(self) -> Option<Usage> {
        if self.input.is_none() && self.output.is_none() {
            None
        } else {
            Some(Usage {
                input_tokens: self.input,
                output_tokens: self.output,
            })
        }
    }
}

/// Build the OpenAI-facing tool list from the MCP registry's aggregated
/// tools, plus the wire-name \u{2192} real MCP id map the loop resolves a
/// model's tool call back through.
/// Build the combined tool list, from both transports.
///
/// # Why built-ins go first
///
/// The model is handed one flat list and does not know — or need to know —
/// that `skills_list` runs as a Rust function in this process while
/// `cua__click` is JSON-RPC to a spawned binary. Both reach it as the same
/// `ToolSpec`. What the ordering decides is who wins a name collision:
/// `unique_wire_name` suffixes the *later* claimant, so listing built-ins
/// first means a remote MCP server cannot take a built-in's name and quietly
/// push `memory` or `skill_view` to `memory-2`. A server that names a tool
/// `memory` gets the suffix instead, which is the right way round — the
/// built-in name is the one that appears in prompts and skill bodies.
fn build_tool_table(tools: &[McpTool]) -> (Vec<ToolSpec>, HashMap<String, String>) {
    let native = crate::native_tools::list();
    let total = native.len() + tools.len();
    let mut specs = Vec::with_capacity(total);
    let mut by_wire_name: HashMap<String, String> = HashMap::with_capacity(total);

    for tool in &native {
        // A built-in's registered name is already a literal chosen in this
        // binary rather than anything a remote server supplied, but it goes
        // through the same normalization so there is exactly one rule about
        // what a model can be shown.
        let name = unique_wire_name(&sanitize_tool_name(&tool.name), &by_wire_name);
        by_wire_name.insert(name.clone(), tool.name.clone());
        specs.push(ToolSpec {
            name,
            description: tool.description.clone(),
            parameters: tool.input_schema.clone(),
        });
    }

    for tool in tools {
        let name = unique_wire_name(&sanitize_tool_name(&tool.id), &by_wire_name);
        by_wire_name.insert(name.clone(), tool.id.clone());
        specs.push(ToolSpec {
            name,
            description: tool.description.clone(),
            parameters: tool.input_schema.clone(),
        });
    }

    (specs, by_wire_name)
}

/// OpenAI (and everything that copies its function-calling shape) requires
/// tool names to match `^[a-zA-Z0-9_-]{1,64}$`. MCP's own `{server}__{tool}`
/// ids are built from a name-validated server plus whatever the server
/// itself calls its tool (see `mcp::valid_server_name`, `mcp::namespaced_id`)
/// so they are usually already legal — but a remote server does not get to
/// violate that silently, so every id is normalized the same way before it
/// ever reaches a model rather than trusted as-is.
fn sanitize_tool_name(id: &str) -> String {
    let mut cleaned: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .take(64)
        .collect();
    if cleaned.is_empty() {
        cleaned.push('_');
    }
    cleaned
}

/// Disambiguate a sanitized name against whatever is already claimed.
/// Sanitizing can collide two different ids — either past character 64, or
/// purely through character replacement — so a repeat is suffixed rather
/// than silently shadowing the tool that claimed the name first, which would
/// otherwise make one of the two tools permanently uncallable.
fn unique_wire_name(base: &str, taken: &HashMap<String, String>) -> String {
    if !taken.contains_key(base) {
        return base.to_string();
    }
    for n in 2..1000 {
        let suffix = format!("-{n}");
        let budget = 64usize.saturating_sub(suffix.len());
        let candidate = format!("{}{suffix}", &base[..base.len().min(budget)]);
        if !taken.contains_key(&candidate) {
            return candidate;
        }
    }
    // Effectively unreachable — it would take hundreds of real collisions on
    // the same truncated prefix — but a tool the model can no longer address
    // safely is a better failure than looping forever.
    format!("{}-x", &base[..base.len().min(62)])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(id: &str, description: &str) -> McpTool {
        McpTool {
            id: id.to_string(),
            server: "srv".into(),
            name: id.to_string(),
            title: None,
            description: description.to_string(),
            input_schema: json!({ "type": "object" }),
        }
    }

    fn is_valid_openai_tool_name(s: &str) -> bool {
        !s.is_empty()
            && s.len() <= 64
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }

    // -----------------------------------------------------------------
    // sanitize_tool_name
    // -----------------------------------------------------------------

    #[test]
    fn an_already_legal_id_passes_through_unchanged() {
        assert_eq!(sanitize_tool_name("filesystem__read_file"), "filesystem__read_file");
    }

    #[test]
    fn illegal_characters_are_replaced_not_dropped() {
        // Dropping instead of replacing could collapse two distinct tools
        // ("a.b" and "ab") into the same name; replacing cannot.
        assert_eq!(sanitize_tool_name("weather.api__get forecast"), "weather_api__get_forecast");
    }

    #[test]
    fn an_overlong_id_is_truncated_to_64() {
        let long = "x".repeat(100);
        let cleaned = sanitize_tool_name(&long);
        assert_eq!(cleaned.len(), 64);
    }

    #[test]
    fn an_id_that_sanitizes_to_nothing_still_yields_a_legal_name() {
        assert_eq!(sanitize_tool_name("\u{1F600}\u{1F600}"), "__");
        assert_eq!(sanitize_tool_name(""), "_");
    }

    #[test]
    fn every_sanitized_name_satisfies_the_openai_pattern() {
        for input in ["ok__tool", "", "...", &"y".repeat(200), "sp ace/slash:colon", "\u{2603}"] {
            assert!(
                is_valid_openai_tool_name(&sanitize_tool_name(input)),
                "sanitize_tool_name({input:?}) violated the ^[a-zA-Z0-9_-]{{1,64}}$ contract"
            );
        }
    }

    // -----------------------------------------------------------------
    // build_tool_table / unique_wire_name
    // -----------------------------------------------------------------

    #[test]
    fn each_tool_becomes_a_spec_with_its_own_schema_and_resolves_back() {
        let tools = vec![tool("fs__read", "Read a file"), tool("fs__write", "Write a file")];
        let (specs, by_name) = build_tool_table(&tools);

        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "fs__read");
        assert_eq!(specs[0].description, "Read a file");
        assert_eq!(specs[0].parameters, json!({ "type": "object" }));

        assert_eq!(by_name.get("fs__read").map(String::as_str), Some("fs__read"));
        assert_eq!(by_name.get("fs__write").map(String::as_str), Some("fs__write"));
    }

    #[test]
    fn colliding_sanitized_names_are_disambiguated_and_both_still_resolve() {
        // "a.b" and "a/b" both sanitize to "a_b" \u{2014} a real (if unlikely)
        // way two different MCP ids could collapse onto the same wire name.
        let tools = vec![tool("a.b", "first"), tool("a/b", "second")];
        let (specs, by_name) = build_tool_table(&tools);

        assert_eq!(specs.len(), 2);
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_ne!(names[0], names[1], "both tools must be independently callable");

        // Each wire name must resolve back to the id that actually produced
        // it, not to whichever tool happened to be inserted last.
        assert_eq!(by_name.get(names[0]).map(String::as_str), Some("a.b"));
        assert_eq!(by_name.get(names[1]).map(String::as_str), Some("a/b"));
    }

    #[test]
    fn three_way_collisions_all_stay_distinct() {
        let tools = vec![tool("x!", "1"), tool("x?", "2"), tool("x#", "3")];
        let (specs, by_name) = build_tool_table(&tools);
        let names: std::collections::HashSet<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names.len(), 3, "all three collided names must remain distinguishable");
        assert_eq!(by_name.len(), 3);
    }

    // -----------------------------------------------------------------
    // parse_tool_arguments
    // -----------------------------------------------------------------

    #[test]
    fn blank_arguments_become_an_empty_object() {
        assert_eq!(parse_tool_arguments("").unwrap(), json!({}));
        assert_eq!(parse_tool_arguments("   ").unwrap(), json!({}));
    }

    #[test]
    fn valid_json_arguments_parse_through() {
        assert_eq!(
            parse_tool_arguments(r#"{"path":"/tmp/x","recursive":true}"#).unwrap(),
            json!({ "path": "/tmp/x", "recursive": true })
        );
    }

    #[test]
    fn invalid_json_arguments_are_an_err_meant_for_the_model_to_read() {
        let err = parse_tool_arguments("{not json").unwrap_err();
        assert!(err.contains("not valid JSON"));
    }

    // -----------------------------------------------------------------
    // UsageAccumulator
    // -----------------------------------------------------------------

    #[test]
    fn usage_sums_across_multiple_requests() {
        let mut acc = UsageAccumulator::default();
        acc.add(Some(&Usage { input_tokens: Some(10), output_tokens: Some(5) }));
        acc.add(Some(&Usage { input_tokens: Some(20), output_tokens: Some(7) }));
        let total = acc.into_usage().unwrap();
        assert_eq!(total.input_tokens, Some(30));
        assert_eq!(total.output_tokens, Some(12));
    }

    #[test]
    fn usage_with_no_requests_reporting_anything_is_none() {
        let mut acc = UsageAccumulator::default();
        acc.add(None);
        assert!(acc.into_usage().is_none());
    }

    #[test]
    fn a_request_reporting_only_one_side_does_not_zero_out_the_other() {
        let mut acc = UsageAccumulator::default();
        acc.add(Some(&Usage { input_tokens: Some(10), output_tokens: None }));
        acc.add(Some(&Usage { input_tokens: None, output_tokens: Some(3) }));
        let total = acc.into_usage().unwrap();
        assert_eq!(total.input_tokens, Some(10));
        assert_eq!(total.output_tokens, Some(3));
    }
}
