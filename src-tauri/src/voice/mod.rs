//! Push-to-talk voice input.
//!
//! On Apple-Silicon macOS with the system STT backend, recording uses local
//! Parakeet/FluidAudio with **live partial transcripts**. Apple Speech remains
//! the Intel/older-macOS fallback; HTTP backends still use cpal batch capture.

pub mod recorder;
pub mod router;
pub mod stt;

#[cfg(target_os = "macos")]
pub mod live_macos;

pub use router::{route, RoutedText};
pub use stt::{SttAvailability, SttBackend, SttError};

use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// Held: the session is alive and the transcript is intact, but no audio is
    /// reaching the recogniser.
    Paused,
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
    /// A start is in flight. Starting a live session is a blocking handshake
    /// with a helper process that can legitimately take minutes on first run
    /// while macOS shows its permission sheets.
    starting: Arc<AtomicBool>,
    /// Set by [`Self::cancel`] while a start is still in flight, so the session
    /// is torn down the moment it finishes instead of becoming a recording
    /// nobody asked for.
    abandon: Arc<AtomicBool>,
    /// Whether the live session is currently held. Read by the recording HUD.
    paused: Arc<AtomicBool>,
}

impl VoiceRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_recording(&self) -> bool {
        self.active.lock().is_some() || self.starting.load(Ordering::SeqCst)
    }

    /// Begin recording.
    ///
    /// The blocking part deliberately runs *outside* the mutex. Holding it
    /// across the handshake meant `stop` and `cancel` — which both take the same
    /// lock — blocked behind a helper that was itself waiting on a permission
    /// prompt. The UI showed "recording" and nothing could end it: not the red
    /// indicator, not the Command Center, not the function key.
    pub fn start<F>(&self, settings: &SettingsManager, on_partial: F) -> Result<(), String>
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        if self.active.lock().is_some() {
            return Ok(());
        }
        // Claim the start. Two rapid triggers must not both spin up a helper.
        if self.starting.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        self.abandon.store(false, Ordering::SeqCst);

        let outcome = self.open_session(settings, on_partial);
        self.starting.store(false, Ordering::SeqCst);

        let session = match outcome {
            Ok(session) => session,
            Err(e) => return Err(e),
        };

        // A cancel that arrived mid-handshake wins: the user has already asked
        // for this to be over.
        if self.abandon.swap(false, Ordering::SeqCst) {
            close_session(session);
            return Ok(());
        }

        *self.active.lock() = Some(session);
        Ok(())
    }

    fn open_session<F>(
        &self,
        settings: &SettingsManager,
        #[allow(unused_variables)] on_partial: F,
    ) -> Result<ActiveRecording, String>
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        let use_live = settings.with(|s| {
            s.voice.stt_backend == SttBackendKind::SystemNative && cfg!(target_os = "macos")
        });

        #[cfg(target_os = "macos")]
        if use_live {
            let language = settings.with(|s| s.voice.stt_language.clone());
            let session = live_macos::LiveSession::start(&language, move |text| on_partial(text))
                .map_err(|e: String| e)?;
            return Ok(ActiveRecording::Live(session));
        }

        let max_secs = settings.with(|s| s.voice.max_recording_secs);
        let recording = recorder::start(max_secs).map_err(|e| e.to_string())?;
        Ok(ActiveRecording::Batch(recording))
    }

    pub fn stop(&self) -> Option<StopOutcome> {
        let active = self.active.lock().take()?;
        Some(match active {
            ActiveRecording::Batch(recording) => {
                StopOutcome::Batch(recording.finish().map_err(|e| e.to_string()))
            }
            #[cfg(target_os = "macos")]
            ActiveRecording::Live(live) => StopOutcome::Live(live.stop()),
        })
    }

    pub fn cancel(&self) {
        // Tell an in-flight start to throw its session away when it lands. The
        // handshake cannot be interrupted from here, but its result can be.
        if self.starting.load(Ordering::SeqCst) {
            self.abandon.store(true, Ordering::SeqCst);
        }
        self.paused.store(false, Ordering::SeqCst);
        if let Some(active) = self.active.lock().take() {
            close_session(active);
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// Hold the recording without ending it.
    ///
    /// The session, the microphone tap and everything said so far stay exactly
    /// where they are; only the feed into the recogniser stops. That is what
    /// makes "hold space to pause" usable — a pause that discarded the
    /// transcript would be a cancel with a friendlier name.
    ///
    /// Only the live macOS backend can do this. Batch capture is a single
    /// recording with no seam to pause at, so it reports honestly rather than
    /// pretending.
    pub fn set_paused(&self, paused: bool) -> Result<bool, String> {
        let mut guard = self.active.lock();
        let Some(active) = guard.as_mut() else {
            return Err("Nothing is recording.".into());
        };
        match active {
            #[cfg(target_os = "macos")]
            ActiveRecording::Live(live) => {
                live.set_paused(paused)?;
                self.paused.store(paused, Ordering::SeqCst);
                Ok(paused)
            }
            ActiveRecording::Batch(_) => Err(
                "Pausing needs the on-device speech backend; this recording cannot be held.".into(),
            ),
        }
    }
}

fn close_session(active: ActiveRecording) {
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
