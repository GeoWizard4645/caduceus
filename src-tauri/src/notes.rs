//! Send text to Apple Notes.
//!
//! Driven with AppleScript rather than a private framework: Notes exposes a
//! documented scripting dictionary, it is the only supported way in, and it
//! keeps Caduceus out of the business of parsing anyone's note database.
//!
//! The first call shows the macOS automation prompt ("Caduceus wants to control
//! Notes"). That is expected and cannot be pre-empted — the entitlement in
//! `entitlements.plist` makes the grant *possible*, the user still has to give
//! it.

use std::process::Command;

/// Notes' own name for the default folder. Creating the note there rather than
/// in a Caduceus-specific folder keeps it where people already look.
const DEFAULT_FOLDER: &str = "Notes";

/// Append `body` to Apple Notes as a new note titled `title`.
///
/// Returns the title actually used, so the UI can say what it made.
pub fn add(title: &str, body: &str) -> Result<String, String> {
    let title = normalise_title(title, body);

    // Notes renders note bodies as HTML, so a plain-text body arrives as one
    // run-on paragraph with the newlines eaten. Escape first, then convert.
    let html = format!(
        "<div><b>{}</b></div>{}",
        escape_html(&title),
        escape_html(body).replace('\n', "<br>")
    );

    let script = format!(
        r#"tell application "Notes"
             tell account 1
               set targetFolder to missing value
               repeat with f in folders
                 if name of f is "{folder}" then set targetFolder to f
               end repeat
               if targetFolder is missing value then set targetFolder to folder 1
               make new note at targetFolder with properties {{name:"{name}", body:"{body}"}}
             end tell
           end tell"#,
        folder = escape_applescript(DEFAULT_FOLDER),
        name = escape_applescript(&title),
        body = escape_applescript(&html),
    );

    let out = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .map_err(|e| format!("Could not run osascript: {e}"))?;

    if out.status.success() {
        return Ok(title);
    }

    let stderr = String::from_utf8_lossy(&out.stderr);
    let stderr = stderr.trim();

    // -1743 is "not authorised to send Apple events". Worth translating: the
    // raw message names an error number and nothing a user can act on.
    if stderr.contains("-1743") || stderr.contains("Not authorized") {
        return Err("Caduceus is not allowed to control Notes yet. \
Grant it in System Settings → Privacy & Security → Automation → Caduceus → Notes."
            .into());
    }
    if stderr.contains("-600") || stderr.contains("isn't running") {
        return Err("Notes is not running and could not be started.".into());
    }
    Err(format!("Notes refused the note: {stderr}"))
}

/// Take the title from the text itself when the caller has nothing better.
fn normalise_title(title: &str, body: &str) -> String {
    let title = title.trim();
    if !title.is_empty() {
        return truncate(title, 60);
    }
    let first = body.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if first.is_empty() {
        "Caduceus note".into()
    } else {
        truncate(first, 60)
    }
}

/// AppleScript string literals escape backslash and double quote only.
fn escape_applescript(s: &str) -> String {
    s.replace('\\', r"\\").replace('"', "\\\"")
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A note whose text contains a quote would otherwise terminate the
    /// AppleScript string early and run whatever followed as code.
    #[test]
    fn quotes_and_backslashes_cannot_break_out_of_the_script() {
        let evil = r#"hi" & (do shell script "rm -rf /") & ""#;
        let escaped = escape_applescript(evil);
        assert!(!escaped.contains("\"\""));
        assert_eq!(escaped.matches("\\\"").count(), evil.matches('"').count());

        assert_eq!(escape_applescript(r"back\slash"), r"back\\slash");
    }

    #[test]
    fn html_special_characters_survive_as_text() {
        assert_eq!(escape_html("a < b & c > d"), "a &lt; b &amp; c &gt; d");
    }

    #[test]
    fn a_missing_title_comes_from_the_first_real_line() {
        assert_eq!(normalise_title("", "\n\n  the answer\nmore"), "the answer");
        assert_eq!(normalise_title("  ", "   "), "Caduceus note");
        assert_eq!(normalise_title("given", "ignored"), "given");
    }

    #[test]
    fn titles_truncate_on_character_boundaries() {
        let title = normalise_title("", &"あ".repeat(200));
        assert_eq!(title.chars().count(), 60);
    }
}
