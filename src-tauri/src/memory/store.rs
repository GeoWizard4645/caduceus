//! The bounded, file-backed memory store — `MEMORY.md` and `USER.md`.
//!
//! This is **not** a knowledge graph and **not** a vector store. It is two
//! flat, human-readable markdown files — one the agent's own notes
//! (environment facts, project conventions, tool quirks), the other what it
//! has learned about the user (preferences, communication style, workflow
//! habits) — each holding a small list of short entries joined by a literal
//! delimiter ([`ENTRY_DELIMITER`]). Deliberately greppable and editable by a
//! human in any text editor (or Obsidian, since it is just markdown) rather
//! than opaque binary state.
//!
//! # The budget *is* the pruning mechanism
//!
//! Each file has a hard character cap (2,200 for memory, 1,375 for user —
//! matching the reference implementation's defaults, both configurable via
//! [`MemoryStore::open`]'s parameters). There is no scoring, no decay, no
//! LRU eviction: a write that would push the file over budget is simply
//! **rejected** ([`MemoryError::OverBudget`]), with the current entries
//! handed back so the caller (an LLM tool-calling loop, via
//! `memory::tool::MemoryTool`) can consolidate — replace two overlapping
//! entries with one shorter one, or remove something stale — and retry. This
//! mirrors the reference implementation's `MemoryStore` exactly: the cap
//! forces the agent to curate its own notes rather than letting them grow
//! without bound, and there is no mechanism here that silently drops an
//! entry on the file's behalf. Anything that leaves the file must be an
//! explicit `remove`.
//!
//! # Why the whole file is injected into the system prompt
//!
//! Because the budget keeps each file small (low thousands of characters),
//! it is cheap to inject *whole* rather than searched or summarized — see
//! [`MemoryStore::snapshot_block`], which renders a live "budget banner"
//! header (`MEMORY (your personal notes) [67% — 1,474/2,200 chars]`) the
//! model can see on every turn. Searching past *conversations* (as opposed
//! to durable facts) is a different, unbounded corpus and is handled by
//! `memory::session_search` instead, over SQLite FTS5 — not by growing this
//! store past its cap.
//!
//! # Atomicity and concurrency
//!
//! Every write goes through [`write_atomic`]: content is written to a
//! sibling temp file (unique name via [`uuid`], so nothing here depends on
//! a single-writer assumption) and then renamed into place.
//! `rename(2)`/`MoveFileEx` is atomic on the same filesystem, so a reader —
//! including a fresh [`MemoryStore::open`] from a concurrently-running
//! Caduceus process — always sees either the complete previous file or the
//! complete new one, never a half-written truncation. Within one process,
//! [`MemoryStore`]'s [`parking_lot::Mutex`] additionally serializes callers
//! so two tool calls landing in the same tick cannot race each other's
//! read-modify-write.
//!
//! # Exact-duplicate writes no-op
//!
//! `add`-ing content that is already present (byte-for-byte, after
//! trimming) is a cheap success rather than a second copy — see the
//! `duplicate` flag on [`WriteReport`]. This is what makes it safe for a
//! model to re-save a fact it is not sure it already recorded.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// Joins entries on disk. A literal `§` (section sign) between newlines —
/// chosen (matching the reference implementation) because it is vanishingly
/// unlikely to occur inside a genuine memory entry, so splitting on it does
/// not need escaping, and the surrounding newlines keep the raw `.md` file
/// readable rather than one long line.
pub const ENTRY_DELIMITER: &str = "\n\u{a7}\n";

/// Default whole-file character budget for `MEMORY.md`. Matches the
/// reference implementation's default exactly; override via
/// [`MemoryStore::open`].
pub const DEFAULT_MEMORY_CHAR_LIMIT: usize = 2200;

/// Default whole-file character budget for `USER.md`.
pub const DEFAULT_USER_CHAR_LIMIT: usize = 1375;

/// Separator rule drawn above each [`MemoryStore::snapshot_block`] header —
/// matches the reference implementation's `"═" * 46` exactly, so anyone who
/// has seen its output recognizes this one.
const BANNER_RULE_CHAR: char = '\u{2550}';
const BANNER_RULE_LEN: usize = 46;

/// Which of the two files an operation addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// The agent's own notes: environment facts, project conventions, tool
    /// quirks, lessons learned. Never task progress — see the `memory` tool's
    /// description for the full "what belongs here" guidance.
    Memory,
    /// What the agent knows about the user: preferences, communication
    /// style, corrections, workflow habits.
    User,
}

impl Target {
    /// Parse the wire-format string a tool call or command argument uses.
    /// `None` for anything else — callers turn that into a validation error
    /// naming the two legal values, rather than silently defaulting.
    pub fn parse(s: &str) -> Option<Target> {
        match s {
            "memory" => Some(Target::Memory),
            "user" => Some(Target::User),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Target::Memory => "memory",
            Target::User => "user",
        }
    }

    fn filename(self) -> &'static str {
        match self {
            Target::Memory => "MEMORY.md",
            Target::User => "USER.md",
        }
    }

    /// The label a [`MemoryStore::snapshot_block`] header opens with —
    /// verbatim from the reference implementation, since a model that has
    /// seen Hermes' own memory blocks should read Caduceus's the same way.
    fn label(self) -> &'static str {
        match self {
            Target::Memory => "MEMORY (your personal notes)",
            Target::User => "USER PROFILE (who the user is)",
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One operation in a [`MemoryStore::apply_batch`] call.
#[derive(Debug, Clone)]
pub struct Operation {
    pub action: OpAction,
    /// Entry text for `Add`/`Replace`.
    pub content: Option<String>,
    /// Substring identifying the target entry for `Replace`/`Remove`.
    pub old_text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpAction {
    Add,
    Replace,
    Remove,
}

/// Character-budget usage for one target, as of the moment it was computed —
/// not live, so a caller holding one across a later write should treat it as
/// a snapshot.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub current: usize,
    pub limit: usize,
    /// 0-100, saturating — a batch that briefly overshoots during validation
    /// never escapes this struct (see [`MemoryError::OverBudget`]), so this
    /// is always a percentage of a committed, in-budget state.
    pub percent: u8,
    pub entry_count: usize,
}

impl Usage {
    fn compute(entries: &[String], limit: usize) -> Self {
        let current = char_count(entries);
        let percent = if limit == 0 {
            0
        } else {
            ((current as f64 / limit as f64) * 100.0).clamp(0.0, 100.0) as u8
        };
        Usage { current, limit, percent, entry_count: entries.len() }
    }
}

/// What a successful [`MemoryStore::add`]/`replace`/`remove`/`apply_batch`
/// call returns.
#[derive(Debug, Clone)]
pub struct WriteReport {
    pub usage: Usage,
    /// True when `add` found the content already present and skipped
    /// writing a duplicate — still `success` from the caller's point of
    /// view (the fact IS in memory now, whichever call put it there).
    pub duplicate: bool,
    pub message: String,
}

/// Everything that can go wrong with a mutation. Every variant that a model
/// could plausibly cause carries enough state (`entries`, `usage`,
/// `previews`) for `memory::tool::MemoryTool` to build the same
/// "here's what's there now, consolidate and retry" response the reference
/// implementation gives — this is the mechanism described in the module
/// doc, not an afterthought bolted onto a plain error string.
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("content cannot be empty")]
    EmptyContent,
    #[error("old_text cannot be empty")]
    EmptyOldText,
    #[error("new content cannot be empty — use 'remove' to delete entries")]
    EmptyReplacement,
    #[error("operations list is empty")]
    EmptyBatch,
    #[error("adding this entry would put {target} over its character budget")]
    OverBudget { target: Target, usage: Usage, entries: Vec<String>, attempted_chars: usize },
    #[error("no entry matched {old_text:?}")]
    NoMatch { old_text: String, usage: Usage, entries: Vec<String> },
    #[error("{old_text:?} matched multiple distinct entries — be more specific")]
    AmbiguousMatch { old_text: String, previews: Vec<String> },
    #[error("operation {index}: {reason}")]
    BatchOperation { index: usize, reason: String, usage: Usage, entries: Vec<String> },
    #[error("memory file I/O error: {0}")]
    Io(#[from] io::Error),
}

/// The two-file store, cheaply [`Clone`]-able (an `Arc` around the real
/// state) so it can be handed to a Tauri-managed-state clone, a
/// `native_tools` handler closure, and the background nudge review all at
/// once — the same shape `clipboard::ClipboardStore` and `chat::ChatStore`
/// already use for the identical reason.
#[derive(Clone)]
pub struct MemoryStore(Arc<Mutex<Inner>>);

struct Inner {
    dir: PathBuf,
    memory_entries: Vec<String>,
    user_entries: Vec<String>,
    memory_char_limit: usize,
    user_char_limit: usize,
}

impl Inner {
    fn limit(&self, target: Target) -> usize {
        match target {
            Target::Memory => self.memory_char_limit,
            Target::User => self.user_char_limit,
        }
    }

    fn entries(&self, target: Target) -> &Vec<String> {
        match target {
            Target::Memory => &self.memory_entries,
            Target::User => &self.user_entries,
        }
    }

    fn set_entries(&mut self, target: Target, entries: Vec<String>) {
        match target {
            Target::Memory => self.memory_entries = entries,
            Target::User => self.user_entries = entries,
        }
    }

    fn path(&self, target: Target) -> PathBuf {
        self.dir.join(target.filename())
    }
}

impl MemoryStore {
    /// Open (creating if needed) the two files under `dir`. Entries are
    /// loaded and de-duplicated (first occurrence wins, order preserved) —
    /// the same "tolerate a hand-edited file with an accidental repeat"
    /// behaviour the reference implementation gives, rather than treating a
    /// duplicate on disk as an error.
    pub fn open(dir: impl AsRef<Path>, memory_char_limit: usize, user_char_limit: usize) -> io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        let memory_entries = read_entries(&dir.join(Target::Memory.filename()))?;
        let user_entries = read_entries(&dir.join(Target::User.filename()))?;
        Ok(Self(Arc::new(Mutex::new(Inner {
            dir,
            memory_entries,
            user_entries,
            memory_char_limit,
            user_char_limit,
        }))))
    }

    /// The directory both files live in — exposed for diagnostics/tests, not
    /// something a caller should write to directly.
    pub fn dir(&self) -> PathBuf {
        self.0.lock().dir.clone()
    }

    /// Every entry currently held for `target`, in file order. For display
    /// (a future Settings panel) or building a tool-error's `current_entries`
    /// list — mutating this `Vec` has no effect on the store.
    pub fn entries(&self, target: Target) -> Vec<String> {
        self.0.lock().entries(target).clone()
    }

    pub fn usage(&self, target: Target) -> Usage {
        let guard = self.0.lock();
        Usage::compute(guard.entries(target), guard.limit(target))
    }

    /// The system-prompt-ready block for `target`: a banner rule, the
    /// `[NN% — current/limit chars]` header, another rule, then every entry
    /// joined by [`ENTRY_DELIMITER`]. `None` when there is nothing to say —
    /// an empty store contributes no block at all, rather than an
    /// almost-empty one that just wastes tokens announcing "0 entries."
    pub fn snapshot_block(&self, target: Target) -> Option<String> {
        let guard = self.0.lock();
        let entries = guard.entries(target);
        if entries.is_empty() {
            return None;
        }
        Some(render_block(target, entries, guard.limit(target)))
    }

    /// Append a new entry. Rejects (without writing anything) if it would
    /// put the file over budget — see the module doc.
    pub fn add(&self, target: Target, content: &str) -> Result<WriteReport, MemoryError> {
        let content = content.trim();
        if content.is_empty() {
            return Err(MemoryError::EmptyContent);
        }
        let content = content.to_string();

        let mut guard = self.0.lock();
        let limit = guard.limit(target);
        let mut entries = guard.entries(target).clone();

        if entries.iter().any(|e| *e == content) {
            let usage = Usage::compute(&entries, limit);
            return Ok(WriteReport {
                usage,
                duplicate: true,
                message: "Entry already exists (no duplicate added).".to_string(),
            });
        }

        let attempted_chars = content.chars().count();
        entries.push(content);
        if char_count(&entries) > limit {
            let current_entries = guard.entries(target).clone();
            let usage = Usage::compute(&current_entries, limit);
            return Err(MemoryError::OverBudget { target, usage, entries: current_entries, attempted_chars });
        }

        commit(&mut guard, target, entries.clone())?;
        Ok(WriteReport { usage: Usage::compute(&entries, limit), duplicate: false, message: "Entry added.".to_string() })
    }

    /// Find the entry containing `old_text` as a substring and replace it
    /// with `content`. Ambiguous when more than one *distinct* entry matches
    /// (identical duplicates all resolving to the same edit are fine — the
    /// first is used).
    pub fn replace(&self, target: Target, old_text: &str, content: &str) -> Result<WriteReport, MemoryError> {
        let old_text = old_text.trim();
        if old_text.is_empty() {
            return Err(MemoryError::EmptyOldText);
        }
        let content = content.trim();
        if content.is_empty() {
            return Err(MemoryError::EmptyReplacement);
        }
        let content = content.to_string();

        let mut guard = self.0.lock();
        let limit = guard.limit(target);
        let mut entries = guard.entries(target).clone();

        let idx = match find_unique_match(&entries, old_text) {
            Ok(idx) => idx,
            Err(MatchError::None) => {
                let usage = Usage::compute(&entries, limit);
                return Err(MemoryError::NoMatch { old_text: old_text.to_string(), usage, entries });
            }
            Err(MatchError::Ambiguous(previews)) => {
                return Err(MemoryError::AmbiguousMatch { old_text: old_text.to_string(), previews });
            }
        };

        let attempted_chars = content.chars().count();
        entries[idx] = content;
        if char_count(&entries) > limit {
            let current_entries = guard.entries(target).clone();
            let usage = Usage::compute(&current_entries, limit);
            return Err(MemoryError::OverBudget { target, usage, entries: current_entries, attempted_chars });
        }

        commit(&mut guard, target, entries.clone())?;
        Ok(WriteReport { usage: Usage::compute(&entries, limit), duplicate: false, message: "Entry replaced.".to_string() })
    }

    /// Remove the entry containing `old_text` as a substring. Never rejected
    /// for budget (removing can only shrink the file).
    pub fn remove(&self, target: Target, old_text: &str) -> Result<WriteReport, MemoryError> {
        let old_text = old_text.trim();
        if old_text.is_empty() {
            return Err(MemoryError::EmptyOldText);
        }

        let mut guard = self.0.lock();
        let limit = guard.limit(target);
        let mut entries = guard.entries(target).clone();

        let idx = match find_unique_match(&entries, old_text) {
            Ok(idx) => idx,
            Err(MatchError::None) => {
                let usage = Usage::compute(&entries, limit);
                return Err(MemoryError::NoMatch { old_text: old_text.to_string(), usage, entries });
            }
            Err(MatchError::Ambiguous(previews)) => {
                return Err(MemoryError::AmbiguousMatch { old_text: old_text.to_string(), previews });
            }
        };

        entries.remove(idx);
        commit(&mut guard, target, entries.clone())?;
        Ok(WriteReport { usage: Usage::compute(&entries, limit), duplicate: false, message: "Entry removed.".to_string() })
    }

    /// Apply a sequence of add/replace/remove operations to one target
    /// atomically, all-or-nothing: the char budget is checked only against
    /// the FINAL state, so a single batch can free room (replace/remove) and
    /// add new entries in one call, even when an `add` alone would have
    /// overflowed. If any operation is malformed, matches nothing, or the
    /// net result is still over budget, NOTHING is written — the store's
    /// on-disk state after a failed batch is byte-identical to before it.
    pub fn apply_batch(&self, target: Target, operations: &[Operation]) -> Result<WriteReport, MemoryError> {
        if operations.is_empty() {
            return Err(MemoryError::EmptyBatch);
        }

        let mut guard = self.0.lock();
        let limit = guard.limit(target);
        let mut working = guard.entries(target).clone();

        for (index, op) in operations.iter().enumerate() {
            match op.action {
                OpAction::Add => {
                    let content = op.content.as_deref().unwrap_or("").trim().to_string();
                    if content.is_empty() {
                        return batch_error(&guard, target, index, "content is required for 'add'");
                    }
                    // Idempotent within the batch, matching `add`'s own
                    // no-duplicate rule — lets a model list a fact it is
                    // unsure was already saved without failing the batch.
                    if !working.contains(&content) {
                        working.push(content);
                    }
                }
                OpAction::Replace => {
                    let old_text = op.old_text.as_deref().unwrap_or("").trim();
                    let content = op.content.as_deref().unwrap_or("").trim().to_string();
                    if old_text.is_empty() {
                        return batch_error(&guard, target, index, "old_text is required for 'replace'");
                    }
                    if content.is_empty() {
                        return batch_error(&guard, target, index, "content is required for 'replace' (use 'remove' to delete)");
                    }
                    match find_unique_match(&working, old_text) {
                        Ok(idx) => working[idx] = content,
                        Err(MatchError::None) => {
                            return batch_error(&guard, target, index, &format!("no entry matched '{old_text}'"));
                        }
                        Err(MatchError::Ambiguous(_)) => {
                            return batch_error(
                                &guard,
                                target,
                                index,
                                &format!("'{old_text}' matched multiple distinct entries — be more specific"),
                            );
                        }
                    }
                }
                OpAction::Remove => {
                    let old_text = op.old_text.as_deref().unwrap_or("").trim();
                    if old_text.is_empty() {
                        return batch_error(&guard, target, index, "old_text is required for 'remove'");
                    }
                    match find_unique_match(&working, old_text) {
                        Ok(idx) => {
                            working.remove(idx);
                        }
                        Err(MatchError::None) => {
                            return batch_error(&guard, target, index, &format!("no entry matched '{old_text}'"));
                        }
                        Err(MatchError::Ambiguous(_)) => {
                            return batch_error(
                                &guard,
                                target,
                                index,
                                &format!("'{old_text}' matched multiple distinct entries — be more specific"),
                            );
                        }
                    }
                }
            }
        }

        if char_count(&working) > limit {
            let current_entries = guard.entries(target).clone();
            let usage = Usage::compute(&current_entries, limit);
            return Err(MemoryError::OverBudget { target, usage, entries: current_entries, attempted_chars: char_count(&working) });
        }

        commit(&mut guard, target, working.clone())?;
        Ok(WriteReport {
            usage: Usage::compute(&working, limit),
            duplicate: false,
            message: format!("Applied {} operation(s).", operations.len()),
        })
    }
}

fn batch_error(guard: &Inner, target: Target, index: usize, reason: &str) -> Result<WriteReport, MemoryError> {
    let entries = guard.entries(target).clone();
    let usage = Usage::compute(&entries, guard.limit(target));
    Err(MemoryError::BatchOperation { index, reason: reason.to_string(), usage, entries })
}

/// Write `entries` to `target`'s file (atomically — see the module doc) and,
/// only once that succeeds, update the in-memory copy. Ordered this way so a
/// failed disk write never leaves `Inner`'s live state ahead of what is
/// actually on disk.
fn commit(guard: &mut Inner, target: Target, entries: Vec<String>) -> io::Result<()> {
    let path = guard.path(target);
    write_atomic(&path, &entries.join(ENTRY_DELIMITER))?;
    guard.set_entries(target, entries);
    Ok(())
}

enum MatchError {
    None,
    /// Previews of every distinct matching entry, for the caller's error.
    Ambiguous(Vec<String>),
}

/// Find the one entry containing `needle` as a substring.
///
/// Multiple *identical* matches (an exact duplicate somehow present twice —
/// should not happen given `add`'s no-duplicate rule, but a hand-edited file
/// could produce one) resolve to the first without complaint, since editing
/// either copy has the same effect. Multiple *distinct* matches are
/// ambiguous and must be reported rather than guessed at — silently picking
/// one could edit or delete the wrong fact.
fn find_unique_match(entries: &[String], needle: &str) -> Result<usize, MatchError> {
    let matches: Vec<usize> = entries.iter().enumerate().filter(|(_, e)| e.contains(needle)).map(|(i, _)| i).collect();
    if matches.is_empty() {
        return Err(MatchError::None);
    }
    if matches.len() > 1 {
        let distinct: HashSet<&str> = matches.iter().map(|&i| entries[i].as_str()).collect();
        if distinct.len() > 1 {
            let previews = matches.iter().map(|&i| preview(&entries[i], 80)).collect();
            return Err(MatchError::Ambiguous(previews));
        }
    }
    Ok(matches[0])
}

fn preview(entry: &str, width: usize) -> String {
    if entry.chars().count() <= width {
        return entry.to_string();
    }
    let mut s: String = entry.chars().take(width).collect();
    s.push('\u{2026}');
    s
}

/// Character count of `entries` as they would be written to disk — i.e. the
/// number [`MemoryStore`]'s budget check compares against the limit. Uses
/// Unicode scalar values (`.chars().count()`), not bytes: the reference
/// implementation's Python `len(str)` counts code points, and matching that
/// means a limit configured against Hermes' own numbers means the same thing
/// here. `0` for an empty list (there is no leading/trailing delimiter to
/// count when there is nothing to join).
fn char_count(entries: &[String]) -> usize {
    if entries.is_empty() {
        return 0;
    }
    entries.join(ENTRY_DELIMITER).chars().count()
}

fn render_block(target: Target, entries: &[String], limit: usize) -> String {
    let content = entries.join(ENTRY_DELIMITER);
    let current = content.chars().count();
    let percent = if limit == 0 { 0 } else { ((current as f64 / limit as f64) * 100.0).clamp(0.0, 100.0) as u8 };
    let rule: String = std::iter::repeat(BANNER_RULE_CHAR).take(BANNER_RULE_LEN).collect();
    let header = format!("{} [{percent}% \u{2014} {}/{} chars]", target.label(), grouped(current), grouped(limit));
    format!("{rule}\n{header}\n{rule}\n{content}")
}

/// `1474` -> `"1,474"`, purely so the budget banner reads the way the
/// reference implementation's does (it formats with Python's `:,`).
fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.chars().rev().collect()
}

/// Read a memory file and split into de-duplicated, trimmed entries.
///
/// No file locking on read: [`write_atomic`] always replaces the file via
/// rename, so a reader — even one racing a concurrent writer — sees either
/// the complete old file or the complete new one, never a partial write.
fn read_entries(path: &Path) -> io::Result<Vec<String>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(path)?;
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut seen = HashSet::new();
    Ok(raw
        .split(ENTRY_DELIMITER)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .filter(|s| seen.insert(s.clone()))
        .collect())
}

/// Write `content` to `path` via temp-file-then-rename, never in-place
/// mutation. `rename` is atomic on the same filesystem (true on every
/// platform Caduceus ships for — NTFS, APFS, ext4/btrfs — for a same-volume
/// rename), so a reader never observes a truncated or half-written file. The
/// temp name includes a fresh UUID so two writers racing on the same target
/// file (two Caduceus processes, which `tauri-plugin-single-instance`
/// otherwise prevents, or simply two threads that both got past the
/// `Mutex` — should not happen, but this makes it harmless either way)
/// cannot collide on the same temp path.
fn write_atomic(path: &Path, content: &str) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "memory file path has no parent directory"))?;
    fs::create_dir_all(dir)?;
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("memory");
    let tmp_path = dir.join(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));

    let write_result = fs::write(&tmp_path, content);
    if write_result.is_err() {
        let _ = fs::remove_file(&tmp_path);
        return write_result;
    }

    let rename_result = fs::rename(&tmp_path, path);
    if rename_result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    rename_result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh temp directory per test, so tests can run concurrently
    /// without treading on each other's `MEMORY.md`/`USER.md`.
    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("caduceus-memory-test-{}", uuid::Uuid::new_v4()));
        dir
    }

    fn store() -> MemoryStore {
        MemoryStore::open(temp_dir(), DEFAULT_MEMORY_CHAR_LIMIT, DEFAULT_USER_CHAR_LIMIT).unwrap()
    }

    // -----------------------------------------------------------------
    // add / duplicate no-op
    // -----------------------------------------------------------------

    #[test]
    fn adding_an_entry_persists_it_and_reports_usage() {
        let s = store();
        let report = s.add(Target::Memory, "user prefers dark mode").unwrap();
        assert!(!report.duplicate);
        assert_eq!(report.usage.entry_count, 1);
        assert_eq!(s.entries(Target::Memory), vec!["user prefers dark mode".to_string()]);
    }

    #[test]
    fn an_exact_duplicate_add_is_a_silent_no_op() {
        let s = store();
        s.add(Target::Memory, "fact one").unwrap();
        let report = s.add(Target::Memory, "fact one").unwrap();
        assert!(report.duplicate);
        assert_eq!(s.entries(Target::Memory).len(), 1, "no second copy should be written");
    }

    #[test]
    fn leading_and_trailing_whitespace_does_not_defeat_duplicate_detection() {
        let s = store();
        s.add(Target::Memory, "fact one").unwrap();
        let report = s.add(Target::Memory, "  fact one  \n").unwrap();
        assert!(report.duplicate);
    }

    #[test]
    fn empty_content_is_rejected() {
        let s = store();
        assert!(matches!(s.add(Target::Memory, "   ").unwrap_err(), MemoryError::EmptyContent));
    }

    // -----------------------------------------------------------------
    // budget rejection
    // -----------------------------------------------------------------

    #[test]
    fn an_over_budget_add_is_rejected_and_writes_nothing() {
        let s = MemoryStore::open(temp_dir(), 20, DEFAULT_USER_CHAR_LIMIT).unwrap();
        s.add(Target::Memory, "12345").unwrap(); // 5 chars, fits in 20
        let err = s.add(Target::Memory, "this is far too long to fit").unwrap_err();
        assert!(matches!(err, MemoryError::OverBudget { .. }));
        // The rejected content must not have been written.
        assert_eq!(s.entries(Target::Memory), vec!["12345".to_string()]);
    }

    #[test]
    fn over_budget_error_carries_the_current_entries_for_consolidation() {
        let s = MemoryStore::open(temp_dir(), 10, DEFAULT_USER_CHAR_LIMIT).unwrap();
        s.add(Target::Memory, "12345").unwrap();
        match s.add(Target::Memory, "way too long for this budget").unwrap_err() {
            MemoryError::OverBudget { entries, usage, .. } => {
                assert_eq!(entries, vec!["12345".to_string()]);
                assert_eq!(usage.current, 5);
                assert_eq!(usage.limit, 10);
            }
            other => panic!("expected OverBudget, got {other:?}"),
        }
    }

    #[test]
    fn a_replace_that_would_overflow_is_rejected_leaving_the_original_intact() {
        let s = MemoryStore::open(temp_dir(), 15, DEFAULT_USER_CHAR_LIMIT).unwrap();
        s.add(Target::Memory, "short").unwrap();
        let err = s.replace(Target::Memory, "short", "a much longer replacement text").unwrap_err();
        assert!(matches!(err, MemoryError::OverBudget { .. }));
        assert_eq!(s.entries(Target::Memory), vec!["short".to_string()]);
    }

    #[test]
    fn exactly_at_the_limit_is_accepted_one_over_is_not() {
        // char_count for a single entry is just its own length (no
        // delimiter joined against anything).
        let s = MemoryStore::open(temp_dir(), 5, DEFAULT_USER_CHAR_LIMIT).unwrap();
        s.add(Target::Memory, "12345").unwrap(); // exactly 5
        assert_eq!(s.usage(Target::Memory).current, 5);
        let s2 = MemoryStore::open(temp_dir(), 4, DEFAULT_USER_CHAR_LIMIT).unwrap();
        assert!(s2.add(Target::Memory, "12345").is_err());
    }

    // -----------------------------------------------------------------
    // replace / remove matching
    // -----------------------------------------------------------------

    #[test]
    fn replace_finds_by_substring_and_replaces_the_whole_entry() {
        // `old_text` only LOCATES the entry (a substring match); the entry's
        // full text becomes `content` verbatim, not a splice within it —
        // matching the reference implementation's `entries[idx] =
        // new_content` exactly.
        let s = store();
        s.add(Target::Memory, "user likes the color blue").unwrap();
        s.replace(Target::Memory, "likes the color blue", "user likes the color green").unwrap();
        assert_eq!(s.entries(Target::Memory), vec!["user likes the color green".to_string()]);
    }

    #[test]
    fn remove_finds_by_substring_and_deletes() {
        let s = store();
        s.add(Target::Memory, "stale fact").unwrap();
        s.add(Target::Memory, "keep this one").unwrap();
        s.remove(Target::Memory, "stale").unwrap();
        assert_eq!(s.entries(Target::Memory), vec!["keep this one".to_string()]);
    }

    #[test]
    fn replace_with_no_match_reports_current_entries() {
        let s = store();
        s.add(Target::Memory, "only entry").unwrap();
        match s.replace(Target::Memory, "nonexistent", "x").unwrap_err() {
            MemoryError::NoMatch { entries, .. } => assert_eq!(entries, vec!["only entry".to_string()]),
            other => panic!("expected NoMatch, got {other:?}"),
        }
    }

    #[test]
    fn ambiguous_substring_across_distinct_entries_is_rejected() {
        let s = store();
        s.add(Target::Memory, "prefers python for scripting").unwrap();
        s.add(Target::Memory, "prefers dark mode everywhere").unwrap();
        let err = s.replace(Target::Memory, "prefers", "x").unwrap_err();
        assert!(matches!(err, MemoryError::AmbiguousMatch { .. }));
    }

    #[test]
    fn empty_old_text_is_rejected_for_replace_and_remove() {
        let s = store();
        assert!(matches!(s.replace(Target::Memory, "", "x").unwrap_err(), MemoryError::EmptyOldText));
        assert!(matches!(s.remove(Target::Memory, "").unwrap_err(), MemoryError::EmptyOldText));
    }

    #[test]
    fn empty_replacement_content_is_rejected() {
        let s = store();
        s.add(Target::Memory, "entry").unwrap();
        assert!(matches!(
            s.replace(Target::Memory, "entry", "").unwrap_err(),
            MemoryError::EmptyReplacement
        ));
    }

    // -----------------------------------------------------------------
    // batch: atomic all-or-nothing, budget checked only on the final state
    // -----------------------------------------------------------------

    #[test]
    fn a_batch_can_free_room_and_add_in_one_call_even_though_add_alone_would_overflow() {
        let s = MemoryStore::open(temp_dir(), 20, DEFAULT_USER_CHAR_LIMIT).unwrap();
        s.add(Target::Memory, "old stale fact").unwrap(); // 14 chars
        // Adding a 15-char fact alone would overflow (14 + delim + 15 > 20).
        assert!(s.add(Target::Memory, "brand new fact!").is_err());

        // But replacing the old one while adding fits within budget.
        let ops = vec![
            Operation { action: OpAction::Remove, content: None, old_text: Some("old stale fact".into()) },
            Operation { action: OpAction::Add, content: Some("new fact".into()), old_text: None },
        ];
        let report = s.apply_batch(Target::Memory, &ops).unwrap();
        assert_eq!(report.usage.entry_count, 1);
        assert_eq!(s.entries(Target::Memory), vec!["new fact".to_string()]);
    }

    #[test]
    fn a_batch_that_ends_over_budget_writes_nothing_at_all() {
        let s = MemoryStore::open(temp_dir(), 10, DEFAULT_USER_CHAR_LIMIT).unwrap();
        s.add(Target::Memory, "abc").unwrap();
        let ops = vec![Operation { action: OpAction::Add, content: Some("this is way too long".into()), old_text: None }];
        assert!(s.apply_batch(Target::Memory, &ops).is_err());
        // Original state must be untouched.
        assert_eq!(s.entries(Target::Memory), vec!["abc".to_string()]);
    }

    #[test]
    fn a_batch_with_a_bad_operation_in_the_middle_commits_nothing_before_it_either() {
        let s = store();
        s.add(Target::Memory, "existing").unwrap();
        let ops = vec![
            Operation { action: OpAction::Add, content: Some("first new entry".into()), old_text: None },
            Operation { action: OpAction::Remove, content: None, old_text: Some("does-not-exist".into()) },
        ];
        assert!(s.apply_batch(Target::Memory, &ops).is_err());
        // The first (valid) op in the batch must not have partially landed.
        assert_eq!(s.entries(Target::Memory), vec!["existing".to_string()]);
    }

    #[test]
    fn an_empty_batch_is_rejected() {
        let s = store();
        assert!(matches!(s.apply_batch(Target::Memory, &[]).unwrap_err(), MemoryError::EmptyBatch));
    }

    #[test]
    fn duplicate_add_within_a_batch_is_skipped_not_an_error() {
        let s = store();
        let ops = vec![
            Operation { action: OpAction::Add, content: Some("same fact".into()), old_text: None },
            Operation { action: OpAction::Add, content: Some("same fact".into()), old_text: None },
        ];
        let report = s.apply_batch(Target::Memory, &ops).unwrap();
        assert_eq!(report.usage.entry_count, 1);
    }

    // -----------------------------------------------------------------
    // atomic write + delimiter round-trip
    // -----------------------------------------------------------------

    #[test]
    fn entries_round_trip_through_the_delimiter_across_a_fresh_open() {
        let dir = temp_dir();
        {
            let s = MemoryStore::open(&dir, DEFAULT_MEMORY_CHAR_LIMIT, DEFAULT_USER_CHAR_LIMIT).unwrap();
            s.add(Target::Memory, "first entry").unwrap();
            s.add(Target::Memory, "second entry, multi\nline even").unwrap();
            s.add(Target::User, "the user's name is Alex").unwrap();
        }
        // Re-open fresh — nothing in-memory carries over, only the files.
        let reopened = MemoryStore::open(&dir, DEFAULT_MEMORY_CHAR_LIMIT, DEFAULT_USER_CHAR_LIMIT).unwrap();
        assert_eq!(
            reopened.entries(Target::Memory),
            vec!["first entry".to_string(), "second entry, multi\nline even".to_string()]
        );
        assert_eq!(reopened.entries(Target::User), vec!["the user's name is Alex".to_string()]);
    }

    #[test]
    fn the_file_on_disk_uses_the_literal_delimiter_between_entries() {
        let dir = temp_dir();
        let s = MemoryStore::open(&dir, DEFAULT_MEMORY_CHAR_LIMIT, DEFAULT_USER_CHAR_LIMIT).unwrap();
        s.add(Target::Memory, "a").unwrap();
        s.add(Target::Memory, "b").unwrap();
        let raw = fs::read_to_string(dir.join("MEMORY.md")).unwrap();
        assert_eq!(raw, format!("a{ENTRY_DELIMITER}b"));
    }

    #[test]
    fn a_write_never_leaves_a_stray_temp_file_behind() {
        let dir = temp_dir();
        let s = MemoryStore::open(&dir, DEFAULT_MEMORY_CHAR_LIMIT, DEFAULT_USER_CHAR_LIMIT).unwrap();
        s.add(Target::Memory, "entry").unwrap();
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp file should be renamed away, not left behind");
    }

    #[test]
    fn duplicates_present_on_disk_before_open_are_collapsed_on_load() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("MEMORY.md"), format!("dup{ENTRY_DELIMITER}dup{ENTRY_DELIMITER}unique")).unwrap();
        let s = MemoryStore::open(&dir, DEFAULT_MEMORY_CHAR_LIMIT, DEFAULT_USER_CHAR_LIMIT).unwrap();
        assert_eq!(s.entries(Target::Memory), vec!["dup".to_string(), "unique".to_string()]);
    }

    #[test]
    fn opening_a_directory_with_no_files_yet_starts_empty_not_erroring() {
        let s = MemoryStore::open(temp_dir(), DEFAULT_MEMORY_CHAR_LIMIT, DEFAULT_USER_CHAR_LIMIT).unwrap();
        assert!(s.entries(Target::Memory).is_empty());
        assert!(s.entries(Target::User).is_empty());
        assert!(s.snapshot_block(Target::Memory).is_none());
    }

    // -----------------------------------------------------------------
    // snapshot_block rendering
    // -----------------------------------------------------------------

    #[test]
    fn snapshot_block_is_none_when_empty_and_some_with_a_banner_once_populated() {
        let s = store();
        assert!(s.snapshot_block(Target::Memory).is_none());
        s.add(Target::Memory, "a fact").unwrap();
        let block = s.snapshot_block(Target::Memory).unwrap();
        assert!(block.contains("MEMORY (your personal notes)"));
        assert!(block.contains("a fact"));
        assert!(block.contains('%'));
    }

    #[test]
    fn user_target_snapshot_uses_the_user_profile_label() {
        let s = store();
        s.add(Target::User, "name is Alex").unwrap();
        let block = s.snapshot_block(Target::User).unwrap();
        assert!(block.contains("USER PROFILE (who the user is)"));
    }

    #[test]
    fn snapshot_block_reports_percent_of_the_configured_limit() {
        let s = MemoryStore::open(temp_dir(), 20, DEFAULT_USER_CHAR_LIMIT).unwrap();
        s.add(Target::Memory, "1234567890").unwrap(); // 10/20 = 50%
        let block = s.snapshot_block(Target::Memory).unwrap();
        assert!(block.contains("50%"), "block was: {block}");
        assert!(block.contains("10/20"), "block was: {block}");
    }

    // -----------------------------------------------------------------
    // Target
    // -----------------------------------------------------------------

    #[test]
    fn target_parses_the_two_legal_wire_values_and_rejects_everything_else() {
        assert_eq!(Target::parse("memory"), Some(Target::Memory));
        assert_eq!(Target::parse("user"), Some(Target::User));
        assert_eq!(Target::parse("nonsense"), None);
        assert_eq!(Target::parse(""), None);
    }

    // -----------------------------------------------------------------
    // grouped
    // -----------------------------------------------------------------

    #[test]
    fn grouped_matches_the_reference_implementations_comma_formatting() {
        assert_eq!(grouped(1474), "1,474");
        assert_eq!(grouped(2200), "2,200");
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
    }
}
