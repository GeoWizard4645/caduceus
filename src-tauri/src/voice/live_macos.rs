//! macOS live dictation: local Parakeet on supported Apple Silicon Macs, with
//! Apple Speech as the compatibility and first-model-download fallback.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::thread;
use std::time::Duration;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;

static PARAKEET_PREPARING: AtomicBool = AtomicBool::new(false);

/// Whether each helper kind has already failed once this run. See
/// [`mark_helper_failed`] for why this is remembered rather than re-learned.
static PARAKEET_FAILED: AtomicBool = AtomicBool::new(false);
static APPLE_SPEECH_FAILED: AtomicBool = AtomicBool::new(false);

/// How long to give the helper to flush its final transcript after `stop`.
///
/// Long enough for Speech to finalise a normal utterance, short enough that a
/// wedged helper is a two-second annoyance rather than a hang. Whatever the
/// last partial was is used if this expires, so the timeout costs accuracy at
/// worst — never the whole transcript.
const FINALISE_TIMEOUT: Duration = Duration::from_secs(6);

/// How long to wait for a killed helper to actually die before giving up on it.
const REAP_TIMEOUT: Duration = Duration::from_millis(600);

/// The live-dictation helpers Caduceus ships, in the order they are tried.
///
/// This used to be a single path picked once by `live_helper_path` and never
/// reconsidered: if the chosen helper failed, dictation simply did not work.
/// Splitting "which helpers exist" from "which one is currently in charge"
/// (that is [`VoiceRuntime::open_session`](crate::voice::VoiceRuntime)) is what
/// lets a bad Parakeet build fall through to Apple Speech, and a bad Apple
/// Speech build fall through to batch capture, instead of taking dictation
/// down with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveHelperKind {
    Parakeet,
    AppleSpeech,
}

impl LiveHelperKind {
    fn binary_name(self) -> &'static str {
        match self {
            Self::Parakeet => "caduceus-parakeet-live",
            Self::AppleSpeech => "caduceus-stt-live",
        }
    }

    /// A human-facing name for error messages — enough to say which helper
    /// broke without expecting the reader to know a binary name.
    fn label(self) -> &'static str {
        match self {
            Self::Parakeet => "The local Parakeet speech helper",
            Self::AppleSpeech => "Apple's on-device Speech helper",
        }
    }

    fn failed_flag(self) -> &'static AtomicBool {
        match self {
            Self::Parakeet => &PARAKEET_FAILED,
            Self::AppleSpeech => &APPLE_SPEECH_FAILED,
        }
    }
}

/// Whether `kind` has already failed once this run — see [`mark_helper_failed`].
pub fn helper_has_failed(kind: LiveHelperKind) -> bool {
    kind.failed_flag().load(Ordering::SeqCst)
}

/// Remember that a helper broke, for the rest of the process's life.
///
/// Without this, every single press of the dictation key would pay the same
/// price again: fifteen seconds waiting for a helper that is going to fail
/// exactly the same way it failed last time, or a session that records
/// something, hands it to a helper that crashes on the first buffer, and
/// produces nothing — every time. A helper is not going to un-crash between
/// one press and the next, so once [`VoiceRuntime::open_session`]
/// (crate::voice::VoiceRuntime) or [`LiveSession::stop`] discovers a failure,
/// [`live_helper_candidates`] simply stops offering it for the rest of this
/// run. The next launch of Caduceus gets a clean slate, in case the failure
/// was caused by something that has since been fixed (a model finishing
/// preparation, a permission being granted).
pub fn mark_helper_failed(kind: LiveHelperKind) {
    kind.failed_flag().store(true, Ordering::SeqCst);
}

/// A live-dictation helper binary Caduceus found on disk and believes is
/// worth trying right now.
pub struct HelperCandidate {
    pub kind: LiveHelperKind,
    pub path: PathBuf,
}

impl HelperCandidate {
    pub fn label(&self) -> &'static str {
        self.kind.label()
    }
}

/// What the reader thread saw before the session was up and running.
enum Handshake {
    Ready,
    /// macOS is asking the user for microphone or speech access.
    Prompting,
    Failed(String),
    /// The helper closed its output without ever saying it was ready.
    Gone,
}

/// How the child ended, as established by [`wait_or_kill`].
///
/// The distinction that matters is whether the helper exited *on its own* —
/// which is exactly what a crash looks like from here, an early exit nobody
/// asked for — or whether Caduceus had to reach for `kill()` because it did
/// not exit in time. Only the former is trustworthy evidence of a crash: a
/// helper that outstays `FINALISE_TIMEOUT` while still finalising a long
/// utterance is not misbehaving, and must not be branded a crash (and
/// remembered as failed) just because it was slow.
enum ChildEnd {
    ExitedOnOwn(std::process::ExitStatus),
    /// It did not exit within the deadline and was killed, or its exit status
    /// could not be read at all.
    Unclear,
}

/// Wait for a child, and kill it if it outstays `limit`.
///
/// `Child::wait` has no timeout, which is how a helper stuck inside Speech's
/// finalisation used to hold whichever thread called `stop` for two minutes.
fn wait_or_kill(child: &mut Child, limit: Duration) -> ChildEnd {
    let deadline = std::time::Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return ChildEnd::ExitedOnOwn(status),
            Ok(None) => {}
            // Already reaped, or a state we cannot recover from either way.
            Err(_) => return ChildEnd::Unclear,
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }

    log::warn!("live speech helper did not exit in time; killing it");
    let _ = child.kill();

    // Reap it, so it does not sit as a zombie for the life of the app. Whether
    // this succeeds or not, killing it ourselves means its exit is not
    // evidence of a crash — see `ChildEnd`.
    let reap_by = std::time::Instant::now() + REAP_TIMEOUT;
    while std::time::Instant::now() < reap_by {
        if matches!(child.try_wait(), Ok(Some(_)) | Err(_)) {
            return ChildEnd::Unclear;
        }
        thread::sleep(Duration::from_millis(20));
    }
    ChildEnd::Unclear
}

/// Describe an exit status for a human, favouring the signal name a crash
/// leaves behind — `EXC_BREAKPOINT` shows up here as "signal 5", which is at
/// least a searchable fact, unlike silence.
fn describe_exit(status: &std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    if let Some(signal) = status.signal() {
        format!("crashed (signal {signal})")
    } else {
        match status.code() {
            Some(code) => format!("exited unexpectedly (code {code})"),
            None => "ended abnormally".to_string(),
        }
    }
}

fn crash_message(kind: LiveHelperKind, end: &ChildEnd) -> String {
    let detail = match end {
        ChildEnd::ExitedOnOwn(status) => describe_exit(status),
        ChildEnd::Unclear => "stopped responding".to_string(),
    };
    format!(
        "{} {detail} before it produced any text \u{2014} that is a bug in the helper, not \
         something you said or did not say. Caduceus will use a different speech backend \
         next time you press the key.",
        kind.label(),
    )
}

pub struct LiveSession {
    child: Child,
    stdin: ChildStdin,
    kind: LiveHelperKind,
    wav_path: Arc<Mutex<Option<PathBuf>>>,
    final_text: Arc<Mutex<Option<String>>>,
    /// The most recent partial, kept as a fallback transcript.
    last_partial: Arc<Mutex<Option<String>>>,
    /// Set just before `stop` writes to the helper's stdin, so the reader
    /// thread can tell "I closed stdout because I was told to" apart from "I
    /// closed stdout because I crashed".
    stopping: Arc<AtomicBool>,
    /// Set by the reader thread if the helper's output closes on its own —
    /// EOF with no `stop` requested — while the session was up and running.
    /// That is what a crash on the audio callback looks like from here.
    died_unexpectedly: Arc<AtomicBool>,
}

impl LiveSession {
    pub fn start(
        candidate: &HelperCandidate,
        language: &str,
        on_partial: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let mut child = Command::new(&candidate.path)
            .arg(language)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Could not start {}: {e}", candidate.label()))?;

        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stdin = child.stdin.take().ok_or("no stdin")?;

        let wav_path = Arc::new(Mutex::new(None::<PathBuf>));
        let final_text = Arc::new(Mutex::new(None::<String>));
        let last_partial = Arc::new(Mutex::new(None::<String>));
        let stopping = Arc::new(AtomicBool::new(false));
        let died_unexpectedly = Arc::new(AtomicBool::new(false));
        let wav_slot = wav_path.clone();
        let final_slot = final_text.clone();
        let partial_slot = last_partial.clone();
        let stopping_flag = stopping.clone();
        let died_flag = died_unexpectedly.clone();

        // The handshake is read on the reader thread rather than here, and
        // reported back over a channel.
        //
        // `read_line` on a pipe has no deadline of its own, so a loop that only
        // checks the clock *between* reads is not bounded at all: a helper that
        // wedges after spawn without writing a line and without closing stdout
        // parks the caller inside the read for ever, which is the hang this
        // whole file exists to make impossible. `recv_timeout` is bounded even
        // when the read is not.
        let (handshake, answered) = std::sync::mpsc::channel::<Handshake>();

        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            let mut ready = false;

            while reader
                .read_line(&mut line)
                .ok()
                .filter(|&n| n > 0)
                .is_some()
            {
                let trimmed = line.trim();

                if !ready {
                    match trimmed {
                        "ready" => {
                            ready = true;
                            let _ = handshake.send(Handshake::Ready);
                        }
                        "prompting" => {
                            let _ = handshake.send(Handshake::Prompting);
                        }
                        _ => {
                            if let Some(msg) = trimmed.strip_prefix("error\t") {
                                let _ = handshake.send(Handshake::Failed(msg.to_string()));
                                return;
                            }
                        }
                    }
                    line.clear();
                    continue;
                }

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

            if !ready {
                let _ = handshake.send(Handshake::Gone);
            } else if !stopping_flag.load(Ordering::SeqCst) {
                // The pipe closed on its own, mid-session, with nobody having
                // asked it to. `LiveSession::stop` had not even been called
                // yet — this is the thread that finds out a helper crashed
                // while it was supposed to be listening.
                died_flag.store(true, Ordering::SeqCst);
            }
        });

        // Long enough for the microphone to spin up, short enough that a broken
        // helper does not leave the UI hanging.
        let mut deadline = std::time::Instant::now() + Duration::from_secs(15);
        let mut ready = false;
        let mut gone_during_handshake = false;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                break;
            }
            match answered.recv_timeout(left) {
                Ok(Handshake::Ready) => {
                    ready = true;
                    break;
                }
                // First run only: macOS is showing its permission sheets and the
                // clock should be the user's, not ours.
                Ok(Handshake::Prompting) => {
                    deadline = std::time::Instant::now() + Duration::from_secs(180)
                }
                Ok(Handshake::Failed(msg)) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(msg);
                }
                Ok(Handshake::Gone) => {
                    // The reader thread hit EOF before `ready` was ever set —
                    // the helper exited (or crashed) during its own start-up,
                    // rather than Caduceus giving up on it. Reported
                    // immediately: this is exactly the case `recv_timeout`
                    // exists for, so it must not sit out the rest of the
                    // fifteen (or, mid-`Prompting`, one-hundred-and-eighty)
                    // second deadline before being noticed.
                    gone_during_handshake = true;
                    break;
                }
                Err(_) => break,
            }
        }
        if !ready {
            // Killing it closes stdout, which is also what ends the reader
            // thread — and stops a wedged helper holding the microphone open.
            let _ = child.kill();
            let _ = child.wait();
            return Err(if gone_during_handshake {
                format!(
                    "{} exited during start-up instead of becoming ready.",
                    candidate.label()
                )
            } else {
                "Live speech helper did not become ready \u{2014} check Microphone and Speech \
                 Recognition are turned on for Caduceus in System Settings, then try dictation \
                 again."
                    .into()
            });
        }

        Ok(Self {
            child,
            stdin,
            kind: candidate.kind,
            wav_path,
            final_text,
            last_partial,
            stopping,
            died_unexpectedly,
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
        // Set before writing "stop", not after: the reader thread must never
        // see the EOF this causes and mistake it for the helper dying on its
        // own. A genuine crash landing in the few microseconds this races
        // against would be misread as an ordinary shutdown instead — a far
        // smaller cost than flagging every clean stop as a crash.
        self.stopping.store(true, Ordering::SeqCst);

        // A helper that has stopped listening to us is not a reason to fail:
        // it may already be exiting, and the transcript we want may already
        // have arrived on the reader thread.
        let _ = writeln!(self.stdin, "stop");
        let _ = self.stdin.flush();
        // Dropping stdin closes the pipe, so a helper blocked in `readLine`
        // gets EOF even if the write above went nowhere.
        drop(self.stdin);

        let end = wait_or_kill(&mut self.child, FINALISE_TIMEOUT);

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
            .filter(|t| !t.trim().is_empty());

        let wav = if let Some(path) = self.wav_path.lock().take() {
            std::fs::read(&path).unwrap_or_default()
        } else {
            Vec::new()
        };

        if let Some(text) = text {
            return Ok((text, wav));
        }

        // No transcript at all. An empty result and a dead helper look
        // identical from the reader thread's point of view — no `final`, no
        // `partial` — so without checking how the child actually ended, every
        // crash was reported to the user as "Nothing was said", which is what
        // happened on every dictation attempt while the Parakeet helper died
        // on the first audio buffer (see the crash reports this file's
        // top-of-module comment refers to).
        let crashed = self.died_unexpectedly.load(Ordering::SeqCst)
            || matches!(&end, ChildEnd::ExitedOnOwn(status) if !status.success());

        if crashed {
            // A helper that crashes once is not going to behave better next
            // time it is handed the microphone; remember it so the next press
            // goes straight to the fallback instead of re-discovering this.
            mark_helper_failed(self.kind);
            return Err(crash_message(self.kind, &end));
        }

        Err("Nothing was said — hold the key a little longer.".to_string())
    }
}

/// Every live-dictation helper bundled with Caduceus that is worth trying
/// right now, in preference order.
///
/// Apple Silicon prefers the same FluidAudio/Parakeet v3 path MacParakeet
/// uses. The original Apple Speech helper stays second: it covers Intel and
/// development/release environments where SwiftPM could not build the
/// optional CoreML helper — and it is what a Parakeet crash falls back to,
/// via [`VoiceRuntime::open_session`](crate::voice::VoiceRuntime).
///
/// A helper that failed earlier this run (see [`mark_helper_failed`]) is left
/// out entirely, so a repeat press does not pay its fifteen-second deadline —
/// or its crash — a second time.
pub fn live_helper_candidates() -> Vec<HelperCandidate> {
    let kinds: &[LiveHelperKind] = if parakeet_supported() {
        &[LiveHelperKind::Parakeet, LiveHelperKind::AppleSpeech]
    } else {
        &[LiveHelperKind::AppleSpeech]
    };

    // Checked in two places so it works both from an installed `.app` and
    // from `npm run start`, where nothing has been bundled yet.
    let mut search_dirs: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for prefix in ["../Resources/bin", "bin", "."] {
                search_dirs.push(dir.join(prefix));
            }
        }
    }
    search_dirs.push(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bin"));

    let mut out = Vec::new();
    for &kind in kinds {
        if helper_has_failed(kind) {
            continue;
        }
        let Some(path) = search_dirs
            .iter()
            .map(|dir| dir.join(kind.binary_name()))
            .find(|candidate| candidate.is_file())
        else {
            continue;
        };
        if kind == LiveHelperKind::Parakeet && !parakeet_model_ready(&path) {
            // First use remains functional through Apple Speech while the
            // model prepares in the background; a later dictation picks
            // Parakeet back up once `--model-ready` succeeds.
            prepare_parakeet_model(&path);
            continue;
        }
        out.push(HelperCandidate { kind, path });
    }
    out
}

fn parakeet_model_ready(helper: &std::path::Path) -> bool {
    Command::new(helper)
        .arg("--model-ready")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn prepare_parakeet_model(helper: &std::path::Path) {
    // First use remains functional through Apple Speech while the ~465 MB
    // local model prepares in an isolated background helper. A later dictation
    // automatically switches to Parakeet once `--model-ready` succeeds.
    if PARAKEET_PREPARING.swap(true, Ordering::SeqCst) {
        return;
    }
    match Command::new(helper)
        .arg("--prepare-model")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(_) => log::info!("preparing the local Parakeet transcription model"),
        Err(error) => {
            PARAKEET_PREPARING.store(false, Ordering::SeqCst);
            log::warn!("could not prepare Parakeet model: {error}");
        }
    }
}

fn parakeet_supported() -> bool {
    if !cfg!(target_arch = "aarch64") {
        return false;
    }
    // FluidAudio 0.15.4 targets macOS 14+. Caduceus itself still supports
    // earlier systems, which must select the Apple Speech helper instead of
    // finding a binary the loader cannot execute.
    Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|version| version.trim().split('.').next()?.parse::<u32>().ok())
        .is_some_and(|major| major >= 14)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Write an executable shell script to a temp file and return its path.
    ///
    /// Every scenario here is a shape a *real* helper can take — exits before
    /// saying `ready`, says `ready` then dies, wedges forever — without
    /// needing a microphone, Speech.framework, or the Parakeet binary to
    /// reproduce it.
    fn script(body: &str) -> tempfile_path::TempScript {
        tempfile_path::TempScript::new(body)
    }

    /// Minimal helper around a scratch file so scripts clean up after
    /// themselves; std has no temp-file crate dependency here otherwise.
    mod tempfile_path {
        use super::*;

        pub struct TempScript {
            pub path: PathBuf,
        }

        impl TempScript {
            pub fn new(body: &str) -> Self {
                let path = std::env::temp_dir().join(format!(
                    "caduceus-live-test-{}-{}.sh",
                    std::process::id(),
                    uuid_ish(),
                ));
                let mut file = std::fs::File::create(&path).expect("create test script");
                file.write_all(format!("#!/bin/sh\n{body}\n").as_bytes())
                    .expect("write test script");
                let mut perms = file.metadata().unwrap().permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&path, perms).expect("chmod test script");
                Self { path }
            }
        }

        impl Drop for TempScript {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.path);
            }
        }

        /// A cheap unique-enough suffix without pulling in the `uuid` crate
        /// for tests.
        fn uuid_ish() -> u128 {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        }
    }

    fn candidate(kind: LiveHelperKind, path: PathBuf) -> HelperCandidate {
        HelperCandidate { kind, path }
    }

    #[test]
    fn a_helper_that_exits_before_ready_is_reported_as_gone_not_a_timeout() {
        // Exits immediately without printing "ready" — the `Handshake::Gone`
        // path, which must fire straight away rather than waiting out the
        // fifteen-second deadline.
        let helper = script("exit 1");
        let started = std::time::Instant::now();
        let result = LiveSession::start(
            &candidate(LiveHelperKind::AppleSpeech, helper.path.clone()),
            "en-US",
            |_| {},
        );
        assert!(started.elapsed() < Duration::from_secs(5), "Gone must not wait out the deadline");
        // `LiveSession` (the `Ok` side) is not `Debug` — it owns a live
        // `Child` — so `unwrap_err` is not available; match instead.
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("a helper that exits before printing \"ready\" must not succeed"),
        };
        assert!(err.contains("exited during start-up"), "got: {err}");
        assert!(!err.to_lowercase().contains("microphone"), "must not misroute to Microphone: {err}");
    }

    #[test]
    fn a_helper_that_crashes_after_ready_names_itself_instead_of_blaming_the_user() {
        // Says "ready", then dies without ever emitting a partial or final —
        // exactly the Parakeet actor-isolation crash from the diagnosis: the
        // helper is fine during the handshake and dies on the first buffer.
        let helper = script("echo ready; sleep 0.2; exit 1");
        let session = LiveSession::start(
            &candidate(LiveHelperKind::Parakeet, helper.path.clone()),
            "en-US",
            |_| {},
        )
        .expect("handshake succeeds; the crash happens after");

        // Give the reader thread time to observe the EOF before stopping.
        thread::sleep(Duration::from_millis(400));

        let err = session.stop().unwrap_err();
        assert!(err.contains("Parakeet"), "must name the helper: {err}");
        assert!(!err.contains("Nothing was said"), "must not blame the user: {err}");
        assert!(helper_has_failed(LiveHelperKind::Parakeet), "must be remembered as failed");

        // Clean up global state for the other tests in this process.
        PARAKEET_FAILED.store(false, Ordering::SeqCst);
    }

    #[test]
    fn a_normal_stop_does_not_get_misread_as_a_crash() {
        // Says "ready", waits for a line on stdin, then exits 0 — a helper
        // behaving exactly as asked must never be marked failed.
        let helper = script("echo ready; read _line; printf 'final\\tit worked\\n'; exit 0");
        let session = LiveSession::start(
            &candidate(LiveHelperKind::AppleSpeech, helper.path.clone()),
            "en-US",
            |_| {},
        )
        .expect("handshake succeeds");

        let (text, _wav) = session.stop().expect("a clean stop must succeed");
        assert_eq!(text, "it worked");
        assert!(!helper_has_failed(LiveHelperKind::AppleSpeech));
    }

    #[test]
    fn a_helper_that_only_partially_finishes_before_dying_still_returns_its_text() {
        // Emits a partial, then dies before ever sending `final`. The partial
        // is still worth returning — losing it because the process happened
        // to crash afterwards would be strictly worse for the user than the
        // crash being silent.
        let helper = script("echo ready; printf 'partial\\thello there\\n'; sleep 0.2; exit 1");
        let session = LiveSession::start(
            &candidate(LiveHelperKind::AppleSpeech, helper.path.clone()),
            "en-US",
            |_| {},
        )
        .expect("handshake succeeds");

        thread::sleep(Duration::from_millis(400));
        let (text, _wav) = session.stop().expect("the partial is still a usable transcript");
        assert_eq!(text, "hello there");
    }

    #[test]
    fn candidates_skip_a_kind_marked_failed() {
        // `live_helper_candidates` depends on real binaries being on disk, so
        // this only exercises the failed-flag filter directly rather than
        // faking the whole search path.
        assert!(!helper_has_failed(LiveHelperKind::AppleSpeech));
        mark_helper_failed(LiveHelperKind::AppleSpeech);
        assert!(helper_has_failed(LiveHelperKind::AppleSpeech));
        APPLE_SPEECH_FAILED.store(false, Ordering::SeqCst);
    }

    #[test]
    fn helper_labels_never_accidentally_name_a_permission() {
        // `permissionFromMessage` on the TypeScript side routes on the words
        // "microphone" and "speech recognition". A crash is not a permission
        // problem, so neither label may contain them — otherwise a crash
        // would send the user to the wrong settings page.
        for kind in [LiveHelperKind::Parakeet, LiveHelperKind::AppleSpeech] {
            let label = kind.label().to_lowercase();
            assert!(!label.contains("microphone"), "{label}");
            assert!(!label.contains("speech recognition"), "{label}");
        }
    }
}
