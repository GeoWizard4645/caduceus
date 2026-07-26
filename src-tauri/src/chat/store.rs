//! SQLite persistence for `/` conversations.
//!
//! Two tables. `conversations` is the thread list; `messages` holds the turns,
//! with `ON DELETE CASCADE` so deleting a thread cannot leave orphans behind.
//!
//! Kept in its own database rather than sharing `clipboard.db`: clipboard
//! history has a retention policy that prunes aggressively, and conversations
//! should outlive it. Separate files also mean clearing one never risks the
//! other.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

pub const DB_FILE: &str = "chats.db";

/// How much of the first user message becomes the thread's title.
const TITLE_MAX: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "assistant" => Role::Assistant,
            _ => Role::User,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: i64,
    pub role: Role,
    pub text: String,
    /// Unix seconds.
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: i64,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub message_count: i64,
    /// First line of the most recent message, for the thread list.
    pub preview: String,
}

#[derive(Clone)]
pub struct ChatStore {
    conn: Arc<Mutex<Connection>>,
    #[allow(dead_code)]
    path: PathBuf,
}

impl ChatStore {
    pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(&path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            path,
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> rusqlite::Result<()> {
        let conn = self.conn.lock();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS conversations (
                 id         INTEGER PRIMARY KEY AUTOINCREMENT,
                 title      TEXT NOT NULL DEFAULT '',
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS messages (
                 id              INTEGER PRIMARY KEY AUTOINCREMENT,
                 conversation_id INTEGER NOT NULL
                     REFERENCES conversations(id) ON DELETE CASCADE,
                 role            TEXT NOT NULL,
                 text            TEXT NOT NULL,
                 created_at      INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_messages_conversation
                 ON messages(conversation_id, id);
             CREATE INDEX IF NOT EXISTS idx_conversations_updated
                 ON conversations(updated_at DESC);",
        )
    }

    pub fn create_conversation(&self) -> rusqlite::Result<i64> {
        let now = now_secs();
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO conversations (title, created_at, updated_at) VALUES ('', ?1, ?2)",
            params![now, now],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Append a turn, titling the thread from its first user message.
    pub fn append(&self, conversation_id: i64, role: Role, text: &str) -> rusqlite::Result<i64> {
        let now = now_secs();
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO messages (conversation_id, role, text, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![conversation_id, role.as_str(), text, now],
        )?;
        let id = conn.last_insert_rowid();

        // An untitled thread is one nobody can find again in the list.
        let existing: String = conn
            .query_row(
                "SELECT title FROM conversations WHERE id = ?1",
                params![conversation_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_default();

        if existing.is_empty() && role == Role::User {
            conn.execute(
                "UPDATE conversations SET title = ?1, updated_at = ?2 WHERE id = ?3",
                params![title_from(text), now, conversation_id],
            )?;
        } else {
            conn.execute(
                "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
                params![now, conversation_id],
            )?;
        }
        Ok(id)
    }

    pub fn messages(&self, conversation_id: i64) -> rusqlite::Result<Vec<ChatMessage>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, role, text, created_at FROM messages
             WHERE conversation_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![conversation_id], |r| {
            Ok(ChatMessage {
                id: r.get(0)?,
                role: Role::parse(&r.get::<_, String>(1)?),
                text: r.get(2)?,
                created_at: r.get(3)?,
            })
        })?;
        rows.collect()
    }

    pub fn conversations(&self, limit: i64) -> rusqlite::Result<Vec<Conversation>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT c.id, c.title, c.created_at, c.updated_at,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id),
                    COALESCE((SELECT m.text FROM messages m
                              WHERE m.conversation_id = c.id
                              ORDER BY m.id DESC LIMIT 1), '')
             FROM conversations c
             ORDER BY c.updated_at DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| {
            let preview: String = r.get(5)?;
            Ok(Conversation {
                id: r.get(0)?,
                title: r.get(1)?,
                created_at: r.get(2)?,
                updated_at: r.get(3)?,
                message_count: r.get(4)?,
                preview: first_line(&preview),
            })
        })?;
        rows.collect()
    }

    pub fn delete_conversation(&self, id: i64) -> rusqlite::Result<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM conversations WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn clear(&self) -> rusqlite::Result<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM conversations", [])?;
        Ok(())
    }

    /// Drop threads that were opened but never used.
    ///
    /// `/` creates a thread as soon as it is asked a question. If the backend
    /// errors before a reply lands, an empty row is left behind and the list
    /// fills with blank entries.
    pub fn prune_empty(&self) -> rusqlite::Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM conversations
             WHERE id NOT IN (SELECT DISTINCT conversation_id FROM messages)",
            [],
        )?;
        Ok(())
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

fn first_line(s: &str) -> String {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    truncate(line, 120)
}

fn title_from(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    let title = truncate(line, TITLE_MAX);
    if title.is_empty() {
        "New chat".into()
    } else {
        title
    }
}

/// Truncate on a character boundary — `&s[..n]` panics mid-codepoint, and a
/// question typed in any non-Latin script would hit that on the first message.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database of its own per call.
    ///
    /// Tests run as threads in one process, so keying the path on the process id
    /// alone gave every test the same file and they read each other's rows.
    fn store() -> ChatStore {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "caduceus-chat-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        ChatStore::open(dir.join("chats.db")).expect("open")
    }

    #[test]
    fn a_thread_takes_its_title_from_the_first_question() {
        let s = store();
        let id = s.create_conversation().unwrap();
        s.append(id, Role::User, "How does OAuth work?").unwrap();
        s.append(id, Role::Assistant, "It is a delegation protocol.").unwrap();

        let list = s.conversations(10).unwrap();
        assert_eq!(list[0].title, "How does OAuth work?");
        assert_eq!(list[0].message_count, 2);
        // The reply is the newest turn, so it is what the list previews.
        assert_eq!(list[0].preview, "It is a delegation protocol.");
    }

    #[test]
    fn the_reply_does_not_retitle_the_thread() {
        let s = store();
        let id = s.create_conversation().unwrap();
        s.append(id, Role::Assistant, "unprompted").unwrap();
        s.append(id, Role::User, "the real question").unwrap();
        assert_eq!(s.conversations(10).unwrap()[0].title, "the real question");
    }

    #[test]
    fn history_comes_back_in_order() {
        let s = store();
        let id = s.create_conversation().unwrap();
        for i in 0..5 {
            s.append(id, Role::User, &format!("q{i}")).unwrap();
        }
        let msgs = s.messages(id).unwrap();
        assert_eq!(msgs.len(), 5);
        assert_eq!(msgs[0].text, "q0");
        assert_eq!(msgs[4].text, "q4");
    }

    #[test]
    fn deleting_a_thread_takes_its_messages_with_it() {
        let s = store();
        let id = s.create_conversation().unwrap();
        s.append(id, Role::User, "x").unwrap();
        s.delete_conversation(id).unwrap();
        assert!(s.messages(id).unwrap().is_empty());
        assert!(s.conversations(10).unwrap().is_empty());
    }

    #[test]
    fn empty_threads_are_pruned_but_used_ones_survive() {
        let s = store();
        let empty = s.create_conversation().unwrap();
        let used = s.create_conversation().unwrap();
        s.append(used, Role::User, "hello").unwrap();

        s.prune_empty().unwrap();
        let ids: Vec<i64> = s.conversations(10).unwrap().iter().map(|c| c.id).collect();
        assert!(!ids.contains(&empty));
        assert!(ids.contains(&used));
    }

    /// `&s[..n]` on a multi-byte character panics, and a question in any
    /// non-Latin script is long before it is 60 characters.
    #[test]
    fn titles_truncate_on_character_boundaries() {
        let s = store();
        let id = s.create_conversation().unwrap();
        let long = "あ".repeat(200);
        s.append(id, Role::User, &long).unwrap();
        let title = &s.conversations(10).unwrap()[0].title;
        assert!(title.chars().count() <= TITLE_MAX);
        assert!(title.ends_with('…'));
    }
}
