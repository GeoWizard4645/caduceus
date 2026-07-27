//! A wallpaper for the Command Center.
//!
//! Same shape as [`crate::staff_mark`] — an image copied into the app's config
//! directory and referred to by a token — with one difference that matters:
//! this one is a **background**, so it is downscaled to something a window can
//! actually use rather than kept at whatever a phone camera produced.
//!
//! # Why the image is copied rather than referenced
//!
//! Pointing at a file in the user's Pictures folder would mean a background
//! that vanishes when they tidy up, and a config file that leaks a path. A copy
//! is a few hundred kilobytes and belongs to the app.

use std::path::{Path, PathBuf};

use image::ImageReader;
use tauri::{AppHandle, Manager, Runtime};

pub const IMAGE_TOKEN: &str = "image:command-center-background.png";
const FILENAME: &str = "command-center-background.png";

/// The largest source image accepted, before downscaling.
///
/// Generous — a 4K wallpaper is comfortably inside it — but bounded, because
/// this is decoded in-process and a 200 MP TIFF is a denial of service.
const MAX_BYTES: u64 = 12 * 1024 * 1024;

/// What it is stored at.
///
/// 2560 wide covers a Retina window at any size the Command Center can be
/// dragged to, and keeps the file small enough that the webview decodes it
/// without a visible pause on open.
const MAX_DIMENSION: u32 = 2560;

pub fn backdrop_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?
        .join("appearance");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

pub fn resolve_path<R: Runtime>(app: &AppHandle<R>, token: &str) -> Option<PathBuf> {
    if token != IMAGE_TOKEN {
        return None;
    }
    let path = backdrop_dir(app).ok()?.join(FILENAME);
    path.is_file().then_some(path)
}

pub fn import<R: Runtime>(app: &AppHandle<R>, source: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(source).map_err(|e| e.to_string())?;
    if meta.len() > MAX_BYTES {
        return Err(format!(
            "That image is {} MB. Use something under {} MB.",
            meta.len() / 1024 / 1024,
            MAX_BYTES / 1024 / 1024
        ));
    }

    let img = ImageReader::open(source)
        .map_err(|e| format!("Could not read the image: {e}"))?
        .with_guessed_format()
        .map_err(|e| format!("Could not work out what kind of image that is: {e}"))?
        .decode()
        .map_err(|e| format!("Could not decode the image: {e}"))?;

    // `thumbnail` preserves the aspect ratio and only ever shrinks, so a small
    // image is stored as it is rather than blown up and blurred.
    let img = img.thumbnail(MAX_DIMENSION, MAX_DIMENSION);

    let dest = backdrop_dir(app)?.join(FILENAME);
    img.save(&dest)
        .map_err(|e| format!("Could not save the background: {e}"))?;

    Ok(IMAGE_TOKEN.into())
}

pub fn clear<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let path = backdrop_dir(app)?.join(FILENAME);
    if path.is_file() {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    Ok(())
}
