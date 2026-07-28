//! Meeting notes: the pop-out live-transcript window, and turning the
//! system-audio half of a finished call recording into text once the call is
//! over.
//!
//! # Wiring this module still needs — deliberately not done here
//!
//! This file was written under a rule that `lib.rs` gets exactly one new
//! line (`pub mod meeting;`) and nothing else, so the two commands below are
//! **not** in `generate_handler!` yet and the pop-out window is **not** yet
//! allow-listed for any capability. Both are ordinary, mechanical additions
//! for whoever wires this in:
//!
//! 1. Add `meeting::meeting_open_popout` and
//!    `meeting::meeting_transcribe_system_audio` to the `generate_handler!`
//!    list in `lib.rs`, next to every other command there.
//! 2. Add `"meeting-popout"` to the `windows` allow-list in
//!    `src-tauri/capabilities/default.json`. That file says it plainly: "a
//!    window missing from it silently loses every permission below,
//!    including the ability to receive events at all." Without this the
//!    pop-out opens as a blank, inert window — it can call no command
//!    (including the two below) and will never see a `voice-partial`. The
//!    same appears to already be true of every dynamically-created
//!    `widget-<uuid>` window, which is not in that list either; not this
//!    module's file to fix, but worth checking before assuming the pop-out
//!    is somehow special-cased.
//!
//! # Why system audio is not transcribed live — the honest answer
//!
//! The product complaint this module exists to fix says "live transcript is
//! both you and computer audio." That is achievable in principle but not
//! inside this task's boundaries, and shipping a UI that *claims* it is true
//! when it is not would be worse than shipping the honest, smaller thing. The
//! evidence:
//!
//! * `voice/live_macos.rs` + `macos/CaduceusSTTLive.swift` tap
//!   `AVAudioEngine.inputNode` — the microphone, and nothing else — and feed
//!   it to one `SFSpeechAudioBufferRecognitionRequest` for the life of a
//!   session. There is no second input anywhere in that path.
//! * `capture/recorder.rs` + `macos/CaduceusRecorder.swift` tap
//!   ScreenCaptureKit's system-audio stream, but only to hand sample buffers
//!   to an `AVAssetWriter` — a file, not a recogniser. Its stdout protocol is
//!   `ready` / `level` / `error` / `done`; it never hands audio samples to
//!   anything downstream, so there is nothing here to intercept even if this
//!   module wanted to.
//!
//! Making system audio live would mean a genuinely new Swift helper: open a
//! second `SFSpeechRecognizer` task and feed it from an `SCStream` audio
//! callback the same way `CaduceusSTTLive.swift` feeds one from the mic, then
//! merge the two partial streams by wall-clock time in the UI. That is a
//! real, buildable design — not a dead end — but it means touching
//! `capture/` and `voice/` and standing up new build/signing/bundling
//! plumbing for a second binary, all of which this task was explicitly told
//! to leave alone.
//!
//! So this ships the fallback the task's own brief allowed for: the
//! microphone stays live, unmodified, driven from the frontend exactly as it
//! already was; once the meeting recording stops, the system-audio *track*
//! of the finished file — kept as a genuinely separate track from the mic by
//! `CaduceusRecorder.swift`'s `Writer`, see the comment on
//! [`meeting_transcribe_system_audio`] — is pulled out with `afconvert` (a
//! binary every Mac ships, not a new dependency) and handed to the same
//! batch speech backend the rest of Caduceus already uses for everything
//! that is not live dictation. The frontend must say plainly that this half
//! arrives after the call ends, not during it — see `MeetingPage.tsx`'s
//! own module doc for the exact wording, and do not soften it.

use std::path::{Path, PathBuf};
use std::process::Command;

use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

use crate::settings::SettingsManager;

// ---------------------------------------------------------------------------
// The pop-out window
// ---------------------------------------------------------------------------

/// Label of the pop-out window. Exactly one at a time — unlike widgets,
/// there is only ever one meeting to be looking at, so this is a fixed
/// label rather than `widgets.rs`'s `widget-<uuid>` scheme.
pub const MEETING_POPOUT_WINDOW: &str = "meeting-popout";

/// The Vite entry point for the pop-out. See `vite.config.ts` and
/// `meeting.html` at the repo root — the same one-HTML-file-per-surface
/// convention `widget.html` uses.
const MEETING_ENTRY: &str = "meeting.html";

const POPOUT_WIDTH: f64 = 340.0;
const POPOUT_HEIGHT: f64 = 440.0;
const POPOUT_MIN_WIDTH: f64 = 260.0;
const POPOUT_MIN_HEIGHT: f64 = 220.0;
/// Clear of a call app's own on-screen controls, which macOS video-call apps
/// (Zoom, Meet, FaceTime) put bottom-centre — landing there top-right keeps
/// the pop-out from ever sitting on top of the mute/camera/leave row.
const POPOUT_MARGIN: f64 = 24.0;

/// Open the pop-out, or bring the existing one back on screen. Idempotent —
/// exactly the same reasoning as `widgets.rs::spawn_widget_window`: a second
/// click on "Pop out" is a request to *see* the window, not to make another
/// one.
///
/// # Why `configure_staff_floating`, not `configure_command_center_floating`
///
/// The task that produced this module flagged the choice explicitly: a
/// `configure_staff_floating` panel (`Kind::Staff` in `window/panel.rs`) can
/// never become the key window, so it can never host a text field —
/// `canBecomeKeyWindow` is hard-wired to `NO` for that whole class. The
/// alternative, `configure_command_center_floating` (`Kind::Command`), is
/// keyable and *could* take a notes field.
///
/// The decision here is to use `configure_staff_floating` and keep notes out
/// of the pop-out entirely — they stay in the Command Center tab, which
/// already has the full editor, "Copy all", and "Save to Notes". Two reasons,
/// not one:
///
/// 1. The pop-out's whole job is to sit on top of a call and be readable
///    without becoming the thing you are interacting with — that is what
///    "just like macparakeet" means in practice. A panel that *can* become
///    key is a panel that is one stray click away from stealing the keyboard
///    focus Zoom's own shortcuts (mute, camera, raise hand, ...) depend on.
///    `Kind::Staff`'s inability to ever take focus is a feature here, not a
///    limitation to work around.
/// 2. Every control the pop-out needs — Start/Stop, Pause, "Open notes" — is
///    a click, and clicks work fine on a panel that is never key (the staff's
///    own pop-out icons prove this daily). Only *typing* needs key-window
///    status, and nothing in the pop-out types.
#[tauri::command]
pub fn meeting_open_popout<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    if let Some(existing) = app.get_webview_window(MEETING_POPOUT_WINDOW) {
        existing.show().map_err(|e| e.to_string())?;
        return Ok(());
    }

    let (x, y) = default_popout_origin(&app);

    let window = WebviewWindowBuilder::new(
        &app,
        MEETING_POPOUT_WINDOW,
        WebviewUrl::App(MEETING_ENTRY.into()),
    )
    .title("Meeting notes")
    .inner_size(POPOUT_WIDTH, POPOUT_HEIGHT)
    .position(x, y)
    .min_inner_size(POPOUT_MIN_WIDTH, POPOUT_MIN_HEIGHT)
    .resizable(true)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    .visible(false)
    .accept_first_mouse(true)
    .build()
    .map_err(|e| e.to_string())?;

    // Configure *then* show, not the other way round — same reasoning as
    // `widgets.rs::spawn_widget_window` and `open_command_center` in
    // `window/mod.rs`: a window ordered in while its collection behaviour
    // still says "one Space" is placed in the Space it was created in, and
    // setting `canJoinAllSpaces` afterwards does not reliably drag it across.
    // That matters more here than almost anywhere else in Caduceus — the
    // entire point of this window is to still be visible once the user is
    // inside Zoom's full-screen Space.
    crate::window::configure_staff_floating(&window);
    crate::window::apply_vibrancy(&window);
    window.show().map_err(|e| e.to_string())?;
    Ok(())
}

/// Top-right of whichever monitor holds the pointer. See [`POPOUT_MARGIN`]
/// for why top-right rather than the more obvious bottom-right.
fn default_popout_origin<R: Runtime>(app: &AppHandle<R>) -> (f64, f64) {
    if let Ok(Some(monitor)) = app.primary_monitor() {
        let scale = monitor.scale_factor();
        let pos = monitor.position().to_logical::<f64>(scale);
        let size = monitor.size().to_logical::<f64>(scale);
        return (
            pos.x + size.width - POPOUT_WIDTH - POPOUT_MARGIN,
            pos.y + POPOUT_MARGIN,
        );
    }
    // Headless CI or no monitor info: land somewhere on screen rather than
    // erroring out — the same fallback `widgets.rs::default_origin` uses.
    (POPOUT_MARGIN, POPOUT_MARGIN)
}

// ---------------------------------------------------------------------------
// System-audio transcription, after the call
// ---------------------------------------------------------------------------

/// Pull the system-audio track out of a finished meeting recording and
/// transcribe it with whatever STT backend the user has configured in
/// Settings → Voice — the same one live dictation and push-to-talk use, see
/// [`crate::voice::stt`].
///
/// `path` is whatever `recording_stop` returned: an `.m4a` written by
/// `CaduceusRecorder.swift`'s `Writer`. Its `init` adds inputs in a fixed
/// order — the video track (screen-recording mode only), then system audio
/// unconditionally, then the microphone *only if* `--mic` was passed — so in
/// any file this module is ever handed (meeting notes always records with
/// `RecordMode::Audio`, never `RecordMode::Screen`, so there is no video
/// track ahead of it) track **0 is always system audio**, whether or not a
/// second, microphone track follows it. That ordering is read directly out of
/// the Swift source, not guessed — see the comment there — but it has not
/// been verified against a real recorded file on real hardware, because
/// doing so needs an actual call and a signed build. Confirm with `afinfo`
/// on a real `.m4a` before treating this as load-bearing.
#[tauri::command]
pub async fn meeting_transcribe_system_audio(
    settings: tauri::State<'_, SettingsManager>,
    path: String,
) -> Result<String, String> {
    let settings = (*settings).clone();
    let source = PathBuf::from(&path);
    if !source.is_file() {
        return Err("The recording is gone — nothing to transcribe.".into());
    }

    // `afconvert` is a genuine, if slow-ish, subprocess spawn; kept off the
    // async executor's own threads the same way every other helper spawn in
    // this codebase is (see `recording_stop`, `voice_stop`).
    let wav = tauri::async_runtime::spawn_blocking(move || extract_system_audio_wav(&source))
        .await
        .map_err(|e| format!("could not extract the call audio: {e}"))??;

    let routed = crate::voice::transcribe_and_route(wav, &settings).await?;
    Ok(routed.text)
}

/// Run `afconvert --read-track 0` to pull system audio out of `source` as
/// 16 kHz mono 16-bit WAV — the exact format
/// [`crate::voice::stt::SttBackend::transcribe`] documents itself as wanting,
/// so the result goes through unchanged, same as the mic path's WAV already
/// does.
///
/// `afconvert` (`/usr/bin/afconvert`) ships with every Mac. Using it instead
/// of, say, `ffmpeg` is what keeps this a zero-new-dependency change — no
/// `cargo add`, no new Swift helper to build and sign, nothing to bundle.
fn extract_system_audio_wav(source: &Path) -> Result<Vec<u8>, String> {
    let out_path = std::env::temp_dir().join(format!(
        "caduceus-meeting-system-audio-{}.wav",
        uuid::Uuid::new_v4()
    ));

    let output = Command::new("afconvert")
        .arg("--read-track")
        .arg("0")
        .arg("-d")
        .arg("LEI16@16000")
        .arg("-c")
        .arg("1")
        .arg("-f")
        .arg("WAVE")
        .arg(source)
        .arg(&out_path)
        .output()
        .map_err(|e| format!("could not run afconvert: {e}"))?;

    if !output.status.success() {
        let _ = std::fs::remove_file(&out_path);
        return Err(format!(
            "afconvert could not read the call audio: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let bytes = std::fs::read(&out_path).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&out_path);
    bytes
}
