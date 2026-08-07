//! Persistent, self-updating memory for Caduceus's agent — matching the
//! reference implementation (Hermes Agent)'s design closely: this is **not**
//! a knowledge graph and **not** a vector store. It is two flat, bounded,
//! human-readable markdown files, a `memory` tool the model calls to curate
//! them, a `session_search` tool over saved conversation history (SQLite
//! FTS5 + BM25, no embeddings), and a background "nudge" that reviews the
//! conversation for durable facts every so often without being asked. See
//! each submodule's doc for the details:
//!
//! - [`store`] — the bounded, atomic-write file store itself (`MEMORY.md` /
//!   `USER.md`, the `§`-delimiter, the reject-and-consolidate budget).
//! - [`tool`] — the `memory` native tool built on top of it, including the
//!   prompt guidance that makes memory useful rather than noisy.
//! - [`session_search`] — the `session_search` native tool, over
//!   [`crate::chat`]'s existing conversation database.
//! - [`nudge`] — the periodic background review ("the nudge").
//! - [`commands`] — Tauri commands for a human-facing surface (a future
//!   Settings panel, or manual testing) outside the agent loop.
//!
//! # Where the files live
//!
//! `MEMORY.md` and `USER.md` live in `<app data dir>/memory/` — a
//! subdirectory of the same `app_data_dir()` [`crate::clipboard`] and
//! [`crate::chat`] already use for their own SQLite files, kept in its own
//! folder (rather than two more files loose at the top level) because,
//! unlike those, this pair is meant to be opened directly by a human in a
//! text editor or Obsidian — see `store`'s module doc.
//!
//! # How this reaches the agent loop
//!
//! Neither tool here is an MCP server. Both register into the process-wide,
//! synchronous-handler [`crate::native_tools`] registry — see that module's
//! doc for the design — which `agent::toolloop::run_tool_loop` merges into
//! the model-facing tool list and dispatches from directly, ahead of MCP
//! resolution. [`register_native_tools`] is the single call site that wires
//! this module into that registry; see `lib.rs::setup()` for where it runs.

pub mod commands;
pub mod nudge;
pub mod session_search;
pub mod store;
pub mod tool;

pub use session_search::{SearchError, SessionSearchHit, SessionSearchIndex};
pub use store::{
    MemoryError, MemoryStore, OpAction, Operation, Target, Usage, WriteReport, DEFAULT_MEMORY_CHAR_LIMIT,
    DEFAULT_USER_CHAR_LIMIT,
};

/// Subdirectory of `app_data_dir()` the two files live in.
pub const MEMORY_DIR: &str = "memory";

/// Build both native tools (`memory`, `session_search`) and register them
/// into the process-wide [`crate::native_tools`] registry. Call exactly
/// once, from `lib.rs::setup()`, after both `store` and `search_index` are
/// constructed.
///
/// Order relative to `chat::ChatStore` matters for `search_index` —
/// [`SessionSearchIndex::open`] adds triggers that reference the `messages`
/// table, so it must exist first. It enforces this itself (opening a
/// `ChatStore` internally before doing anything else), so misordering this
/// call in `lib.rs` would surface as a loud open error rather than a silent
/// no-op — see that function's doc.
pub fn register_native_tools(store: MemoryStore, search_index: SessionSearchIndex) {
    tool::register(store);
    session_search::register(search_index);
}
