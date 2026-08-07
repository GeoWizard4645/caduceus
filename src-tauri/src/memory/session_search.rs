//! `session_search` — full-text recall over past `/` conversations.
//!
//! Distinct from [`super::store`]'s bounded MEMORY.md/USER.md: those hold a
//! small number of durable, hand-curated *facts*. This searches the
//! **unbounded** transcript of everything ever said in a saved conversation
//! — task progress, one-off details, anything that would be "stale in a
//! week" and so does not belong in memory, but that the user or the model
//! might still want to find again later. Matches the reference
//! implementation's own split: SQLite FTS5 + BM25, no embeddings, no vector
//! index.
//!
//! # Reuse, not duplication
//!
//! This module does not maintain its own copy of conversation history. It
//! opens a **second** `rusqlite::Connection` to the exact same
//! `chats.db` file [`crate::chat::ChatStore`] already owns, and adds a
//! search-only layer on top of the `messages` table already there:
//!
//!   - an FTS5 *external content* virtual table (`messages_fts`), which
//!     indexes `messages.text` without copying it — `content='messages'`
//!     means FTS5 reads the real row through `content_rowid` at query time
//!     rather than storing a second copy of every message body;
//!   - three triggers (`AFTER INSERT|UPDATE|DELETE ON messages`) that keep
//!     that index in sync with every write [`crate::chat::ChatStore`] makes.
//!
//! Triggers are schema objects, not Rust code: SQLite stores them in the
//! database file itself, so creating them from *this* connection is enough
//! to fire whenever *`ChatStore`'s own* connection later inserts a message —
//! no change to `chat/store.rs` is needed, and this module never has to be
//! told when a write happened. [`SessionSearchIndex::open`] opens
//! [`crate::chat::ChatStore`] first (idempotent — `CREATE TABLE IF NOT
//! EXISTS`) purely to guarantee `messages`/`conversations` exist before
//! these triggers reference them; it does not keep that store around.
//!
//! `tools::semantic.rs` also implements BM25 (hand-rolled, over its own
//! `files`/`postings` tables for local-file search) but its tokenizer, IDF,
//! and scoring helpers are private to that module and built around a
//! completely different schema — there was nothing importable to reuse
//! there. Reaching for SQLite's *native* FTS5 module here instead is a
//! closer match to what this feature actually needs (and to the reference
//! implementation's own "SQLite FTS5 + BM25" design) than porting a
//! file-indexing engine would have been.
//!
//! # Query safety
//!
//! A user's search text is not a trusted FTS5 query string — it can contain
//! `MATCH` syntax characters (`-`, `:`, `*`, parentheses, quotes) that would
//! otherwise throw a syntax error or accidentally trigger a column filter or
//! prefix search. [`build_match_expression`] tokenizes on non-alphanumeric
//! boundaries and quotes + ANDs every token, which neutralizes all of that.

use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::{json, Value};

use crate::native_tools::{self, NativeTool};

const NAME: &str = "session_search";
const DEFAULT_RESULTS: usize = 8;
const MAX_RESULTS: usize = 25;

const DESCRIPTION: &str = "\
Full-text search over the user's past `/` conversations in Caduceus, ranked by \
relevance (SQLite FTS5 + BM25 \u{2014} lexical, not semantic: it matches words, not \
meaning, so search with terms the user is likely to have typed, not a rephrased \
question). Use this to recall what was discussed before, find a detail or decision \
from an earlier session, or check whether something already came up. Task progress, \
one-off details, and anything that would be stale in a week belong here \u{2014} \
recalled on demand \u{2014} not memorized into the memory tool.";

/// One matching message, already joined against its parent conversation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchHit {
    pub conversation_id: i64,
    pub conversation_title: String,
    pub message_id: i64,
    pub role: String,
    /// A short excerpt around the match (via FTS5's `snippet()`), not the
    /// full message — a matched message can be long, and the model only
    /// needs enough context to decide whether to look further.
    pub snippet: String,
    /// Unix seconds.
    pub created_at: i64,
}

/// Thread-safe handle to the search index. Cheap to [`Clone`] (an `Arc`
/// around the real connection), the same shape [`crate::chat::ChatStore`]
/// and [`crate::clipboard::ClipboardStore`] already use.
#[derive(Clone)]
pub struct SessionSearchIndex {
    conn: Arc<Mutex<Connection>>,
}

impl SessionSearchIndex {
    /// Open the index against the chat database at `path` (the same path
    /// [`crate::chat::ChatStore::open`] is given), creating the FTS5 table
    /// and sync triggers if they are not already there, and backfilling the
    /// index from any pre-existing `messages` rows (an upgrade from before
    /// this feature shipped, or the very first run). See the module doc.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SearchError> {
        // Guarantee `messages`/`conversations` exist before the triggers
        // below reference them. Idempotent even if a `ChatStore` already
        // opened this exact file — `migrate()` is all `IF NOT EXISTS`.
        crate::chat::ChatStore::open(path.as_ref())?;

        let conn = Connection::open(path.as_ref())?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let index = Self { conn: Arc::new(Mutex::new(conn)) };
        index.migrate()?;
        Ok(index)
    }

    fn migrate(&self) -> Result<(), SearchError> {
        let conn = self.conn.lock();

        // Detect "this is the first time this schema has existed in this
        // database file" BEFORE creating it -- see below for why this, and
        // not a row count, is the right signal for whether a backfill is
        // needed.
        let already_existed: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'messages_fts'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)?;

        // The FTS5 table's columns are declared in the SAME ORDER as
        // `messages`' own columns, excluding `id` (the rowid) --
        // `conversation_id`, `role`, `text`, `created_at`. This is not
        // cosmetic: FTS5 external-content tables map their columns to the
        // content table POSITIONALLY, not by name. Declaring only `text`
        // here (as an earlier version of this module did) would silently
        // make FTS5's column 0 read from `messages`' first non-rowid column
        // -- `conversation_id`, not `text` -- for every read-through path
        // that does not supply values explicitly (`rebuild`, `snippet()`,
        // `integrity-check`). The triggers below always supply every column
        // explicitly, which works either way, but `rebuild` (used for the
        // backfill just below) does not, which is what actually surfaced
        // this. `UNINDEXED` keeps `conversation_id`/`role`/`created_at`
        // out of the full-text index and out of `MATCH`/`bm25()` scoring --
        // only `text` is ever searched or ranked -- while still keeping the
        // column layout aligned so the content table can be read through
        // correctly.
        conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                 conversation_id UNINDEXED,
                 role UNINDEXED,
                 text,
                 created_at UNINDEXED,
                 content='messages',
                 content_rowid='id',
                 tokenize='unicode61'
             );
             CREATE TRIGGER IF NOT EXISTS messages_fts_ai AFTER INSERT ON messages BEGIN
                 INSERT INTO messages_fts(rowid, conversation_id, role, text, created_at)
                 VALUES (new.id, new.conversation_id, new.role, new.text, new.created_at);
             END;
             CREATE TRIGGER IF NOT EXISTS messages_fts_ad AFTER DELETE ON messages BEGIN
                 INSERT INTO messages_fts(messages_fts, rowid, conversation_id, role, text, created_at)
                 VALUES('delete', old.id, old.conversation_id, old.role, old.text, old.created_at);
             END;
             CREATE TRIGGER IF NOT EXISTS messages_fts_au AFTER UPDATE ON messages BEGIN
                 INSERT INTO messages_fts(messages_fts, rowid, conversation_id, role, text, created_at)
                 VALUES('delete', old.id, old.conversation_id, old.role, old.text, old.created_at);
                 INSERT INTO messages_fts(rowid, conversation_id, role, text, created_at)
                 VALUES (new.id, new.conversation_id, new.role, new.text, new.created_at);
             END;",
        )?;

        // Self-healing backfill, first-creation only: an upgrade from before
        // this feature shipped (or, in a test, seeding data via `ChatStore`
        // before ever opening the index) leaves pre-existing `messages` rows
        // the triggers above never saw insert. FTS5's documented `rebuild`
        // maintenance command repopulates an external-content table by
        // re-reading the content table through `content_rowid`.
        //
        // This cannot be gated on a row count the way an ordinary table's
        // "is it empty" check would be: for an EXTERNAL CONTENT table, a
        // plain (non-`MATCH`) `SELECT COUNT(*) FROM messages_fts` reflects
        // row *existence*, which external-content mode defines via the
        // content table (`messages`) itself -- it returns the same count as
        // `messages` regardless of whether the inverted index has actually
        // been populated, since there is no separate row-presence structure
        // to be behind. `already_existed`, captured above before the
        // `CREATE VIRTUAL TABLE`, is the only reliable signal: only a
        // brand-new table needs a backfill, since a table that already
        // existed has been kept in sync by the triggers for as long as it
        // has been receiving writes.
        if !already_existed {
            conn.execute("INSERT INTO messages_fts(messages_fts) VALUES('rebuild')", [])?;
        }
        Ok(())
    }

    /// Rank every message matching `query` by BM25 (best first) and return
    /// the top `limit`. Empty (never an error) when `query` tokenizes to
    /// nothing at all.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SessionSearchHit>, SearchError> {
        let Some(match_expr) = build_match_expression(query) else {
            return Ok(Vec::new());
        };

        let conn = self.conn.lock();
        // snippet()'s column index is 2 -- `text` is the third column
        // declared on `messages_fts` (after `conversation_id`, `role`), per
        // `migrate`'s doc on positional content-table alignment.
        let mut stmt = conn.prepare(
            "SELECT m.id, m.conversation_id, c.title, m.role, m.created_at,
                    snippet(messages_fts, 2, '', '', '\u{2026}', 12) AS snip
             FROM messages_fts
             JOIN messages m ON m.id = messages_fts.rowid
             JOIN conversations c ON c.id = m.conversation_id
             WHERE messages_fts MATCH ?1
             ORDER BY bm25(messages_fts)
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![match_expr, limit as i64], |r| {
            Ok(SessionSearchHit {
                message_id: r.get(0)?,
                conversation_id: r.get(1)?,
                conversation_title: r.get(2)?,
                role: r.get(3)?,
                created_at: r.get(4)?,
                snippet: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

/// Turn free-text `query` into a safe FTS5 `MATCH` expression: every maximal
/// run of alphanumeric characters, quoted and ANDed together. Quoting a
/// token neutralizes FTS5 query-syntax characters (`-`, `:`, `*`, `(`, `)`,
/// `"` cannot appear inside a token since it is itself a split point) so
/// arbitrary user text can never produce a syntax error or an accidental
/// column filter / prefix search. `None` when there are no tokens at all
/// (empty or punctuation-only query) — callers treat that as "no results"
/// rather than sending FTS5 an empty `MATCH ''`, which is itself a syntax
/// error.
fn build_match_expression(query: &str) -> Option<String> {
    let tokens: Vec<String> =
        query.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(|t| format!("\"{t}\"")).collect();
    if tokens.is_empty() {
        return None;
    }
    Some(tokens.join(" AND "))
}

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("session search database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

// ---------------------------------------------------------------------------
// Native tool
// ---------------------------------------------------------------------------

/// Build the `session_search` tool over `index` and register it. Call once,
/// from `lib.rs::setup()` (via [`super::register_native_tools`]).
pub fn register(index: SessionSearchIndex) {
    native_tools::register(NativeTool::new(NAME, DESCRIPTION, schema(), move |args| Ok(handle(&index, args))));
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Search terms \u{2014} matched as words (lexical search), not a natural-language question."
            },
            "limit": {
                "type": "integer",
                "description": "Maximum results to return (default 8, max 25)."
            }
        },
        "required": ["query"]
    })
}

fn handle(index: &SessionSearchIndex, args: Value) -> Value {
    let obj = args.as_object();
    let query = obj.and_then(|o| o.get("query")).and_then(Value::as_str).unwrap_or("").trim().to_string();
    if query.is_empty() {
        return json!({ "success": false, "error": "query is required and cannot be empty." });
    }
    let limit = obj
        .and_then(|o| o.get("limit"))
        .and_then(Value::as_u64)
        .map(|n| n.clamp(1, MAX_RESULTS as u64) as usize)
        .unwrap_or(DEFAULT_RESULTS);

    match index.search(&query, limit) {
        Ok(hits) if hits.is_empty() => json!({
            "success": true,
            "results": [],
            "message": "No past conversations matched.",
        }),
        Ok(hits) => json!({
            "success": true,
            "results": hits,
        }),
        Err(e) => json!({ "success": false, "error": format!("session search failed: {e}") }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{ChatStore, Role};
    use std::path::PathBuf;

    fn temp_db_path() -> PathBuf {
        std::env::temp_dir().join(format!("caduceus-session-search-test-{}.db", uuid::Uuid::new_v4()))
    }

    #[test]
    fn finds_a_message_by_word_match() {
        let path = temp_db_path();
        let chat = ChatStore::open(&path).unwrap();
        let convo = chat.create_conversation().unwrap();
        chat.append(convo, Role::User, "What's the best way to deploy a Rust binary on macOS?").unwrap();
        chat.append(convo, Role::Assistant, "You can notarize and staple it, or distribute via Homebrew.").unwrap();

        let index = SessionSearchIndex::open(&path).unwrap();
        let hits = index.search("notarize", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].conversation_id, convo);
        assert_eq!(hits[0].role, "assistant");
    }

    #[test]
    fn a_query_with_no_matches_is_an_empty_list_not_an_error() {
        let path = temp_db_path();
        let chat = ChatStore::open(&path).unwrap();
        let convo = chat.create_conversation().unwrap();
        chat.append(convo, Role::User, "hello there").unwrap();

        let index = SessionSearchIndex::open(&path).unwrap();
        assert!(index.search("xyzzyplugh", 10).unwrap().is_empty());
    }

    #[test]
    fn pre_existing_history_is_backfilled_when_the_index_is_created_later() {
        // Simulates upgrading an existing chats.db: rows are written via
        // ChatStore alone, well before SessionSearchIndex ever opens this
        // file and installs the triggers.
        let path = temp_db_path();
        let chat = ChatStore::open(&path).unwrap();
        let convo = chat.create_conversation().unwrap();
        chat.append(convo, Role::User, "an older message about kubernetes networking").unwrap();
        drop(chat);

        let index = SessionSearchIndex::open(&path).unwrap();
        let hits = index.search("kubernetes", 10).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn deleting_a_conversation_removes_its_messages_from_the_index() {
        let path = temp_db_path();
        let chat = ChatStore::open(&path).unwrap();
        let convo = chat.create_conversation().unwrap();
        chat.append(convo, Role::User, "a message about quantum computing").unwrap();

        let index = SessionSearchIndex::open(&path).unwrap();
        assert_eq!(index.search("quantum", 10).unwrap().len(), 1);

        chat.delete_conversation(convo).unwrap(); // ON DELETE CASCADE removes the message row too
        assert!(index.search("quantum", 10).unwrap().is_empty());
    }

    #[test]
    fn newly_appended_messages_after_the_index_is_open_are_found_immediately() {
        let path = temp_db_path();
        let chat = ChatStore::open(&path).unwrap();
        let index = SessionSearchIndex::open(&path).unwrap();

        let convo = chat.create_conversation().unwrap();
        chat.append(convo, Role::User, "a brand new message about octopuses").unwrap();

        assert_eq!(index.search("octopuses", 10).unwrap().len(), 1);
    }

    #[test]
    fn results_rank_the_message_with_more_query_term_overlap_first() {
        let path = temp_db_path();
        let chat = ChatStore::open(&path).unwrap();
        let a = chat.create_conversation().unwrap();
        chat.append(a, Role::User, "database migration plan for the analytics warehouse").unwrap();
        let b = chat.create_conversation().unwrap();
        chat.append(b, Role::User, "database migration and database schema and database indexes").unwrap();

        let index = SessionSearchIndex::open(&path).unwrap();
        let hits = index.search("database migration", 10).unwrap();
        assert_eq!(hits.len(), 2);
        // `b` repeats "database" three times -- higher term frequency must
        // rank it first under BM25.
        assert_eq!(hits[0].conversation_id, b);
    }

    // -----------------------------------------------------------------
    // build_match_expression / query safety
    // -----------------------------------------------------------------

    #[test]
    fn tokenizes_and_ands_plain_words() {
        assert_eq!(build_match_expression("hello world").as_deref(), Some("\"hello\" AND \"world\""));
    }

    #[test]
    fn special_fts5_syntax_characters_are_neutralized_by_quoting_each_token() {
        // A raw `-`, `:`, `*`, or unbalanced `"` sent straight to MATCH
        // would either error or change query semantics (negation, column
        // filter, prefix search). Splitting on non-alphanumerics removes
        // every one of them from what actually reaches SQLite.
        let expr = build_match_expression("column:value -exclude* \"quoted\"").unwrap();
        assert!(!expr.contains('-'));
        assert!(!expr.contains(':'));
        assert!(!expr.contains('*'));
        assert_eq!(expr, "\"column\" AND \"value\" AND \"exclude\" AND \"quoted\"");
    }

    #[test]
    fn an_empty_or_punctuation_only_query_has_no_tokens() {
        assert!(build_match_expression("").is_none());
        assert!(build_match_expression("   ").is_none());
        assert!(build_match_expression("---***").is_none());
    }

    #[test]
    fn a_query_with_special_characters_does_not_error_against_a_real_index() {
        let path = temp_db_path();
        let chat = ChatStore::open(&path).unwrap();
        let convo = chat.create_conversation().unwrap();
        chat.append(convo, Role::User, "checking the api-key rotation schedule").unwrap();

        let index = SessionSearchIndex::open(&path).unwrap();
        // Must not error even though it contains FTS5-meaningful characters.
        let hits = index.search("api-key: rotation* (schedule)", 10).unwrap();
        assert_eq!(hits.len(), 1);
    }

    // -----------------------------------------------------------------
    // native tool handler
    // -----------------------------------------------------------------

    #[test]
    fn handle_reports_a_friendly_message_on_no_results() {
        let path = temp_db_path();
        let chat = ChatStore::open(&path).unwrap();
        let convo = chat.create_conversation().unwrap();
        chat.append(convo, Role::User, "something").unwrap();
        let index = SessionSearchIndex::open(&path).unwrap();

        let result = handle(&index, json!({"query": "nothing-matches-this"}));
        assert_eq!(result["success"], true);
        assert_eq!(result["results"], json!([]));
    }

    #[test]
    fn handle_requires_a_non_empty_query() {
        let path = temp_db_path();
        let _chat = ChatStore::open(&path).unwrap();
        let index = SessionSearchIndex::open(&path).unwrap();
        let result = handle(&index, json!({"query": "   "}));
        assert_eq!(result["success"], false);
    }

    #[test]
    fn handle_clamps_an_out_of_range_limit_rather_than_erroring() {
        let path = temp_db_path();
        let chat = ChatStore::open(&path).unwrap();
        let convo = chat.create_conversation().unwrap();
        for i in 0..5 {
            chat.append(convo, Role::User, &format!("repeated widget note number {i}")).unwrap();
        }
        let index = SessionSearchIndex::open(&path).unwrap();
        let result = handle(&index, json!({"query": "widget", "limit": 999}));
        assert_eq!(result["success"], true);
        let results = result["results"].as_array().unwrap();
        // Non-empty is the meaningful assertion here -- `<= MAX_RESULTS`
        // alone would pass even if clamping silently broke the search and
        // returned nothing at all.
        assert_eq!(results.len(), 5);
        assert!(results.len() <= MAX_RESULTS);
    }
}
