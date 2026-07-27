//! macOS live dictation: AVAudioEngine + Speech partial results via `caduceus-stt-live`.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::thread;
use std::time::Duration;

use std::sync::Arc;

use parking_lot::Mutex;

/// How long to give the helper to flush its final transcript after `stop`.
///
/// Long enough for Speech to finalise a normal utterance, short enough that a
/// wedged helper is a two-second annoyance rather than a hang. Whatever the
/// last partial was is used if this expires, so the timeout costs accuracy at
/// worst — never the whole transcript.
const FINALISE_TIMEOUT: Duration = Duration::from_secs(6);

/// How long to wait for a killed helper to actually die before giving up on it.
const REAP_TIMEOUT: Duration = Duration::from_millis(600);

pub struct LiveSession {
    child: Child,
    stdin: ChildStdin,
    wav_path: Arc<Mutex<Option<PathBuf>>>,
    final_text: Arc<Mutex<Option<String>>>,
    /// The most recent partial, kept as a fallback transcript.
    last_partial: Arc<Mutex<Option<String>>>,
}

/// Wait for a child, and kill it if it outstays `limit`.
///
/// `Child::wait` has no timeout, which is how a helper stuck inside Speech's
/// finalisation used to hold whichever thread called `stop` for two minutes.
fn wait_or_kill(child: &mut Child, limit: Duration) {
    let deadline = std::time::Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => {}
            // Already reaped, or a state we cannot recover from either way.
            Err(_) => return,
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }

    log::warn!("live speech helper did not exit in time; killing it");
    let _ = child.kill();

    // Reap it, so it does not sit as a zombie for the life of the app.
    let reap_by = std::time::Instant::now() + REAP_TIMEOUT;
    while std::time::Instant::now() < reap_by {
        if matches!(child.try_wait(), Ok(Some(_)) | Err(_)) {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

impl LiveSession {
    pub fn start(
        language: &str,
        on_partial: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let helper = live_helper_path().ok_or_else(|| {
            String::from(
                "Live speech helper missing. Rebuild Caduceus with Xcode Command Line Tools.",
            )
        })?;

        let mut child = Command::new(&helper)
            .arg(language)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Could not start live speech helper: {e}"))?;

        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stdin = child.stdin.take().ok_or("no stdin")?;

        let mut reader = BufReader::new(stdout);
        let mut ready = false;
        let mut line = String::new();
        // Long enough for the microphone to spin up, short enough that a broken
        // helper does not leave the UI hanging.
        let mut deadline = std::time::Instant::now() + Duration::from_secs(15);
        while std::time::Instant::now() < deadline {
            line.clear();
            if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                break;
            }
            let trimmed = line.trim();
            if trimmed == "ready" {
                ready = true;
                break;
            }
            // First run only: macOS is showing its permission sheets and the
            // clock should be the user's, not ours.
            if trimmed == "prompting" {
                deadline = std::time::Instant::now() + Duration::from_secs(180);
                continue;
            }
            if let Some(msg) = trimmed.strip_prefix("error\t") {
                return Err(msg.to_string());
            }
        }
        if !ready {
            return Err(
                "Live speech helper did not become ready. If macOS never asked for \
                 microphone or speech-recognition access, enable Caduceus under System \
                 Settings → Privacy & Security for both."
                    .into(),
            );
        }

        let wav_path = Arc::new(Mutex::new(None::<PathBuf>));
        let final_text = Arc::new(Mutex::new(None::<String>));
        let last_partial = Arc::new(Mutex::new(None::<String>));
        let wav_slot = wav_path.clone();
        let final_slot = final_text.clone();
        let partial_slot = last_partial.clone();

        thread::spawn(move || {
            let mut line = String::new();
            while reader.read_line(&mut line).ok().filter(|&n| n > 0).is_some() {
                let trimmed = line.trim();
                let mut parts = trimmed.splitn(2, '\t');
                let kind = parts.next().unwrap_or("");
                let payload = parts.next().unwrap_or("").to_string();
                match kind {
                    "partial" => {
                        // Kept as well as forwarded: it is the fallback if the
                        // final result never arrives.
                        *partial_slot.lock() = Some(payload.clone());
                        on_partial(payload);
                    }
                    "final" => *final_slot.lock() = Some(payload),
                    "wav" => *wav_slot.lock() = Some(PathBuf::from(payload)),
                    "error" => log::error!("live speech: {payload}"),
                    _ => {}
                }
                line.clear();
            }
        });

        Ok(Self {
            child,
            stdin,
            wav_path,
            final_text,
            last_partial,
        })
    }

    /// Ask the helper to stop feeding audio to the recogniser, or resume.
    ///
    /// Pausing keeps the session, the transcript so far and the microphone tap
    /// alive; it simply stops appending buffers. That is what makes "hold space
    /// to pause" cheap enough to be instant, and what lets the recording HUD
    /// offer a pause at all — tearing the session down and building a new one
    /// would lose everything said before the pause.
    pub fn set_paused(&mut self, paused: bool) -> Result<(), String> {
        writeln!(self.stdin, "{}", if paused { "pause" } else { "resume" })
            .map_err(|e| format!("the speech helper stopped listening: {e}"))?;
        self.stdin.flush().map_err(|e| e.to_string())
    }

    pub fn stop(mut self) -> Result<(String, Vec<u8>), String> {
        // A helper that has stopped listening to us is not a reason to fail:
        // it may already be exiting, and the transcript we want may already
        // have arrived on the reader thread.
        let _ = writeln!(self.stdin, "stop");
        let _ = self.stdin.flush();
        // Dropping stdin closes the pipe, so a helper blocked in `readLine`
        // gets EOF even if the write above went nowhere.
        drop(self.stdin);

        wait_or_kill(&mut self.child, FINALISE_TIMEOUT);

        // The last partial is a perfectly good transcript. Speech occasionally
        // never delivers an `isFinal` result — most often when on-device
        // recognition is selected but its language model has not finished
        // downloading — and the old code treated that as "you said nothing"
        // after making everyone wait two minutes to be told so.
        let text = self
            .final_text
            .lock()
            .clone()
            .or_else(|| self.last_partial.lock().clone())
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| "Nothing was said — hold the key a little longer.".to_string())?;

        let wav = if let Some(path) = self.wav_path.lock().take() {
            std::fs::read(&path).unwrap_or_default()
        } else {
            Vec::new()
        };

        Ok((text, wav))
    }
}

fn live_helper_path() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        for relative in [
            "../Resources/bin/caduceus-stt-live",
            "bin/caduceus-stt-live",
            "caduceus-stt-live",
        ] {
            if let Some(dir) = exe.parent() {
                let candidate = dir.join(relative);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bin/caduceus-stt-live");
    dev.is_file().then_some(dev)
}
