//! Speech-to-text backends.
//!
//! Mirrors the [`AgentBackend`](crate::agent::AgentBackend) pattern: a small
//! trait, a couple of implementations, and a resolver that maps the user's
//! setting to one of them. Adding a new recogniser means one impl and one match
//! arm — see `docs/PLUGIN_GUIDE.md`.
//!
//! Two ship by default:
//!
//! * [`SystemNativeStt`] — Apple's `Speech.framework`, on-device where the
//!   language pack allows. macOS only, and only when the bundled Swift helper
//!   compiled successfully.
//! * [`OpenAiCompatibleStt`] — any `/audio/transcriptions` endpoint. Covers
//!   `whisper.cpp`'s server, `faster-whisper-server`, LM Studio, Speaches, and
//!   OpenAI itself.

use async_trait::async_trait;
use serde::Serialize;
use thiserror::Error;

use crate::settings::{secrets, SttBackendKind, VoiceSettings};

#[derive(Debug, Error)]
pub enum SttError {
    #[error("speech-to-text is turned off. Pick a backend in Settings \u{2192} Voice.")]
    Disabled,
    #[error("{0}")]
    Unavailable(String),
    #[error("could not reach the transcription endpoint at {endpoint}: {detail}")]
    Transport { endpoint: String, detail: String },
    #[error("the transcription service returned {status}: {body}")]
    Api { status: u16, body: String },
    #[error("transcription failed: {0}")]
    Failed(String),
    #[error("nothing was said")]
    Empty,
}

pub type SttResult<T> = Result<T, SttError>;

/// Whether a backend can run right now, and why not if it cannot.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SttAvailability {
    pub id: String,
    pub display_name: String,
    pub available: bool,
    /// Explanation shown in Settings when `available` is false.
    pub detail: String,
}

/// A speech recogniser Caduceus can use.
#[async_trait]
pub trait SttBackend: Send + Sync {
    fn id(&self) -> &str;
    fn display_name(&self) -> &str;

    /// Transcribe 16 kHz mono 16-bit WAV bytes.
    async fn transcribe(&self, wav: Vec<u8>, settings: &VoiceSettings) -> SttResult<String>;

    /// Cheap, side-effect-free check used to populate Settings.
    fn availability(&self, settings: &VoiceSettings) -> SttAvailability;
}

/// Resolve the configured backend.
pub fn backend_for(kind: SttBackendKind) -> Box<dyn SttBackend> {
    match kind {
        SttBackendKind::Disabled => Box::new(DisabledStt),
        SttBackendKind::SystemNative => Box::new(SystemNativeStt),
        SttBackendKind::OpenAiCompatible => Box::new(OpenAiCompatibleStt),
    }
}

/// Availability of every backend, for the Settings → Voice picker.
pub fn all_availability(settings: &VoiceSettings) -> Vec<SttAvailability> {
    [
        SttBackendKind::SystemNative,
        SttBackendKind::OpenAiCompatible,
        SttBackendKind::Disabled,
    ]
    .into_iter()
    .map(|k| backend_for(k).availability(settings))
    .collect()
}

// ---------------------------------------------------------------------------
// Disabled
// ---------------------------------------------------------------------------

pub struct DisabledStt;

#[async_trait]
impl SttBackend for DisabledStt {
    fn id(&self) -> &str {
        "disabled"
    }
    fn display_name(&self) -> &str {
        "Off"
    }
    async fn transcribe(&self, _wav: Vec<u8>, _settings: &VoiceSettings) -> SttResult<String> {
        Err(SttError::Disabled)
    }
    fn availability(&self, _settings: &VoiceSettings) -> SttAvailability {
        SttAvailability {
            id: "disabled".into(),
            display_name: "Off".into(),
            available: true,
            detail: "Push-to-talk does nothing.".into(),
        }
    }
}

// ---------------------------------------------------------------------------
// System (macOS Speech.framework)
// ---------------------------------------------------------------------------

pub struct SystemNativeStt;

/// Locate the bundled Swift helper.
///
/// Checked in two places so it works both from an installed `.app` and from
/// `npm run start`, where nothing has been bundled yet.
fn stt_helper_path() -> Option<std::path::PathBuf> {
    // 1. Next to the running executable (Contents/Resources in a macOS bundle).
    if let Ok(exe) = std::env::current_exe() {
        for relative in ["../Resources/bin/caduceus-stt", "bin/caduceus-stt", "caduceus-stt"] {
            if let Some(dir) = exe.parent() {
                let candidate = dir.join(relative);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    // 2. The source tree, for development builds.
    let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bin/caduceus-stt");
    dev.is_file().then_some(dev)
}

#[async_trait]
impl SttBackend for SystemNativeStt {
    fn id(&self) -> &str {
        "system_native"
    }

    fn display_name(&self) -> &str {
        "System (on-device)"
    }

    async fn transcribe(&self, wav: Vec<u8>, settings: &VoiceSettings) -> SttResult<String> {
        if !cfg!(target_os = "macos") {
            return Err(SttError::Unavailable(
                "On-device speech recognition is only implemented for macOS. \
                 Switch to an HTTP endpoint in Settings \u{2192} Voice \u{2014} a local \
                 Whisper server works well."
                    .into(),
            ));
        }

        let helper = stt_helper_path().ok_or_else(|| {
            SttError::Unavailable(
                "The bundled speech helper is missing. It is built by `swiftc` at compile \
                 time; install the Xcode Command Line Tools and rebuild, or switch to an \
                 HTTP endpoint in Settings \u{2192} Voice."
                    .into(),
            )
        })?;

        // The framework wants a file URL, so the recording goes to a temp file
        // that is removed as soon as the helper exits.
        let path = std::env::temp_dir().join(format!("caduceus-stt-{}.wav", uuid::Uuid::new_v4()));
        tokio::fs::write(&path, &wav)
            .await
            .map_err(|e| SttError::Failed(format!("could not write the recording: {e}")))?;

        let mut command = tokio::process::Command::new(&helper);
        command.arg(&path);
        if !settings.stt_language.trim().is_empty() {
            command.arg(settings.stt_language.trim());
        }

        let output = command.output().await;
        let _ = tokio::fs::remove_file(&path).await;

        let output = output.map_err(|e| SttError::Failed(format!("could not run the speech helper: {e}")))?;

        if !output.status.success() {
            return Err(SttError::Failed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }

        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if text.is_empty() {
            return Err(SttError::Empty);
        }
        Ok(text)
    }

    fn availability(&self, _settings: &VoiceSettings) -> SttAvailability {
        let (available, detail) = if !cfg!(target_os = "macos") {
            (
                false,
                "Only available on macOS. Use an HTTP endpoint instead.".to_string(),
            )
        } else if stt_helper_path().is_some() {
            (
                true,
                "Uses Apple's Speech framework. Runs on-device when the language pack is \
                 installed, so audio never leaves your Mac."
                    .to_string(),
            )
        } else {
            (
                false,
                "The speech helper was not built. Install the Xcode Command Line Tools \
                 (`xcode-select --install`) and rebuild Caduceus."
                    .to_string(),
            )
        };

        SttAvailability {
            id: self.id().into(),
            display_name: self.display_name().into(),
            available,
            detail,
        }
    }
}

// ---------------------------------------------------------------------------
// OpenAI-compatible /audio/transcriptions
// ---------------------------------------------------------------------------

pub struct OpenAiCompatibleStt;

#[async_trait]
impl SttBackend for OpenAiCompatibleStt {
    fn id(&self) -> &str {
        "openai_compatible"
    }

    fn display_name(&self) -> &str {
        "HTTP endpoint (Whisper-compatible)"
    }

    async fn transcribe(&self, wav: Vec<u8>, settings: &VoiceSettings) -> SttResult<String> {
        let endpoint = settings.stt_endpoint.trim();
        if endpoint.is_empty() {
            return Err(SttError::Unavailable(
                "No transcription endpoint is set in Settings \u{2192} Voice.".into(),
            ));
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| SttError::Failed(e.to_string()))?;

        let part = reqwest::multipart::Part::bytes(wav)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| SttError::Failed(e.to_string()))?;

        let mut form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", settings.stt_model.clone())
            .text("response_format", "json");
        if !settings.stt_language.trim().is_empty() {
            // The API wants a bare ISO-639-1 code, so "en-US" becomes "en".
            let code = settings
                .stt_language
                .trim()
                .split(['-', '_'])
                .next()
                .unwrap_or("")
                .to_string();
            form = form.text("language", code);
        }

        let mut request = client.post(endpoint).multipart(form);
        if let Some(key) = secrets::get_stt_api_key_opt() {
            request = request.bearer_auth(key);
        }

        let response = request.send().await.map_err(|e| SttError::Transport {
            endpoint: endpoint.to_string(),
            detail: e.to_string(),
        })?;

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(SttError::Api {
                status: status.as_u16(),
                body: crate::agent::http_error_message(&body),
            });
        }

        let text = parse_transcription(&body)?;
        if text.is_empty() {
            return Err(SttError::Empty);
        }
        Ok(text)
    }

    fn availability(&self, settings: &VoiceSettings) -> SttAvailability {
        let configured = !settings.stt_endpoint.trim().is_empty();
        SttAvailability {
            id: self.id().into(),
            display_name: self.display_name().into(),
            available: configured,
            detail: if configured {
                format!(
                    "Posts your recording to {}. Point this at a local Whisper server to keep \
                     audio on your machine.",
                    settings.stt_endpoint.trim()
                )
            } else {
                "Set an endpoint URL to use this.".into()
            },
        }
    }
}

/// Read the transcript out of a response.
///
/// The spec says `{"text": "..."}`, but `whisper.cpp`'s server and a few others
/// return the verbose form with a `segments` array instead, so both are handled.
fn parse_transcription(body: &str) -> SttResult<String> {
    // Some servers honour `response_format=text` regardless of what we asked
    // for and return a bare string, which is not valid JSON.
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        let text = body.trim();
        return if text.is_empty() {
            Err(SttError::Empty)
        } else {
            Ok(text.to_string())
        };
    };

    if let Some(s) = json.as_str() {
        return Ok(s.trim().to_string());
    }
    if let Some(text) = json.get("text").and_then(|v| v.as_str()) {
        return Ok(text.trim().to_string());
    }
    if let Some(segments) = json.get("segments").and_then(|v| v.as_array()) {
        let joined: String = segments
            .iter()
            .filter_map(|s| s.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("");
        return Ok(joined.trim().to_string());
    }
    Err(SttError::Failed(
        "the transcription response had no text field".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_standard_response() {
        assert_eq!(
            parse_transcription(r#"{"text":"  hello world  "}"#).unwrap(),
            "hello world"
        );
    }

    #[test]
    fn parses_whisper_cpp_segment_arrays() {
        let body = r#"{"segments":[{"text":"hello "},{"text":"world"}]}"#;
        assert_eq!(parse_transcription(body).unwrap(), "hello world");
    }

    #[test]
    fn accepts_a_bare_text_body() {
        assert_eq!(parse_transcription("just text\n").unwrap(), "just text");
    }

    #[test]
    fn reports_an_unrecognisable_response() {
        assert!(parse_transcription(r#"{"unexpected":1}"#).is_err());
    }

    #[test]
    fn every_backend_reports_availability_without_side_effects() {
        let settings = VoiceSettings::default();
        let all = all_availability(&settings);
        assert_eq!(all.len(), 3);
        assert!(all.iter().any(|a| a.id == "openai_compatible"));
        // The default endpoint is pre-filled, so the HTTP backend is usable.
        assert!(all.iter().find(|a| a.id == "openai_compatible").unwrap().available);
    }

    #[test]
    fn an_empty_endpoint_marks_the_http_backend_unavailable() {
        let settings = VoiceSettings {
            stt_endpoint: String::new(),
            ..Default::default()
        };
        let a = OpenAiCompatibleStt.availability(&settings);
        assert!(!a.available);
        assert!(a.detail.contains("endpoint"));
    }
}
