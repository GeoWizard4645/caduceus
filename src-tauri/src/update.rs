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

    #[test]
    fn newer_patch_is_detected() {
        assert!(is_newer("3.1.3", "3.1.2"));
        assert!(!is_newer("3.1.2", "3.1.2"));
        assert!(!is_newer("3.1.1", "3.1.2"));
    }
}
