//! Push-to-talk voice input.
//!
//! On macOS with the system STT backend, recording uses Apple's Speech framework
//! with **live partial transcripts** (`caduceus-stt-live`). Other platforms and
//! HTTP backends still use cpal batch capture.

pub mod recorder;
pub mod router;
pub mod stt;

#[cfg(target_os = "macos")]
pub mod live_macos;

pub use router::{route, RoutedText};
pub use stt::{SttAvailability, SttBackend, SttError};

use parking_lot::Mutex;
use std::sync::Arc;

use crate::settings::{SettingsManager, SttBackendKind};

pub const VOICE_STATE_EVENT: &str = "caduceus://voice-state";
pub const VOICE_PARTIAL_EVENT: &str = "caduceus://voice-partial";
pub const VOICE_RESULT_EVENT: &str = "caduceus://voice-result";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum VoiceState {
    Idle,
    Recording,
    Transcribing,
}

enum ActiveRecording {
    Batch(recorder::Recording),
    #[cfg(target_os = "macos")]
    Live(live_macos::LiveSession),
}

#[derive(Clone, Default)]
pub struct VoiceRuntime {
    active: Arc<Mutex<Option<ActiveRecording>>>,
}

impl VoiceRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_recording(&self) -> bool {
        self.active.lock().is_some()
    }

    pub fn start<F>(&self, settings: &SettingsManager, on_partial: F) -> Result<(), String>
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        let mut slot = self.active.lock();
        if slot.is_some() {
            return Ok(());
        }

        let use_live = settings.with(|s| {
            s.voice.stt_backend == SttBackendKind::SystemNative && cfg!(target_os = "macos")
        });

        #[cfg(target_os = "macos")]
        if use_live {
            let language = settings.with(|s| s.voice.stt_language.clone());
            let session = live_macos::LiveSession::start(&language, move |text| on_partial(text))
            .map_err(|e: String| e)?;
            *slot = Some(ActiveRecording::Live(session));
            return Ok(());
        }

        let max_secs = settings.with(|s| s.voice.max_recording_secs);
        let recording = recorder::start(max_secs).map_err(|e| e.to_string())?;
        *slot = Some(ActiveRecording::Batch(recording));
        Ok(())
    }

    pub fn stop(&self) -> Option<StopOutcome> {
        let active = self.active.lock().take()?;
        Some(match active {
            ActiveRecording::Batch(recording) => StopOutcome::Batch(
                recording.finish().map_err(|e| e.to_string()),
            ),
            #[cfg(target_os = "macos")]
            ActiveRecording::Live(live) => StopOutcome::Live(live.stop()),
        })
    }

    pub fn cancel(&self) {
        if let Some(active) = self.active.lock().take() {
            match active {
                ActiveRecording::Batch(recording) => {
                    let _ = recording.finish();
                }
                #[cfg(target_os = "macos")]
                ActiveRecording::Live(live) => {
                    let _ = live.stop();
                }
            }
        }
    }
}

pub enum StopOutcome {
    Batch(Result<Vec<u8>, String>),
    Live(Result<(String, Vec<u8>), String>),
}

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

pub fn route_transcript(transcript: &str, settings: &SettingsManager) -> RoutedText {
    let voice = settings.with(|s| s.voice.clone());
    route(transcript, &voice)
}
