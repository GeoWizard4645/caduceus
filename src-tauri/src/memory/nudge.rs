//! The background memory "nudge" — Caduceus's equivalent of the reference
//! implementation's periodic self-review.
//!
//! Memory must build itself without the user asking. Every
//! [`NUDGE_INTERVAL_TURNS`] user turns, [`maybe_spawn_review`] forks a
//! *separate*, restricted mini conversation — offering only the `memory`
//! tool — and asks it whether anything durable came up. Writes land through
//! the exact same [`super::store::MemoryStore`] (and so the exact same
//! budget/dedup rules) a normal in-conversation `memory` call would use.
//!
//! # Never blocks the user's turn
//!
//! [`maybe_spawn_review`] is synchronous and returns immediately: the actual
//! review runs on [`tauri::async_runtime::spawn`], detached from whatever
//! call triggered it. A slow or failing review is logged and forgotten —
//! see the `Err` arm in [`maybe_spawn_review`] — never propagated back to
//! the turn that triggered it, which has typically already been shown to
//! the user by the time the review even starts.
//!
//! # Where this hooks in, and the one honest gap
//!
//! [`crate::agent::toolloop::run_tool_loop`] calls [`maybe_spawn_review`]
//! once per completed turn (see that function's `StopReason::Completed`
//! arm), counting [`Role::User`] messages in the *conversation it was
//! given* — architecturally the right hook: it is Caduceus's equivalent of
//! the reference implementation's own tool-calling conversation loop, and
//! already has the full transcript and an `AppHandle` in scope. The gap:
//! today's only caller of `run_tool_loop` (`agent::start_tool_session`)
//! starts every session from a single `Message::user(task)` rather than a
//! persisted, growing thread, so in practice the turn count rarely reaches
//! double digits yet. The counting logic itself is correct and
//! forward-compatible: a caller that passes real multi-turn history (as
//! `run_tool_loop`'s own doc says a caller should) gets a real nudge
//! cadence with no change needed here.
//!
//! # Tool whitelist
//!
//! The review conversation's `tools` array contains *only* the `memory`
//! spec (see [`memory_only_tool_specs`]) — not merely a dispatch-time
//! filter, but the actual wire list the model sees, so it cannot wander into
//! browsing/filesystem/other tools mid-review. The reference implementation
//! restricts its own review fork to `{memory, skills}`; Caduceus does not
//! yet have a `skills` native tool to add to that list — extending the
//! whitelist once `crate::skills` registers one is a one-line change to
//! [`memory_only_tool_specs`].

use tauri::{AppHandle, Manager, Runtime};

use crate::agent::openai::{self, ToolSpec};
use crate::agent::{AgentResult, Message, Role, ToolCall};
use crate::native_tools;
use crate::settings::BackendConfig;

use super::store::MemoryStore;

/// How many user turns pass between review sweeps. Matches the reference
/// implementation's own default (`memory.nudge_interval` there) — not
/// user-configurable here yet, since Caduceus's memory feature has no
/// settings surface at all yet. See the module doc's "one honest gap" for
/// why this rarely fires in practice today regardless of the exact number.
pub const NUDGE_INTERVAL_TURNS: usize = 10;

/// Upper bound on how many tool-calling round trips one review gets. A
/// review that needs more than a handful of `memory` calls to consolidate
/// is not going to be fixed by a higher number — better to stop and let the
/// next scheduled review pick up where this one left off.
const REVIEW_MAX_ITERATIONS: u32 = 4;

/// How many trailing messages of the conversation the review replays.
/// Bounds the review's own cost regardless of how long the real conversation
/// has grown. The reference implementation solves the same problem with a
/// digest-and-summarize step for a routed auxiliary model; Caduceus has no
/// such routing concept yet, so this simply keeps the most recent messages
/// verbatim and drops anything older rather than summarizing it.
const REVIEW_TRANSCRIPT_TURNS: usize = 40;

const REVIEW_PROMPT: &str = "\
Review the conversation above and consider saving to memory if appropriate.

Focus on:
1. Has the user revealed things about themselves \u{2014} their persona, desires, \
preferences, or personal details worth remembering?
2. Has the user expressed expectations about how you should behave, their work \
style, or ways they want you to operate?

If something stands out, save it using the memory tool. If nothing is worth saving, \
just say \"Nothing to save.\" and stop.";

/// True once `user_turns` has just reached a positive multiple of
/// [`NUDGE_INTERVAL_TURNS`] — i.e. the 10th, 20th, 30th... user turn.
/// Stateless by design: rather than tracking "turns since the last review"
/// (which would need to persist across process restarts to survive a
/// resumed conversation), this is recomputed from the live turn count on
/// every call — the same effect as the reference implementation's own
/// *hydration* of that counter from persisted history on session resume
/// (`prior_turns % interval`), just without a separate counter to hydrate.
fn should_review(user_turns: usize) -> bool {
    user_turns > 0 && user_turns % NUDGE_INTERVAL_TURNS == 0
}

fn count_user_turns(messages: &[Message]) -> usize {
    messages.iter().filter(|m| m.role == Role::User).count()
}

/// Check whether this completed turn lands on a review boundary and, if so,
/// spawn the review in the background. Returns immediately either way — see
/// the module doc's "never blocks the user's turn."
pub fn maybe_spawn_review<R: Runtime>(app: &AppHandle<R>, config: &BackendConfig, messages: &[Message]) {
    if !should_review(count_user_turns(messages)) {
        return;
    }
    // The memory feature may be unavailable (its store failed to open at
    // startup — see `lib.rs::setup`'s "a feature, not a prerequisite"
    // handling for the same pattern on clipboard/chat). A missing store is
    // not an error here, just nothing to review into. Only presence is
    // checked here — dispatch below goes through the `native_tools`
    // registry (which closed over its own store clone at registration
    // time), not this handle.
    if app.try_state::<MemoryStore>().is_none() {
        return;
    }
    let config = config.clone();
    let transcript = recent_tail(messages, REVIEW_TRANSCRIPT_TURNS);

    tauri::async_runtime::spawn(async move {
        match review_once(&config, transcript).await {
            Ok(Outcome::Saved(note)) => log::info!("memory nudge: {note}"),
            Ok(Outcome::NothingToSave) => log::debug!("memory nudge: nothing worth saving this round"),
            // The main turn has already answered the user by the time this
            // runs; a failed background review is worth a log line, never a
            // user-facing error.
            Err(e) => log::warn!("memory nudge failed (non-fatal): {}", e.user_message()),
        }
    });
}

enum Outcome {
    Saved(String),
    NothingToSave,
}

/// Run one review conversation to completion: replay `transcript` plus the
/// review prompt, offering only the `memory` tool, dispatching any calls the
/// model makes, and stopping when it answers with no more tool calls or
/// [`REVIEW_MAX_ITERATIONS`] is hit.
async fn review_once(config: &BackendConfig, mut transcript: Vec<Message>) -> AgentResult<Outcome> {
    let tool_specs = memory_only_tool_specs();
    transcript.push(Message::user(REVIEW_PROMPT));

    let mut saved_notes: Vec<String> = Vec::new();

    for _ in 0..REVIEW_MAX_ITERATIONS {
        let turn = openai::stream_chat_with_tools(&transcript, config, &tool_specs, |_| {}).await?;
        if turn.tool_calls.is_empty() {
            break;
        }
        transcript.push(Message::assistant_tool_calls(turn.text.clone(), turn.tool_calls.clone()));

        for call in &turn.tool_calls {
            let text = run_whitelisted_call(call);
            if is_successful_write(&text) {
                saved_notes.push(text.clone());
            }
            transcript.push(Message::tool_result(call.id.clone(), text));
        }
    }

    if saved_notes.is_empty() {
        Ok(Outcome::NothingToSave)
    } else {
        Ok(Outcome::Saved(format!("{} write(s) landed", saved_notes.len())))
    }
}

/// Run one tool call from the review conversation, denying anything that is
/// not `memory` at dispatch time — belt-and-suspenders alongside
/// `tool_specs` only ever offering `memory` in the first place (see the
/// module doc's "tool whitelist"), in case a provider ever echoes back a
/// call for a tool it was not actually offered.
fn run_whitelisted_call(call: &ToolCall) -> String {
    if call.name != "memory" {
        return "Denied: the background memory review may only call the memory tool.".to_string();
    }
    let args = match crate::agent::toolloop::parse_tool_arguments(&call.arguments) {
        Ok(v) => v,
        Err(detail) => return detail,
    };
    // Dispatched through the same process-wide registry `agent::toolloop`
    // itself calls, so this hits the exact `memory::tool::handle` a normal
    // in-conversation call would — the store it writes to was bound into
    // the registered closure once, at `lib.rs::setup()` time (see
    // `memory::register_native_tools`), not re-resolved here. `memory`'s
    // handler is synchronous file I/O against a tiny (<2,200 char) file —
    // unlike `agent::toolloop`'s own dispatch, this runs directly rather
    // than via `spawn_blocking`, since this whole function already executes
    // inside a detached background task, off the request-serving path.
    match native_tools::call("memory", args) {
        Ok(value) => match value {
            serde_json::Value::String(s) => s,
            other => other.to_string(),
        },
        Err(e) => e,
    }
}

/// The last `max` messages of `messages`, or all of them if there are fewer.
fn recent_tail(messages: &[Message], max: usize) -> Vec<Message> {
    if messages.len() <= max {
        messages.to_vec()
    } else {
        messages[messages.len() - max..].to_vec()
    }
}

/// Just the `memory` tool's spec, sourced from the live `native_tools`
/// registry rather than duplicated by hand — if `memory::tool`'s schema or
/// description ever changes, the review picks it up automatically. Empty
/// (not an error) if the memory tool is somehow not registered; the review
/// conversation then simply runs with no tools and answers in prose, which
/// [`review_once`]'s loop already treats as "nothing to save."
fn memory_only_tool_specs() -> Vec<ToolSpec> {
    native_tools::list()
        .into_iter()
        .filter(|t| t.name == "memory")
        .map(|t| ToolSpec { name: t.name, description: t.description, parameters: t.input_schema })
        .collect()
}

/// Best-effort read of a `memory` tool result's `"success": true` field, so
/// only genuine writes count toward the action summary logged by
/// [`maybe_spawn_review`]. A no-op duplicate is still `success: true` (see
/// `memory::store::MemoryStore::add`) and so still counts — matching the
/// reference implementation's own "a duplicate is still a successful save,
/// whichever call put it there."
fn is_successful_write(tool_result_text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(tool_result_text)
        .ok()
        .and_then(|v| v.get("success").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> Message {
        Message::user(text)
    }
    fn assistant(text: &str) -> Message {
        Message::assistant(text)
    }

    // -----------------------------------------------------------------
    // should_review / count_user_turns
    // -----------------------------------------------------------------

    #[test]
    fn fires_on_exact_multiples_of_the_interval_only() {
        for n in 0..NUDGE_INTERVAL_TURNS {
            assert!(!should_review(n), "{n} must not fire");
        }
        assert!(should_review(NUDGE_INTERVAL_TURNS));
        assert!(should_review(NUDGE_INTERVAL_TURNS * 2));
        assert!(!should_review(NUDGE_INTERVAL_TURNS + 1));
    }

    #[test]
    fn zero_turns_never_fires() {
        assert!(!should_review(0));
    }

    #[test]
    fn counts_only_user_messages() {
        let messages = vec![
            Message::system("sys"),
            user("one"),
            assistant("a1"),
            user("two"),
            Message::tool_result("call1", "result"),
        ];
        assert_eq!(count_user_turns(&messages), 2);
    }

    // -----------------------------------------------------------------
    // recent_tail
    // -----------------------------------------------------------------

    #[test]
    fn recent_tail_keeps_everything_when_under_the_cap() {
        let messages = vec![user("a"), assistant("b")];
        assert_eq!(recent_tail(&messages, 10).len(), 2);
    }

    #[test]
    fn recent_tail_truncates_to_the_most_recent_messages() {
        let messages: Vec<Message> = (0..10).map(|i| user(&format!("m{i}"))).collect();
        let tail = recent_tail(&messages, 3);
        assert_eq!(tail.len(), 3);
        assert_eq!(tail[0].content, "m7");
        assert_eq!(tail[2].content, "m9");
    }

    // -----------------------------------------------------------------
    // is_successful_write
    // -----------------------------------------------------------------

    #[test]
    fn recognises_a_successful_tool_result() {
        assert!(is_successful_write(r#"{"success": true, "message": "Entry added."}"#));
    }

    #[test]
    fn a_failed_or_malformed_result_is_not_a_successful_write() {
        assert!(!is_successful_write(r#"{"success": false, "error": "over budget"}"#));
        assert!(!is_successful_write("not even json"));
        assert!(!is_successful_write(""));
    }

    // -----------------------------------------------------------------
    // run_whitelisted_call
    // -----------------------------------------------------------------

    #[test]
    fn a_call_to_anything_other_than_memory_is_denied() {
        let call = ToolCall { id: "1".into(), name: "shell".into(), arguments: "{}".into() };
        let result = run_whitelisted_call(&call);
        assert!(result.contains("Denied"));
    }
}
