//! Driving other applications: AppleScript and the Shortcuts app.
//!
//! Both are how macOS lets one program ask another to do something, and both
//! fail in the same two ways — the app is not running, or Automation permission
//! has not been granted for it. Neither of those failures explains itself, so
//! this module's real job is translating them.
//!
//! Nothing here launches an application. `tell application "Spotify" to
//! playpause` will *start Spotify* if it is closed, which is never what
//! "next track" meant, so every caller guards on `is running` first.

use std::process::Command;

/// How long to give a script before assuming the other app has wedged.
///
/// AppleScript blocks until the target answers, and an app showing a modal
/// dialog never answers. Without a bound, one confused copy of Word freezes the
/// palette.
const TIMEOUT: std::time::Duration = super::TOOL_TIMEOUT;

/// What the user is told when [`TIMEOUT`] expires, wherever it expires.
const WEDGED: &str = "The other app did not answer. It may be showing a dialog that needs attention.";

/// Run a script and return its output.
///
/// Source is piped on stdin rather than passed with `-e`, so multiline scripts
/// and embedded quotes do not go through another layer of shell escaping.
pub fn run_script(script: &str) -> Result<String, String> {
    let mut command = Command::new("osascript");
    command.arg("-");
    let output = spawn_with_stdin(&mut command, script)?;

    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_string());
    }

    Err(translate(&String::from_utf8_lossy(&output.stderr)))
}

/// Run a shortcut by name, optionally piping text in as its input.
pub fn run_shortcut(name: &str, input: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Which shortcut? Give it the name exactly as the Shortcuts app shows it.".into());
    }

    let mut command = Command::new("shortcuts");
    command.arg("run").arg(name);

    // `--input-path -` reads stdin, which avoids putting the text on a command
    // line where it would be visible in the process list.
    let with_input = !input.trim().is_empty();
    if with_input {
        command.arg("--input-path").arg("-");
    }

    let output = if with_input {
        spawn_with_stdin(&mut command, input)?
    } else {
        spawn_with_timeout(&mut command)?
    };

    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_string());
    }

    let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if reason.contains("not find") || reason.contains("No shortcut") {
        format!(
            "There is no shortcut called “{name}”. Names have to match the Shortcuts app \
             exactly — run “List your Apple Shortcuts” to see them."
        )
    } else if reason.is_empty() {
        format!("“{name}” did not finish.")
    } else {
        reason
    })
}

/// Every shortcut, by name.
pub fn list_shortcuts() -> Result<Vec<String>, String> {
    let output = spawn_with_timeout(Command::new("shortcuts").arg("list"))?;

    if !output.status.success() {
        return Err(
            "Could not read your shortcuts. The Shortcuts app ships with macOS 12 and newer."
                .into(),
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

/// Run a command, killing it if it outstays [`TIMEOUT`].
fn spawn_with_timeout(command: &mut Command) -> Result<std::process::Output, String> {
    super::output_with_timeout(command, TIMEOUT, WEDGED)
}

/// The same bound, for the one path that has to keep stdin.
///
/// [`super::output_with_timeout`] nulls stdin, so a shortcut being fed text
/// cannot use it — and a shortcut that pipes input is exactly the kind that
/// then sits waiting on a dialog. Without the deadline it holds a blocking-pool
/// worker, and the `invoke()` behind it, for ever.
fn spawn_with_stdin(command: &mut Command, input: &str) -> Result<std::process::Output, String> {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start it: {e}"))?;

    // Dropped at the end of this block, which closes the pipe: a shortcut
    // reading its input to EOF never gets there otherwise.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.as_bytes());
    }

    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(e) => return Err(format!("Could not wait for it: {e}")),
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(WEDGED.to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    child.wait_with_output().map_err(|e| format!("Could not read the result: {e}"))
}

/// Turn an osascript error into something worth reading.
///
/// The wording of the Automation case is kept in step with
/// `PERMISSION_WALL.automation` in `shared/permissions.ts`, because the webview
/// reads the permission back out of this sentence to decide which walkthrough
/// to open. `scripts/check-permissions.mjs` asserts that round trip.
fn translate(stderr: &str) -> String {
    let text = stderr.trim();

    if text.contains("-1743") || text.contains("Not authorized") {
        return "Caduceus needs Automation permission to control that app. Grant it in System \
                Settings → Privacy & Security → Automation."
            .into();
    }
    if text.contains("-1728") {
        return "That app does not have what the command asked for — usually because no window \
                is open."
            .into();
    }
    if text.contains("-600") || text.contains("not running") {
        return "That app is not running.".into();
    }
    if text.contains("-25211") || text.contains("assistive access") {
        return "Caduceus needs Accessibility permission for this. Grant it in System Settings → \
                Privacy & Security → Accessibility."
            .into();
    }
    if text.is_empty() {
        return "The script failed without saying why.".into();
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_automation_error_is_translated_into_the_canonical_sentence() {
        // -1743 is what osascript returns when Automation has not been granted,
        // and on its own it tells the user nothing at all.
        let message = translate("execution error: Not authorized to send Apple events. (-1743)");
        assert!(message.contains("Automation permission"));
        assert!(message.contains("System Settings"));
    }

    #[test]
    fn the_other_errors_people_hit_are_named() {
        assert!(translate("error -1728").contains("no window"));
        assert!(translate("application isn't running (-600)").contains("not running"));
        assert!(translate("assistive access").contains("Accessibility"));
    }

    #[test]
    fn an_unrecognised_error_is_passed_through_rather_than_swallowed() {
        // Inventing a friendly message for an error we do not understand hides
        // the only information there was.
        assert_eq!(translate("syntax error: Expected end of line"), "syntax error: Expected end of line");
    }

    #[test]
    fn an_empty_error_still_says_something() {
        assert!(!translate("   ").is_empty());
    }

    #[test]
    fn a_shortcut_needs_a_name() {
        let err = run_shortcut("   ", "").unwrap_err();
        assert!(err.contains("Which shortcut"));
    }

    #[test]
    fn multiline_scripts_run_via_stdin() {
        let out = run_script(
            "if false then\nreturn \"nope\"\nelse\nreturn \"ok\"\nend if",
        )
        .unwrap();
        assert_eq!(out, "ok");
    }
}
