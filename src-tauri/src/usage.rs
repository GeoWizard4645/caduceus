//! How often each thing in the palette gets used, so the list can learn.
//!
//! A launcher that shows the same order on day 200 as on day 1 is making every
//! user pay for the average user's habits. This records a count and a timestamp
//! per result id and hands them back to the ranking code, which is enough for
//! "the four things you actually run" to rise to the top on their own.
//!
//! # What is not here
//!
//! No telemetry. The file never leaves the machine, is not sent anywhere, and
//! holds nothing but ids Caduceus itself defined plus integers. `Settings →
//! Command Center` can clear it.
//!
//! # Why a JSON file rather than the clipboard database
//!
//! It is a map of a few hundred small integers, read once at launch and written
//! on use. SQLite would mean a second connection and a migration for something
//! that fits in a few kilobytes, and losing the file costs nothing worse than a
//! ranking that starts over.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

pub const USAGE_FILE: &str = "usage.json";

/// How many distinct ids to remember.
///
/// Reached only by someone who has launched a thousand different applications;
/// the cap exists so a long-lived install cannot grow the file without bound.
const MAX_ENTRIES: usize = 2000;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEntry {
    pub count: u32,
    /// Unix milliseconds. Breaks ties between equally-used entries, so the one
    /// you touched this morning wins over the one you last ran in March.
    pub last_used_ms: i64,
}

pub struct UsageStore {
    path: PathBuf,
    entries: Mutex<HashMap<String, UsageEntry>>,
}

impl UsageStore {
    /// Load the file, or start empty if it is missing or unreadable.
    ///
    /// A corrupt file is not an error worth surfacing: the worst case is that
    /// ranking starts from the built-in order again.
    pub fn open(path: PathBuf) -> Self {
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<HashMap<String, UsageEntry>>(&text).ok())
            .unwrap_or_default();

        Self { path, entries: Mutex::new(entries) }
    }

    pub fn snapshot(&self) -> HashMap<String, UsageEntry> {
        self.entries.lock().clone()
    }

    /// Count one use of `id` and persist.
    pub fn record(&self, id: &str, now_ms: i64) -> UsageEntry {
        let updated = {
            let mut entries = self.entries.lock();

            let entry = entries.entry(id.to_string()).or_default();
            entry.count = entry.count.saturating_add(1);
            entry.last_used_ms = now_ms;
            let updated = *entry;

            if entries.len() > MAX_ENTRIES {
                prune(&mut entries);
            }
            updated
        };

        self.persist();
        updated
    }

    pub fn clear(&self) {
        self.entries.lock().clear();
        self.persist();
    }

    fn persist(&self) {
        let entries = self.entries.lock().clone();
        let Ok(text) = serde_json::to_string(&entries) else {
            return;
        };
        write_atomically(&self.path, &text);
    }
}

/// Drop the least valuable quarter, oldest and least used first.
fn prune(entries: &mut HashMap<String, UsageEntry>) {
    let mut ordered: Vec<(String, UsageEntry)> =
        entries.iter().map(|(k, v)| (k.clone(), *v)).collect();
    // Most used first, then most recent; the tail of this is what goes.
    ordered.sort_by(|a, b| {
        b.1.count.cmp(&a.1.count).then_with(|| b.1.last_used_ms.cmp(&a.1.last_used_ms))
    });
    ordered.truncate(MAX_ENTRIES * 3 / 4);
    *entries = ordered.into_iter().collect();
}

/// Write via a temporary file and rename.
///
/// A power cut halfway through a direct write leaves a truncated file, and this
/// is written on every single palette action. The rename is atomic, so the file
/// on disk is always either the old contents or the new ones.
fn write_atomically(path: &Path, contents: &str) {
    let temporary = path.with_extension("json.tmp");
    if std::fs::write(&temporary, contents).is_err() {
        return;
    }
    if std::fs::rename(&temporary, path).is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (UsageStore, PathBuf) {
        let path = std::env::temp_dir().join(format!("caduceus-usage-{}.json", uuid::Uuid::new_v4()));
        (UsageStore::open(path.clone()), path)
    }

    #[test]
    fn a_missing_file_starts_empty_rather_than_failing() {
        let (store, path) = store();
        assert!(store.snapshot().is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn uses_accumulate_and_are_persisted() {
        let (store, path) = store();
        store.record("command:window.left_half", 1000);
        store.record("command:window.left_half", 2000);
        store.record("command:tool.uuid", 1500);

        let counts = store.snapshot();
        assert_eq!(counts["command:window.left_half"].count, 2);
        assert_eq!(counts["command:window.left_half"].last_used_ms, 2000);
        assert_eq!(counts["command:tool.uuid"].count, 1);

        // A second store over the same path sees the same numbers.
        let reopened = UsageStore::open(path.clone());
        assert_eq!(reopened.snapshot()["command:window.left_half"].count, 2);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_corrupt_file_is_ignored_rather_than_crashing() {
        let path = std::env::temp_dir().join(format!("caduceus-bad-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(&path, "{ this is not json").unwrap();

        let store = UsageStore::open(path.clone());
        assert!(store.snapshot().is_empty());

        // And it recovers: the next write replaces the bad file.
        store.record("x", 1);
        assert_eq!(UsageStore::open(path.clone()).snapshot()["x"].count, 1);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn clearing_removes_everything_from_disk_too() {
        let (store, path) = store();
        store.record("a", 1);
        store.clear();
        assert!(store.snapshot().is_empty());
        assert!(UsageStore::open(path.clone()).snapshot().is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn pruning_keeps_the_most_used_and_drops_the_tail() {
        let mut entries: HashMap<String, UsageEntry> = HashMap::new();
        for i in 0..(MAX_ENTRIES + 10) {
            entries.insert(
                format!("id-{i}"),
                // Later ids are used more, so the low-numbered ones should go.
                UsageEntry { count: i as u32, last_used_ms: i as i64 },
            );
        }
        prune(&mut entries);

        assert_eq!(entries.len(), MAX_ENTRIES * 3 / 4);
        // The most-used survived; the least-used did not.
        assert!(entries.contains_key(&format!("id-{}", MAX_ENTRIES + 9)));
        assert!(!entries.contains_key("id-0"));
    }

    #[test]
    fn the_count_saturates_rather_than_wrapping() {
        let (store, path) = store();
        {
            let mut entries = store.entries.lock();
            entries.insert("busy".into(), UsageEntry { count: u32::MAX, last_used_ms: 0 });
        }
        assert_eq!(store.record("busy", 5).count, u32::MAX);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_temporary_file_is_never_left_behind() {
        let (store, path) = store();
        store.record("a", 1);
        assert!(!path.with_extension("json.tmp").exists());
        std::fs::remove_file(&path).ok();
    }
}
