//! The `.usage.json` sidecar: per-skill telemetry the rest of the module
//! reads to decide lifecycle state, entirely separate from a skill's own
//! `SKILL.md`.
//!
//! Keeping this out of frontmatter is deliberate, not incidental: a view
//! count is operational data about *this install*, not something that
//! belongs in a file a user might hand-edit, diff, or share as a skill
//! package. A bundled or hub-installed skill gets exactly the same
//! telemetry treatment as a user-authored one — this file tracks every
//! skill by name, with no notion of where the skill came from (see
//! [`crate::skills::bundled`] for that question instead).
//!
//! # Concurrency
//!
//! Every mutating call here does its own read-JSON / mutate / write-JSON
//! round trip through [`with_usage_file`], serialized by one process-global
//! lock. That is enough — not a cross-process file lock — because Caduceus
//! is enforced single-instance at the OS level (see
//! `tauri_plugin_single_instance` in `lib.rs::run`), so "two writers" can
//! only ever mean two tool calls racing inside this one process, which the
//! lock already covers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::write_atomically;

pub const USAGE_FILE: &str = ".usage.json";

pub fn path_under(skills_root: &Path) -> PathBuf {
    skills_root.join(USAGE_FILE)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillState {
    Active,
    Stale,
    Archived,
}

impl Default for SkillState {
    fn default() -> Self {
        SkillState::Active
    }
}

/// One skill's usage record. Every field defaults sensibly for a skill this
/// sidecar has never seen before — see [`get_record`] — so callers never
/// have to special-case "no record yet" versus "a record with all zeros."
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageRecord {
    pub view_count: u32,
    pub use_count: u32,
    pub patch_count: u32,
    pub state: SkillState,
    /// Opts a skill out of every automatic lifecycle transition
    /// ([`crate::skills::lifecycle`]) and out of deletion via `skill_manage`
    /// — never out of `patch`/`edit`, which stay allowed so pitfalls can
    /// still be folded in as they're discovered.
    pub pinned: bool,
    /// `"agent"` when created via `skill_manage(action = "create")`; `None`
    /// for anything already on disk before this sidecar started tracking it
    /// (bundled skills, hand-placed files). Reporting-only in Caduceus today
    /// — there is no autonomous curator here that gates on it the way
    /// Hermes' background review fork does.
    pub created_by: Option<String>,
    /// RFC 3339. Used by [`crate::skills::lifecycle::decide`] as the
    /// inactivity anchor when a skill has never been viewed, used, or
    /// patched.
    pub created_at: String,
    pub last_viewed_at: Option<String>,
    pub last_used_at: Option<String>,
    pub last_patched_at: Option<String>,
    pub archived_at: Option<String>,
}

impl Default for UsageRecord {
    fn default() -> Self {
        Self {
            view_count: 0,
            use_count: 0,
            patch_count: 0,
            state: SkillState::Active,
            pinned: false,
            created_by: None,
            created_at: now_iso(),
            last_viewed_at: None,
            last_used_at: None,
            last_patched_at: None,
            archived_at: None,
        }
    }
}

fn now_iso() -> String {
    Utc::now().to_rfc3339()
}

static USAGE_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Read the whole sidecar. A missing or corrupt file reads as empty rather
/// than erroring — the worst case is that lifecycle decisions fall back to
/// treating every skill as freshly created, never a crash or a blocked tool
/// call over a telemetry file.
pub fn load(usage_path: &Path) -> HashMap<String, UsageRecord> {
    let Ok(text) = std::fs::read_to_string(usage_path) else {
        return HashMap::new();
    };
    serde_json::from_str(&text).unwrap_or_else(|e| {
        log::warn!("skills: {} is corrupt ({e}); treating usage as empty", usage_path.display());
        HashMap::new()
    })
}

pub fn save(usage_path: &Path, data: &HashMap<String, UsageRecord>) {
    let Ok(text) = serde_json::to_string_pretty(data) else { return };
    if let Err(e) = write_atomically(usage_path, &text) {
        log::warn!("skills: could not write {}: {e}", usage_path.display());
    }
}

fn with_usage_file<T>(usage_path: &Path, f: impl FnOnce(&mut HashMap<String, UsageRecord>) -> T) -> T {
    let _guard = USAGE_LOCK.lock();
    let mut data = load(usage_path);
    let result = f(&mut data);
    save(usage_path, &data);
    result
}

/// The record for `name`, or a fresh default if the sidecar has never seen
/// it — never a `None`/error a caller would have to branch on for what is,
/// functionally, "no telemetry yet."
pub fn get_record(usage_path: &Path, name: &str) -> UsageRecord {
    load(usage_path).remove(name).unwrap_or_default()
}

/// `skill_view` calls this on every load. Tracks any skill by name,
/// regardless of provenance — usage telemetry is observability, not a
/// curation gate.
pub fn bump_view(usage_path: &Path, name: &str) {
    with_usage_file(usage_path, |data| {
        let record = data.entry(name.to_string()).or_default();
        record.view_count += 1;
        record.last_viewed_at = Some(now_iso());
    });
}

/// Called when a skill is actively invoked, not just browsed — resets the
/// inactivity clock [`crate::skills::lifecycle::decide`] measures from.
pub fn bump_use(usage_path: &Path, name: &str) {
    with_usage_file(usage_path, |data| {
        let record = data.entry(name.to_string()).or_default();
        record.use_count += 1;
        record.last_used_at = Some(now_iso());
    });
}

/// `skill_manage`'s patch/edit/write_file/remove_file actions call this.
pub fn bump_patch(usage_path: &Path, name: &str) {
    with_usage_file(usage_path, |data| {
        let record = data.entry(name.to_string()).or_default();
        record.patch_count += 1;
        record.last_patched_at = Some(now_iso());
    });
}

pub fn set_pinned(usage_path: &Path, name: &str, pinned: bool) {
    with_usage_file(usage_path, |data| {
        data.entry(name.to_string()).or_default().pinned = pinned;
    });
}

/// Set lifecycle state directly. Stamps or clears `archived_at` to match, so
/// the two fields never drift out of sync with each other.
pub fn set_state(usage_path: &Path, name: &str, state: SkillState) {
    with_usage_file(usage_path, |data| {
        let record = data.entry(name.to_string()).or_default();
        record.state = state;
        record.archived_at = match state {
            SkillState::Archived => Some(now_iso()),
            _ => None,
        };
    });
}

/// Marks a skill as created by the agent (via `skill_manage(create)`) —
/// distinct from a skill that merely exists on disk, so provenance
/// reporting does not have to guess from location alone.
pub fn mark_created_by_agent(usage_path: &Path, name: &str) {
    with_usage_file(usage_path, |data| {
        data.entry(name.to_string()).or_default().created_by = Some("agent".to_string());
    });
}

/// Drop a skill's usage entry entirely — called when a skill is deleted, so
/// the sidecar never accumulates rows for skills that no longer exist.
pub fn forget(usage_path: &Path, name: &str) {
    with_usage_file(usage_path, |data| {
        data.remove(name);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch_usage_path(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("caduceus-skills-usage-test-{label}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(USAGE_FILE)
    }

    #[test]
    fn a_missing_file_loads_as_an_empty_map() {
        let path = scratch_usage_path("missing").parent().unwrap().join("nope.json");
        assert_eq!(load(&path), HashMap::new());
    }

    #[test]
    fn a_corrupt_file_loads_as_an_empty_map_not_a_panic() {
        let path = scratch_usage_path("corrupt");
        std::fs::write(&path, "{ not json ").unwrap();
        assert_eq!(load(&path), HashMap::new());
    }

    #[test]
    fn get_record_returns_a_fresh_default_for_an_unknown_skill() {
        let path = scratch_usage_path("fresh");
        let record = get_record(&path, "never-seen");
        assert_eq!(record.view_count, 0);
        assert_eq!(record.use_count, 0);
        assert_eq!(record.state, SkillState::Active);
        assert!(!record.pinned);
    }

    #[test]
    fn bump_view_increments_the_counter_and_stamps_a_timestamp() {
        let path = scratch_usage_path("bump-view");
        bump_view(&path, "s");
        bump_view(&path, "s");
        let record = get_record(&path, "s");
        assert_eq!(record.view_count, 2);
        assert!(record.last_viewed_at.is_some());
        assert_eq!(record.use_count, 0, "bumping view must not also bump use");
    }

    #[test]
    fn bump_use_and_bump_patch_track_independently() {
        let path = scratch_usage_path("bump-use-patch");
        bump_use(&path, "s");
        bump_patch(&path, "s");
        bump_patch(&path, "s");
        let record = get_record(&path, "s");
        assert_eq!(record.use_count, 1);
        assert_eq!(record.patch_count, 2);
        assert!(record.last_used_at.is_some());
        assert!(record.last_patched_at.is_some());
    }

    #[test]
    fn set_pinned_round_trips() {
        let path = scratch_usage_path("pinned");
        set_pinned(&path, "s", true);
        assert!(get_record(&path, "s").pinned);
        set_pinned(&path, "s", false);
        assert!(!get_record(&path, "s").pinned);
    }

    #[test]
    fn set_state_to_archived_stamps_archived_at_and_clearing_it_unstamps() {
        let path = scratch_usage_path("state");
        set_state(&path, "s", SkillState::Archived);
        let record = get_record(&path, "s");
        assert_eq!(record.state, SkillState::Archived);
        assert!(record.archived_at.is_some());

        set_state(&path, "s", SkillState::Active);
        let record = get_record(&path, "s");
        assert_eq!(record.state, SkillState::Active);
        assert!(record.archived_at.is_none());
    }

    #[test]
    fn mark_created_by_agent_sets_provenance() {
        let path = scratch_usage_path("provenance");
        mark_created_by_agent(&path, "s");
        assert_eq!(get_record(&path, "s").created_by, Some("agent".to_string()));
    }

    #[test]
    fn forget_removes_the_record_entirely() {
        let path = scratch_usage_path("forget");
        bump_view(&path, "s");
        assert!(load(&path).contains_key("s"));
        forget(&path, "s");
        assert!(!load(&path).contains_key("s"));
    }

    #[test]
    fn records_for_different_skills_do_not_interfere() {
        let path = scratch_usage_path("multi");
        bump_view(&path, "a");
        bump_view(&path, "a");
        bump_view(&path, "b");
        assert_eq!(get_record(&path, "a").view_count, 2);
        assert_eq!(get_record(&path, "b").view_count, 1);
    }

    #[test]
    fn save_then_load_round_trips_every_field() {
        let path = scratch_usage_path("round-trip");
        let mut data = HashMap::new();
        let mut record = UsageRecord::default();
        record.view_count = 5;
        record.pinned = true;
        record.created_by = Some("agent".to_string());
        record.state = SkillState::Stale;
        data.insert("s".to_string(), record.clone());
        save(&path, &data);

        let reloaded = load(&path);
        assert_eq!(reloaded.get("s"), Some(&record));
    }

    #[test]
    fn a_record_missing_newer_fields_backfills_defaults_on_load() {
        // Simulates a sidecar written by an older version of this module
        // that did not yet know about some field — `#[serde(default)]`
        // must backfill rather than fail to deserialize the whole file.
        let path = scratch_usage_path("backfill");
        // Field names on the wire are camelCase (`#[serde(rename_all =
        // "camelCase")]`) — "viewCount" here, not "view_count".
        std::fs::write(&path, r#"{"old-skill": {"viewCount": 3}}"#).unwrap();
        let record = get_record(&path, "old-skill");
        assert_eq!(record.view_count, 3);
        assert_eq!(record.use_count, 0);
        assert_eq!(record.state, SkillState::Active);
        assert!(!record.pinned);
    }
}
