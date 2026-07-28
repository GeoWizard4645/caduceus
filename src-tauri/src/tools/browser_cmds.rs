//! Thin `#[tauri::command]` wrappers around [`super::browsertabs`].
//!
//! Everything in `browsertabs.rs` either scripts a browser over AppleScript
//! (`osascript`), shells to `plutil` to read Safari's binary plist, or copies
//! and opens a SQLite file for Firefox — all real subprocess/file-system
//! work, never something safe to run on the thread that is also drawing
//! every window. So, per the same reasoning `security_cmds.rs` documents in
//! its own header, every wrapper here is `async` + `spawn_blocking`, with no
//! synchronous exception: unlike `security.rs`, nothing in `browsertabs.rs`
//! is a pure in-process computation.
//!
//! # Where these are meant to surface
//!
//! Per the task brief this file was built under: browser tabs, bookmarks and
//! downloads are **palette search results**, not a page with its own tab —
//! searching "docs" should turn up the open "Docs" tab the same way it turns
//! up a command. Wiring the palette provider and adding `COMMANDS` entries is
//! explicitly out of this file's scope (`commands.ts`'s `COMMANDS` array
//! belongs to another agent); this file only makes the four functions
//! reachable at all. See this agent's final report for the entries to add.

use tauri::async_runtime::spawn_blocking;

use super::browsertabs;
use super::ToolOutcome;

// ---------------------------------------------------------------------------
// Tabs
// ---------------------------------------------------------------------------

/// Search every open tab across every scriptable browser. An empty `query`
/// lists everything open — see `browsertabs::search_tabs`'s doc comment.
#[tauri::command]
pub async fn browser_search_tabs(query: String) -> Vec<browsertabs::TabHit> {
    spawn_blocking(move || browsertabs::search_tabs(&query)).await.unwrap_or_default()
}

/// Bring one open tab to the front. `browser`/`window_id`/`tab_index` are
/// meant to be handed back exactly as `browser_search_tabs` produced them —
/// see `browsertabs::switch_tab`'s doc comment on why a tab found earlier and
/// then closed can make this fail with a "no longer valid" message rather
/// than switching to the wrong thing.
#[tauri::command]
pub async fn browser_switch_tab(browser: String, window_id: i64, tab_index: i64) -> ToolOutcome {
    spawn_blocking(move || browsertabs::switch_tab(&browser, window_id, tab_index))
        .await
        .unwrap_or_else(|e| ToolOutcome::err(format!("It could not be run: {e}")))
}

// ---------------------------------------------------------------------------
// Bookmarks
// ---------------------------------------------------------------------------

/// Search bookmarks across Safari, the Chromium family and Firefox. `limit`
/// bounds the result count *after* ranking — see `search_bookmarks`'s doc
/// comment — so a small limit still returns the best matches.
#[tauri::command]
pub async fn browser_search_bookmarks(query: String, limit: Option<usize>) -> Vec<browsertabs::BookmarkHit> {
    spawn_blocking(move || browsertabs::search_bookmarks(&query, limit.unwrap_or(40)))
        .await
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Downloads
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn browser_recent_downloads(limit: Option<usize>) -> Vec<browsertabs::DownloadHit> {
    spawn_blocking(move || browsertabs::recent_downloads(limit.unwrap_or(20)))
        .await
        .unwrap_or_default()
}
