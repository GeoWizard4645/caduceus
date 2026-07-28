//! Small self-contained utilities exposed as Command Center actions.
//!
//! Everything here uses tools macOS already ships — `sips`, `mdfind`,
//! `diskutil`, `caffeinate`, AppleScript. Nothing needs an account, an API key
//! or a subscription, which is the bar for anything built in: a feature that
//! cannot work for free does not belong in the app, it belongs on the website's
//! list of things Caduceus deliberately does not do.

pub mod apple;
pub mod awake;
pub mod birthdays;
pub mod citation;
pub mod cleaner;
pub mod cron;
pub mod csv_clean;
pub mod dev;
pub mod devenv;
pub mod files;
pub mod habits;
pub mod media;
pub mod native;
pub mod markets;
pub mod markets_widget;
pub mod net;
pub mod qr;
pub mod redactor;
pub mod semantic;
pub mod knowledge;
pub mod routing;
pub mod browsertabs;
pub mod browser_cmds;
pub mod security;
pub mod security_cmds;
pub mod expander;
pub mod images;
pub mod vision;
pub mod devextra;
pub mod calendar;
pub mod documents;
pub mod subscriptions;
pub mod textai;
pub mod sports;
pub mod rates;
pub mod regex_tool;
pub mod shapes;
pub mod sorter;
pub mod system;
pub mod text;
pub mod timekeeping;
pub mod totp;
pub mod wallpaper;

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde::Serialize;

/// How long a command-line tool gets before it is assumed wedged.
///
/// Everything in here shells out to something that can stop answering forever:
/// Finder with a modal dialog open, a Docker daemon that has hung, a resolver
/// that never replies. An unbounded subprocess freezes whichever thread is
/// waiting on it, so every one of them is given a deadline.
pub const TOOL_TIMEOUT: Duration = Duration::from_secs(10);

/// Run a command, killing it if it outstays `timeout`.
///
/// `wedged` is what the user is told when the deadline passes, because "it
/// timed out" does not say which of their apps is holding things up.
pub fn output_with_timeout(
    command: &mut Command,
    timeout: Duration,
    wedged: &str,
) -> Result<std::process::Output, String> {
    use std::process::Stdio;

    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start it: {e}"))?;

    // Both pipes are drained on their own threads *while* the deadline is
    // polled, and this is not optional.
    //
    // This loop used to call `try_wait()` and nothing else. A pipe holds about
    // 64 KB on macOS; once a child fills stdout, it blocks on the next write
    // and never exits, so `try_wait` never reports it finished and the loop
    // spun until the deadline — then blamed the *user's app* for hanging, with
    // a "did not answer" message, for a command that was working perfectly and
    // simply had a lot to say.
    //
    // Nothing about that is visible from reading the call sites, which is why
    // it survived: it only bites on large output. `docker logs`, `git diff` on
    // a real change, `lsof` on a busy machine and a long PDF's text all clear
    // 64 KB easily, and every one of them goes through here.
    //
    // Threads rather than `wait_with_output()` after the loop, because that
    // reads to EOF with no deadline at all — the very hang this function
    // exists to prevent. This is what `wait_with_output` does internally,
    // with a clock attached.
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let drain = |pipe: Option<std::process::ChildStdout>| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buffer = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut buffer);
            }
            buffer
        })
    };
    let stdout_reader = drain(stdout_pipe.take());
    let stderr_reader = {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buffer = Vec::new();
            if let Some(pipe) = stderr_pipe.as_mut() {
                let _ = pipe.read_to_end(&mut buffer);
            }
            buffer
        })
    };

    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => return Err(format!("Could not wait for it: {e}")),
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(wedged.to_string());
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    // Killing the child closes the pipes, so these joins cannot outlive the
    // deadline above even when the command was killed mid-sentence.
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();

    Ok(std::process::Output { status, stdout, stderr })
}

/// The result of a one-shot utility, shaped for a palette toast.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutcome {
    pub ok: bool,
    pub message: String,
    /// Text the caller should put on the clipboard, if any.
    pub copied: Option<String>,
}

impl ToolOutcome {
    pub fn ok(message: impl Into<String>) -> Self {
        Self { ok: true, message: message.into(), copied: None }
    }
    pub fn err(message: impl Into<String>) -> Self {
        Self { ok: false, message: message.into(), copied: None }
    }
    pub fn copied(text: String, message: impl Into<String>) -> Self {
        Self { ok: true, message: message.into(), copied: Some(text) }
    }
}

fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("could not run {program}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

// ---------------------------------------------------------------------------
// Downloads
// ---------------------------------------------------------------------------

/// The most recently modified file in ~/Downloads.
///
/// Skips the part-files browsers leave behind mid-download — offering someone a
/// `.crdownload` as "your latest download" is never what they meant.
pub fn latest_download() -> Result<PathBuf, String> {
    let dir = dirs_downloads().ok_or("Could not find your Downloads folder.")?;
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;

    for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("crdownload" | "download" | "part" | "partial" | "tmp")
        ) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else { continue };
        if best.as_ref().is_none_or(|(t, _)| modified > *t) {
            best = Some((modified, path));
        }
    }

    best.map(|(_, p)| p)
        .ok_or_else(|| "Your Downloads folder is empty.".to_string())
}

fn dirs_downloads() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Downloads"))
}

pub fn copy_latest_download() -> ToolOutcome {
    match latest_download() {
        Ok(path) => {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            ToolOutcome::copied(path.to_string_lossy().to_string(), format!("Copied path to {name}"))
        }
        Err(e) => ToolOutcome::err(e),
    }
}

pub fn open_latest_download() -> ToolOutcome {
    match latest_download() {
        Ok(path) => match run("open", &[&path.to_string_lossy()]) {
            Ok(_) => ToolOutcome::ok(format!(
                "Opened {}",
                path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
            )),
            Err(e) => ToolOutcome::err(format!("Could not open it: {e}")),
        },
        Err(e) => ToolOutcome::err(e),
    }
}

// ---------------------------------------------------------------------------
// Finder
// ---------------------------------------------------------------------------

/// POSIX paths of the current Finder selection, newline separated.
pub fn copy_finder_path() -> ToolOutcome {
    let script = r#"tell application "Finder"
        set sel to selection
        if (count of sel) is 0 then return ""
        set out to ""
        repeat with i in sel
            set out to out & POSIX path of (i as alias) & linefeed
        end repeat
        return out
    end tell"#;

    match run("osascript", &["-e", script]) {
        Ok(paths) if !paths.trim().is_empty() => {
            let n = paths.lines().filter(|l| !l.trim().is_empty()).count();
            ToolOutcome::copied(
                paths.trim().to_string(),
                if n == 1 { "Copied path".into() } else { format!("Copied {n} paths") },
            )
        }
        Ok(_) => ToolOutcome::err("Nothing is selected in Finder."),
        Err(e) if e.contains("-1743") => ToolOutcome::err(
            "Caduceus is not allowed to control Finder yet. Grant it in System Settings → \
             Privacy & Security → Automation.",
        ),
        Err(e) => ToolOutcome::err(format!("Finder said: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Disks
// ---------------------------------------------------------------------------

/// Eject every ejectable volume under /Volumes.
pub fn eject_all_disks() -> ToolOutcome {
    let Ok(entries) = std::fs::read_dir("/Volumes") else {
        return ToolOutcome::err("Could not read /Volumes.");
    };

    let mut ejected = Vec::new();
    let mut failed = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        // The boot volume lives here too and is not ejectable; asking anyway
        // produces a scary error for something the user never meant.
        if path.symlink_metadata().map(|m| m.file_type().is_symlink()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        match run("diskutil", &["eject", &path.to_string_lossy()]) {
            Ok(_) => ejected.push(name),
            Err(_) => failed.push(name),
        }
    }

    if ejected.is_empty() && failed.is_empty() {
        return ToolOutcome::ok("No removable volumes are mounted.");
    }
    if ejected.is_empty() {
        return ToolOutcome::err(format!("Could not eject: {}", failed.join(", ")));
    }
    ToolOutcome::ok(format!("Ejected {}", ejected.join(", ")))
}

// ---------------------------------------------------------------------------
// Stay awake
// ---------------------------------------------------------------------------

/// Whether `caffeinate` is currently held open by Caduceus.
pub fn awake_state() -> bool {
    run("pgrep", &["-f", "caffeinate -dimsu -w"]).map(|s| !s.is_empty()).unwrap_or(false)
}

/// Toggle "keep this Mac awake".
///
/// Uses `caffeinate`, which ships with macOS, rather than requiring Amphetamine.
/// `-w` ties the assertion to Caduceus's own pid, so quitting the app releases
/// it — a stray `caffeinate` outliving the app that started it would keep a
/// laptop awake in a bag.
pub fn set_awake(on: bool, owner_pid: u32) -> ToolOutcome {
    if !on {
        let _ = run("pkill", &["-f", "caffeinate -dimsu -w"]);
        return ToolOutcome::ok("Sleep re-enabled.");
    }
    if awake_state() {
        return ToolOutcome::ok("Already staying awake.");
    }
    match Command::new("caffeinate")
        .args(["-dimsu", "-w", &owner_pid.to_string()])
        .spawn()
    {
        Ok(_) => ToolOutcome::ok("Staying awake until you turn this off."),
        Err(e) => ToolOutcome::err(format!("Could not start caffeinate: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHit {
    pub path: String,
    pub name: String,
}

/// Spotlight search, via `mdfind`.
pub fn search_files(query: &str, limit: usize) -> Vec<FileHit> {
    if query.trim().is_empty() {
        return Vec::new();
    }
    let Ok(out) = run("mdfind", &["-name", query.trim()]) else {
        return Vec::new();
    };
    out.lines()
        .filter(|l| !l.trim().is_empty())
        .take(limit)
        .map(|p| FileHit {
            name: PathBuf::from(p)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| p.to_string()),
            path: p.to_string(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Dictionary
// ---------------------------------------------------------------------------

/// Look a word up in the built-in Dictionary.
pub fn define_word(word: &str) -> ToolOutcome {
    let word = word.trim();
    if word.is_empty() {
        return ToolOutcome::err("Type a word to look up.");
    }
    // `dict://` is the scheme Dictionary.app registers; no third-party service
    // and no network involved.
    match run("open", &[&format!("dict://{}", urlencode(word))]) {
        Ok(_) => ToolOutcome::ok(format!("Looking up “{word}”")),
        Err(e) => ToolOutcome::err(format!("Could not open Dictionary: {e}")),
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "%20".to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Images
// ---------------------------------------------------------------------------

/// Resize and/or convert an image with `sips`, writing a new file beside it.
///
/// `sips` is part of macOS, so this needs nothing installed. The original is
/// never modified — a destructive default on someone's only copy of a photo is
/// not a trade worth making for one less file.
pub fn convert_image(path: &str, width: Option<u32>, format: Option<&str>) -> ToolOutcome {
    let source = PathBuf::from(path);
    if !source.is_file() {
        return ToolOutcome::err("That file does not exist.");
    }

    let ext = format.unwrap_or_else(|| {
        source.extension().and_then(|e| e.to_str()).unwrap_or("png")
    });
    let stem = source.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let suffix = match width {
        Some(w) => format!("-{w}w"),
        None => "-converted".into(),
    };
    let dest = source.with_file_name(format!("{stem}{suffix}.{ext}"));

    let mut args: Vec<String> = Vec::new();
    if let Some(w) = width {
        // `--resampleWidth` keeps the aspect ratio, which is what "make it
        // 800 wide" means to everyone who is not a graphics programmer.
        args.push("--resampleWidth".into());
        args.push(w.to_string());
    }
    if let Some(f) = format {
        args.push("-s".into());
        args.push("format".into());
        args.push(f.into());
    }
    args.push(source.to_string_lossy().to_string());
    args.push("--out".into());
    args.push(dest.to_string_lossy().to_string());

    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    match run("sips", &refs) {
        Ok(_) => ToolOutcome::copied(
            dest.to_string_lossy().to_string(),
            format!("Wrote {}", dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
        ),
        Err(e) => ToolOutcome::err(format!("sips said: {e}")),
    }
}

#[cfg(test)]
mod output_timeout_tests {
    use super::*;
    use std::process::Command;

    /// The regression this function was silently failing at.
    ///
    /// A pipe holds ~64 KB. Before the readers were drained concurrently, a
    /// command that wrote more than that blocked forever and this returned the
    /// "wedged" message — for a command that had already done its job.
    #[test]
    fn output_larger_than_a_pipe_buffer_is_not_mistaken_for_a_hang() {
        let mut command = Command::new("sh");
        // ~2 MB, comfortably past any pipe buffer.
        command.arg("-c").arg("yes abcdefghijklmnopqrstuvwxyz | head -n 80000");

        let output = output_with_timeout(&mut command, Duration::from_secs(20), "wedged")
            .expect("a chatty command is not a hung one");

        assert!(output.status.success());
        assert!(
            output.stdout.len() > 128 * 1024,
            "expected well over a pipe buffer, got {} bytes",
            output.stdout.len()
        );
    }

    /// stderr must be drained too, or a command that is loud on the error
    /// stream deadlocks exactly the same way.
    #[test]
    fn large_stderr_also_survives() {
        let mut command = Command::new("sh");
        command.arg("-c").arg("yes error-line | head -n 40000 1>&2");

        let output = output_with_timeout(&mut command, Duration::from_secs(20), "wedged")
            .expect("a command that is loud on stderr is not a hung one");
        assert!(output.stderr.len() > 128 * 1024);
    }

    /// And a genuinely hung command must still be caught.
    #[test]
    fn a_command_that_never_returns_still_times_out() {
        let mut command = Command::new("sleep");
        command.arg("30");

        let err = output_with_timeout(&mut command, Duration::from_millis(150), "wedged")
            .unwrap_err();
        assert_eq!(err, "wedged");
    }

    #[test]
    fn the_exit_status_survives_the_rewrite() {
        let mut command = Command::new("sh");
        command.arg("-c").arg("exit 3");
        let output = output_with_timeout(&mut command, Duration::from_secs(5), "wedged").unwrap();
        assert_eq!(output.status.code(), Some(3));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencoding_escapes_what_a_url_cannot_carry() {
        assert_eq!(urlencode("hello"), "hello");
        assert_eq!(urlencode("two words"), "two%20words");
        assert_eq!(urlencode("caf\u{e9}"), "%C3%A9".to_string().replace("%C3%A9", "caf%C3%A9"));
        assert!(urlencode("a/b?c#d").contains("%2F"));
    }

    #[test]
    fn a_missing_image_is_reported_not_attempted() {
        let out = convert_image("/nope/definitely/not/here.png", Some(100), None);
        assert!(!out.ok);
    }

    #[test]
    fn an_empty_word_is_refused_before_launching_anything() {
        assert!(!define_word("   ").ok);
    }

    #[test]
    fn an_empty_file_query_short_circuits() {
        assert!(search_files("  ", 10).is_empty());
    }
}
