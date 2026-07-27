//! `ctx.storage` — the one place an extension is allowed to keep something.
//!
//! An extension has no filesystem. That is deliberate, and it would make a
//! surprising number of useful extensions impossible: an exchange-rate lookup
//! wants to cache today's table, a "recent" list wants the list. So there is
//! exactly one store, it is keyed by extension id, and it holds JSON.
//!
//! One file per extension rather than one shared database, because that is what
//! makes "two extensions cannot read each other's keys" a property of the layout
//! instead of a rule someone has to keep obeying. The id has already been
//! through `id_from_filename`, so it is a filename and not a path.
//!
//! Both sizes below are caps, not budgets to spend. An extension that needs
//! more than a quarter of a megabyte of key/value state wants a file, and the
//! answer to that is still no.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::Value;

/// The most one extension may keep, serialised.
const MAX_BYTES: usize = 2 * 1024 * 1024;

/// The longest key. Long enough for a URL, short enough that a key cannot be
/// used as the value.
const MAX_KEY: usize = 256;

/// Serialises read-modify-write across extensions.
///
/// One lock for all of them rather than one per file: writes are rare, small,
/// and never on a path where a few microseconds of contention is visible.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

fn dir(app_data: &Path) -> PathBuf {
    super::extensions_dir(app_data).join("storage")
}

fn file(id: &str, app_data: &Path) -> PathBuf {
    dir(app_data).join(format!("{}.json", super::safe_id(id)))
}

fn read(id: &str, app_data: &Path) -> BTreeMap<String, Value> {
    std::fs::read_to_string(file(id, app_data))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn check_key(key: &str) -> Result<(), String> {
    if key.is_empty() {
        return Err("A storage key cannot be empty.".into());
    }
    if key.len() > MAX_KEY {
        return Err(format!("That storage key is longer than {MAX_KEY} characters."));
    }
    Ok(())
}

pub fn get(id: &str, app_data: &Path, key: &str) -> Result<Option<Value>, String> {
    check_key(key)?;
    Ok(read(id, app_data).get(key).cloned())
}

/// Write a key, or remove it when `value` is `None`.
pub fn set(id: &str, app_data: &Path, key: &str, value: Option<Value>) -> Result<(), String> {
    check_key(key)?;

    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let mut map = read(id, app_data);
    match value {
        Some(value) => {
            map.insert(key.to_string(), value);
        }
        None => {
            map.remove(key);
        }
    }

    let encoded = serde_json::to_string(&map).map_err(|e| format!("Could not save that: {e}"))?;
    if encoded.len() > MAX_BYTES {
        return Err(format!(
            "That would put this extension's storage over {} KB. Keep less, or keep it smaller.",
            MAX_BYTES / 1024
        ));
    }

    let dir = dir(app_data);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create {dir:?}: {e}"))?;
    std::fs::write(file(id, app_data), encoded).map_err(|e| format!("Could not save that: {e}"))
}

/// Drop everything an extension saved. Called when it is uninstalled.
pub fn forget(id: &str, app_data: &Path) {
    let _ = std::fs::remove_file(file(id, app_data));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "caduceus-ext-storage-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_value_survives_a_round_trip() {
        let data = tmp();
        set("thing", &data, "last", Some(Value::from("hello"))).unwrap();
        assert_eq!(get("thing", &data, "last").unwrap(), Some(Value::from("hello")));
    }

    #[test]
    fn setting_nothing_removes_the_key() {
        let data = tmp();
        set("thing", &data, "last", Some(Value::from(1))).unwrap();
        set("thing", &data, "last", None).unwrap();
        assert_eq!(get("thing", &data, "last").unwrap(), None);
    }

    /// The whole point of one file per extension.
    #[test]
    fn two_extensions_cannot_read_each_others_keys() {
        let data = tmp();
        set("alice", &data, "secret", Some(Value::from("a"))).unwrap();
        set("bob", &data, "secret", Some(Value::from("b"))).unwrap();
        assert_eq!(get("alice", &data, "secret").unwrap(), Some(Value::from("a")));
        assert_eq!(get("bob", &data, "secret").unwrap(), Some(Value::from("b")));
    }

    /// An id is a filename, not a path — including on the way into storage.
    #[test]
    fn a_traversing_id_cannot_write_outside_the_storage_directory() {
        let data = tmp();
        set("../../escape", &data, "k", Some(Value::from(1))).unwrap();
        assert!(dir(&data).join("escape.json").is_file());
        assert!(!data.join("escape.json").exists());
    }

    #[test]
    fn an_oversized_value_is_refused_rather_than_written() {
        let data = tmp();
        let huge = "x".repeat(MAX_BYTES + 1);
        let err = set("thing", &data, "big", Some(Value::from(huge))).unwrap_err();
        assert!(err.contains("KB"));
        assert_eq!(get("thing", &data, "big").unwrap(), None);
    }

    #[test]
    fn uninstalling_takes_the_storage_with_it() {
        let data = tmp();
        set("thing", &data, "k", Some(Value::from(1))).unwrap();
        forget("thing", &data);
        assert_eq!(get("thing", &data, "k").unwrap(), None);
    }
}
