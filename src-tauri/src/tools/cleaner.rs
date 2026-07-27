//! Disk space: what is taking it, and getting it back.
//!
//! The job CleanMyMac, CCleaner and Clean-Me do, with three rules that most of
//! that category does not follow:
//!
//! 1. **Nothing is deleted. Everything goes to the Trash.** A cleaner that
//!    unlinks files is one bug away from being the worst program on the
//!    machine, and "I got it back out of the Trash" is the difference between
//!    an annoyance and a disaster.
//! 2. **Nothing is selected for you.** Every category is listed with its size
//!    and what it actually is; you tick what you want gone. A cleaner with a
//!    big green button decides on your behalf what you were not using.
//! 3. **Nothing that is not safely regenerable is offered.** No documents, no
//!    Photos library, no mail store. Caches, logs, and the leftovers of apps
//!    you have already removed — things whose worst case is that something is
//!    slow once while it rebuilds them.
//!
//! Sizes are measured, not estimated, which is why scanning takes a moment on a
//! full disk. Guessing would be faster and would make the numbers fiction.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::files::human_size;

/// A kind of reclaimable space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JunkKind {
    UserCaches,
    UserLogs,
    Trash,
    Downloads,
    DerivedData,
    Simulators,
    BrewCache,
    NpmCache,
    PipCache,
    CargoRegistry,
    DockerCache,
    Screenshots,
    MailDownloads,
    OldIosBackups,
}

impl JunkKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::UserCaches => "Application caches",
            Self::UserLogs => "Logs",
            Self::Trash => "Trash",
            Self::Downloads => "Downloads older than 30 days",
            Self::DerivedData => "Xcode derived data",
            Self::Simulators => "iOS simulator devices",
            Self::BrewCache => "Homebrew downloads",
            Self::NpmCache => "npm cache",
            Self::PipCache => "pip cache",
            Self::CargoRegistry => "Cargo registry cache",
            Self::DockerCache => "Docker build cache",
            Self::Screenshots => "Screenshots on the Desktop",
            Self::MailDownloads => "Mail attachment downloads",
            Self::OldIosBackups => "iOS device backups",
        }
    }

    /// What it is and what it costs you, in a sentence. Shown next to the tick.
    pub fn detail(self) -> &'static str {
        match self {
            Self::UserCaches => {
                "Files apps keep so they do not have to redo work. Every one of them is \
                 rebuilt on demand; the cost of removing them is that a few apps are slow once."
            }
            Self::UserLogs => "Diagnostic text apps write and almost never read back.",
            Self::Trash => "Already deleted. This empties it.",
            Self::Downloads => {
                "Files in ~/Downloads untouched for over a month. Look through this one — it is \
                 the only category here that can contain something you meant to keep."
            }
            Self::DerivedData => {
                "Xcode's build intermediates. Removing them means the next build of each \
                 project is a full one."
            }
            Self::Simulators => "Unavailable simulator runtimes and their device images.",
            Self::BrewCache => "Downloaded bottles and archives Homebrew has already installed.",
            Self::NpmCache => "npm's package tarball cache. Re-downloaded when next needed.",
            Self::PipCache => "pip's wheel cache. Re-downloaded when next needed.",
            Self::CargoRegistry => {
                "Cargo's downloaded crate sources and their caches. Re-fetched on the next build \
                 that needs them."
            }
            Self::DockerCache => "Dangling images and the build cache.",
            Self::Screenshots => "Files named like screenshots sitting on your Desktop.",
            Self::MailDownloads => "Attachments Mail saved while you were reading messages.",
            Self::OldIosBackups => {
                "Full device backups. Large, and worth keeping unless you know you have another \
                 copy — this one is off by default for a reason."
            }
        }
    }

    /// Whether removing this could plausibly lose something the user wanted.
    ///
    /// Drives a warning in the UI, and these are never pre-ticked.
    pub fn risky(self) -> bool {
        matches!(self, Self::Downloads | Self::OldIosBackups | Self::Screenshots)
    }

    /// Where it lives. Several kinds are more than one directory.
    fn roots(self) -> Vec<PathBuf> {
        let home = match dirs::home_dir() {
            Some(home) => home,
            None => return Vec::new(),
        };
        let library = home.join("Library");

        match self {
            Self::UserCaches => vec![library.join("Caches")],
            Self::UserLogs => vec![library.join("Logs")],
            Self::Trash => vec![home.join(".Trash")],
            Self::Downloads => vec![home.join("Downloads")],
            Self::DerivedData => vec![library.join("Developer/Xcode/DerivedData")],
            Self::Simulators => vec![library.join("Developer/CoreSimulator/Caches")],
            Self::BrewCache => vec![library.join("Caches/Homebrew")],
            Self::NpmCache => vec![home.join(".npm/_cacache")],
            Self::PipCache => vec![library.join("Caches/pip")],
            Self::CargoRegistry => vec![home.join(".cargo/registry/cache")],
            Self::DockerCache => vec![home.join(".docker/buildx")],
            Self::Screenshots => vec![home.join("Desktop")],
            Self::MailDownloads => vec![library.join("Containers/com.apple.mail/Data/Library/Mail Downloads")],
            Self::OldIosBackups => vec![library.join("Application Support/MobileSync/Backup")],
        }
    }

    pub const ALL: &'static [JunkKind] = &[
        JunkKind::UserCaches,
        JunkKind::UserLogs,
        JunkKind::Trash,
        JunkKind::Downloads,
        JunkKind::DerivedData,
        JunkKind::Simulators,
        JunkKind::BrewCache,
        JunkKind::NpmCache,
        JunkKind::PipCache,
        JunkKind::CargoRegistry,
        JunkKind::DockerCache,
        JunkKind::Screenshots,
        JunkKind::MailDownloads,
        JunkKind::OldIosBackups,
    ];
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JunkGroup {
    pub kind: JunkKind,
    pub label: String,
    pub detail: String,
    pub risky: bool,
    pub bytes: u64,
    pub human: String,
    pub items: usize,
    /// The paths that would go to the Trash. Sent so the UI can show them and
    /// so removal operates on exactly what was measured.
    pub paths: Vec<String>,
}

/// How deep to walk when measuring, and how many entries to keep per group.
///
/// A cache directory can hold hundreds of thousands of files. Measuring every
/// one is what makes this honest; *listing* every one would be a megabyte of
/// JSON describing a list nobody scrolls to the end of.
const MAX_LISTED: usize = 400;

/// Measure everything reclaimable. Reads only; changes nothing.
pub fn scan() -> Vec<JunkGroup> {
    JunkKind::ALL.iter().map(|kind| scan_kind(*kind)).collect()
}

fn scan_kind(kind: JunkKind) -> JunkGroup {
    let mut bytes = 0u64;
    let mut paths = Vec::new();
    let mut items = 0usize;

    for root in kind.roots() {
        let Ok(entries) = std::fs::read_dir(&root) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_candidate(kind, &path) {
                continue;
            }
            let size = directory_size(&path);
            if size == 0 {
                continue;
            }
            bytes += size;
            items += 1;
            if paths.len() < MAX_LISTED {
                paths.push(path.to_string_lossy().into_owned());
            }
        }
    }

    JunkGroup {
        kind,
        label: kind.label().into(),
        detail: kind.detail().into(),
        risky: kind.risky(),
        bytes,
        human: human_size(bytes),
        items,
        paths,
    }
}

/// Whether a particular entry belongs to this category.
///
/// Most categories are "everything in this directory". The three that are not
/// are the three that live in a directory full of things the user cares about.
fn is_candidate(kind: JunkKind, path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if name.starts_with('.') && kind != JunkKind::Trash {
        return false;
    }

    match kind {
        // ~/Downloads holds things people meant to keep. Only what has sat
        // untouched for a month is even offered, and it is never pre-ticked.
        JunkKind::Downloads => older_than_days(path, 30),
        // The Desktop is somebody's working surface. Only files macOS itself
        // named as screenshots.
        JunkKind::Screenshots => {
            let lower = name.to_lowercase();
            (lower.starts_with("screenshot") || lower.starts_with("screen shot"))
                && (lower.ends_with(".png") || lower.ends_with(".jpg"))
        }
        // Caduceus's own cache is not junk to be offered while it is running.
        JunkKind::UserCaches => !name.contains("com.caduceus"),
        _ => true,
    }
}

fn older_than_days(path: &Path, days: u64) -> bool {
    let Ok(meta) = std::fs::metadata(path) else { return false };
    let Ok(modified) = meta.modified() else { return false };
    modified
        .elapsed()
        .map(|age| age.as_secs() > days * 86_400)
        .unwrap_or(false)
}

/// Recursive size, in bytes.
///
/// Symlinks are counted as their own (tiny) size and never followed: a cache
/// containing a link to your home directory should not report your home
/// directory's size, and must certainly not be walked.
fn directory_size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    if meta.file_type().is_symlink() {
        return meta.len();
    }
    if meta.is_file() {
        return meta.len();
    }

    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];
    // Bounded so a pathological tree cannot spin here forever.
    let mut visited = 0usize;

    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            visited += 1;
            if visited > 200_000 {
                return total;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    total
}

// ---------------------------------------------------------------------------
// Installed applications
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledApp {
    pub name: String,
    pub path: String,
    pub bundle_id: Option<String>,
    pub bytes: u64,
    pub human: String,
    /// Epoch seconds of last use, where macOS records it.
    pub last_opened: Option<u64>,
}

/// Every application in /Applications and ~/Applications, with its real size.
pub fn installed_apps() -> Vec<InstalledApp> {
    let mut seen = BTreeSet::new();
    let mut apps = Vec::new();

    let mut roots = vec![PathBuf::from("/Applications")];
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join("Applications"));
    }

    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("app") {
                continue;
            }
            let name = path
                .file_stem()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if !seen.insert(name.clone()) {
                continue;
            }

            let bytes = directory_size(&path);
            apps.push(InstalledApp {
                bundle_id: super::files::bundle_id(&path.to_string_lossy()),
                name,
                path: path.to_string_lossy().into_owned(),
                bytes,
                human: human_size(bytes),
                last_opened: last_opened(&path),
            });
        }
    }

    apps.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    apps
}

/// When the app was last used, if macOS's metadata store knows.
///
/// `mdls` rather than the file's own timestamps: an app's mtime is when it was
/// installed or updated, which says nothing about whether anyone has opened it.
fn last_opened(path: &Path) -> Option<u64> {
    let out = std::process::Command::new("mdls")
        .args(["-raw", "-name", "kMDItemLastUsedDate"])
        .arg(path)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed == "(null)" {
        return None;
    }
    // "2026-07-21 18:04:11 +0000"
    let date = chrono::NaiveDateTime::parse_from_str(&trimmed[..19.min(trimmed.len())], "%Y-%m-%d %H:%M:%S")
        .ok()?;
    Some(date.and_utc().timestamp().max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_is_described_well_enough_to_tick_confidently() {
        // A cleaner that offers "Caches — 4.2 GB" with no explanation is asking
        // for a decision it has given you nothing to make.
        for kind in JunkKind::ALL {
            assert!(!kind.label().is_empty(), "{kind:?} has no label");
            assert!(
                kind.detail().len() > 30,
                "{kind:?} does not say what it is or what removing it costs"
            );
        }
    }

    #[test]
    fn the_categories_that_can_lose_something_are_marked() {
        // These three sit in directories full of things people care about, so
        // the UI has to warn and must never pre-tick them.
        assert!(JunkKind::Downloads.risky());
        assert!(JunkKind::Screenshots.risky());
        assert!(JunkKind::OldIosBackups.risky());
        // Caches and logs regenerate; treating them as risky would be crying wolf.
        assert!(!JunkKind::UserCaches.risky());
        assert!(!JunkKind::UserLogs.risky());
    }

    #[test]
    fn only_month_old_downloads_are_offered() {
        // The guard that keeps this from suggesting the file you saved an hour
        // ago. A fresh temporary file stands in for exactly that.
        let dir = std::env::temp_dir().join(format!("caduceus-clean-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let fresh = dir.join("just-downloaded.zip");
        std::fs::write(&fresh, b"x").unwrap();

        assert!(!is_candidate(JunkKind::Downloads, &fresh));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_screenshots_are_taken_from_the_desktop() {
        let desktop = Path::new("/Users/someone/Desktop");
        assert!(is_candidate(JunkKind::Screenshots, &desktop.join("Screenshot 2026-01-01.png")));
        // Somebody's actual work, which happens to live on the Desktop.
        assert!(!is_candidate(JunkKind::Screenshots, &desktop.join("thesis-final.png")));
        assert!(!is_candidate(JunkKind::Screenshots, &desktop.join("Screenshot notes.txt")));
    }

    #[test]
    fn caduceus_does_not_offer_to_delete_its_own_running_cache() {
        let caches = Path::new("/Users/someone/Library/Caches");
        assert!(!is_candidate(JunkKind::UserCaches, &caches.join("com.caduceus.desktop")));
        assert!(is_candidate(JunkKind::UserCaches, &caches.join("com.example.other")));
    }

    #[test]
    fn hidden_entries_are_skipped_except_in_the_trash() {
        let caches = Path::new("/Users/someone/Library/Caches");
        assert!(!is_candidate(JunkKind::UserCaches, &caches.join(".DS_Store")));
        // The Trash is full of dotfiles and emptying it means emptying it.
        let trash = Path::new("/Users/someone/.Trash");
        assert!(is_candidate(JunkKind::Trash, &trash.join(".hidden-thing")));
    }

    #[test]
    fn measuring_a_missing_path_is_zero_rather_than_an_error() {
        assert_eq!(directory_size(Path::new("/nowhere/at/all")), 0);
    }
}
