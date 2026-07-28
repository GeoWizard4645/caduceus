//! Browser discovery and launching.
//!
//! Two families, deliberately handled differently:
//!
//! * **Chromium-family** (Chrome, Brave, Edge, Arc, Vivaldi…) keep a
//!   `Local State` JSON file listing their profiles. Reading it lets Settings
//!   offer a real "Personal / Work / School" dropdown instead of asking someone
//!   to guess that their work profile lives in a directory called `Profile 3`.
//!   These are launched by *executable* so `--profile-directory` can be passed.
//! * **Everything else** (Safari, Firefox, Orion…) has no equivalent
//!   command-line profile switch, so they are launched by bundle id / binary
//!   name and the profile setting simply does not apply. The UI hides the
//!   profile control for them rather than showing one that silently does
//!   nothing.
//!
//! Everything here degrades to `None` / an empty list rather than erroring: a
//! machine with no browser but Safari is a perfectly valid machine.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Which browser a URL should open in.
///
/// An empty `browser_id` means "whatever the OS considers the default". That is
/// both the safe default and the only correct answer on a machine whose browser
/// Caduceus does not know about, so it stays the fallback everywhere rather
/// than an error. `profile` is a Chromium `--profile-directory` value and is
/// ignored by browsers that have no equivalent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BrowserChoice {
    pub browser_id: String,
    pub profile: Option<String>,
}

impl BrowserChoice {
    /// True when this defers to the OS default browser.
    pub fn is_system_default(&self) -> bool {
        self.browser_id.trim().is_empty()
    }
}

/// One profile inside a Chromium install.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProfile {
    /// The `--profile-directory` value, e.g. `Default`, `Profile 1`.
    pub directory: String,
    /// The human name shown in the browser's profile switcher.
    pub name: String,
    /// Signed-in account, when the browser recorded one.
    pub email: Option<String>,
}

/// A detected browser and, for Chromium forks, the profiles inside it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserInstall {
    /// Stable id, e.g. `chrome`, `brave`, `safari`.
    pub id: String,
    pub display_name: String,
    /// Bundle id (macOS) or executable name, used to launch it.
    pub launch_target: String,
    /// Whether `--profile-directory` is supported. Drives whether the Settings
    /// UI offers a profile picker at all.
    pub chromium: bool,
    pub profiles: Vec<BrowserProfile>,
}

struct Candidate {
    id: &'static str,
    display_name: &'static str,
    /// Path segments under the platform's application-data root. `None` for
    /// browsers with no Chromium-style profile directory.
    user_data_rel: Option<&'static [&'static str]>,
    /// Bundle id on macOS, executable name elsewhere.
    launch_target: &'static str,
    /// macOS `.app` bundle name, used both to detect the install and to reach
    /// the executable inside it for `--profile-directory`. Unused elsewhere,
    /// where `launch_target` is already the binary.
    app_name: &'static str,
}

impl Candidate {
    fn chromium(&self) -> bool {
        self.user_data_rel.is_some()
    }
}

#[cfg(target_os = "macos")]
const CANDIDATES: &[Candidate] = &[
    Candidate {
        id: "chrome",
        display_name: "Google Chrome",
        user_data_rel: Some(&["Google", "Chrome"]),
        launch_target: "com.google.Chrome",
        app_name: "Google Chrome",
    },
    Candidate {
        id: "chrome-beta",
        display_name: "Chrome Beta",
        user_data_rel: Some(&["Google", "Chrome Beta"]),
        launch_target: "com.google.Chrome.beta",
        app_name: "Google Chrome Beta",
    },
    Candidate {
        id: "chromium",
        display_name: "Chromium",
        user_data_rel: Some(&["Chromium"]),
        launch_target: "org.chromium.Chromium",
        app_name: "Chromium",
    },
    Candidate {
        id: "brave",
        display_name: "Brave",
        user_data_rel: Some(&["BraveSoftware", "Brave-Browser"]),
        launch_target: "com.brave.Browser",
        app_name: "Brave Browser",
    },
    Candidate {
        id: "edge",
        display_name: "Microsoft Edge",
        user_data_rel: Some(&["Microsoft Edge"]),
        launch_target: "com.microsoft.edgemac",
        app_name: "Microsoft Edge",
    },
    Candidate {
        id: "vivaldi",
        display_name: "Vivaldi",
        user_data_rel: Some(&["Vivaldi"]),
        launch_target: "com.vivaldi.Vivaldi",
        app_name: "Vivaldi",
    },
    Candidate {
        id: "arc",
        display_name: "Arc",
        user_data_rel: Some(&["Arc", "User Data"]),
        launch_target: "company.thebrowser.Browser",
        app_name: "Arc",
    },
    Candidate {
        id: "safari",
        display_name: "Safari",
        user_data_rel: None,
        launch_target: "com.apple.Safari",
        app_name: "Safari",
    },
    Candidate {
        id: "firefox",
        display_name: "Firefox",
        user_data_rel: None,
        launch_target: "org.mozilla.firefox",
        app_name: "Firefox",
    },
    Candidate {
        id: "zen",
        display_name: "Zen Browser",
        user_data_rel: None,
        launch_target: "app.zen-browser.zen",
        app_name: "Zen Browser",
    },
    Candidate {
        id: "orion",
        display_name: "Orion",
        user_data_rel: None,
        launch_target: "com.kagi.kagimacOS",
        app_name: "Orion",
    },
];

#[cfg(target_os = "windows")]
const CANDIDATES: &[Candidate] = &[
    Candidate {
        id: "chrome",
        display_name: "Google Chrome",
        user_data_rel: Some(&["Google", "Chrome", "User Data"]),
        launch_target: "chrome.exe",
        app_name: "chrome.exe",
    },
    Candidate {
        id: "chromium",
        display_name: "Chromium",
        user_data_rel: Some(&["Chromium", "User Data"]),
        launch_target: "chromium.exe",
        app_name: "chromium.exe",
    },
    Candidate {
        id: "brave",
        display_name: "Brave",
        user_data_rel: Some(&["BraveSoftware", "Brave-Browser", "User Data"]),
        launch_target: "brave.exe",
        app_name: "brave.exe",
    },
    Candidate {
        id: "edge",
        display_name: "Microsoft Edge",
        user_data_rel: Some(&["Microsoft", "Edge", "User Data"]),
        launch_target: "msedge.exe",
        app_name: "msedge.exe",
    },
    Candidate {
        id: "firefox",
        display_name: "Firefox",
        user_data_rel: None,
        launch_target: "firefox.exe",
        app_name: "firefox.exe",
    },
];

#[cfg(all(unix, not(target_os = "macos")))]
const CANDIDATES: &[Candidate] = &[
    Candidate {
        id: "chrome",
        display_name: "Google Chrome",
        user_data_rel: Some(&["google-chrome"]),
        launch_target: "google-chrome",
        app_name: "google-chrome",
    },
    Candidate {
        id: "chromium",
        display_name: "Chromium",
        user_data_rel: Some(&["chromium"]),
        launch_target: "chromium",
        app_name: "chromium",
    },
    Candidate {
        id: "brave",
        display_name: "Brave",
        user_data_rel: Some(&["BraveSoftware", "Brave-Browser"]),
        launch_target: "brave-browser",
        app_name: "brave-browser",
    },
    Candidate {
        id: "edge",
        display_name: "Microsoft Edge",
        user_data_rel: Some(&["microsoft-edge"]),
        launch_target: "microsoft-edge",
        app_name: "microsoft-edge",
    },
    Candidate {
        id: "firefox",
        display_name: "Firefox",
        user_data_rel: None,
        launch_target: "firefox",
        app_name: "firefox",
    },
];

/// Root directory that browser user-data lives under on this platform.
fn app_data_root() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        // ~/Library/Application Support
        dirs::config_dir()
    }
    #[cfg(target_os = "windows")]
    {
        // %LOCALAPPDATA%
        dirs::data_local_dir()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // ~/.config
        dirs::config_dir()
    }
}

/// Locate the executable for a candidate, if it is installed.
///
/// On macOS this is the binary *inside* the bundle rather than the bundle id,
/// because `open -b` cannot forward `--profile-directory` to Chromium.
fn executable(candidate: &Candidate) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let roots = [
            PathBuf::from("/Applications"),
            PathBuf::from("/System/Applications"),
            dirs::home_dir()?.join("Applications"),
        ];
        for root in roots {
            let p = root.join(format!(
                "{name}.app/Contents/MacOS/{name}",
                name = candidate.app_name
            ));
            if p.is_file() {
                return Some(p);
            }
        }
        None
    }

    #[cfg(target_os = "windows")]
    {
        let roots = [
            std::env::var_os("PROGRAMFILES").map(PathBuf::from),
            std::env::var_os("PROGRAMFILES(X86)").map(PathBuf::from),
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        ];
        // Chromium installers nest the exe under a vendor/Application path; the
        // user-data segments happen to describe the same vendor folders.
        let vendor: Vec<&str> = candidate
            .user_data_rel
            .unwrap_or(&[])
            .iter()
            .filter(|s| **s != "User Data")
            .copied()
            .collect();
        for root in roots.into_iter().flatten() {
            let mut p = root.clone();
            for seg in &vendor {
                p.push(seg);
            }
            p.push("Application");
            p.push(candidate.app_name);
            if p.is_file() {
                return Some(p);
            }
            let flat = root.join(candidate.app_name);
            if flat.is_file() {
                return Some(flat);
            }
        }
        None
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let path = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path) {
            let p = dir.join(candidate.app_name);
            if p.is_file() {
                return Some(p);
            }
        }
        None
    }
}

/// Enumerate installed browsers and, for Chromium forks, their profiles.
///
/// Returns an empty vector when nothing is found, which the Settings UI turns
/// into "use the system default" rather than an error.
pub fn detect_browsers() -> Vec<BrowserInstall> {
    let root = app_data_root();

    CANDIDATES
        .iter()
        .filter_map(|c| {
            let profiles = match (c.user_data_rel, root.as_ref()) {
                (Some(rel), Some(root)) => {
                    let mut user_data = root.clone();
                    for seg in rel {
                        user_data.push(seg);
                    }
                    if !user_data.exists() {
                        return None;
                    }
                    let found = read_local_state(&user_data.join("Local State"))
                        .unwrap_or_else(|| scan_profile_dirs(&user_data));
                    if found.is_empty() {
                        return None;
                    }
                    found
                }
                // Chromium fork on a platform with no app-data root: unusable.
                (Some(_), None) => return None,
                // Non-Chromium: presence of the application is the only signal.
                (None, _) => {
                    executable(c)?;
                    Vec::new()
                }
            };

            Some(BrowserInstall {
                id: c.id.to_string(),
                display_name: c.display_name.to_string(),
                launch_target: c.launch_target.to_string(),
                chromium: c.chromium(),
                profiles,
            })
        })
        .collect()
}

/// Parse `Local State` → `profile.info_cache`, which maps a profile directory
/// name to its metadata.
fn read_local_state(path: &Path) -> Option<Vec<BrowserProfile>> {
    let text = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let cache = json.get("profile")?.get("info_cache")?.as_object()?;

    let mut profiles: Vec<BrowserProfile> = cache
        .iter()
        .map(|(directory, meta)| BrowserProfile {
            directory: directory.clone(),
            name: meta
                .get("name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(directory)
                .to_string(),
            email: meta
                .get("user_name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        })
        .collect();

    // "Default" first, then Profile 1, Profile 2, … then anything else.
    profiles.sort_by_key(|p| {
        (
            p.directory != "Default",
            p.directory
                .strip_prefix("Profile ")
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(u32::MAX),
            p.directory.clone(),
        )
    });
    Some(profiles)
}

/// Fallback for installs whose `Local State` is missing or unparseable: look for
/// directories that contain a `Preferences` file.
fn scan_profile_dirs(user_data: &Path) -> Vec<BrowserProfile> {
    let Ok(entries) = std::fs::read_dir(user_data) else {
        return Vec::new();
    };
    let mut out: Vec<BrowserProfile> = entries
        .flatten()
        .filter(|e| e.path().join("Preferences").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name == "Default" || name.starts_with("Profile "))
        .map(|name| BrowserProfile {
            directory: name.clone(),
            name,
            email: None,
        })
        .collect();
    out.sort_by(|a, b| a.directory.cmp(&b.directory));
    out
}

/// How a chosen browser should be launched.
pub enum Launch {
    /// Spawn this executable directly, so `--profile-directory` can be passed.
    Binary(PathBuf),
    /// Hand the URL to the OS with this bundle id / executable name.
    Target(&'static str),
}

/// Resolve a browser id to something launchable, or `None` if it is unknown or
/// not installed.
pub fn resolve(browser_id: &str) -> Option<Launch> {
    let candidate = CANDIDATES.iter().find(|c| c.id == browser_id)?;
    match executable(candidate) {
        Some(path) if candidate.chromium() => Some(Launch::Binary(path)),
        Some(_) | None if !candidate.chromium() => Some(Launch::Target(candidate.launch_target)),
        _ => None,
    }
}

/// Whether a browser id supports `--profile-directory`.
pub fn supports_profiles(browser_id: &str) -> bool {
    CANDIDATES
        .iter()
        .find(|c| c.id == browser_id)
        .is_some_and(Candidate::chromium)
}

/// Find a Chromium candidate by the string an `OpenApp` shortcut's `target`
/// carries — a bundle id on macOS, an executable name elsewhere — rather than
/// by our own internal id. Case-insensitive because bundle ids are typed by
/// hand into Settings and macOS itself treats them without regard to case.
fn candidate_by_launch_target(target: &str) -> Option<&'static Candidate> {
    let target = target.trim();
    CANDIDATES
        .iter()
        .find(|c| c.chromium() && c.launch_target.eq_ignore_ascii_case(target))
}

/// The binary for a Chromium browser an `OpenApp` shortcut is launching, if
/// that browser is installed and findable on disk.
///
/// `open_app` needs this for the exact reason `open_url` launches Chromium
/// browsers by binary instead of through `open -b/-a … --args`: macOS only
/// forwards `--args` when it starts the app fresh. If the browser is already
/// running — the common case — `open` hands the request to that process and
/// silently drops everything after `--args`, so a shortcut like "open Chrome
/// in my Work profile" just reopens whatever profile happened to be active.
pub fn chromium_binary_for_target(target: &str) -> Option<PathBuf> {
    executable(candidate_by_launch_target(target)?)
}

/// The default Chromium launch target on this platform, for the seeded
/// "Chrome" shortcut.
pub fn default_chrome_launch_target() -> &'static str {
    CANDIDATES
        .iter()
        .find(|c| c.id == "chrome")
        .map(|c| c.launch_target)
        .unwrap_or("google-chrome")
}

#[cfg(test)]
mod tests {
    use super::*;

    // These pin down `candidate_by_launch_target` — the matching `open_app`
    // relies on to know whether a shortcut's target needs the `--args`
    // workaround — without touching disk, so they run the same whether or not
    // the browser is actually installed on the machine running the tests.

    #[cfg(target_os = "macos")]
    #[test]
    fn matches_a_chromium_bundle_id() {
        let c = candidate_by_launch_target("com.google.Chrome").unwrap();
        assert_eq!(c.id, "chrome");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn matching_is_case_insensitive() {
        // Bundle ids are hand-typed into Settings; macOS itself does not care
        // about case, so a shortcut should not silently miss the workaround
        // over a capitalization mismatch.
        let c = candidate_by_launch_target("COM.GOOGLE.CHROME").unwrap();
        assert_eq!(c.id, "chrome");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn tolerates_surrounding_whitespace() {
        let c = candidate_by_launch_target("  com.google.Chrome  ").unwrap();
        assert_eq!(c.id, "chrome");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn non_chromium_browsers_are_excluded() {
        // Safari has no `--profile-directory` equivalent, so routing it
        // through a binary instead of `open -b` would buy nothing and only
        // risks resolving a path `open` would have found some other way.
        assert!(candidate_by_launch_target("com.apple.Safari").is_none());
    }

    #[test]
    fn unknown_targets_do_not_match() {
        assert!(candidate_by_launch_target("com.example.NotABrowser").is_none());
        assert!(candidate_by_launch_target("").is_none());
    }
}
