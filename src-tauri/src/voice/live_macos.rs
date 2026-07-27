//! macOS live dictation: AVAudioEngine + Speech partial results via `caduceus-stt-live`.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::thread;
use std::time::Duration;

use std::sync::Arc;

use parking_lot::Mutex;

pub struct LiveSession {
    child: Child,
    stdin: ChildStdin,
    wav_path: Arc<Mutex<Option<PathBuf>>>,
    final_text: Arc<Mutex<Option<String>>>,
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
        let wav_slot = wav_path.clone();
        let final_slot = final_text.clone();

        thread::spawn(move || {
            let mut line = String::new();
            while reader.read_line(&mut line).ok().filter(|&n| n > 0).is_some() {
                let trimmed = line.trim();
                let mut parts = trimmed.splitn(2, '\t');
                let kind = parts.next().unwrap_or("");
                let payload = parts.next().unwrap_or("").to_string();
                match kind {
                    "partial" => on_partial(payload),
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
        })
    }

    pub fn stop(mut self) -> Result<(String, Vec<u8>), String> {
        writeln!(self.stdin, "stop").map_err(|e| e.to_string())?;
        let _ = self.stdin.flush();
        let _ = self.child.wait();

        let text = self
            .final_text
            .lock()
            .clone()
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
