//! Text-to-speech backends.
//!
//! Mirrors [`crate::voice::stt`] exactly: a small trait, a couple of
//! implementations, and a resolver that maps the user's setting to one of
//! them. Adding a new voice means one impl and one match arm — see
//! `docs/PLUGIN_GUIDE.md`.
//!
//! Two ship by default:
//!
//! * [`SystemNativeTts`] — the built-in `/usr/bin/say`. macOS only, and fully
//!   local: unlike the STT helpers, `say` needs no bundled Swift binary, no
//!   entitlement and no permission prompt, so there is nothing here for
//!   `build.rs` to compile.
//! * [`OpenAiCompatibleTts`] — any `/audio/speech` endpoint. Covers OpenAI
//!   itself and any local server that copies its request shape.
//!
//! Neither backend can stop mid-sentence on its own — that is
//! [`crate::voice::TtsRuntime`]'s job. See its doc for why cancellation lives
//! at that layer rather than here, which is the same split `VoiceRuntime`
//! already makes between owning a recording's lifecycle and merely resolving
//! an [`SttBackend`](crate::voice::SttBackend) to transcribe it with.

use async_trait::async_trait;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use serde::Serialize;
use std::sync::Arc;
use thiserror::Error;

use crate::settings::{secrets, TtsBackendKind, VoiceSettings};

#[derive(Debug, Error)]
pub enum TtsError {
    #[error("text-to-speech is turned off. Pick a backend in Settings \u{2192} Voice.")]
    Disabled,
    #[error("{0}")]
    Unavailable(String),
    #[error("could not reach the speech endpoint at {endpoint}: {detail}")]
    Transport { endpoint: String, detail: String },
    #[error("the speech service returned {status}: {body}")]
    Api { status: u16, body: String },
    #[error("speech failed: {0}")]
    Failed(String),
    #[error("there is nothing to say")]
    Empty,
}

pub type TtsResult<T> = Result<T, TtsError>;

/// Whether a backend can run right now, and why not if it cannot.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TtsAvailability {
    pub id: String,
    pub display_name: String,
    pub available: bool,
    /// Explanation shown in Settings when `available` is false.
    pub detail: String,
}

/// A voice Caduceus can speak replies with.
#[async_trait]
pub trait TtsBackend: Send + Sync {
    fn id(&self) -> &str;
    fn display_name(&self) -> &str;

    /// Speak `text` aloud. Resolves once playback finishes naturally or
    /// `stop` cuts it off — an interruption is **not** reported as an error,
    /// so a caller cannot tell "finished" from "barged in" apart from the
    /// `Result` alone. That is deliberate: both mean the same thing to
    /// whoever is waiting on this call, which is "Caduceus has gone quiet".
    async fn speak(&self, text: &str, settings: &VoiceSettings) -> TtsResult<()>;

    /// Immediately end whatever this instance is currently speaking. A no-op
    /// if it is not speaking.
    ///
    /// This only reaches an utterance in progress if `stop` is called on the
    /// *same* trait object `speak` was called on — a fresh instance from
    /// [`backend_for`] has nothing of its own to interrupt.
    /// [`crate::voice::TtsRuntime`] is what makes that useful in practice: it
    /// keeps the one instance currently speaking reachable for exactly as
    /// long as it is speaking, the same way `VoiceRuntime` — not
    /// `SttBackend`, which has no `stop`/`cancel` of its own either — is what
    /// makes a recording cancellable.
    fn stop(&self);

    /// Cheap, side-effect-free check used to populate Settings.
    fn availability(&self, settings: &VoiceSettings) -> TtsAvailability;
}

/// Resolve the configured backend.
///
/// A fresh instance every call, exactly like `stt::backend_for` — cheap, and
/// what makes the `stop` doc above true: nothing returned here outlives the
/// one utterance [`crate::voice::TtsRuntime`] resolved it for.
pub fn backend_for(kind: TtsBackendKind) -> Box<dyn TtsBackend> {
    match kind {
        TtsBackendKind::Disabled => Box::new(DisabledTts),
        TtsBackendKind::SystemNative => Box::new(SystemNativeTts::default()),
        TtsBackendKind::OpenAiCompatible => Box::new(OpenAiCompatibleTts::default()),
    }
}

/// Availability of every backend, for the Settings → Voice picker.
pub fn all_availability(settings: &VoiceSettings) -> Vec<TtsAvailability> {
    [
        TtsBackendKind::SystemNative,
        TtsBackendKind::OpenAiCompatible,
        TtsBackendKind::Disabled,
    ]
    .into_iter()
    .map(|k| backend_for(k).availability(settings))
    .collect()
}

// ---------------------------------------------------------------------------
// Disabled
// ---------------------------------------------------------------------------

pub struct DisabledTts;

#[async_trait]
impl TtsBackend for DisabledTts {
    fn id(&self) -> &str {
        "disabled"
    }
    fn display_name(&self) -> &str {
        "Off"
    }
    async fn speak(&self, _text: &str, _settings: &VoiceSettings) -> TtsResult<()> {
        Err(TtsError::Disabled)
    }
    fn stop(&self) {}
    fn availability(&self, _settings: &VoiceSettings) -> TtsAvailability {
        TtsAvailability {
            id: "disabled".into(),
            display_name: "Off".into(),
            available: true,
            detail: "Replies stay text-only.".into(),
        }
    }
}

// ---------------------------------------------------------------------------
// System (macOS `say`)
// ---------------------------------------------------------------------------

/// `say`'s fixed path rather than a `$PATH` lookup.
///
/// Caduceus runs from a `.app` bundle with a minimal inherited environment,
/// and this exact path has shipped on every macOS release the rest of the app
/// supports — the same reasoning `shortcuts::exec` uses for
/// `/usr/bin/osascript`. Unlike the STT/OCR/recording helpers next to this
/// file, there is nothing to build or sign here: `say` is a stock system
/// binary, not a Caduceus-authored one, so it needs neither a `build.rs` step
/// nor a TCC usage-description string to run.
const SAY_PATH: &str = "/usr/bin/say";

#[derive(Default)]
pub struct SystemNativeTts {
    /// Signalled by `stop` to interrupt whichever `say` invocation `speak` is
    /// currently waiting on. A plain field is enough — not a global — because
    /// `TtsRuntime` guarantees only one `speak` call is ever in flight on a
    /// given instance; see the trait's `stop` doc.
    interrupt: tokio::sync::Notify,
}

#[async_trait]
impl TtsBackend for SystemNativeTts {
    fn id(&self) -> &str {
        "system_native"
    }

    fn display_name(&self) -> &str {
        "System (say)"
    }

    async fn speak(&self, text: &str, settings: &VoiceSettings) -> TtsResult<()> {
        if !cfg!(target_os = "macos") {
            return Err(TtsError::Unavailable(
                "Built-in speech is only implemented for macOS. Switch to an HTTP endpoint \
                 in Settings \u{2192} Voice."
                    .into(),
            ));
        }
        if text.trim().is_empty() {
            return Err(TtsError::Empty);
        }
        if !std::path::Path::new(SAY_PATH).is_file() {
            // Not expected to ever fire on a real Mac — `say` ships with the
            // OS — but a stripped-down CI image or sandbox is not
            // impossible, and doing nothing silently would be worse than
            // saying why.
            return Err(TtsError::Unavailable(format!(
                "{SAY_PATH} was not found on this system."
            )));
        }

        let mut command = tokio::process::Command::new(SAY_PATH);
        if !settings.tts_voice.trim().is_empty() {
            command.arg("-v").arg(settings.tts_voice.trim());
        }
        if settings.tts_rate > 0.0 {
            // `say -r` wants an integer words-per-minute; `0.0` (the
            // default) means "leave it at `say`'s own default" and omits the
            // flag entirely rather than passing a meaningless `-r 0`.
            command
                .arg("-r")
                .arg((settings.tts_rate.round() as i32).to_string());
        }
        command.arg(text);
        // Mirrors `SttBackend`'s helper invocation in `stt.rs`: dropping the
        // child — which happens on the interrupted branch below — must
        // actually end the process rather than merely abandon the future
        // that was waiting on it.
        command.kill_on_drop(true);

        let mut child = command
            .spawn()
            .map_err(|e| TtsError::Failed(format!("could not run {SAY_PATH}: {e}")))?;

        tokio::select! {
            status = child.wait() => {
                let status = status
                    .map_err(|e| TtsError::Failed(format!("{SAY_PATH} did not answer: {e}")))?;
                if !status.success() {
                    return Err(TtsError::Failed(format!("{SAY_PATH} exited with {status}")));
                }
                Ok(())
            }
            _ = self.interrupt.notified() => {
                // `child` drops here; `kill_on_drop` above is what actually
                // ends the process. Being talked over is exactly what
                // barge-in is for, not a failure — see the trait's `speak` doc.
                Ok(())
            }
        }
    }

    fn stop(&self) {
        self.interrupt.notify_one();
    }

    fn availability(&self, _settings: &VoiceSettings) -> TtsAvailability {
        let (available, detail) = if !cfg!(target_os = "macos") {
            (
                false,
                "Only available on macOS. Use an HTTP endpoint instead.".to_string(),
            )
        } else if std::path::Path::new(SAY_PATH).is_file() {
            (
                true,
                "Uses the built-in `say` command. Fully offline, nothing to install.".to_string(),
            )
        } else {
            (false, format!("{SAY_PATH} was not found on this system."))
        };

        TtsAvailability {
            id: self.id().into(),
            display_name: self.display_name().into(),
            available,
            detail,
        }
    }
}

/// List installed voice names via `say -v ?`, for the Settings voice picker.
///
/// Never errors — an empty list just means the picker falls back to a
/// free-text field, which is still a working (if less friendly) way to set
/// [`VoiceSettings::tts_voice`].
pub async fn list_say_voices() -> Vec<String> {
    if !cfg!(target_os = "macos") {
        return Vec::new();
    }
    let Ok(output) = tokio::process::Command::new(SAY_PATH)
        .arg("-v")
        .arg("?")
        .output()
        .await
    else {
        return Vec::new();
    };
    // Each line looks like `Alex      en_US    # Most people recognize me by
    // my voice.`; the voice name is always the first whitespace-separated
    // token.
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

// ---------------------------------------------------------------------------
// OpenAI-compatible /audio/speech
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct OpenAiCompatibleTts {
    /// See [`SystemNativeTts::interrupt`] — identical role, different backend.
    interrupt: tokio::sync::Notify,
}

#[async_trait]
impl TtsBackend for OpenAiCompatibleTts {
    fn id(&self) -> &str {
        "openai_compatible"
    }

    fn display_name(&self) -> &str {
        "HTTP endpoint (OpenAI-compatible)"
    }

    async fn speak(&self, text: &str, settings: &VoiceSettings) -> TtsResult<()> {
        let endpoint = settings.tts_endpoint.trim();
        if endpoint.is_empty() {
            return Err(TtsError::Unavailable(
                "No speech endpoint is set in Settings \u{2192} Voice.".into(),
            ));
        }
        if text.trim().is_empty() {
            return Err(TtsError::Empty);
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| TtsError::Failed(e.to_string()))?;

        let mut body = serde_json::json!({
            "model": settings.tts_model,
            "input": text,
            // WAV, not the API's mp3 default: Caduceus already carries
            // `hound` for reading WAV and deliberately gained no crate that
            // can decode compressed audio for this feature. Every server
            // this backend targets — OpenAI, and the local servers that copy
            // its request shape — accepts this value.
            "response_format": "wav",
        });
        if !settings.tts_voice.trim().is_empty() {
            body["voice"] = serde_json::Value::String(settings.tts_voice.trim().to_string());
        }
        if settings.tts_rate > 0.0 {
            // The API's `speed` is a 0.25-4.0 multiplier — a different unit
            // from `say -r`'s words-per-minute; see `VoiceSettings::tts_rate`.
            // A value outside the accepted range is rejected by the server
            // with an ordinary 400, surfaced below like any other bad request.
            body["speed"] = serde_json::json!(settings.tts_rate);
        }

        let mut request = client.post(endpoint).json(&body);
        if let Some(key) = secrets::get_tts_api_key_opt() {
            request = request.bearer_auth(key);
        }

        let response = tokio::select! {
            result = request.send() => result.map_err(|e| TtsError::Transport {
                endpoint: endpoint.to_string(),
                detail: e.to_string(),
            })?,
            // Interrupted before a single byte came back — nothing was ever
            // said, so this is exactly as much "success" as finishing is.
            _ = self.interrupt.notified() => return Ok(()),
        };

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            return Err(TtsError::Api {
                status: status.as_u16(),
                body: crate::agent::http_error_message(&body_text),
            });
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| TtsError::Failed(e.to_string()))?;

        play_wav_interruptible(bytes.to_vec(), &self.interrupt).await
    }

    fn stop(&self) {
        self.interrupt.notify_one();
    }

    fn availability(&self, settings: &VoiceSettings) -> TtsAvailability {
        let configured = !settings.tts_endpoint.trim().is_empty();
        TtsAvailability {
            id: self.id().into(),
            display_name: self.display_name().into(),
            available: configured,
            detail: if configured {
                format!(
                    "Posts replies to {} and plays back what it returns.",
                    settings.tts_endpoint.trim()
                )
            } else {
                "Set an endpoint URL to use this.".into()
            },
        }
    }
}

/// Play a WAV clip on a dedicated thread, watching `interrupt` so a barge-in
/// silences it within one poll slice instead of waiting out the whole reply.
///
/// `cpal::Stream` is not `Send`, so — exactly like `recorder::start` —
/// playback happens on its own thread rather than wherever this future is
/// polled, and is controlled through a channel instead of shared state.
async fn play_wav_interruptible(wav: Vec<u8>, interrupt: &tokio::sync::Notify) -> TtsResult<()> {
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();

    let mut playback =
        tauri::async_runtime::spawn_blocking(move || play_wav_blocking(&wav, stop_rx));

    loop {
        tokio::select! {
            result = &mut playback => {
                return result
                    .map_err(|e| TtsError::Failed(format!("playback thread panicked: {e}")))?;
            }
            _ = interrupt.notified() => {
                // Ask the thread to drop the stream, then keep waiting on the
                // same handle — `speak` should not return until audio has
                // actually stopped, not just until stopping was requested.
                let _ = stop_tx.send(());
            }
        }
    }
}

/// Decode and play one WAV clip, blocking the calling thread until playback
/// finishes or `stop_rx` receives a signal.
fn play_wav_blocking(wav: &[u8], stop_rx: std::sync::mpsc::Receiver<()>) -> TtsResult<()> {
    let mut reader = hound::WavReader::new(std::io::Cursor::new(wav)).map_err(|e| {
        TtsError::Failed(format!("the speech endpoint did not return readable audio: {e}"))
    })?;
    let spec = reader.spec();

    // Normalised to f32 regardless of what the endpoint sent, so the output
    // callback below only ever handles one sample type — the same reason
    // every capture callback in `recorder::start` funnels into `f32`.
    let max_amplitude = ((1i64 << (spec.bits_per_sample.max(1) - 1)) - 1) as f32;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .samples::<i32>()
            .filter_map(Result::ok)
            .map(|s| s as f32 / max_amplitude)
            .collect(),
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(Result::ok).collect(),
    };
    if samples.is_empty() {
        return Ok(());
    }

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| TtsError::Unavailable("no audio output device found".into()))?;
    let config = cpal::StreamConfig {
        channels: spec.channels,
        sample_rate: spec.sample_rate,
        buffer_size: cpal::BufferSize::Default,
    };

    let total = samples.len();
    let position = Arc::new(Mutex::new(0usize));
    let cb_position = position.clone();
    let stream = device
        .build_output_stream(
            // Owned, not `&config` — cpal 0.18's builder takes the config by
            // value; see `recorder::start`'s identical `build_input_stream`
            // call for the same convention on the capture side.
            config,
            move |data: &mut [f32], _: &_| {
                let mut pos = cb_position.lock();
                for slot in data.iter_mut() {
                    *slot = samples.get(*pos).copied().unwrap_or(0.0);
                    *pos += 1;
                }
            },
            |e| log::error!("speech playback error: {e}"),
            None,
        )
        .map_err(|e| TtsError::Failed(e.to_string()))?;

    stream.play().map_err(|e| TtsError::Failed(e.to_string()))?;

    // A hard ceiling derived from the clip's own length, so a device that
    // never reports "done" cannot hang `speak` forever — the same
    // safety-net reasoning as `recorder::start`'s `max_secs`.
    let frame_rate = (spec.sample_rate as usize * spec.channels as usize).max(1);
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_secs_f64(total as f64 / frame_rate as f64)
        + std::time::Duration::from_millis(300);

    loop {
        if *position.lock() >= total || std::time::Instant::now() >= deadline {
            break;
        }
        // Polled in short slices rather than slept for the full clip, so a
        // `stop` lands quickly instead of waiting out the reply.
        if stop_rx
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_ok()
        {
            break;
        }
    }
    drop(stream); // silences the device immediately; nothing needs the tail
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_backend_reports_availability_without_side_effects() {
        let settings = VoiceSettings::default();
        let all = all_availability(&settings);
        assert_eq!(all.len(), 3);
        assert!(all.iter().any(|a| a.id == "openai_compatible"));
        assert!(all.iter().any(|a| a.id == "system_native"));
        assert!(all.iter().any(|a| a.id == "disabled"));
    }

    #[test]
    fn an_empty_endpoint_marks_the_http_backend_unavailable() {
        let settings = VoiceSettings {
            tts_endpoint: String::new(),
            ..Default::default()
        };
        let a = OpenAiCompatibleTts::default().availability(&settings);
        assert!(!a.available);
        assert!(a.detail.contains("endpoint"));
    }

    #[tokio::test]
    async fn the_disabled_backend_always_refuses_to_speak() {
        let err = DisabledTts.speak("hello", &VoiceSettings::default()).await;
        assert!(matches!(err, Err(TtsError::Disabled)));
    }

    #[tokio::test]
    async fn empty_text_is_rejected_before_any_network_call() {
        // A non-empty endpoint but empty text must fail on the text check,
        // not the endpoint check — and must do so without attempting a
        // request, which is what makes this test fast and deterministic.
        let settings = VoiceSettings {
            tts_endpoint: "http://127.0.0.1:1/audio/speech".into(),
            ..Default::default()
        };
        let err = OpenAiCompatibleTts::default().speak("   ", &settings).await;
        assert!(matches!(err, Err(TtsError::Empty)));
    }

    #[tokio::test]
    async fn stop_before_speaking_is_a_safe_no_op() {
        // `TtsBackend::stop`'s contract is "safe to call any time", including
        // before `speak` has ever run on this instance.
        let tts = SystemNativeTts::default();
        tts.stop();
        let openai = OpenAiCompatibleTts::default();
        openai.stop();
    }

    #[test]
    fn tts_backend_kind_uses_the_shared_openai_compatible_spelling() {
        let json = serde_json::to_string(&TtsBackendKind::OpenAiCompatible).unwrap();
        assert_eq!(json, "\"openai_compatible\"");

        let parsed: TtsBackendKind = serde_json::from_str("\"open_ai_compatible\"").unwrap();
        assert_eq!(parsed, TtsBackendKind::OpenAiCompatible);
    }
}
