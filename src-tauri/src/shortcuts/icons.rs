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

    // Encode first so the filename can carry a hash of the bytes.
    //
    // The name used to be just `<id>.png`, which meant replacing a shortcut's
    // icon wrote different pixels to the same path — and the webview, which
    // loads it through `asset://`, went on showing the cached first one. The
    // upload had worked and looked like it had not. A content-addressed name
    // changes the URL whenever the image changes, so there is nothing to
    // invalidate.
    let mut png = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| format!("Could not encode the icon: {e}"))?;
    let png = png.into_inner();

    let filename = format!("{safe_id}-{}.png", short_hash(&png));
    std::fs::write(dir.join(&filename), &png)
        .map_err(|e| format!("Could not save the icon: {e}"))?;

    // Drop this shortcut's previous icons. Without it the directory grows by
    // one file per upload forever, and they are never referenced again.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let prefix = format!("{safe_id}-");
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(&prefix) && name != filename {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    Ok(icon_token(&filename))
}

/// A short, stable, non-cryptographic digest — enough to tell two images apart.
///
/// FNV-1a rather than a hashing crate: this names a cache file, so the only
/// property that matters is that different bytes usually produce different
/// names.
fn short_hash(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{hash:08x}")
}
