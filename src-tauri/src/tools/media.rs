//! Controlling whatever is playing.
//!
//! Music and Spotify both expose the same small AppleScript vocabulary, so one
//! set of verbs drives either. Which one gets the command is decided by what is
//! actually running: sending `playpause` to a launched-but-idle Music while
//! Spotify is playing would start a second stream on top of the first.

use std::process::Command;

use serde::{Deserialize, Serialize};

use super::ToolOutcome;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaAction {
    PlayPause,
    Next,
    Previous,
    NowPlaying,
}

/// The two players Caduceus knows how to drive, in preference order.
const PLAYERS: [&str; 2] = ["Spotify", "Music"];

fn run_tool(program: &str, args: &[&str]) -> Result<String, String> {
    let out = super::output_with_timeout(
        Command::new(program).args(args),
        super::TOOL_TIMEOUT,
        &format!("{program} did not answer in time."),
    )?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn osa(script: &str) -> Result<String, String> {
    run_tool("osascript", &["-e", script]).map_err(|e| {
        if e.contains("-1743") {
            "Caduceus is not allowed to control that player yet. Grant it in System Settings → \
             Privacy & Security → Automation."
                .to_string()
        } else {
            e
        }
    })
}

/// Whether an app is running, without launching it.
///
/// `is running` on a `System Events` process query never launches the target,
/// which `tell application "Music" to ...` would.
fn is_running(app: &str) -> bool {
    osa(&format!(
        "tell application \"System Events\" to (name of processes) contains \"{app}\""
    ))
    .map(|out| out.trim() == "true")
    .unwrap_or(false)
}

/// The player to send a command to: the one that is playing, else the one that
/// is merely running, else nothing.
fn active_player() -> Option<&'static str> {
    let running: Vec<&'static str> = PLAYERS.iter().copied().filter(|app| is_running(app)).collect();
    if running.is_empty() {
        return None;
    }
    running
        .iter()
        .copied()
        .find(|app| {
            osa(&format!("tell application \"{app}\" to player state as string"))
                .map(|state| state.trim() == "playing")
                .unwrap_or(false)
        })
        .or_else(|| running.first().copied())
}

pub fn run(action: MediaAction) -> ToolOutcome {
    let Some(player) = active_player() else {
        return ToolOutcome::err("Neither Music nor Spotify is running.");
    };

    match action {
        MediaAction::PlayPause => match osa(&format!("tell application \"{player}\" to playpause")) {
            Ok(_) => {
                let state = osa(&format!("tell application \"{player}\" to player state as string"))
                    .unwrap_or_default();
                ToolOutcome::ok(if state.trim() == "playing" {
                    format!("Playing in {player}")
                } else {
                    format!("Paused {player}")
                })
            }
            Err(e) => ToolOutcome::err(e),
        },

        MediaAction::Next => match osa(&format!("tell application \"{player}\" to next track")) {
            Ok(_) => ToolOutcome::ok(now_playing_text(player).unwrap_or_else(|| "Next track".into())),
            Err(e) => ToolOutcome::err(e),
        },

        MediaAction::Previous => {
            match osa(&format!("tell application \"{player}\" to previous track")) {
                Ok(_) => {
                    ToolOutcome::ok(now_playing_text(player).unwrap_or_else(|| "Previous track".into()))
                }
                Err(e) => ToolOutcome::err(e),
            }
        }

        MediaAction::NowPlaying => match now_playing_text(player) {
            Some(text) => ToolOutcome::copied(text.clone(), text),
            None => ToolOutcome::ok(format!("{player} is not playing anything.")),
        },
    }
}

fn now_playing_text(player: &str) -> Option<String> {
    let script = format!(
        "tell application \"{player}\"\n\
         if player state is stopped then return \"\"\n\
         return (name of current track) & \" — \" & (artist of current track)\n\
         end tell"
    );
    let text = osa(&script).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty() && trimmed != "—").then(|| trimmed.to_string())
}

/// Pause playback and report whether anything was actually paused.
///
/// Used by dictation: talking over a podcast produces a transcript of the
/// podcast. The boolean lets the caller resume only what it stopped.
pub fn pause_if_playing() -> bool {
    let Some(player) = active_player() else {
        return false;
    };
    let playing = osa(&format!("tell application \"{player}\" to player state as string"))
        .map(|state| state.trim() == "playing")
        .unwrap_or(false);
    if !playing {
        return false;
    }
    osa(&format!("tell application \"{player}\" to pause")).is_ok()
}

/// Resume the player that [`pause_if_playing`] stopped.
pub fn resume() {
    if let Some(player) = active_player() {
        let _ = osa(&format!("tell application \"{player}\" to play"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asking_about_a_process_never_launches_it() {
        // "Caduceus Nonexistent Player" is not installed, so this must be false
        // and must not leave anything running.
        assert!(!is_running("CaduceusNonexistentPlayer"));
    }

    #[test]
    fn media_commands_report_plainly_when_nothing_is_running() {
        // Whichever branch this machine is in, the result is a sentence.
        let outcome = run(MediaAction::NowPlaying);
        assert!(!outcome.message.is_empty());
        if !outcome.ok {
            assert!(
                outcome.message.contains("running") || outcome.message.contains("allowed"),
                "unhelpful message: {}",
                outcome.message
            );
        }
    }

    #[test]
    fn pausing_when_nothing_plays_reports_that_it_did_nothing() {
        // Must not claim to have paused something it did not, or dictation would
        // resume a player the user had deliberately stopped.
        if active_player().is_none() {
            assert!(!pause_if_playing());
        }
    }
}
