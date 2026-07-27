//! Bridge to the `caduceus-native` Swift helper.
//!
//! Two capabilities live behind this: Vision text recognition and CoreAudio
//! device switching. Both need frameworks with no C interface Rust can call, and
//! both are better off in a separate process — if the helper is missing, out of
//! date, or crashes, the caller gets an error instead of taking Caduceus down.
//!
//! The helper needs no privacy permission of its own, which is why it can be
//! ad-hoc signed and rebuilt freely. Anything requiring a TCC grant stays in the
//! main binary; see `window::accessibility` for why.

use std::path::PathBuf;
use std::process::Command;

use serde::{Deserialize, Serialize};

use super::ToolOutcome;

/// Locate the helper, in bundle order then development order.
///
/// Mirrors the lookup the speech helpers use so there is one story for "where do
/// Caduceus's helpers live" rather than three.
fn helper_path() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for relative in ["../Resources/bin/caduceus-native", "bin/caduceus-native", "caduceus-native"] {
                let candidate = dir.join(relative);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bin/caduceus-native");
    dev.is_file().then_some(dev)
}

const MISSING: &str =
    "The native helper is missing from this build. Reinstall Caduceus, or rebuild it on a Mac with \
     the Xcode command line tools installed.";

fn invoke(args: &[&str]) -> Result<String, String> {
    let helper = helper_path().ok_or(MISSING)?;
    let output = Command::new(&helper)
        .args(args)
        .output()
        .map_err(|e| format!("Could not start the native helper: {e}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_string())
    } else {
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if reason.is_empty() { "The native helper failed.".into() } else { reason })
    }
}

/// Pick a colour from anywhere on screen, with macOS's own loupe.
///
/// `Ok(None)` means the user pressed Escape, which is not a failure and must
/// not be reported as one.
///
/// Needs no Screen Recording grant: `NSColorSampler` has the user point at what
/// they want rather than the app reading the screen, which is both a smaller
/// ask and a nicer interaction than a custom overlay would be.
pub fn pick_screen_color() -> Result<Option<String>, String> {
    let helper = helper_path().ok_or(MISSING)?;
    let output = Command::new(&helper)
        .arg("pick-color")
        .output()
        .map_err(|e| format!("Could not start the native helper: {e}"))?;

    // 3 is the helper's "cancelled" code.
    if output.status.code() == Some(3) {
        return Ok(None);
    }
    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if reason.is_empty() {
            "The colour picker did not start.".into()
        } else {
            reason
        });
    }

    let hex = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(if hex.is_empty() { None } else { Some(hex) })
}

/// Whether the helper is present, so the UI can say why a feature is missing
/// instead of failing when it is used.
pub fn available() -> bool {
    helper_path().is_some()
}

// ---------------------------------------------------------------------------
// OCR
// ---------------------------------------------------------------------------

/// Read the text out of an image file, entirely on-device.
pub fn ocr_image(path: &str) -> Result<String, String> {
    if !std::path::Path::new(path).is_file() {
        return Err("That image does not exist.".into());
    }
    invoke(&["ocr", path])
}

/// Select a region of the screen and read the text in it.
///
/// `screencapture -i` draws macOS's own selection crosshair, so there is no
/// custom overlay to get wrong, and Escape cancels the way it does everywhere
/// else. The capture is written to a temporary file that is deleted before this
/// function returns, whether or not recognition succeeded — a screenshot of
/// whatever was on screen is not something to leave in `/tmp`.
pub fn ocr_screen_selection() -> ToolOutcome {
    let path = std::env::temp_dir().join(format!("caduceus-ocr-{}.png", uuid::Uuid::new_v4()));

    let status = Command::new("screencapture").arg("-i").arg("-x").arg(&path).status();

    let cleanup = || {
        let _ = std::fs::remove_file(&path);
    };

    match status {
        Ok(status) if status.success() => {}
        Ok(_) => {
            cleanup();
            return ToolOutcome::err("Screen capture failed.");
        }
        Err(e) => {
            cleanup();
            return ToolOutcome::err(format!("Could not run screencapture: {e}"));
        }
    }

    // Cancelling the selection exits successfully but writes nothing.
    if !path.is_file() {
        return ToolOutcome::err("Cancelled.");
    }

    let result = ocr_image(&path.to_string_lossy());
    cleanup();

    match result {
        Ok(text) if !text.trim().is_empty() => {
            let lines = text.lines().count();
            ToolOutcome::copied(
                text,
                format!("Copied {lines} line(s) of text"),
            )
        }
        Ok(_) => ToolOutcome::err("No text was found in that selection."),
        Err(e) => ToolOutcome::err(e),
    }
}

// ---------------------------------------------------------------------------
// Audio devices
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDevice {
    /// Stable across reboots and reconnections, unlike the numeric device ID.
    pub uid: String,
    pub name: String,
    pub is_input: bool,
    pub is_output: bool,
    pub is_default_input: bool,
    pub is_default_output: bool,
}

pub fn audio_devices() -> Result<Vec<AudioDevice>, String> {
    let json = invoke(&["audio-list"])?;
    serde_json::from_str(&json).map_err(|e| format!("Could not read the device list: {e}"))
}

/// Make a device the system default. `input` picks which side to change.
pub fn set_audio_device(uid: &str, input: bool) -> ToolOutcome {
    match invoke(&["audio-set", if input { "in" } else { "out" }, uid]) {
        Ok(name) => ToolOutcome::ok(format!(
            "{} is now the {}",
            name,
            if input { "microphone" } else { "sound output" }
        )),
        Err(e) => ToolOutcome::err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_image_is_refused_before_the_helper_is_started() {
        let error = ocr_image("/definitely/not/here.png").unwrap_err();
        assert!(error.contains("does not exist"), "{error}");
    }

    #[test]
    fn the_helper_is_present_in_a_development_build() {
        // build.rs compiles it, so a checkout that can run these tests has it.
        // If this fails, the Swift build step did — which is worth knowing.
        assert!(available(), "caduceus-native was not built");
    }

    #[test]
    fn every_connected_audio_device_has_a_uid_and_a_name() {
        let devices = audio_devices().expect("the helper should list devices");
        // Every Mac has at least one output.
        assert!(!devices.is_empty(), "no audio devices were reported");
        for device in &devices {
            assert!(!device.uid.is_empty(), "{device:?} has no UID");
            assert!(!device.name.is_empty(), "{device:?} has no name");
            assert!(device.is_input || device.is_output, "{device:?} is neither");
        }
    }

    #[test]
    fn an_unknown_device_uid_is_reported_not_silently_ignored() {
        let outcome = set_audio_device("no-such-device-uid", false);
        assert!(!outcome.ok);
        assert!(outcome.message.contains("no audio device"), "{}", outcome.message);
    }
}
