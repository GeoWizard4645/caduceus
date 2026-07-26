//! Custom staff mark image stored under the app config directory.

use std::path::{Path, PathBuf};

use image::ImageReader;
use tauri::{AppHandle, Manager, Runtime};

pub const IMAGE_TOKEN: &str = "image:staff-mark.png";
const MARK_FILENAME: &str = "staff-mark.png";
const MAX_BYTES: u64 = 512 * 1024;

pub fn mark_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?
        .join("staff");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

pub fn resolve_path<R: Runtime>(app: &AppHandle<R>, token: &str) -> Option<PathBuf> {
    if token != IMAGE_TOKEN {
        return None;
    }
    let path = mark_dir(app).ok()?.join(MARK_FILENAME);
    path.is_file().then_some(path)
}

pub fn import_mark<R: Runtime>(app: &AppHandle<R>, source: &Path) -> Result<String, String> {
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

    // Keep pixel art sharp; cap dimensions so the staff window stays lightweight.
    let img = img.thumbnail(256, 256);
    let dir = mark_dir(app)?;
    let dest = dir.join(MARK_FILENAME);
    img.save(&dest)
        .map_err(|e| format!("Could not save the staff mark: {e}"))?;

    Ok(IMAGE_TOKEN.into())
}

pub fn clear_mark<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let path = mark_dir(app)?.join(MARK_FILENAME);
    if path.is_file() {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    Ok(())
}
