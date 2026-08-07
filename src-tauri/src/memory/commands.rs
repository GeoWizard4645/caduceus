//! Tauri commands over the memory feature.
//!
//! Neither the `memory` nor `session_search` *tool* (see `tool.rs` /
//! `session_search.rs`) goes through these — the agent tool-calling loop
//! dispatches to them via the `native_tools` registry directly, without any
//! IPC involved. These commands exist for the human-facing surface: showing
//! what memory currently holds, editing it by hand with the exact same
//! rules a model's call would follow, and searching past conversations from
//! something other than the agent loop (a future Settings panel, or manual
//! testing via the devtools console).

use serde::Serialize;
use tauri::State;

use super::session_search::{SessionSearchHit, SessionSearchIndex};
use super::store::{MemoryStore, Target, Usage};
use super::tool;

type Res<T> = Result<T, String>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryTargetSnapshot {
    pub target: String,
    pub entries: Vec<String>,
    pub usage: Usage,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySnapshot {
    pub memory: MemoryTargetSnapshot,
    pub user: MemoryTargetSnapshot,
}

/// Every entry currently held in both stores, plus budget usage — the whole
/// state a Settings panel would need to render `MEMORY.md`/`USER.md` and
/// let someone inspect (or, via [`memory_write`], edit) them without going
/// through the model.
#[tauri::command]
pub fn memory_snapshot(store: State<'_, MemoryStore>) -> Res<MemorySnapshot> {
    Ok(MemorySnapshot { memory: target_snapshot(&store, Target::Memory), user: target_snapshot(&store, Target::User) })
}

fn target_snapshot(store: &MemoryStore, target: Target) -> MemoryTargetSnapshot {
    MemoryTargetSnapshot { target: target.as_str().to_string(), entries: store.entries(target), usage: store.usage(target) }
}

/// Manual passthrough to the same add/replace/remove/batch logic the
/// `memory` tool uses (`target`/`action`/`content`/`old_text`/`operations`,
/// identical to the tool's own arguments — see `tool.rs`'s schema) — lets a
/// Settings panel edit memory the same way the agent does, with the same
/// budget and duplicate rules, rather than a second, parallel write path.
/// Returns the same JSON shape the tool itself returns to a model.
#[tauri::command]
pub fn memory_write(store: State<'_, MemoryStore>, args: serde_json::Value) -> Res<serde_json::Value> {
    Ok(tool::handle(&store, args))
}

/// Search past `/` conversations. The same [`SessionSearchIndex::search`]
/// the `session_search` agent tool calls (see that module's doc for the
/// FTS5/BM25 design), exposed directly for a UI that wants typed results
/// rather than a JSON tool-result string.
#[tauri::command]
pub fn memory_session_search(
    index: State<'_, SessionSearchIndex>,
    query: String,
    limit: Option<usize>,
) -> Res<Vec<SessionSearchHit>> {
    index.search(&query, limit.unwrap_or(8).clamp(1, 25)).map_err(|e| e.to_string())
}
