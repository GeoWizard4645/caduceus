//! Chromium profile discovery and browser launching.
//!
//! Chrome (and every Chromium fork) keeps a `Local State` JSON file listing the
//! profiles it knows about. Reading it lets Settings offer a real dropdown of
//! "Personal / Work / School" instead of asking a user to guess that their work
//! profile lives in a directory literally called `Profile 3`.
//!
//! Everything here degrades to `None` / an empty list rather than erroring: a
//! machine with no Chrome installed is a perfectly valid machine.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One profile inside a Chromium install.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChromeProfile {
    /// The `--profile-directory` value, e.g. `Default`, `Profile 1`.
    pub directory: String,
    /// The human name shown in Chrome's profile switcher.
    pub name: String,
    /// Signed-in account, when Chrome recorded one.
    pub email: Option<String>,
}

/// A detected Chromium-family browser and the profiles inside it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChromeInstall {
    /// Stable id, e.g. `chrome`, `brave`, `edge`.
    pub id: String,
    pub display_name: String,
    /// Path/bundle-id used to launch it.
    pub launch_target: String,
    pub profiles: Vec<ChromeProfile>,
}

/// Every Chromium-family browser Caduceus knows how to introspect.
///
/// `(id, display name, user-data dir relative to the platform root, launch target)`
struct Candidate {
    id: &'static str,
    display_name: &'static str,
    /// Path segments appended to the platform's application-data root.
    user_data_rel: &'static [&'static str],
    launch_target: &'static str,
}

#[cfg(target_os = "macos")]
const CANDIDATES: &[Candidate] = &[
    Candidate {
        id: "chrome",
        display_name: "Google Chrome",
        user_data_rel: &["Google", "Chrome"],
        launch_target: "com.google.Chrome",
    },
    Candidate {
        id: "chrome-beta",
        display_name: "Chrome Beta",
        user_data_rel: &["Google", "Chrome Beta"],
        launch_target: "com.google.Chrome.beta",
    },
    Candidate {
        id: "chromium",
        display_name: "Chromium",
        user_data_rel: &["Chromium"],
        launch_target: "org.chromium.Chromium",
    },
    Candidate {
        id: "brave",
        display_name: "Brave",
        user_data_rel: &["BraveSoftware", "Brave-Browser"],
        launch_target: "com.brave.Browser",
    },
    Candidate {
        id: "edge",
        display_name: "Microsoft Edge",
        user_data_rel: &["Microsoft Edge"],
        launch_target: "com.microsoft.edgemac",
    },
    Candidate {
        id: "vivaldi",
        display_name: "Vivaldi",
        user_data_rel: &["Vivaldi"],
        launch_target: "com.vivaldi.Vivaldi",
    },
    Candidate {
        id: "arc",
        display_name: "Arc",
        user_data_rel: &["Arc", "User Data"],
        launch_target: "company.thebrowser.Browser",
    },
];

#[cfg(target_os = "windows")]
const CANDIDATES: &[Candidate] = &[
    Candidate {
        id: "chrome",
        display_name: "Google Chrome",
        user_data_rel: &["Google", "Chrome", "User Data"],
        launch_target: "chrome.exe",
    },
    Candidate {
        id: "chromium",
        display_name: "Chromium",
        user_data_rel: &["Chromium", "User Data"],
        launch_target: "chromium.exe",
    },
    Candidate {
        id: "brave",
        display_name: "Brave",
        user_data_rel: &["BraveSoftware", "Brave-Browser", "User Data"],
        launch_target: "brave.exe",
    },
    Candidate {
        id: "edge",
        display_name: "Microsoft Edge",
        user_data_rel: &["Microsoft", "Edge", "User Data"],
        launch_target: "msedge.exe",
    },
];

#[cfg(all(unix, not(target_os = "macos")))]
const CANDIDATES: &[Candidate] = &[
    Candidate {
        id: "chrome",
        display_name: "Google Chrome",
        user_data_rel: &["google-chrome"],
        launch_target: "google-chrome",
    },
    Candidate {
        id: "chromium",
        display_name: "Chromium",
        user_data_rel: &["chromium"],
        launch_target: "chromium",
    },
    Candidate {
        id: "brave",
        display_name: "Brave",
        user_data_rel: &["BraveSoftware", "Brave-Browser"],
        launch_target: "brave-browser",
    },
    Candidate {
        id: "edge",
        display_name: "Microsoft Edge",
        user_data_rel: &["microsoft-edge"],
        launch_target: "microsoft-edge",
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

/// Enumerate installed Chromium-family browsers and their profiles.
///
/// Returns an empty vector when nothing is found, which the Settings UI turns
/// into a manual text-entry fallback rather than an error.
pub fn detect_chrome_profiles() -> Vec<ChromeInstall> {
    let Some(root) = app_data_root() else {
        log::debug!("no application-data directory on this platform");
        return Vec::new();
    };

    CANDIDATES
        .iter()
        .filter_map(|c| {
            let mut user_data = root.clone();
            for seg in c.user_data_rel {
                user_data.push(seg);
            }
            if !user_data.exists() {
                return None;
            }
            let profiles = read_local_state(&user_data.join("Local State"))
                .unwrap_or_else(|| scan_profile_dirs(&user_data));
            if profiles.is_empty() {
                return None;
            }
            Some(ChromeInstall {
                id: c.id.to_string(),
                display_name: c.display_name.to_string(),
                launch_target: c.launch_target.to_string(),
                profiles,
            })
        })
        .collect()
}

/// Parse `Local State` → `profile.info_cache`, which maps a profile directory
/// name to its metadata.
fn read_local_state(path: &Path) -> Option<Vec<ChromeProfile>> {
    let text = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let cache = json.get("profile")?.get("info_cache")?.as_object()?;

    let mut profiles: Vec<ChromeProfile> = cache
        .iter()
        .map(|(directory, meta)| ChromeProfile {
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
fn scan_profile_dirs(user_data: &Path) -> Vec<ChromeProfile> {
    let Ok(entries) = std::fs::read_dir(user_data) else {
        return Vec::new();
    };
    let mut out: Vec<ChromeProfile> = entries
        .flatten()
        .filter(|e| e.path().join("Preferences").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name == "Default" || name.starts_with("Profile "))
        .map(|name| ChromeProfile {
            directory: name.clone(),
            name,
            email: None,
        })
        .collect();
    out.sort_by(|a, b| a.directory.cmp(&b.directory));
    out
}

/// Resolve the launch target for a browser id, for use by the executor.
pub fn launch_target_for(browser_id: &str) -> Option<&'static str> {
    CANDIDATES
        .iter()
        .find(|c| c.id == browser_id)
        .map(|c| c.launch_target)
}

/// The default Chromium launch target on this platform.
pub fn default_chrome_launch_target() -> &'static str {
    CANDIDATES
        .first()
        .map(|c| c.launch_target)
        .unwrap_or("google-chrome")
}
