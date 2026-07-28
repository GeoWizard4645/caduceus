//! Wallpaper switching.
//!
//! Driven by `osascript` telling System Events to set every desktop's
//! picture, the same transport (and the same `output_with_timeout` deadline)
//! every other AppleScript-backed tool in this crate uses — see
//! `tools::mod`'s module docs on why this file needs nothing installed and no
//! private framework.
//!
//! "Every desktop" rather than just the main one: someone with two monitors
//! or several Spaces who asks Caduceus to change their wallpaper means all of
//! them, the same way changing it by hand in System Settings does.

use std::path::Path;
use std::process::Command;

use super::{output_with_timeout, ToolOutcome, TOOL_TIMEOUT};

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "heic", "tiff", "tif", "gif", "bmp"];

/// Build the AppleScript source for setting every desktop's picture to
/// `path`. Split out from [`set_wallpaper`] so the escaping can be checked
/// without actually running `osascript`.
fn build_script(path: &str) -> String {
    format!(
        "tell application \"System Events\"\n\
         \tset thePicture to \"{}\"\n\
         \ttell every desktop\n\
         \t\tset picture to thePicture\n\
         \tend tell\n\
         end tell",
        crate::shortcuts::escape_applescript(path)
    )
}

pub fn set_wallpaper(path: &str) -> ToolOutcome {
    let path = path.trim();
    if path.is_empty() {
        return ToolOutcome::err("Choose an image first.");
    }
    let file = Path::new(path);
    if !file.is_file() {
        return ToolOutcome::err("That file does not exist.");
    }
    let ext_ok = file
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false);
    if !ext_ok {
        return ToolOutcome::err(
            "That doesn't look like an image file (expected .png, .jpg, .heic, or similar).",
        );
    }

    let script = build_script(&file.to_string_lossy());
    let mut cmd = Command::new("osascript");
    cmd.arg("-e").arg(&script);

    match output_with_timeout(&mut cmd, TOOL_TIMEOUT, "System Events did not answer") {
        Ok(out) if out.status.success() => {
            let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            ToolOutcome::ok(format!("Wallpaper set to {name}"))
        }
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            ToolOutcome::err(format!("System Events said: {}", err.trim()))
        }
        Err(e) => ToolOutcome::err(e),
    }
}

#[tauri::command]
pub fn wallpaper_set(path: String) -> ToolOutcome {
    set_wallpaper(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_path_is_refused_before_touching_the_filesystem() {
        assert!(!set_wallpaper("   ").ok);
    }

    #[test]
    fn a_missing_file_is_refused() {
        assert!(!set_wallpaper("/nope/definitely/not/here.png").ok);
    }

    #[test]
    fn a_non_image_extension_is_refused_even_if_the_file_exists() {
        // Cargo.toml certainly exists relative to the crate root at test time.
        let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
        let path = format!("{manifest}/Cargo.toml");
        if Path::new(&path).is_file() {
            assert!(!set_wallpaper(&path).ok);
        }
    }

    #[test]
    fn the_applescript_embeds_the_escaped_path_for_every_desktop() {
        let script = build_script("/Users/ada/Pictures/sunset.png");
        assert!(script.contains("tell every desktop"));
        assert!(script.contains("/Users/ada/Pictures/sunset.png"));
    }

    #[test]
    fn a_quote_in_the_path_is_escaped_rather_than_breaking_the_script() {
        let script = build_script("/Users/ada/My \"Vacation\" Photos/sunset.png");
        assert!(script.contains("\\\""));
    }
}
