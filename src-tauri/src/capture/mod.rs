//! Screenshots and screen recording exposed as explicit commands.

pub mod recorder;

use std::path::PathBuf;
use std::process::Command;

use arboard::Clipboard;
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotResult {
    pub ok: bool,
    pub path: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingState {
    pub active: bool,
    pub path: Option<String>,
    pub message: String,
}

pub struct CaptureRuntime {
    /// Path we told the user to save to, when using the Screenshot app flow.
    hint_path: Mutex<Option<PathBuf>>,
}

impl Default for CaptureRuntime {
    fn default() -> Self {
        Self {
            hint_path: Mutex::new(None),
        }
    }
}

impl CaptureRuntime {
    pub fn new() -> Self {
        Self::default()
    }
}

pub fn screenshot_full(save_to_downloads: bool) -> Result<ScreenshotResult, String> {
    #[cfg(target_os = "macos")]
    {
        let dir = if save_to_downloads {
            dirs::download_dir().ok_or("Could not find Downloads.")?
        } else {
            std::env::temp_dir()
        };
        let path = dir.join(format!(
            "Caduceus-screenshot-{}.png",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        ));

        // Bounded: without the Screen Recording grant, `screencapture` can sit
        // waiting on macOS's consent machinery rather than returning.
        let output = crate::tools::output_with_timeout(
            Command::new("screencapture").arg("-x").arg(&path),
            crate::tools::TOOL_TIMEOUT,
            "The screenshot took too long. Grant Screen Recording permission in System Settings.",
        )?;
        if !output.status.success() {
            return Err(
                "Screenshot failed. Grant Screen Recording permission in System Settings.".into(),
            );
        }

        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        let img = image::load_from_memory(&bytes)
            .map_err(|e| format!("Screenshot file invalid: {e}"))?
            .to_rgba8();
        let (w, h) = (img.width() as usize, img.height() as usize);
        Clipboard::new()
            .map_err(|e| e.to_string())?
            .set_image(arboard::ImageData {
                width: w,
                height: h,
                bytes: std::borrow::Cow::Owned(img.into_raw()),
            })
            .map_err(|e| e.to_string())?;

        let saved = if save_to_downloads {
            Some(path.to_string_lossy().into_owned())
        } else {
            let _ = std::fs::remove_file(&path);
            None
        };

        return Ok(ScreenshotResult {
            ok: true,
            path: saved.clone(),
            message: if saved.is_some() {
                "Screenshot copied to the clipboard and saved to Downloads.".into()
            } else {
                "Screenshot copied to the clipboard.".into()
            },
        });
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = save_to_downloads;
        Err("Screenshots are only implemented on macOS.".into())
    }
}

pub fn recording_state<R: Runtime>(app: &AppHandle<R>) -> RecordingState {
    let path = app
        .try_state::<CaptureRuntime>()
        .and_then(|r| r.hint_path.lock().as_ref().map(|p| p.to_string_lossy().into_owned()));
    RecordingState {
        active: false,
        path,
        message: String::new(),
    }
}

/// Opens Apple's Screenshot app so you can record the screen with the same
/// controls macOS uses everywhere (mic toggle in the toolbar).
pub fn start_recording<R: Runtime>(
    app: &AppHandle<R>,
    mic: bool,
    system_audio: bool,
) -> Result<RecordingState, String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, mic, system_audio);
        return Err("Screen recording is only implemented on macOS.".into());
    }

    #[cfg(target_os = "macos")]
    {
        let dir = dirs::download_dir().ok_or("Could not find Downloads.")?;
        let path = dir.join(format!(
            "Caduceus-recording-{}.mov",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        ));

        if let Some(runtime) = app.try_state::<CaptureRuntime>() {
            *runtime.hint_path.lock() = Some(path.clone());
        }

        Command::new("open")
            .arg("-a")
            .arg("Screenshot")
            .status()
            .map_err(|e| format!("Could not open the Screenshot app: {e}"))?;

        let mut message = "Opened the Screenshot app — choose Record Screen, then pick \
                           microphone / system audio in its toolbar."
            .to_string();
        if !mic && !system_audio {
            message = "Opened the Screenshot app — choose Record Screen and turn off audio \
                       sources in its toolbar if you want video only."
                .into();
        } else if mic && system_audio {
            message = "Opened the Screenshot app — choose Record Screen; enable the mic and \
                       use Options for system audio when your macOS version supports it."
                .into();
        } else if system_audio {
            message = "Opened the Screenshot app — choose Record Screen and enable system \
                       audio from Options (macOS 14+ on supported Macs)."
                .into();
        }

        Ok(RecordingState {
            active: false,
            path: Some(path.to_string_lossy().into_owned()),
            message,
        })
    }
}

pub fn stop_recording<R: Runtime>(app: &AppHandle<R>) -> Result<RecordingState, String> {
    let _ = app;
    Ok(RecordingState {
        active: false,
        path: None,
        message: "Stop the recording from the Screenshot app menu bar control.".into(),
    })
}
