//! Driving `caduceus-record`.
//!
//! One recording at a time, started and stopped from anywhere, with the state
//! readable so the HUD and the page always agree about whether something is
//! running. The helper does the hard part (see `macos/CaduceusRecorder.swift`);
//! this owns its lifetime and makes sure it never outlives the app.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// What is being recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecordMode {
    /// Screen video plus system audio.
    Screen,
    /// System audio only — for meetings, where the video is somebody's slides.
    Audio,
}

impl RecordMode {
    fn helper_arg(self) -> &'static str {
        match self {
            Self::Screen => "screen",
            Self::Audio => "audio",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Screen => "mp4",
            Self::Audio => "m4a",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingStatus {
    pub active: bool,
    pub paused: bool,
    pub mode: Option<RecordMode>,
    pub path: Option<String>,
    pub seconds: u64,
    /// Rough input level, 0–1, for a meter. Only while the microphone is on.
    pub level: f32,
    /// Set when the last recording ended badly, so the UI can say why.
    pub error: Option<String>,
}

struct Session {
    child: Child,
    stdin: ChildStdin,
    mode: RecordMode,
    path: PathBuf,
    started: Instant,
    /// Total time spent paused, so the clock reports recorded length.
    paused_for: Duration,
    paused_at: Option<Instant>,
}

#[derive(Clone, Default)]
pub struct RecorderRuntime {
    session: Arc<Mutex<Option<Session>>>,
    level: Arc<Mutex<f32>>,
    error: Arc<Mutex<Option<String>>>,
    ready: Arc<AtomicBool>,
}

/// How long to let the helper finish writing the file after `stop`.
///
/// Encoding the tail of an hour-long recording genuinely takes a few seconds,
/// so this is generous — but bounded, because the alternative is a UI that
/// waits forever on a helper that has wedged.
const FINALISE_TIMEOUT: Duration = Duration::from_secs(25);

impl RecorderRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn status(&self) -> RecordingStatus {
        let guard = self.session.lock();
        match guard.as_ref() {
            Some(session) => {
                let paused_now = session.paused_at.map(|at| at.elapsed()).unwrap_or_default();
                let elapsed = session
                    .started
                    .elapsed()
                    .saturating_sub(session.paused_for + paused_now);
                RecordingStatus {
                    active: true,
                    paused: session.paused_at.is_some(),
                    mode: Some(session.mode),
                    path: Some(session.path.to_string_lossy().into_owned()),
                    seconds: elapsed.as_secs(),
                    level: *self.level.lock(),
                    error: self.error.lock().clone(),
                }
            }
            None => RecordingStatus {
                active: false,
                paused: false,
                mode: None,
                path: None,
                seconds: 0,
                level: 0.0,
                error: self.error.lock().clone(),
            },
        }
    }

    /// Begin recording. `Err` if one is already running.
    pub fn start<F>(
        &self,
        mode: RecordMode,
        with_microphone: bool,
        on_partial: F,
    ) -> Result<String, String>
    where
        F: Fn(String) + Send + 'static,
    {
        if self.session.lock().is_some() {
            return Err("Something is already being recorded.".into());
        }

        let helper = helper_path().ok_or(
            "The recorder is missing from this build. Reinstall Caduceus, or rebuild it on a Mac \
             with the Xcode command line tools.",
        )?;

        let directory = dirs::home_dir()
            .map(|home| home.join("Movies"))
            .filter(|p| p.is_dir())
            .or_else(dirs::download_dir)
            .ok_or("Could not find anywhere to save the recording.")?;

        let path = directory.join(format!(
            "Caduceus {}.{}",
            chrono::Local::now().format("%Y-%m-%d %H.%M.%S"),
            mode.extension()
        ));

        let mut command = Command::new(&helper);
        command
            .arg(mode.helper_arg())
            .arg(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if with_microphone {
            command.arg("--mic");
        }

        let mut child = command
            .spawn()
            .map_err(|e| format!("Could not start the recorder: {e}"))?;

        let stdin = child.stdin.take().ok_or("the recorder has no stdin")?;
        let stdout = child.stdout.take().ok_or("the recorder has no stdout")?;

        // Taken only now: `status()` is polled continuously by the HUD, and the
        // spawn above can take a moment the first time macOS verifies the
        // helper's signature.
        let mut guard = self.session.lock();
        if guard.is_some() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Something is already being recorded.".into());
        }

        *self.error.lock() = None;
        self.ready.store(false, Ordering::SeqCst);

        // Read the helper's events on a thread. Deliberately not waited on here:
        // `startCapture` can take a second, and blocking a caller — which may be
        // the main thread via a hotkey — is the bug this whole design avoids.
        {
            let level = self.level.clone();
            let error = self.error.clone();
            let ready = self.ready.clone();
            thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines().map_while(Result::ok) {
                    let mut parts = line.splitn(2, '\t');
                    match (parts.next().unwrap_or(""), parts.next().unwrap_or("")) {
                        ("ready", _) => ready.store(true, Ordering::SeqCst),
                        ("level", value) => {
                            if let Ok(parsed) = value.parse::<f32>() {
                                *level.lock() = parsed;
                            }
                        }
                        ("partial", text) => on_partial(text.to_string()),
                        ("transcription", message) => {
                            log::info!("meeting transcription: {message}")
                        }
                        ("transcription-error", message) => {
                            // Preview failure is deliberately non-fatal: the
                            // durable recording still gets its final pass.
                            log::warn!("meeting live transcription: {message}")
                        }
                        ("error", message) => {
                            log::error!("recorder: {message}");
                            *error.lock() = Some(message.to_string());
                        }
                        ("done", where_) => log::info!("recording saved to {where_}"),
                        _ => {}
                    }
                }
            });
        }

        *guard = Some(Session {
            child,
            stdin,
            mode,
            path: path.clone(),
            started: Instant::now(),
            paused_for: Duration::ZERO,
            paused_at: None,
        });

        Ok(path.to_string_lossy().into_owned())
    }

    pub fn set_paused(&self, paused: bool) -> Result<bool, String> {
        let mut guard = self.session.lock();
        let session = guard.as_mut().ok_or("Nothing is being recorded.")?;

        writeln!(session.stdin, "{}", if paused { "pause" } else { "resume" })
            .map_err(|e| format!("The recorder stopped listening: {e}"))?;
        let _ = session.stdin.flush();

        if paused {
            session.paused_at.get_or_insert_with(Instant::now);
        } else if let Some(at) = session.paused_at.take() {
            session.paused_for += at.elapsed();
        }
        Ok(paused)
    }

    /// Stop and finish the file. Returns where it was saved.
    pub fn stop(&self) -> Result<String, String> {
        let Some(mut session) = self.session.lock().take() else {
            return Err("Nothing is being recorded.".into());
        };

        let _ = writeln!(session.stdin, "stop");
        let _ = session.stdin.flush();
        // Closing the pipe is the backstop: a helper that has stopped reading
        // still gets EOF and shuts down.
        drop(session.stdin);

        let deadline = Instant::now() + FINALISE_TIMEOUT;
        loop {
            match session.child.try_wait() {
                Ok(Some(_)) | Err(_) => break,
                Ok(None) => {}
            }
            if Instant::now() >= deadline {
                log::warn!("the recorder did not finish in time; killing it");
                let _ = session.child.kill();
                let _ = session.child.wait();
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }

        *self.level.lock() = 0.0;

        if let Some(message) = self.error.lock().clone() {
            return Err(message);
        }
        if !session.path.exists() {
            return Err(
                "The recording produced no file. Grant Caduceus Screen Recording in System \
                 Settings → Privacy & Security, then quit and reopen it — macOS requires a \
                 restart for that one."
                    .into(),
            );
        }
        Ok(session.path.to_string_lossy().into_owned())
    }

    /// Kill any recording at shutdown, so nothing outlives the app.
    pub fn shutdown(&self) {
        if self.session.lock().is_some() {
            let _ = self.stop();
        }
    }
}

fn helper_path() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for relative in [
                "../Resources/bin/caduceus-record",
                "bin/caduceus-record",
                "caduceus-record",
            ] {
                let candidate = dir.join(relative);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bin/caduceus-record");
    dev.is_file().then_some(dev)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_modes_write_the_right_container() {
        // An .mp4 with no video track confuses QuickLook, and an .m4a with one
        // is a video file people cannot scrub.
        assert_eq!(RecordMode::Screen.extension(), "mp4");
        assert_eq!(RecordMode::Audio.extension(), "m4a");
    }

    #[test]
    fn nothing_is_recording_before_anything_starts() {
        let runtime = RecorderRuntime::new();
        let status = runtime.status();
        assert!(!status.active);
        assert_eq!(status.seconds, 0);
        assert!(status.path.is_none());
    }

    #[test]
    fn stopping_nothing_is_an_error_rather_than_a_panic() {
        let runtime = RecorderRuntime::new();
        assert!(runtime.stop().is_err());
        assert!(runtime.set_paused(true).is_err());
    }
}
