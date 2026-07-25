//! Custom shortcut icon files stored under the app config directory.

use std::path::{Path, PathBuf};

use image::ImageReader;
use tauri::{AppHandle, Manager, Runtime};

const ICON_PREFIX: &str = "image:";
const MAX_BYTES: u64 = 512 * 1024;

pub fn icon_token(filename: &str) -> String {
    format!("{ICON_PREFIX}{filename}")
}

pub fn icons_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?
        .join("shortcut-icons");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

pub fn resolve_path<R: Runtime>(app: &AppHandle<R>, icon: &str) -> Option<PathBuf> {
    let name = icon.strip_prefix(ICON_PREFIX)?;
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return None;
    }
    let path = icons_dir(app).ok()?.join(name);
    path.is_file().then_some(path)
}

/// Copy and normalise a user-selected image; returns an `image:<filename>` token.
pub fn import_icon<R: Runtime>(
    app: &AppHandle<R>,
    shortcut_id: &str,
    source: &Path,
) -> Result<String, String> {
    let meta = std::fs::metadata(source).map_err(|e| e.to_string())?;
    if meta.len() > MAX_BYTES {
        return Err(format!(
            "Image is too large ({} KB). Use something under 512 KB.",
            meta.len() / 1024
        ));
    }

    let img = ImageReader::open(source)
        .map_err(|e| format!("Could not read the image: {e}"))?
        .decode()
        .map_err(|e| format!("Could not decode the image: {e}"))?;

    let img = img.thumbnail(128, 128);
    let dir = icons_dir(app)?;
    let safe_id: String = shortcut_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    let filename = format!("{safe_id}.png");
    let dest = dir.join(&filename);
    img.save(&dest)
        .map_err(|e| format!("Could not save the icon: {e}"))?;

    Ok(icon_token(&filename))
}
