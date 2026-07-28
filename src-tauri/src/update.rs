//! Compare the running build to the latest GitHub release.

use serde::Serialize;

const REPO: &str = "GeoWizard4645/caduceus";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    pub current_version: String,
    pub update_available: bool,
    pub latest_version: Option<String>,
    pub release_url: Option<String>,
    pub download_url: Option<String>,
}

pub async fn check() -> UpdateCheck {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let mut out = UpdateCheck {
        current_version: current.clone(),
        update_available: false,
        latest_version: None,
        release_url: None,
        download_url: None,
    };

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .user_agent(format!("Caduceus/{}", current))
        .build()
    {
        Ok(c) => c,
        Err(_) => return out,
    };

    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let Ok(response) = client.get(&url).send().await else {
        return out;
    };
    if !response.status().is_success() {
        return out;
    }

    let Ok(body) = response.json::<serde_json::Value>().await else {
        return out;
    };

    let tag = body.get("tag_name").and_then(|v| v.as_str()).unwrap_or("");
    let latest = tag.trim_start_matches('v').to_string();
    if latest.is_empty() {
        return out;
    }

    out.latest_version = Some(latest.clone());
    out.release_url = body
        .get("html_url")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    out.download_url = pick_dmg_url(body.get("assets"));
    out.update_available = is_newer(&latest, &current);
    out
}

/// The one-liner from the website. A constant, and deliberately not built from
/// anything the UI passes in — this string becomes an executable file.
pub const INSTALL_COMMAND: &str =
    "curl -fsSL https://raw.githubusercontent.com/GeoWizard4645/caduceus/main/website/install.sh | bash";

/// The contents of the `.command` file, split out so it can be checked.
///
/// A shell script assembled by string formatting is a thing that compiles
/// happily and then fails at the one moment it runs, in a Terminal window, on
/// somebody else's machine. `bash -n` in the tests below is cheap insurance.
fn update_script() -> String {
    format!(
        r#"#!/bin/bash
echo "Updating Caduceus…"
echo "This is the same command as on the website:"
echo
echo "    {INSTALL_COMMAND}"
echo
{INSTALL_COMMAND}
status=$?
echo
if [ $status -eq 0 ]; then
  echo "Done. Caduceus should reopen on its own."
else
  echo "The update did not finish (exit $status). Caduceus is unchanged."
fi
echo "You can close this window."
"#
    )
}

/// Update in place by running the installer in Terminal.
///
/// # Why Terminal, and not a child process
///
/// The installer's update path quits the running copy (`osascript … to quit`,
/// then `pkill`) and `rm -rf`s the bundle before copying the new one. Run as a
/// child of Caduceus, it would be killing its own parent half way through, and
/// whether the rest of the script survives that depends on process-group
/// signalling nobody should be relying on. Terminal owns it instead, so
/// Caduceus quitting is exactly what the script expects rather than a hazard.
///
/// It is also the honest option. Caduceus is not notarised and asks people to
/// run this same command to install it; an update that replaces the app should
/// be the thing you can watch, not a silent download.
///
/// A `.command` file rather than `tell application "Terminal"`: AppleScript to
/// Terminal needs the Automation grant, and asking for permission to control a
/// terminal in order to run an update is a worse trade than writing a file.
#[cfg(target_os = "macos")]
pub fn run_installer() -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let path = std::env::temp_dir().join("caduceus-update.command");
    let script = update_script();

    let mut file = std::fs::File::create(&path)
        .map_err(|e| format!("Could not write the update script: {e}"))?;
    file.write_all(script.as_bytes())
        .map_err(|e| format!("Could not write the update script: {e}"))?;
    drop(file);

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("Could not make the update script runnable: {e}"))?;

    // `open` on a `.command` hands it to Terminal, which becomes its owner —
    // so it outlives this process quitting, which the script is about to do.
    std::process::Command::new("open")
        .arg(&path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open Terminal: {e}"))
}

#[cfg(not(target_os = "macos"))]
pub fn run_installer() -> Result<(), String> {
    Err("The installer is macOS-only.".into())
}

fn pick_dmg_url(assets: Option<&serde_json::Value>) -> Option<String> {
    let arr = assets?.as_array()?;
    for asset in arr {
        let name = asset.get("name")?.as_str()?;
        if name.ends_with(".dmg") && name.contains("universal") {
            return asset.get("browser_download_url")?.as_str().map(str::to_string);
        }
    }
    for asset in arr {
        let name = asset.get("name")?.as_str()?;
        if name.ends_with(".dmg") {
            return asset.get("browser_download_url")?.as_str().map(str::to_string);
        }
    }
    None
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    parse_version(latest) > parse_version(current)
}

fn parse_version(raw: &str) -> (u32, u32, u32) {
    let mut parts = raw
        .trim_start_matches('v')
        .split('.')
        .map(|p| p.parse::<u32>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The generated file has to be valid shell, and has to actually contain
    /// the install command rather than a mangled version of it.
    #[test]
    fn the_update_script_is_valid_shell() {
        let script = update_script();
        assert!(script.starts_with("#!/bin/bash\n"));
        assert!(script.contains(INSTALL_COMMAND));

        let dir = std::env::temp_dir().join(format!("caduceus-update-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("update.command");
        std::fs::write(&path, &script).unwrap();

        let out = std::process::Command::new("bash")
            .arg("-n")
            .arg(&path)
            .output()
            .expect("bash should be available");
        let _ = std::fs::remove_dir_all(&dir);

        assert!(
            out.status.success(),
            "generated script is not valid shell: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// It must not silently succeed when the install failed.
    #[test]
    fn the_update_script_reports_a_failed_install() {
        let script = update_script();
        assert!(script.contains("status=$?"));
        assert!(script.contains("did not finish"));
    }

    #[test]
    fn newer_patch_is_detected() {
        assert!(is_newer("3.1.3", "3.1.2"));
        assert!(!is_newer("3.1.2", "3.1.2"));
        assert!(!is_newer("3.1.1", "3.1.2"));
    }
}
