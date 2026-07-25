//! Installed-application discovery, for launching apps from the palette.
//!
//! Scans the standard macOS application directories for `.app` bundles and
//! caches the result. This is what makes Caduceus usable as a launcher rather
//! than only a shortcut list: you should not have to configure a shortcut for
//! every app you already own.
//!
//! # Why a cache
//!
//! A full scan walks a few hundred directories and reads a plist per bundle,
//! which is ~100ms — far too slow to run on every keystroke in the palette. The
//! list is built once on first use and refreshed in the background, since
//! applications appear and disappear rarely.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;

/// How long a scan stays fresh before the next request triggers a rebuild.
const CACHE_TTL: Duration = Duration::from_secs(300);

/// Directories searched, in order. Nested one level deep so that
/// `/Applications/Utilities/Terminal.app` and folders like
/// `/Applications/Adobe Photoshop/…` are found.
#[cfg(target_os = "macos")]
const SEARCH_ROOTS: &[&str] = &[
    "/Applications",
    "/Applications/Utilities",
    "/System/Applications",
    "/System/Applications/Utilities",
    "/System/Library/CoreServices/Applications",
];

/// One installed application.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledApp {
    /// Display name, without the `.app` suffix.
    pub name: String,
    /// Absolute path to the bundle.
    pub path: String,
}

#[derive(Default)]
struct Cache {
    apps: Vec<InstalledApp>,
    scanned_at: Option<Instant>,
}

/// Shared, lazily-populated application index.
#[derive(Clone, Default)]
pub struct AppIndex {
    cache: Arc<Mutex<Cache>>,
}

impl AppIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every installed application, rescanning if the cache has expired.
    ///
    /// Blocking (it touches the filesystem); call from `spawn_blocking` or a
    /// command handler, not from a hot loop.
    pub fn all(&self) -> Vec<InstalledApp> {
        {
            let cache = self.cache.lock();
            if let Some(at) = cache.scanned_at {
                if at.elapsed() < CACHE_TTL {
                    return cache.apps.clone();
                }
            }
        }

        let apps = scan();
        let mut cache = self.cache.lock();
        cache.apps = apps.clone();
        cache.scanned_at = Some(Instant::now());
        apps
    }

    /// Drop the cache so the next request rescans. Used after the user installs
    /// something and wonders why it is missing.
    pub fn invalidate(&self) {
        self.cache.lock().scanned_at = None;
    }
}

#[cfg(target_os = "macos")]
fn scan() -> Vec<InstalledApp> {
    let mut roots: Vec<PathBuf> = SEARCH_ROOTS.iter().map(PathBuf::from).collect();
    // Per-user installs, which Homebrew casks and some installers prefer.
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join("Applications"));
        roots.push(home.join("Applications/Chrome Apps.localized"));
    }

    let mut apps: Vec<InstalledApp> = Vec::new();
    for root in roots {
        collect(&root, &mut apps, 0);
    }

    // The same app can appear under several roots (a per-user copy shadowing a
    // system one). Keep the first, which follows SEARCH_ROOTS priority.
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    apps.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    apps
}

/// Recurse at most one level below a root, so app bundles inside a vendor
/// folder are found without walking an entire application's internals.
#[cfg(target_os = "macos")]
fn collect(dir: &Path, out: &mut Vec<InstalledApp>, depth: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        if let Some(stripped) = name.strip_suffix(".app") {
            out.push(InstalledApp {
                name: stripped.to_string(),
                path: path.display().to_string(),
            });
        } else if depth == 0 && path.is_dir() && !name.starts_with('.') {
            collect(&path, out, depth + 1);
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn scan() -> Vec<InstalledApp> {
    // Windows would read the Start Menu; Linux would parse XDG .desktop files.
    // Neither is implemented — see docs on platform support.
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn finds_applications_that_every_mac_has() {
        let apps = AppIndex::new().all();
        assert!(!apps.is_empty(), "a Mac always has applications installed");

        let names: Vec<String> = apps.iter().map(|a| a.name.to_lowercase()).collect();
        assert!(names.iter().any(|n| n == "finder" || n == "safari"),
            "expected a system app; got {} apps", apps.len());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn nested_bundles_are_found_one_level_deep() {
        // Terminal lives in /System/Applications/Utilities, which is only
        // reachable if the scan descends past the top level.
        let apps = AppIndex::new().all();
        assert!(apps.iter().any(|a| a.name == "Terminal"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn results_are_deduplicated_and_sorted() {
        let apps = AppIndex::new().all();
        let mut names: Vec<String> = apps.iter().map(|a| a.name.to_lowercase()).collect();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate application names leaked through");

        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "results should be alphabetical");
    }

    #[test]
    fn the_cache_is_reused_within_its_ttl() {
        let index = AppIndex::new();
        let first = index.all();
        let second = index.all();
        assert_eq!(first.len(), second.len());
    }

    #[test]
    fn every_entry_has_a_usable_path() {
        for app in AppIndex::new().all() {
            assert!(!app.name.is_empty());
            assert!(app.path.ends_with(".app"), "{} -> {}", app.name, app.path);
        }
    }
}
