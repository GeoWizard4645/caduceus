//! Executing a [`Shortcut`].
//!
//! Everything here runs in the Rust process, never in the webview. The frontend
//! can only say "run shortcut with id X" — it can never hand Orbit a command
//! string to execute. That is why `capabilities/default.json` does not enable
//! the shell plugin.

use std::path::PathBuf;
use std::process::Stdio;

use serde::Serialize;
use tokio::process::Command;

use super::{percent_encode, substitute_query, Shortcut, ShortcutKind};

/// What happened when a shortcut ran. Returned to the frontend so it can show a
/// toast rather than failing silently.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecOutcome {
    pub ok: bool,
    /// Human-readable summary, e.g. "Opened https://claude.ai".
    pub message: String,
    /// Set when the frontend has follow-up work to do — currently only
    /// `"clipboard_view"`, which asks the Command Center to open in clipboard
    /// mode. Keeps UI-only shortcut kinds out of the Rust side.
    pub frontend_action: Option<String>,
    /// Captured stdout/stderr, only populated by `run_command_capture`.
    pub output: Option<String>,
}

impl ExecOutcome {
    fn ok(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: message.into(),
            frontend_action: None,
            output: None,
        }
    }

    fn err(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: message.into(),
            frontend_action: None,
            output: None,
        }
    }
}

/// Run a shortcut.
///
/// `query` is the trailing text when the shortcut was invoked from the Command
/// Center (e.g. typing `gmail unread` after selecting the Gmail shortcut). It is
/// substituted for `{query}` in the target.
///
/// `prefer_chrome` and `default_profile` come from Command Center settings and
/// only affect `OpenUrl`.
pub async fn execute_shortcut(
    shortcut: &Shortcut,
    query: &str,
    prefer_chrome: bool,
    default_profile: Option<&str>,
) -> ExecOutcome {
    match shortcut.kind {
        ShortcutKind::ClipboardView => ExecOutcome {
            ok: true,
            message: "Opening clipboard history".into(),
            frontend_action: Some("clipboard_view".into()),
            output: None,
        },

        ShortcutKind::OpenUrl => {
            if shortcut.target.trim().is_empty() {
                return ExecOutcome::err(format!(
                    "\u{201c}{}\u{201d} has no URL set. Add one in Settings \u{2192} Shortcuts.",
                    shortcut.label
                ));
            }
            // URL targets get the query percent-encoded so it survives being
            // dropped into a query string.
            let url = substitute_query(&shortcut.target, &percent_encode(query));
            let profile = shortcut
                .chrome_profile_directory
                .as_deref()
                .or(default_profile);
            open_url(&url, profile, prefer_chrome).await
        }

        ShortcutKind::OpenApp => {
            if shortcut.target.trim().is_empty() {
                return ExecOutcome::err(format!(
                    "\u{201c}{}\u{201d} has no application set. Pick one in Settings \u{2192} Shortcuts.",
                    shortcut.label
                ));
            }
            open_app(&shortcut.target, &shortcut.args).await
        }

        ShortcutKind::RunCommand => {
            if shortcut.target.trim().is_empty() {
                return ExecOutcome::err("This shortcut has no command set.");
            }
            let command = substitute_query(&shortcut.target, &shell_quote(query));
            match spawn_shell(&command, query, false).await {
                Ok(_) => ExecOutcome::ok(format!("Ran \u{201c}{}\u{201d}", shortcut.label)),
                Err(e) => ExecOutcome::err(format!("Command failed to start: {e}")),
            }
        }

        ShortcutKind::RunAppleScript => {
            if !cfg!(target_os = "macos") {
                return ExecOutcome::err("AppleScript shortcuts only run on macOS.");
            }
            let source = substitute_query(&shortcut.target, query);
            match run_applescript(&source).await {
                Ok(out) if out.trim().is_empty() => {
                    ExecOutcome::ok(format!("Ran \u{201c}{}\u{201d}", shortcut.label))
                }
                Ok(out) => ExecOutcome {
                    ok: true,
                    message: out.trim().to_string(),
                    frontend_action: None,
                    output: Some(out),
                },
                Err(e) => ExecOutcome::err(format!("AppleScript failed: {e}")),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// URLs
// ---------------------------------------------------------------------------

/// Open a URL, optionally forcing it into a specific Chrome profile.
///
/// When no profile is requested we hand the URL to the OS, which respects the
/// user's default-browser choice. When a profile *is* requested we must invoke
/// the Chromium binary directly: macOS `open -b … --args` silently drops the
/// arguments if the browser is already running, which would send every link to
/// whichever profile happened to be open.
pub async fn open_url(url: &str, chrome_profile: Option<&str>, prefer_chrome: bool) -> ExecOutcome {
    if !is_safe_url(url) {
        return ExecOutcome::err(format!(
            "Refusing to open \u{201c}{url}\u{201d}: only http and https URLs are allowed."
        ));
    }

    let wants_chrome = prefer_chrome || chrome_profile.is_some();

    if wants_chrome {
        if let Some(bin) = find_chromium_binary() {
            let mut cmd = Command::new(&bin);
            if let Some(profile) = chrome_profile.filter(|p| !p.is_empty()) {
                cmd.arg(format!("--profile-directory={profile}"));
            }
            cmd.arg(url);
            detach(&mut cmd);
            match cmd.spawn() {
                Ok(_) => return ExecOutcome::ok(format!("Opened {url}")),
                Err(e) => {
                    log::warn!("chromium launch failed ({e}); falling back to the default browser");
                }
            }
        } else if chrome_profile.is_some() {
            log::warn!("a Chrome profile was requested but no Chromium binary was found");
        }
    }

    match tauri_plugin_opener::open_url(url, None::<&str>) {
        Ok(()) => ExecOutcome::ok(format!("Opened {url}")),
        Err(e) => ExecOutcome::err(format!("Could not open {url}: {e}")),
    }
}

/// Only `http(s)` is ever opened.
///
/// This matters because URLs can reach Orbit from a model response or a
/// clipboard entry, and schemes like `file:`, `javascript:` or a custom
/// app-handler scheme are a real escalation path.
fn is_safe_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// Locate a Chromium-family executable to pass `--profile-directory` to.
fn find_chromium_binary() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        const APPS: &[&str] = &[
            "Google Chrome",
            "Google Chrome Beta",
            "Chromium",
            "Brave Browser",
            "Microsoft Edge",
            "Vivaldi",
        ];
        let roots = [
            PathBuf::from("/Applications"),
            dirs::home_dir()?.join("Applications"),
        ];
        for root in roots {
            for app in APPS {
                let p = root.join(format!("{app}.app/Contents/MacOS/{app}"));
                if p.is_file() {
                    return Some(p);
                }
            }
        }
        None
    }

    #[cfg(target_os = "windows")]
    {
        const RELATIVE: &[&str] = &[
            r"Google\Chrome\Application\chrome.exe",
            r"BraveSoftware\Brave-Browser\Application\brave.exe",
            r"Microsoft\Edge\Application\msedge.exe",
        ];
        let roots = [
            std::env::var_os("PROGRAMFILES").map(PathBuf::from),
            std::env::var_os("PROGRAMFILES(X86)").map(PathBuf::from),
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        ];
        for root in roots.into_iter().flatten() {
            for rel in RELATIVE {
                let p = root.join(rel);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
        None
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        const BINS: &[&str] = &[
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "brave-browser",
            "microsoft-edge",
        ];
        let path = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path) {
            for bin in BINS {
                let p = dir.join(bin);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Applications
// ---------------------------------------------------------------------------

/// Launch an application by bundle id, path, or executable name.
pub async fn open_app(target: &str, args: &[String]) -> ExecOutcome {
    let target = target.trim();

    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        // A bundle id looks like `com.foo.Bar`; a path contains a separator or
        // ends in `.app`.
        let looks_like_bundle_id =
            target.contains('.') && !target.contains('/') && !target.ends_with(".app");
        if looks_like_bundle_id {
            c.arg("-b").arg(target);
        } else {
            c.arg("-a").arg(target);
        }
        if !args.is_empty() {
            c.arg("--args").args(args);
        }
        c
    };

    #[cfg(target_os = "windows")]
    let mut cmd = {
        // `start` needs an empty title argument first, or it treats a quoted
        // path as the window title.
        let mut c = Command::new("cmd");
        c.arg("/C").arg("start").arg("").arg(target).args(args);
        c
    };

    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = Command::new(target);
        c.args(args);
        c
    };

    detach(&mut cmd);
    match cmd.spawn() {
        Ok(_) => ExecOutcome::ok(format!("Launched {target}")),
        Err(e) => ExecOutcome::err(format!("Could not launch {target}: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Shell + AppleScript
// ---------------------------------------------------------------------------

/// Spawn a shell command.
///
/// The raw (unquoted) query is also exported as `$ORBIT_QUERY`, which is the
/// recommended way to consume it — `{query}` substitution is shell-quoted, so
/// it is safe but awkward to use inside an existing quoted string.
async fn spawn_shell(command: &str, raw_query: &str, capture: bool) -> std::io::Result<String> {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(command);
        c
    };

    #[cfg(not(windows))]
    let mut cmd = {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let mut c = Command::new(shell);
        c.arg("-c").arg(command);
        c
    };

    cmd.env("ORBIT_QUERY", raw_query);

    if capture {
        let out = cmd.output().await?;
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        let err = String::from_utf8_lossy(&out.stderr);
        if !err.trim().is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&err);
        }
        Ok(text)
    } else {
        detach(&mut cmd);
        cmd.spawn()?;
        Ok(String::new())
    }
}

/// Run a shell command and wait for its output, with a hard timeout.
///
/// Used by the Settings "Test" button and by `RunCommand` prefix rules, where
/// seeing the result is the whole point. Plain shortcuts are fire-and-forget.
pub async fn run_command_capture(command: &str, query: &str, timeout_secs: u64) -> ExecOutcome {
    let quoted = substitute_query(command, &shell_quote(query));
    let fut = spawn_shell(&quoted, query, true);
    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fut).await {
        Ok(Ok(output)) => ExecOutcome {
            ok: true,
            message: if output.trim().is_empty() {
                "Command finished with no output.".into()
            } else {
                output.trim().to_string()
            },
            frontend_action: None,
            output: Some(output),
        },
        Ok(Err(e)) => ExecOutcome::err(format!("Command failed: {e}")),
        Err(_) => ExecOutcome::err(format!("Command timed out after {timeout_secs}s.")),
    }
}

/// Run AppleScript by piping the source to `osascript` on stdin.
///
/// Passing the script on stdin instead of via `-e` avoids every layer of
/// quote-escaping, which matters because scripts routinely contain both kinds
/// of quote.
pub async fn run_applescript(source: &str) -> std::io::Result<String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = source;
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "AppleScript is only available on macOS",
        ));
    }

    #[cfg(target_os = "macos")]
    {
        use tokio::io::AsyncWriteExt;

        let mut child = Command::new("osascript")
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(source.as_bytes()).await?;
            stdin.shutdown().await?;
        }

        let out = child.wait_with_output().await?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(std::io::Error::other(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Quote a string so a POSIX shell (or `cmd`) treats it as one literal token.
fn shell_quote(s: &str) -> String {
    #[cfg(windows)]
    {
        // `cmd` has no general quoting mechanism; strip the metacharacters that
        // would let a query break out of its argument.
        let cleaned: String = s
            .chars()
            .filter(|c| !matches!(c, '&' | '|' | '<' | '>' | '^' | '"' | '%' | '\n' | '\r'))
            .collect();
        format!("\"{cleaned}\"")
    }
    #[cfg(not(windows))]
    {
        // Single quotes disable every expansion; the only character needing
        // care is the single quote itself.
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Detach a child so it outlives Orbit and does not inherit our stdio.
fn detach(cmd: &mut Command) {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(false);

    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW — otherwise every shortcut flashes a console.
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_http_schemes() {
        assert!(is_safe_url("https://example.com"));
        assert!(is_safe_url("HTTP://example.com"));
        assert!(!is_safe_url("file:///etc/passwd"));
        assert!(!is_safe_url("javascript:alert(1)"));
        assert!(!is_safe_url("  data:text/html,<h1>x"));
    }

    #[cfg(not(windows))]
    #[test]
    fn shell_quoting_neutralises_injection() {
        assert_eq!(shell_quote("hello"), "'hello'");
        assert_eq!(shell_quote("a'; rm -rf /"), r"'a'\''; rm -rf /'");
        assert_eq!(shell_quote("$(whoami)"), "'$(whoami)'");
    }
}
