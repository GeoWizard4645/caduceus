//! Push-to-talk voice input.
//!
//! On Apple-Silicon macOS with the system STT backend, recording uses local
//! Parakeet/FluidAudio with **live partial transcripts**. Apple Speech remains
//! the Intel/older-macOS fallback; HTTP backends still use cpal batch capture.

pub mod recorder;
pub mod router;
pub mod stt;
pub mod tts;

#[cfg(target_os = "macos")]
pub mod live_macos;

pub use router::{ai_is_configured, route, RoutedText};
pub use stt::{SttAvailability, SttBackend, SttError};
pub use tts::{TtsAvailability, TtsBackend, TtsError};

use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::settings::{SettingsManager, SttBackendKind, VoiceSettings};

pub const VOICE_STATE_EVENT: &str = "caduceus://voice-state";
pub const VOICE_PARTIAL_EVENT: &str = "caduceus://voice-partial";
pub const VOICE_RESULT_EVENT: &str = "caduceus://voice-result";
/// Emitted whenever a spoken reply starts or stops, for a speaking indicator
/// in the UI. Kept separate from `VOICE_STATE_EVENT`: that one describes the
/// microphone, this one the speaker, and both can legitimately be true for a
/// moment during barge-in — see `VoiceRuntime::start`.
pub const TTS_STATE_EVENT: &str = "caduceus://tts-state";

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

/// Whether a spoken reply is currently playing. See [`TTS_STATE_EVENT`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TtsState {
    Idle,
    Speaking,
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
    /// Whatever is currently being spoken aloud, if anything. `start` below
    /// cuts it off the instant a recording begins — "barge-in" — by calling
    /// straight into this rather than trusting every *caller* of `start`
    /// (the hotkey handler, the `voice_start` command, the function-key
    /// dispatcher) to each remember to do it separately. See [`TtsRuntime`]'s
    /// own doc for why cancellation lives on it rather than on
    /// [`tts::TtsBackend`].
    tts: TtsRuntime,
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
        // Barge-in: whatever Caduceus is saying stops the instant the user
        // asks to talk. Done first, ahead of every check below — including
        // the already-recording early return just after — so a press that
        // turns out to be a no-op for recording purposes still silences a
        // reply in progress.
        self.tts.stop();

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

    /// Pick a way to capture audio, falling back through everything Caduceus
    /// has before giving up.
    ///
    /// This used to pick exactly one live helper (whichever
    /// `live_helper_path` preferred) and either use it or fail outright. That
    /// meant a single broken binary — one Swift actor-isolation bug — took
    /// dictation down completely, on every machine that preferred it, with no
    /// way back short of a rebuild. Walking every live candidate in order,
    /// and falling further back to batch capture if every one of them fails,
    /// means a broken helper costs *quality* (a worse recogniser, or no live
    /// partials) rather than the whole feature. See `live_macos::mark_helper_failed`
    /// for why a helper that fails once is not retried again this run.
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
            let on_partial = Arc::new(on_partial);
            let candidates = live_macos::live_helper_candidates();
            let mut last_error: Option<String> = None;

            for candidate in &candidates {
                let partial = on_partial.clone();
                match live_macos::LiveSession::start(candidate, &language, move |text| partial(text))
                {
                    Ok(session) => return Ok(ActiveRecording::Live(session)),
                    Err(e) => {
                        log::warn!("voice: {} failed to start: {e}", candidate.label());
                        live_macos::mark_helper_failed(candidate.kind);
                        last_error = Some(e);
                    }
                }
            }

            // Every live helper either does not exist or just failed. Batch
            // capture below is the last line: it needs nothing but a
            // microphone and whichever STT backend is configured, so it is
            // the one path a broken speech helper cannot take down.
            if let Some(e) = last_error {
                log::warn!("voice: falling back to batch capture after live dictation failed: {e}");
            } else {
                log::warn!("voice: no live dictation helper is available; falling back to batch capture");
            }
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
        // A dismissal is as much "the user wants quiet" as a fresh recording
        // is — see `start`'s identical call.
        self.tts.stop();
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

    /// The speech runtime backing spoken replies.
    ///
    /// Exposed so `lib.rs` can `app.manage` this *same* instance under its own
    /// type — letting the `speak`/`stop_speaking` commands depend on just
    /// `TtsRuntime` rather than the whole of `VoiceRuntime` — while this
    /// struct's own barge-in calls above keep working on the identical shared
    /// state. Two independently-constructed `TtsRuntime`s would each think
    /// nothing was ever speaking on the other's side.
    pub fn tts(&self) -> TtsRuntime {
        self.tts.clone()
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
    // Both read together under one lock so routing sees a consistent snapshot
    // of voice settings and agent settings, rather than risking a settings
    // change landing between the two reads.
    let (voice, ai_configured) =
        settings.with(|s| (s.voice.clone(), ai_is_configured(&s.agents)));
    let backend = stt::backend_for(voice.stt_backend);
    let transcript = backend
        .transcribe(wav, &voice)
        .await
        .map_err(|e| e.to_string())?;
    Ok(route(&transcript, &voice, ai_configured))
}

pub fn route_transcript(transcript: &str, settings: &SettingsManager) -> RoutedText {
    let (voice, ai_configured) =
        settings.with(|s| (s.voice.clone(), ai_is_configured(&s.agents)));
    route(transcript, &voice, ai_configured)
}

// ---------------------------------------------------------------------------
// Text-to-speech playback
// ---------------------------------------------------------------------------

/// Orchestrates spoken replies: which backend is currently talking, and how to
/// cut it off.
///
/// This is deliberately a separate, thin layer rather than a method on
/// [`tts::TtsBackend`] itself, for the same reason capture and transcription
/// are split above: [`SttBackend`] has no `cancel` of its own either, because
/// *interrupting something already in progress* is a lifecycle concern, not a
/// backend-strategy one, and belongs to whatever owns the lifecycle. For
/// recording that owner is [`VoiceRuntime`] itself; for speech it is this
/// type, which `VoiceRuntime` keeps one of specifically so that starting a
/// recording can always reach it — see [`VoiceRuntime::start`].
///
/// `TtsBackend::stop` only interrupts the instance it is called on, and
/// [`tts::backend_for`] hands back a fresh instance every call — the same
/// stateless-resolver shape as `stt::backend_for`. What makes `stop` useful
/// despite that is this type keeping the *one* `Arc`-shared instance
/// currently speaking reachable for exactly as long as it is speaking, so a
/// `stop()` from anywhere finds the same object `speak()` is awaiting rather
/// than a disconnected new one.
#[derive(Clone, Default)]
pub struct TtsRuntime {
    active: Arc<Mutex<Option<Arc<dyn tts::TtsBackend>>>>,
}

impl TtsRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    /// Speak `text` aloud with the configured backend.
    ///
    /// Resolves once playback finishes naturally or is cut off by
    /// [`Self::stop`] — an interruption is reported as success, not failure,
    /// because from the caller's point of view both mean the same thing:
    /// Caduceus has gone quiet. Errors are reserved for cases where nothing
    /// was ever said at all (TTS disabled, no endpoint configured, the
    /// backend unavailable).
    pub async fn speak(&self, text: &str, settings: &VoiceSettings) -> tts::TtsResult<()> {
        if !settings.tts_enabled {
            return Err(tts::TtsError::Disabled);
        }

        let backend: Arc<dyn tts::TtsBackend> = tts::backend_for(settings.tts_backend).into();
        *self.active.lock() = Some(backend.clone());

        let result = backend.speak(text, settings).await;

        // Clear the slot only if nothing newer has already taken it — two
        // overlapping `speak` calls should not let the first one's cleanup
        // erase the second's still-active handle. Nothing in this codebase
        // calls `speak` concurrently today, but the check is cheap and the
        // alternative — a `stop()` that silently does nothing because the
        // wrong instance got cleared — is not worth risking.
        let mut guard = self.active.lock();
        if matches!(&*guard, Some(current) if Arc::ptr_eq(current, &backend)) {
            *guard = None;
        }
        drop(guard);

        result
    }

    /// Cut off whatever is currently being spoken. Safe — and a no-op — when
    /// nothing is: every push-to-talk press calls this unconditionally (see
    /// [`VoiceRuntime::start`]), so it cannot be allowed to mind being called
    /// when there is nothing to interrupt.
    pub fn stop(&self) {
        if let Some(backend) = self.active.lock().clone() {
            backend.stop();
        }
    }

    pub fn is_speaking(&self) -> bool {
        self.active.lock().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_with_nothing_speaking_does_not_panic() {
        let tts = TtsRuntime::new();
        assert!(!tts.is_speaking());
        tts.stop();
        assert!(!tts.is_speaking());
    }

    #[tokio::test]
    async fn speak_with_tts_disabled_returns_the_disabled_error() {
        let tts = TtsRuntime::new();
        let settings = VoiceSettings {
            tts_enabled: false,
            ..Default::default()
        };
        let err = tts.speak("hello", &settings).await;
        assert!(matches!(err, Err(tts::TtsError::Disabled)));
        // And it never registered anything as active in the process.
        assert!(!tts.is_speaking());
    }

    #[tokio::test]
    async fn a_finished_utterance_clears_the_active_slot() {
        // The disabled backend errors immediately without ever really
        // "speaking", but it still exercises `speak`'s install-then-clear
        // path around `backend.speak(...)`.
        let tts = TtsRuntime::new();
        let settings = VoiceSettings {
            tts_enabled: true,
            tts_backend: crate::settings::TtsBackendKind::Disabled,
            ..Default::default()
        };
        let _ = tts.speak("hello", &settings).await;
        assert!(!tts.is_speaking());
    }
}
