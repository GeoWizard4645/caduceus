//! The `memory` native tool — the model-facing surface over
//! [`super::store::MemoryStore`], registered into the process-wide
//! [`crate::native_tools`] registry (see that module's doc for the registry
//! itself) by [`register`], which `lib.rs::setup()` calls once via
//! [`super::register_native_tools`].
//!
//! # The description is the product
//!
//! [`DESCRIPTION`] is not boilerplate — it is the entire behavioural
//! contract a model reads before deciding whether to save something, and it
//! is the single biggest lever over whether memory ends up full of durable,
//! high-signal facts or noisy task-progress notes nobody wanted persisted.
//! The budget-rejection guidance ("IF FULL: ... reissue as ONE call") is what
//! makes the reject-and-consolidate mechanism in `store.rs` actually work in
//! practice: a model that has never been told *how* to respond to
//! `OverBudget` will often just retry the same failing `add` verbatim.
//!
//! # Argument parsing is deliberately tolerant, not `serde`-strict
//!
//! Real tool-calling clients occasionally send `"target": null` for an
//! omitted optional field instead of leaving the key out entirely, or send
//! `replace`/`remove` with no `old_text` at all. A `#[derive(Deserialize)]`
//! struct would hard-fail the whole call on either — this module instead
//! reads fields with [`str_field`], which treats "absent," "present but
//! null," and "present but the wrong JSON type" identically as "not given,"
//! and turns a missing `old_text` into a recoverable response (the current
//! entries plus a retry instruction — see [`missing_old_text`]) rather than
//! a dead-end error. This mirrors hard-won handling in the reference
//! implementation (see its `_missing_old_text_error`).

use serde_json::{json, Map, Value};

use crate::native_tools::{self, NativeTool};

use super::store::{MemoryError, MemoryStore, OpAction, Operation, Target, Usage, WriteReport};

const NAME: &str = "memory";

const DESCRIPTION: &str = "\
Save durable facts to persistent memory that survive across sessions. Both stores are \
injected into the system prompt on every future session, so keep entries compact and \
high-signal.\n\
\n\
HOW: for more than one change in a turn, prefer a single call with an 'operations' \
array (each item: {action, content?, old_text?}) — it applies atomically and the \
character budget is checked only on the FINAL result, so one call can remove or \
shorten stale entries to free room AND add a new one, even when an add alone would \
overflow. Use the bare action/content/old_text fields only for one simple change.\n\
\n\
WHEN: save proactively when the user states a preference, a correction, or a personal \
detail, or a stable fact about their environment, conventions, or workflow becomes \
clear. Prioritize what reduces future user steering — the best memory stops the user \
repeating themselves. Do NOT save task progress. If a fact will be stale in a week, it \
does not belong in memory. Write memories as declarative facts, not instructions to \
yourself: \"User prefers concise responses\" is right; \"Always respond concisely\" is \
wrong.\n\
\n\
IF FULL: an add is rejected with the current entries shown in the error. Reissue as \
ONE call (use 'operations') that removes or shortens enough stale entries and adds the \
new one together, all in the same turn.\n\
\n\
TARGETS: 'user' = who the user is (name, role, preferences, communication style). \
'memory' = your own notes (environment facts, project conventions, tool quirks, \
lessons learned).\n\
\n\
SKIP: trivial or easily re-discovered facts, raw data dumps, task progress, \
completed-work logs, or temporary state — use session_search to recall a past \
conversation instead of memorizing its contents. Procedures and workflows belong in a \
skill, not memory.";

/// Build the `memory` tool over `store` and register it. Call once, from
/// `lib.rs::setup()` (via [`super::register_native_tools`]) — a second
/// registration under the same name would panic at startup, per
/// `native_tools::register`'s replace-with-a-warning behaviour, which is
/// exactly why this is a single explicit call site rather than something
/// that could run twice by accident.
pub fn register(store: MemoryStore) {
    native_tools::register(NativeTool::new(NAME, DESCRIPTION, schema(), move |args| {
        Ok(handle(&store, args))
    }));
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "target": {
                "type": "string",
                "enum": ["memory", "user"],
                "description": "Which store: 'memory' for the agent's own notes, 'user' for the user profile."
            },
            "action": {
                "type": "string",
                "enum": ["add", "replace", "remove"],
                "description": "The action to perform (single-op shape). Omit when using 'operations'."
            },
            "content": {
                "type": "string",
                "description": "The entry text. Required for 'add' and 'replace' (single-op shape)."
            },
            "old_text": {
                "type": "string",
                "description": "A short substring that uniquely identifies the existing entry. Required for 'replace' and 'remove' (single-op shape)."
            },
            "operations": {
                "type": "array",
                "description": "Batch shape: operations applied atomically in one call, checked against the final character budget. Each item is {action, content?, old_text?}. Preferred whenever making more than one change, or to free room and add in the same call.",
                "items": {
                    "type": "object",
                    "properties": {
                        "action": {"type": "string", "enum": ["add", "replace", "remove"]},
                        "content": {"type": "string"},
                        "old_text": {"type": "string"}
                    },
                    "required": ["action"]
                }
            }
        },
        "required": ["target"]
    })
}

/// Dispatch one call. Infallible by design — every outcome, including a
/// validation failure, is a `{"success": false, ...}` JSON value the model
/// reads back as its tool result, never a Rust-level `Err` from this
/// function itself. See the module doc and `native_tools`' own doc on why a
/// tool never "fails outright."
///
/// `pub(crate)` rather than private: `memory::commands::memory_write` reuses
/// this directly so a hand-edit from a future Settings panel goes through
/// the exact same budget/dedup/validation rules a model's call would, rather
/// than a second, parallel write path.
pub(crate) fn handle(store: &MemoryStore, args: Value) -> Value {
    let obj = args.as_object();
    let target_str = str_field(obj, "target").unwrap_or_else(|| "memory".to_string());
    let Some(target) = Target::parse(&target_str) else {
        return error(&format!("Invalid target '{target_str}'. Use 'memory' or 'user'."));
    };

    // Batch path takes priority when a non-empty `operations` array is
    // present, exactly like the reference implementation's tool.
    if let Some(ops) = obj.and_then(|o| o.get("operations")).and_then(Value::as_array) {
        if !ops.is_empty() {
            return run_batch(store, target, ops);
        }
    }

    let action = str_field(obj, "action").unwrap_or_default();
    let content = str_field(obj, "content");
    let old_text = str_field(obj, "old_text");

    let result = match action.as_str() {
        "add" => match content {
            Some(c) => store.add(target, &c),
            None => return error("Content is required for 'add' action."),
        },
        "replace" => match (old_text, content) {
            (Some(ot), Some(c)) => store.replace(target, &ot, &c),
            (None, _) => return missing_old_text(store, target, "replace"),
            (Some(_), None) => return error("content is required for 'replace' action (use 'remove' to delete an entry)."),
        },
        "remove" => match old_text {
            Some(ot) => store.remove(target, &ot),
            None => return missing_old_text(store, target, "remove"),
        },
        "" => return error("action is required (add, replace, or remove) unless 'operations' is used."),
        other => return error(&format!("Unknown action '{other}'. Use: add, replace, remove")),
    };

    outcome(target, result)
}

fn run_batch(store: &MemoryStore, target: Target, ops: &[Value]) -> Value {
    let mut parsed = Vec::with_capacity(ops.len());
    for (i, op) in ops.iter().enumerate() {
        let obj = op.as_object();
        let action_str = str_field(obj, "action").unwrap_or_default();
        let Some(action) = parse_action(&action_str) else {
            return error(&format!(
                "Operation {}: unknown action '{action_str}'. Use add, replace, or remove.",
                i + 1
            ));
        };
        parsed.push(Operation { action, content: str_field(obj, "content"), old_text: str_field(obj, "old_text") });
    }
    outcome(target, store.apply_batch(target, &parsed))
}

fn parse_action(s: &str) -> Option<OpAction> {
    match s {
        "add" => Some(OpAction::Add),
        "replace" => Some(OpAction::Replace),
        "remove" => Some(OpAction::Remove),
        _ => None,
    }
}

/// `replace`/`remove` are inherently targeted — without `old_text` there is
/// no entry to act on. Rather than a dead-end error, hand back the current
/// inventory and an explicit retry instruction, so the model can reissue the
/// call with `old_text` set to a substring of the entry it meant. Mirrors
/// the reference implementation's identical accommodation for structured-
/// output clients that fill an omitted optional field with JSON `null`.
fn missing_old_text(store: &MemoryStore, target: Target, action: &str) -> Value {
    let entries = store.entries(target);
    let usage = store.usage(target);
    json!({
        "success": false,
        "error": format!(
            "'{action}' needs old_text \u{2014} a short unique substring of the entry to {action}. \
             None was provided. Reissue with old_text set to part of one of the current_entries below."
        ),
        "current_entries": entries,
        "usage": usage_string(&usage),
    })
}

fn outcome(target: Target, result: Result<WriteReport, MemoryError>) -> Value {
    match result {
        Ok(report) => success(target, &report),
        Err(e) => failure(e),
    }
}

fn success(target: Target, report: &WriteReport) -> Value {
    json!({
        "success": true,
        "target": target.as_str(),
        "usage": usage_string(&report.usage),
        "entry_count": report.usage.entry_count,
        "message": report.message,
        // Deliberately terminal, matching the reference implementation: the
        // full entries list is NOT echoed back on success. Dumping it here
        // invites the model to "find more to fix" and re-issue redundant
        // calls; entries are only shown on the error paths below, where the
        // model genuinely needs them to decide what to consolidate.
        "note": "Write saved. This update is complete \u{2014} do not repeat it.",
    })
}

fn failure(e: MemoryError) -> Value {
    let message = e.to_string();
    match e {
        MemoryError::OverBudget { entries, usage, .. } => json!({
            "success": false,
            "error": message,
            "current_entries": entries,
            "usage": usage_string(&usage),
        }),
        MemoryError::NoMatch { entries, usage, .. } => json!({
            "success": false,
            "error": message,
            "current_entries": entries,
            "usage": usage_string(&usage),
        }),
        MemoryError::AmbiguousMatch { previews, .. } => json!({
            "success": false,
            "error": message,
            "matches": previews,
        }),
        MemoryError::BatchOperation { entries, usage, .. } => json!({
            "success": false,
            "error": format!("{message} \u{2014} no operations were applied (batch is all-or-nothing)."),
            "current_entries": entries,
            "usage": usage_string(&usage),
        }),
        MemoryError::EmptyContent
        | MemoryError::EmptyOldText
        | MemoryError::EmptyReplacement
        | MemoryError::EmptyBatch
        | MemoryError::Io(_) => error(&message),
    }
}

fn usage_string(usage: &Usage) -> String {
    format!("{}% \u{2014} {}/{} chars", usage.percent, usage.current, usage.limit)
}

fn error(message: &str) -> Value {
    json!({ "success": false, "error": message })
}

/// Read `key` from `obj` as a string, treating "absent," "explicit JSON
/// `null`," and "present but not a string" all as "not given" — see the
/// module doc's "argument parsing" section for why this is deliberately more
/// forgiving than a typed `serde::Deserialize`.
fn str_field(obj: Option<&Map<String, Value>>, key: &str) -> Option<String> {
    obj?.get(key)?.as_str().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_store() -> MemoryStore {
        let dir = std::env::temp_dir().join(format!("caduceus-memory-tool-test-{}", uuid::Uuid::new_v4()));
        let _: PathBuf = dir.clone();
        MemoryStore::open(dir, super::super::store::DEFAULT_MEMORY_CHAR_LIMIT, super::super::store::DEFAULT_USER_CHAR_LIMIT).unwrap()
    }

    #[test]
    fn add_via_json_args_persists_and_reports_success() {
        let store = temp_store();
        let result = handle(&store, json!({"target": "memory", "action": "add", "content": "user is named Alex"}));
        assert_eq!(result["success"], true);
        assert_eq!(store.entries(Target::Memory), vec!["user is named Alex".to_string()]);
    }

    #[test]
    fn missing_target_defaults_to_memory() {
        let store = temp_store();
        let result = handle(&store, json!({"action": "add", "content": "fact"}));
        assert_eq!(result["success"], true);
        assert_eq!(result["target"], "memory");
    }

    #[test]
    fn a_null_target_is_treated_as_omitted_not_a_type_error() {
        let store = temp_store();
        let result = handle(&store, json!({"target": null, "action": "add", "content": "fact"}));
        assert_eq!(result["success"], true);
        assert_eq!(result["target"], "memory");
    }

    #[test]
    fn an_invalid_target_names_the_two_legal_values() {
        let store = temp_store();
        let result = handle(&store, json!({"target": "nonsense", "action": "add", "content": "x"}));
        assert_eq!(result["success"], false);
        assert!(result["error"].as_str().unwrap().contains("memory"));
        assert!(result["error"].as_str().unwrap().contains("user"));
    }

    #[test]
    fn replace_without_old_text_returns_current_entries_instead_of_a_dead_end() {
        let store = temp_store();
        store.add(Target::Memory, "existing fact").unwrap();
        let result = handle(&store, json!({"target": "memory", "action": "replace", "content": "new text"}));
        assert_eq!(result["success"], false);
        assert_eq!(result["current_entries"][0], "existing fact");
    }

    #[test]
    fn an_over_budget_add_reports_current_entries_and_usage() {
        let store = MemoryStore::open(
            std::env::temp_dir().join(format!("caduceus-memory-tool-test-{}", uuid::Uuid::new_v4())),
            10,
            super::super::store::DEFAULT_USER_CHAR_LIMIT,
        )
        .unwrap();
        store.add(Target::Memory, "12345").unwrap();
        let result = handle(&store, json!({"target": "memory", "action": "add", "content": "way too long for ten chars"}));
        assert_eq!(result["success"], false);
        assert!(result["current_entries"].as_array().unwrap().contains(&json!("12345")));
        assert!(result["usage"].as_str().unwrap().contains("10"));
    }

    #[test]
    fn batch_operations_array_applies_atomically() {
        let store = temp_store();
        store.add(Target::Memory, "old fact").unwrap();
        let result = handle(
            &store,
            json!({
                "target": "memory",
                "operations": [
                    {"action": "remove", "old_text": "old fact"},
                    {"action": "add", "content": "new fact"}
                ]
            }),
        );
        assert_eq!(result["success"], true);
        assert_eq!(store.entries(Target::Memory), vec!["new fact".to_string()]);
    }

    #[test]
    fn an_unknown_action_names_the_legal_ones() {
        let store = temp_store();
        let result = handle(&store, json!({"target": "memory", "action": "delete", "content": "x"}));
        assert_eq!(result["success"], false);
        let err = result["error"].as_str().unwrap();
        assert!(err.contains("add") && err.contains("replace") && err.contains("remove"));
    }

    #[test]
    fn registering_builds_a_schema_requiring_only_target() {
        let schema = schema();
        assert_eq!(schema["required"], json!(["target"]));
        assert!(schema["properties"]["operations"].is_object());
    }
}
