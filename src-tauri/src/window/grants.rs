//! Repairing a macOS privacy grant that has gone stale.
//!
//! # The problem this exists for
//!
//! Caduceus is signed ad-hoc. It has no Apple Developer certificate, so its
//! code signature is a hash of the binary and **changes with every build**.
//!
//! macOS records privacy grants against that signature. So the sequence is:
//!
//! 1. You grant Accessibility. It works.
//! 2. Caduceus updates.
//! 3. The switch in System Settings is still on — the *name* is still there.
//! 4. `AXIsProcessTrusted()` returns false, because the entry behind that
//!    switch describes a binary that no longer exists.
//!
//! Both halves are telling the truth, which is why it is such a maddening
//! failure: the app insists it has no permission while pointing at a Settings
//! pane that plainly says it does.
//!
//! # The repair
//!
//! `tccutil reset <service> <bundle id>` deletes the entry. macOS can then be
//! asked again and records one for the build that is actually running. No
//! administrator password: TCC entries for an ordinary app live in the user's
//! own database.
//!
//! The alternative — "switch it off and on again" — works too, and is offered
//! as the manual route on the permission page. This is the button that does it
//! for you.

use std::process::Command;
use std::time::{Duration, Instant};

/// The grants Caduceus can repair, and what `tccutil` calls each of them.
///
/// Deliberately a closed list. `tccutil reset` with no service resets *every*
/// permission for the app, and with the wrong one silently does nothing;
/// neither is something a webview should be able to ask for by string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Grant {
    Accessibility,
    ScreenRecording,
    Microphone,
    Automation,
    SpeechRecognition,
}

impl Grant {
    /// The service name `tccutil` understands, which is not always the name the
    /// Settings pane shows.
    fn service(self) -> &'static str {
        match self {
            Self::Accessibility => "Accessibility",
            // "Screen & System Audio Recording" in Settings; ScreenCapture here.
            Self::ScreenRecording => "ScreenCapture",
            Self::Microphone => "Microphone",
            // Automation is per-target-app, and this resets Caduceus's whole set.
            Self::Automation => "AppleEvents",
            Self::SpeechRecognition => "SpeechRecognition",
        }
    }
}

/// What happened, in a sentence the permission page can show as-is.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairOutcome {
    pub ok: bool,
    pub message: String,
    /// Whether the grant is held *now*, where that can be read back.
    pub granted: bool,
}

const BUNDLE_ID: &str = "com.caduceus.desktop";

/// Run a command, killing it if it outstays `limit`.
fn run_bounded(command: &mut Command, limit: Duration) -> std::io::Result<std::process::Output> {
    use std::process::Stdio;

    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait()? {
            Some(_) => break,
            None => {}
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "tccutil did not answer",
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    child.wait_with_output()
}

/// Delete the stale entry and ask macOS for the grant again.
#[cfg(target_os = "macos")]
pub fn repair(grant: Grant) -> RepairOutcome {
    // Bounded, like every other subprocess Caduceus starts. `tccutil` talks to
    // the TCC daemon, and a daemon that does not answer would otherwise hold
    // whichever thread is servicing this call indefinitely.
    let output = run_bounded(
        Command::new("tccutil").arg("reset").arg(grant.service()).arg(BUNDLE_ID),
        Duration::from_secs(8),
    );

    match output {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            // Not fatal. `tccutil` fails when there is no entry to remove,
            // which is the normal state on a first run — and the prompt below
            // is exactly what that case wants anyway.
            let detail = String::from_utf8_lossy(&out.stderr);
            log::debug!("tccutil reset {} said: {}", grant.service(), detail.trim());
        }
        Err(e) => {
            return RepairOutcome {
                ok: false,
                message: format!(
                    "Could not run tccutil ({e}). Switch Caduceus off and then on again in \
                     System Settings — that does the same thing."
                ),
                granted: currently_granted(grant),
            };
        }
    }

    // Only Accessibility has an API that both reports the grant and asks for
    // it. The rest are prompted for by the framework that needs them, at the
    // moment it needs them, so the honest thing after a reset is to say the
    // slate is clean and let the next attempt trigger the prompt.
    if grant == Grant::Accessibility {
        let trusted = super::accessibility::prompt_for_trust();
        return RepairOutcome {
            ok: true,
            granted: trusted,
            message: if trusted {
                "Accessibility is on. Everything that needed it works now.".into()
            } else {
                "Cleared the stale entry and asked macOS again — approve the prompt, or turn \
                 Caduceus on in the list that just opened."
                    .into()
            },
        };
    }

    RepairOutcome {
        ok: true,
        granted: false,
        message: "Cleared the old entry. Try the thing that needed this permission again — \
                  macOS will ask, and this time it will ask about the build you are running."
            .into(),
    }
}

#[cfg(not(target_os = "macos"))]
pub fn repair(_grant: Grant) -> RepairOutcome {
    RepairOutcome {
        ok: false,
        message: "Privacy grants are a macOS idea.".into(),
        granted: true,
    }
}

/// Read a grant back, where macOS allows that without prompting.
pub fn currently_granted(grant: Grant) -> bool {
    match grant {
        Grant::Accessibility => super::accessibility::is_trusted(),
        Grant::ScreenRecording => crate::tools::system::permissions().screen_recording,
        // No prompt-free read exists for these. Reporting `false` would put a
        // permanent warning on a page for something that is probably fine.
        Grant::Microphone | Grant::Automation | Grant::SpeechRecognition => true,
    }
}

/// Trigger the system consent flow where macOS provides one, without resetting TCC.
#[cfg(target_os = "macos")]
pub fn request(grant: Grant) -> bool {
    match grant {
        Grant::Accessibility => super::accessibility::prompt_for_trust(),
        Grant::ScreenRecording => crate::tools::system::request_screen_recording(),
        Grant::Microphone | Grant::Automation | Grant::SpeechRecognition => false,
    }
}

#[cfg(not(target_os = "macos"))]
pub fn request(_grant: Grant) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_grant_maps_to_a_service_tccutil_knows() {
        // The names differ from the Settings labels in ways that are easy to
        // get wrong, and a wrong one fails silently — `tccutil` exits non-zero
        // and nothing is repaired.
        for (grant, expected) in [
            (Grant::Accessibility, "Accessibility"),
            (Grant::ScreenRecording, "ScreenCapture"),
            (Grant::Microphone, "Microphone"),
            (Grant::Automation, "AppleEvents"),
            (Grant::SpeechRecognition, "SpeechRecognition"),
        ] {
            assert_eq!(grant.service(), expected);
        }
    }

    #[test]
    fn the_service_list_is_never_empty_or_spaced() {
        // A blank service would make `tccutil reset` clear *every* permission
        // Caduceus holds, which is the opposite of a repair.
        for grant in [
            Grant::Accessibility,
            Grant::ScreenRecording,
            Grant::Microphone,
            Grant::Automation,
            Grant::SpeechRecognition,
        ] {
            let service = grant.service();
            assert!(!service.is_empty());
            assert!(!service.contains(' '), "{service} would be two arguments");
        }
    }
}
