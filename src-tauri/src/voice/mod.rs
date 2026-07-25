//! Push-to-talk voice input.
//!
//! ```text
//!   hotkey down ──▶ record (cpal) ──▶ hotkey up ──▶ WAV ──▶ SttBackend ──▶ text
//!                                                                          │
//!                                              keyword router ◀────────────┘
//!                                                     │
//!                          web search / AI chat / computer use / just insert
//! ```
//!
//! # An explicit scope decision
//!
//! There is **no always-on wake-word listening**. Orbit only opens the
//! microphone while you are physically holding a key, and closes it the moment
//! you let go. A background listener would mean a process with permanent
//! microphone access on a tool that also has screen capture and input
//! simulation — too much to ask of someone installing a utility from GitHub.
//! "Hey Orbit" is not a v1 feature and is not a small change.

pub mod recorder;
pub mod router;
pub mod stt;

pub use router::{route, RoutedText};
pub use stt::{SttAvailability, SttBackend, SttError};

use parking_lot::Mutex;
use std::sync::Arc;

use crate::settings::SettingsManager;

/// Events emitted while push-to-talk is running, so the Command Center can show
/// a live "listening…" state.
pub const VOICE_STATE_EVENT: &str = "orbit://voice-state";
/// Emitted with the routed transcript once transcription finishes.
pub const VOICE_RESULT_EVENT: &str = "orbit://voice-result";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum VoiceState {
    Idle,
    Recording,
    Transcribing,
}

/// Holds the in-flight recording between hotkey press and release.
#[derive(Clone, Default)]
pub struct VoiceRuntime {
    active: Arc<Mutex<Option<recorder::Recording>>>,
}

impl VoiceRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_recording(&self) -> bool {
        self.active.lock().is_some()
    }

    /// Begin recording. Idempotent: a repeat key-down (which some keyboards
    /// send while held) does not restart the recording.
    pub fn start(&self, settings: &SettingsManager) -> Result<(), String> {
        let mut slot = self.active.lock();
        if slot.is_some() {
            return Ok(());
        }
        let max_secs = settings.with(|s| s.voice.max_recording_secs);
        let recording = recorder::start(max_secs).map_err(|e| e.to_string())?;
        *slot = Some(recording);
        Ok(())
    }

    /// Stop recording and return the WAV bytes, or `None` if nothing was
    /// running.
    pub fn stop(&self) -> Option<Result<Vec<u8>, String>> {
        let recording = self.active.lock().take()?;
        Some(recording.finish().map_err(|e| e.to_string()))
    }

    /// Discard an in-flight recording without transcribing it.
    pub fn cancel(&self) {
        if let Some(recording) = self.active.lock().take() {
            let _ = recording.finish();
        }
    }
}

/// Transcribe and route in one step. Returns the routed text ready to act on.
pub async fn transcribe_and_route(
    wav: Vec<u8>,
    settings: &SettingsManager,
) -> Result<RoutedText, String> {
    let voice = settings.with(|s| s.voice.clone());
    let backend = stt::backend_for(voice.stt_backend);
    let transcript = backend
        .transcribe(wav, &voice)
        .await
        .map_err(|e| e.to_string())?;
    Ok(route(&transcript, &voice))
}
