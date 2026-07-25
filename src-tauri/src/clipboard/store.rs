//! SQLite persistence for clipboard history.
//!
//! One table, `entries`, holding text / image / file-list records. Content and
//! preview columns are stored either as plaintext or as AEAD records depending
//! on the `encrypted` flag, which is **per row** rather than global — that is
//! what makes the encryption toggle safe to flip at any time: the re-encryption
//! pass rewrites rows one at a time and can be interrupted without corrupting
//! anything.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::crypto::{self, KEY_LEN};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Text,
    Image,
    Files,
}

impl EntryKind {
    fn as_str(self) -> &'static str {
        match self {
            EntryKind::Text => "text",
            EntryKind::Image => "image",
            EntryKind::Files => "files",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "image" => EntryKind::Image,
            "files" => EntryKind::Files,
            _ => EntryKind::Text,
        }
    }
}

/// A history row as sent to the frontend.
///
/// `content` is omitted for images (they can be megabytes); the UI shows
/// `thumbnail` and fetches full bytes only when the entry is actually used.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardEntry {
    pub id: i64,
    pub kind: EntryKind,
    /// One-line preview, truncated for display.
    pub preview: String,
    /// Full text for `Text` and `Files` entries; `None` for images.
    pub content: Option<String>,
    /// `data:image/png;base64,…` thumbnail for `Image` entries.
    pub thumbnail: Option<String>,
    pub byte_len: i64,
    pub source_app: Option<String>,
    pub pinned: bool,
    /// Unix milliseconds.
    pub created_at: i64,
    /// True when the row could not be decrypted (key rotated or lost).
    pub unreadable: bool,
    /// Image pixel dimensions, when known.
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// A record on its way *into* the store.
pub struct NewEntry {
    pub kind: EntryKind,
    /// Raw bytes: UTF-8 text, PNG image data, or newline-separated paths.
    pub content: Vec<u8>,
    pub preview: String,
    /// Stable digest of `content`, used for de-duplication.
    pub hash: String,
    pub source_app: Option<String>,
    /// PNG thumbnail bytes for images.
    pub thumbnail: Option<Vec<u8>>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Thread-safe handle to the history database.
///
/// A single connection behind a mutex: writes come from one watcher thread and
/// reads from IPC handlers, at human speed. Connection pooling would be
/// complexity without a workload to justify it.
#[derive(Clone)]
pub struct ClipboardStore {
    conn: Arc<Mutex<Connection>>,
    path: PathBuf,
}

impl ClipboardStore {
    pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(&path)?;

        // WAL keeps the watcher's writes from blocking palette reads.
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

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn migrate(&self) -> rusqlite::Result<()> {
        let conn = self.conn.lock();
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS entries (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                kind        TEXT    NOT NULL,
                content     BLOB    NOT NULL,
                preview     BLOB    NOT NULL,
                thumbnail   BLOB,
                encrypted   INTEGER NOT NULL DEFAULT 0,
                hash        TEXT    NOT NULL,
                byte_len    INTEGER NOT NULL,
                width       INTEGER,
                height      INTEGER,
                source_app  TEXT,
                pinned      INTEGER NOT NULL DEFAULT 0,
                created_at  INTEGER NOT NULL,
                -- Recency ordering key, incremented on every insert *and* every
                -- bump. `created_at` cannot be used for this: it has
                -- millisecond resolution, and copying two things inside the
                -- same millisecond is entirely normal (a script, a paste
                -- manager, or just a fast double-copy), which would make the
                -- history order non-deterministic.
                seq         INTEGER NOT NULL DEFAULT 0
            );

            CREATE INDEX IF NOT EXISTS idx_entries_seq      ON entries (seq DESC);
            CREATE INDEX IF NOT EXISTS idx_entries_created  ON entries (created_at);
            CREATE INDEX IF NOT EXISTS idx_entries_hash     ON entries (hash);
            CREATE INDEX IF NOT EXISTS idx_entries_pinned   ON entries (pinned, seq DESC);

            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )?;

        // Databases created before `seq` existed. `ALTER TABLE ADD COLUMN`
        // errors if the column is already there, which is the common case, so
        // the result is deliberately discarded.
        if conn
            .execute("ALTER TABLE entries ADD COLUMN seq INTEGER NOT NULL DEFAULT 0", [])
            .is_ok()
        {
            // Seed the new column from insertion order so existing history
            // keeps the ordering it had.
            conn.execute("UPDATE entries SET seq = id WHERE seq = 0", [])?;
        }
        Ok(())
    }

    /// Next value for the recency ordering key.
    fn next_seq(conn: &Connection) -> rusqlite::Result<i64> {
        conn.query_row("SELECT COALESCE(MAX(seq), 0) + 1 FROM entries", [], |r| r.get(0))
    }

    // -----------------------------------------------------------------------
    // Writes
    // -----------------------------------------------------------------------

    /// Insert a new entry, or bump an existing identical one to the top.
    ///
    /// Returns the row id, and whether it was a fresh insert.
    pub fn insert(
        &self,
        entry: NewEntry,
        key: Option<&[u8; KEY_LEN]>,
    ) -> Result<(i64, bool), StoreError> {
        let now = chrono::Utc::now().timestamp_millis();
        let conn = self.conn.lock();

        // Re-copying something you copied before should move it to the top of
        // the list, not create a duplicate row.
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM entries WHERE hash = ?1 ORDER BY seq DESC LIMIT 1",
                params![&entry.hash],
                |r| r.get(0),
            )
            .optional()?;

        if let Some(id) = existing {
            let seq = Self::next_seq(&conn)?;
            conn.execute(
                "UPDATE entries SET created_at = ?1, seq = ?2, source_app = COALESCE(?3, source_app)
                 WHERE id = ?4",
                params![now, seq, entry.source_app, id],
            )?;
            return Ok((id, false));
        }

        let byte_len = entry.content.len() as i64;
        let (content, preview, encrypted) = match key {
            Some(k) => (
                crypto::encrypt(k, &entry.content)?,
                crypto::encrypt_str(k, &entry.preview)?,
                1,
            ),
            None => (entry.content, entry.preview.into_bytes(), 0),
        };

        let seq = Self::next_seq(&conn)?;
        conn.execute(
            "INSERT INTO entries
                (kind, content, preview, thumbnail, encrypted, hash, byte_len, width, height, source_app, pinned, created_at, seq)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, ?11, ?12)",
            params![
                entry.kind.as_str(),
                content,
                preview,
                entry.thumbnail,
                encrypted,
                entry.hash,
                byte_len,
                entry.width,
                entry.height,
                entry.source_app,
                now,
                seq,
            ],
        )?;
        Ok((conn.last_insert_rowid(), true))
    }

    pub fn set_pinned(&self, id: i64, pinned: bool) -> Result<(), StoreError> {
        self.conn.lock().execute(
            "UPDATE entries SET pinned = ?1 WHERE id = ?2",
            params![pinned as i32, id],
        )?;
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<(), StoreError> {
        self.conn
            .lock()
            .execute("DELETE FROM entries WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Delete everything. `keep_pinned` protects favourites.
    pub fn clear(&self, keep_pinned: bool) -> Result<usize, StoreError> {
        let conn = self.conn.lock();
        let n = if keep_pinned {
            conn.execute("DELETE FROM entries WHERE pinned = 0", [])?
        } else {
            conn.execute("DELETE FROM entries", [])?
        };
        Ok(n)
    }

    /// Enforce the retention policy. Pinned entries are exempt from both rules.
    pub fn prune(&self, max_items: usize, max_age_days: Option<u32>) -> Result<usize, StoreError> {
        let conn = self.conn.lock();
        let mut removed = 0;

        if let Some(days) = max_age_days {
            let cutoff = chrono::Utc::now().timestamp_millis() - (days as i64 * 86_400_000);
            removed += conn.execute(
                "DELETE FROM entries WHERE pinned = 0 AND created_at < ?1",
                params![cutoff],
            )?;
        }

        removed += conn.execute(
            "DELETE FROM entries
             WHERE pinned = 0
               AND id NOT IN (
                   SELECT id FROM entries WHERE pinned = 0
                   ORDER BY seq DESC LIMIT ?1
               )",
            params![max_items as i64],
        )?;

        Ok(removed)
    }

    // -----------------------------------------------------------------------
    // Reads
    // -----------------------------------------------------------------------

    /// List entries, newest first, pinned entries always on top.
    ///
    /// `query` is matched against the preview: every whitespace-separated token
    /// must appear (case-insensitive substring). When rows are encrypted the
    /// match necessarily happens in memory after decryption, so the scan is
    /// capped — see [`ENCRYPTED_SCAN_LIMIT`].
    pub fn list(
        &self,
        query: &str,
        limit: usize,
        pinned_only: bool,
        key: Option<&[u8; KEY_LEN]>,
    ) -> Result<Vec<ClipboardEntry>, StoreError> {
        let conn = self.conn.lock();
        let tokens: Vec<String> = query
            .split_whitespace()
            .map(|t| t.to_lowercase())
            .filter(|t| !t.is_empty())
            .collect();

        let scan_limit = if key.is_some() {
            ENCRYPTED_SCAN_LIMIT
        } else {
            // Unencrypted rows are filtered by SQL, so we only need `limit`
            // rows back — but ask for a few extra in case of ties.
            limit.saturating_mul(2).max(limit + 16)
        };

        // With plaintext rows we can push the filter into SQL.
        let (sql, sql_params): (String, Vec<Box<dyn rusqlite::ToSql>>) =
            if key.is_none() && !tokens.is_empty() {
                let mut where_parts = vec!["encrypted = 0".to_string()];
                let mut p: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
                for (i, t) in tokens.iter().enumerate() {
                    where_parts.push(format!("LOWER(CAST(preview AS TEXT)) LIKE ?{}", i + 1));
                    p.push(Box::new(format!("%{t}%")));
                }
                if pinned_only {
                    where_parts.push("pinned = 1".into());
                }
                p.push(Box::new(scan_limit as i64));
                let idx = tokens.len() + 1;
                (
                    format!(
                        "SELECT id, kind, content, preview, thumbnail, encrypted, byte_len, width, height, source_app, pinned, created_at
                         FROM entries WHERE {} ORDER BY pinned DESC, seq DESC LIMIT ?{idx}",
                        where_parts.join(" AND ")
                    ),
                    p,
                )
            } else {
                let mut where_clause = String::from("1 = 1");
                if pinned_only {
                    where_clause.push_str(" AND pinned = 1");
                }
                (
                    format!(
                        "SELECT id, kind, content, preview, thumbnail, encrypted, byte_len, width, height, source_app, pinned, created_at
                         FROM entries WHERE {where_clause} ORDER BY pinned DESC, seq DESC LIMIT ?1"
                    ),
                    vec![Box::new(scan_limit as i64)],
                )
            };

        let mut stmt = conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> = sql_params.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(refs.as_slice(), |row| Ok(RawRow::from_row(row)))?;

        let mut out = Vec::with_capacity(limit);
        for raw in rows {
            let raw = raw?;
            let entry = raw.decode(key);
            // Encrypted rows (and any row when the query could not be pushed
            // into SQL) are filtered here.
            if !tokens.is_empty() && !entry.unreadable {
                let hay = entry.preview.to_lowercase();
                if !tokens.iter().all(|t| hay.contains(t.as_str())) {
                    continue;
                }
            } else if !tokens.is_empty() && entry.unreadable {
                continue;
            }
            out.push(entry);
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    /// Fetch one entry's full content, decrypting if needed.
    pub fn get_content(
        &self,
        id: i64,
        key: Option<&[u8; KEY_LEN]>,
    ) -> Result<Option<(EntryKind, Vec<u8>)>, StoreError> {
        let conn = self.conn.lock();
        let row: Option<(String, Vec<u8>, i64)> = conn
            .query_row(
                "SELECT kind, content, encrypted FROM entries WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;

        let Some((kind, blob, encrypted)) = row else {
            return Ok(None);
        };

        let bytes = if encrypted == 1 {
            let k = key.ok_or(StoreError::KeyRequired)?;
            crypto::decrypt(k, &blob)?
        } else {
            blob
        };
        Ok(Some((EntryKind::parse(&kind), bytes)))
    }

    pub fn count(&self) -> Result<i64, StoreError> {
        Ok(self
            .conn
            .lock()
            .query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))?)
    }

    /// Total size of stored content, for the Settings "using N MB" line.
    pub fn total_bytes(&self) -> Result<i64, StoreError> {
        Ok(self.conn.lock().query_row(
            "SELECT COALESCE(SUM(LENGTH(content) + COALESCE(LENGTH(thumbnail), 0)), 0) FROM entries",
            [],
            |r| r.get(0),
        )?)
    }

    /// Most recent entry's hash, so the watcher can skip work when nothing has
    /// changed across a restart.
    pub fn latest_hash(&self) -> Option<String> {
        self.conn
            .lock()
            .query_row(
                "SELECT hash FROM entries ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .ok()
    }

    // -----------------------------------------------------------------------
    // Encryption transitions
    // -----------------------------------------------------------------------

    /// Rewrite every row so it matches the requested encryption state.
    ///
    /// Called when the user flips the encryption toggle. Runs row-by-row in
    /// small transactions so a crash mid-pass leaves a mix of encrypted and
    /// plaintext rows — which is fine, because `encrypted` is per row.
    ///
    /// Rows that cannot be decrypted (key was lost or rotated) are **deleted**,
    /// because they are permanently unreadable and keeping them would mean the
    /// list is full of error rows forever. The count is reported back so the UI
    /// can say so out loud.
    pub fn transition_encryption(
        &self,
        key: Option<&[u8; KEY_LEN]>,
        old_key: Option<&[u8; KEY_LEN]>,
    ) -> Result<TransitionReport, StoreError> {
        let mut report = TransitionReport::default();
        let conn = self.conn.lock();

        let ids: Vec<(i64, i64)> = {
            let mut stmt = conn.prepare("SELECT id, encrypted FROM entries ORDER BY id")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };

        let target_encrypted = key.is_some() as i64;

        for (id, encrypted) in ids {
            if encrypted == target_encrypted {
                report.skipped += 1;
                continue;
            }

            let (content, preview): (Vec<u8>, Vec<u8>) = conn.query_row(
                "SELECT content, preview FROM entries WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;

            // Step 1: get back to plaintext.
            let (plain_content, plain_preview) = if encrypted == 1 {
                let Some(k) = old_key else {
                    report.dropped += 1;
                    conn.execute("DELETE FROM entries WHERE id = ?1", params![id])?;
                    continue;
                };
                match (crypto::decrypt(k, &content), crypto::decrypt(k, &preview)) {
                    (Ok(c), Ok(p)) => (c, p),
                    _ => {
                        report.dropped += 1;
                        conn.execute("DELETE FROM entries WHERE id = ?1", params![id])?;
                        continue;
                    }
                }
            } else {
                (content, preview)
            };

            // Step 2: write it back in the requested form.
            let (next_content, next_preview) = match key {
                Some(k) => (
                    crypto::encrypt(k, &plain_content)?,
                    crypto::encrypt(k, &plain_preview)?,
                ),
                None => (plain_content, plain_preview),
            };

            conn.execute(
                "UPDATE entries SET content = ?1, preview = ?2, encrypted = ?3 WHERE id = ?4",
                params![next_content, next_preview, target_encrypted, id],
            )?;
            report.converted += 1;
        }

        // Reclaim the space the rewritten blobs left behind.
        let _ = conn.execute_batch("VACUUM");
        Ok(report)
    }
}

/// How many rows an encrypted search will decrypt before giving up.
///
/// Encrypted previews cannot be filtered by SQL, so search has to decrypt as it
/// scans. 5,000 rows is a few milliseconds of ChaCha20 and comfortably above
/// the default 500-item retention.
pub const ENCRYPTED_SCAN_LIMIT: usize = 5_000;

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitionReport {
    pub converted: usize,
    pub skipped: usize,
    /// Rows deleted because they could not be decrypted with the available key.
    pub dropped: usize,
}

// ---------------------------------------------------------------------------
// Row decoding
// ---------------------------------------------------------------------------

struct RawRow {
    id: i64,
    kind: String,
    content: Vec<u8>,
    preview: Vec<u8>,
    thumbnail: Option<Vec<u8>>,
    encrypted: i64,
    byte_len: i64,
    width: Option<u32>,
    height: Option<u32>,
    source_app: Option<String>,
    pinned: i64,
    created_at: i64,
}

impl RawRow {
    fn from_row(row: &rusqlite::Row<'_>) -> Self {
        Self {
            id: row.get(0).unwrap_or_default(),
            kind: row.get(1).unwrap_or_else(|_| "text".into()),
            content: row.get(2).unwrap_or_default(),
            preview: row.get(3).unwrap_or_default(),
            thumbnail: row.get(4).ok().flatten(),
            encrypted: row.get(5).unwrap_or(0),
            byte_len: row.get(6).unwrap_or(0),
            width: row.get(7).ok().flatten(),
            height: row.get(8).ok().flatten(),
            source_app: row.get(9).ok().flatten(),
            pinned: row.get(10).unwrap_or(0),
            created_at: row.get(11).unwrap_or(0),
        }
    }

    fn decode(self, key: Option<&[u8; KEY_LEN]>) -> ClipboardEntry {
        let kind = EntryKind::parse(&self.kind);
        let mut unreadable = false;

        let preview = if self.encrypted == 1 {
            match key.and_then(|k| crypto::decrypt_str(k, &self.preview).ok()) {
                Some(p) => p,
                None => {
                    unreadable = true;
                    "\u{1f512} Encrypted \u{2014} key unavailable".to_string()
                }
            }
        } else {
            String::from_utf8_lossy(&self.preview).into_owned()
        };

        // Images can be megabytes; the list only ever carries the thumbnail.
        let content = if unreadable || kind == EntryKind::Image {
            None
        } else if self.encrypted == 1 {
            key.and_then(|k| crypto::decrypt_str(k, &self.content).ok())
        } else {
            Some(String::from_utf8_lossy(&self.content).into_owned())
        };

        let thumbnail = self.thumbnail.as_ref().map(|bytes| {
            use base64::Engine as _;
            format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(bytes)
            )
        });

        ClipboardEntry {
            id: self.id,
            kind,
            preview,
            content,
            thumbnail,
            byte_len: self.byte_len,
            source_app: self.source_app,
            pinned: self.pinned == 1,
            created_at: self.created_at,
            unreadable,
            width: self.width,
            height: self.height,
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("clipboard database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Crypto(#[from] crypto::CryptoError),
    #[error("this entry is encrypted but no key is available")]
    KeyRequired,
}

impl From<StoreError> for String {
    fn from(e: StoreError) -> Self {
        e.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> ClipboardStore {
        // A per-test file in the OS temp dir; `:memory:` would not exercise the
        // WAL pragmas or VACUUM.
        let p = std::env::temp_dir().join(format!("caduceus-test-{}.db", uuid::Uuid::new_v4()));
        ClipboardStore::open(p).unwrap()
    }

    fn text(body: &str) -> NewEntry {
        NewEntry {
            kind: EntryKind::Text,
            content: body.as_bytes().to_vec(),
            preview: body.to_string(),
            hash: format!("h:{body}"),
            source_app: None,
            thumbnail: None,
            width: None,
            height: None,
        }
    }

    #[test]
    fn inserts_and_lists_newest_first() {
        let s = store();
        s.insert(text("one"), None).unwrap();
        s.insert(text("two"), None).unwrap();
        let list = s.list("", 10, false, None).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].preview, "two");
    }

    #[test]
    fn recopying_bumps_instead_of_duplicating() {
        let s = store();
        let (first, inserted) = s.insert(text("dup"), None).unwrap();
        assert!(inserted);
        s.insert(text("other"), None).unwrap();
        let (again, inserted) = s.insert(text("dup"), None).unwrap();
        assert!(!inserted, "identical content must not create a second row");
        assert_eq!(first, again);
        assert_eq!(s.count().unwrap(), 2);
        assert_eq!(s.list("", 10, false, None).unwrap()[0].preview, "dup");
    }

    #[test]
    fn search_requires_every_token() {
        let s = store();
        s.insert(text("alpha beta gamma"), None).unwrap();
        s.insert(text("alpha only"), None).unwrap();
        assert_eq!(s.list("alpha", 10, false, None).unwrap().len(), 2);
        assert_eq!(s.list("alpha gamma", 10, false, None).unwrap().len(), 1);
        assert_eq!(s.list("nothing", 10, false, None).unwrap().len(), 0);
    }

    #[test]
    fn search_is_case_insensitive() {
        let s = store();
        s.insert(text("Hello World"), None).unwrap();
        assert_eq!(s.list("hello", 10, false, None).unwrap().len(), 1);
        assert_eq!(s.list("WORLD", 10, false, None).unwrap().len(), 1);
    }

    #[test]
    fn pinned_entries_sort_first_and_survive_pruning() {
        let s = store();
        let (pinned_id, _) = s.insert(text("keep me"), None).unwrap();
        s.set_pinned(pinned_id, true).unwrap();
        for i in 0..10 {
            s.insert(text(&format!("noise {i}")), None).unwrap();
        }
        assert_eq!(s.list("", 20, false, None).unwrap()[0].preview, "keep me");

        s.prune(3, None).unwrap();
        let remaining = s.list("", 20, false, None).unwrap();
        assert!(remaining.iter().any(|e| e.preview == "keep me"));
        assert_eq!(remaining.iter().filter(|e| !e.pinned).count(), 3);
    }

    #[test]
    fn encryption_round_trips_through_the_store() {
        let key = [3u8; KEY_LEN];
        let s = store();
        s.insert(text("classified"), Some(&key)).unwrap();

        // Correct key: readable.
        let list = s.list("", 10, false, Some(&key)).unwrap();
        assert_eq!(list[0].preview, "classified");
        assert!(!list[0].unreadable);

        // No key: present but explicitly unreadable, never silently blank.
        let list = s.list("", 10, false, None).unwrap();
        assert!(list[0].unreadable);
        assert!(!list[0].preview.contains("classified"));
    }

    #[test]
    fn encrypted_search_matches_after_decryption() {
        let key = [4u8; KEY_LEN];
        let s = store();
        s.insert(text("needle in here"), Some(&key)).unwrap();
        s.insert(text("unrelated"), Some(&key)).unwrap();
        assert_eq!(s.list("needle", 10, false, Some(&key)).unwrap().len(), 1);
    }

    #[test]
    fn transition_encrypts_existing_plaintext_history() {
        let key = [5u8; KEY_LEN];
        let s = store();
        s.insert(text("was plaintext"), None).unwrap();

        let report = s.transition_encryption(Some(&key), None).unwrap();
        assert_eq!(report.converted, 1);
        assert_eq!(report.dropped, 0);

        assert_eq!(s.list("", 10, false, Some(&key)).unwrap()[0].preview, "was plaintext");
        assert!(s.list("", 10, false, None).unwrap()[0].unreadable);
    }

    #[test]
    fn transition_decrypts_back_to_plaintext() {
        let key = [6u8; KEY_LEN];
        let s = store();
        s.insert(text("secret"), Some(&key)).unwrap();

        let report = s.transition_encryption(None, Some(&key)).unwrap();
        assert_eq!(report.converted, 1);
        assert_eq!(s.list("", 10, false, None).unwrap()[0].preview, "secret");
    }

    #[test]
    fn transition_drops_rows_whose_key_is_gone() {
        let s = store();
        s.insert(text("lost"), Some(&[7u8; KEY_LEN])).unwrap();
        // Simulate a keychain reset: decrypting is attempted with no old key.
        let report = s.transition_encryption(None, None).unwrap();
        assert_eq!(report.dropped, 1);
        assert_eq!(s.count().unwrap(), 0);
    }

    #[test]
    fn clear_can_spare_pinned_entries() {
        let s = store();
        let (id, _) = s.insert(text("pinned"), None).unwrap();
        s.set_pinned(id, true).unwrap();
        s.insert(text("transient"), None).unwrap();
        s.clear(true).unwrap();
        assert_eq!(s.count().unwrap(), 1);
        s.clear(false).unwrap();
        assert_eq!(s.count().unwrap(), 0);
    }

    #[test]
    fn age_based_pruning_uses_the_cutoff() {
        let s = store();
        let (old_id, _) = s.insert(text("ancient"), None).unwrap();
        // Backdate it 10 days.
        {
            let conn = s.conn.lock();
            let ts = chrono::Utc::now().timestamp_millis() - 10 * 86_400_000;
            conn.execute(
                "UPDATE entries SET created_at = ?1 WHERE id = ?2",
                params![ts, old_id],
            )
            .unwrap();
        }
        s.insert(text("fresh"), None).unwrap();
        s.prune(1000, Some(7)).unwrap();
        let list = s.list("", 10, false, None).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].preview, "fresh");
    }
}
