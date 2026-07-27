//! Keep-awake sessions: indefinite, timed, or until a clock time.
//!
//! The engine under the Manage window's "Keep Awake" page. It does what
//! Amphetamine's core does — sessions with a duration, an optional "display may
//! sleep" mode, and a live countdown — on top of `caffeinate`, which macOS
//! ships.
//!
//! # How a timed session ends
//!
//! `caffeinate` is always started with `-w <our pid>`, so the assertion can
//! never outlive Caduceus — a stray one would keep a laptop hot in a bag. The
//! countdown is enforced by us: a background task kills the child when the
//! deadline passes. `caffeinate -t` is deliberately not used, because its
//! interaction with `-w` is "whichever ends first" only by folklore — the man
//! page says `-t` is ignored with a utility and is vague with `-w` — and a
//! sleep-blocker whose end condition is folklore is how machines stay awake all
//! night.

use std::process::{Child, Command};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;

use super::ToolOutcome;

/// A running keep-awake session.
struct Session {
    child: Child,
    started: Instant,
    /// `None` = until turned off.
    duration: Option<Duration>,
    display_may_sleep: bool,
    /// Distinguishes this session from a later one in the reaper task, so an
    /// old reaper firing late cannot kill a session it did not start.
    generation: u64,
}

#[derive(Default)]
pub struct AwakeRuntime {
    session: Arc<Mutex<Option<Session>>>,
}

/// What the UI shows: whether a session is running and how long it has left.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AwakeStatus {
    pub active: bool,
    /// Seconds remaining, or `None` for an indefinite session.
    pub remaining_secs: Option<u64>,
    /// Total length of the running session, for a progress bar.
    pub total_secs: Option<u64>,
    pub display_may_sleep: bool,
}

impl AwakeRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn status(&self) -> AwakeStatus {
        let mut guard = self.session.lock();

        // A session whose caffeinate died (killed by hand, or the timer fired)
        // is over regardless of what we remember about it.
        let ended = match guard.as_mut() {
            Some(session) => session.child.try_wait().map(|s| s.is_some()).unwrap_or(true),
            None => true,
        };
        if ended {
            *guard = None;
            return AwakeStatus {
                active: false,
                remaining_secs: None,
                total_secs: None,
                display_may_sleep: false,
            };
        }

        let session = guard.as_ref().expect("checked above");
        let remaining = session.duration.map(|total| {
            total.saturating_sub(session.started.elapsed()).as_secs()
        });
        AwakeStatus {
            active: true,
            remaining_secs: remaining,
            total_secs: session.duration.map(|d| d.as_secs()),
            display_may_sleep: session.display_may_sleep,
        }
    }

    /// Start a session, replacing any running one.
    ///
    /// `duration` of `None` means until turned off. `display_may_sleep` keeps
    /// the *system* awake while letting the screen dim — Amphetamine's "allow
    /// display sleep", and the right mode for overnight downloads.
    pub fn start(&self, duration: Option<Duration>, display_may_sleep: bool) -> ToolOutcome {
        // `-i` idle, `-m` disk, `-s` system (AC), and `-d` only when the
        // display is to be held on too.
        let mut args: Vec<String> = if display_may_sleep {
            vec!["-ims".into()]
        } else {
            vec!["-dims".into()]
        };
        args.push("-w".into());
        args.push(std::process::id().to_string());

        let child = match Command::new("caffeinate").args(&args).spawn() {
            Ok(child) => child,
            Err(e) => return ToolOutcome::err(format!("Could not start caffeinate: {e}")),
        };

        let generation = {
            let mut guard = self.session.lock();
            // End the previous session's caffeinate before forgetting it.
            if let Some(mut old) = guard.take() {
                let _ = old.child.kill();
                let _ = old.child.wait();
            }
            let generation = std::time::UNIX_EPOCH
                .elapsed()
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            *guard = Some(Session {
                child,
                started: Instant::now(),
                duration,
                display_may_sleep,
                generation,
            });
            generation
        };

        // The reaper for a timed session. Holds only a weak-ish handle (the
        // Arc) and checks the generation, so replacing the session orphans the
        // old reaper harmlessly rather than letting it kill the new one.
        if let Some(total) = duration {
            let sessions = Arc::clone(&self.session);
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(total).await;
                let mut guard = sessions.lock();
                if let Some(session) = guard.as_mut() {
                    if session.generation == generation {
                        let _ = session.child.kill();
                        let _ = session.child.wait();
                        *guard = None;
                    }
                }
            });
        }

        ToolOutcome::ok(match duration {
            None => "Staying awake until you turn this off.".to_string(),
            Some(total) => format!("Staying awake for {}.", human_duration(total)),
        })
    }

    pub fn stop(&self) -> ToolOutcome {
        let mut guard = self.session.lock();
        match guard.take() {
            Some(mut session) => {
                let _ = session.child.kill();
                let _ = session.child.wait();
                ToolOutcome::ok("Sleep re-enabled.")
            }
            None => ToolOutcome::ok("Nothing was keeping this Mac awake."),
        }
    }
}

/// "90" → 90 minutes; "2h" → 2 hours; "45m", "1h30m", "2:30" all as expected.
///
/// The palette's `awake 45` path goes through this, so it accepts the ways
/// people actually type a duration rather than one blessed format.
pub fn parse_duration(input: &str) -> Option<Duration> {
    let text = input.trim().to_lowercase();
    if text.is_empty() {
        return None;
    }

    // "2:30" = hours:minutes.
    if let Some((h, m)) = text.split_once(':') {
        let hours: u64 = h.trim().parse().ok()?;
        let minutes: u64 = m.trim().parse().ok()?;
        if minutes >= 60 {
            return None;
        }
        return checked(hours * 60 + minutes);
    }

    // Unit-tagged pieces: "1h30m", "45m", "2h", "90s".
    if text.chars().any(|c| c.is_ascii_alphabetic()) {
        let mut total_secs: u64 = 0;
        let mut number = String::new();
        for c in text.chars() {
            if c.is_ascii_digit() {
                number.push(c);
            } else if !c.is_whitespace() {
                let value: u64 = number.parse().ok()?;
                number.clear();
                total_secs += match c {
                    'h' => value * 3600,
                    'm' => value * 60,
                    's' => value,
                    _ => return None,
                };
            }
        }
        if !number.is_empty() {
            // A trailing bare number after a unit ("1h30") reads as minutes.
            total_secs += number.parse::<u64>().ok()? * 60;
        }
        if total_secs == 0 {
            return None;
        }
        return checked(total_secs / 60).map(|_| Duration::from_secs(total_secs));
    }

    // A bare number is minutes.
    let minutes: u64 = text.parse().ok()?;
    checked(minutes)
}

/// Cap at 7 days: longer is a typo, and honouring "awake 999999" for real is
/// worse than refusing it.
fn checked(minutes: u64) -> Option<Duration> {
    (1..=7 * 24 * 60).contains(&minutes).then(|| Duration::from_secs(minutes * 60))
}

pub fn human_duration(d: Duration) -> String {
    let total_minutes = d.as_secs() / 60;
    let hours = total_minutes / 60;
    let minutes = total_minutes % 60;
    match (hours, minutes) {
        (0, m) => format!("{m} minute{}", if m == 1 { "" } else { "s" }),
        (h, 0) => format!("{h} hour{}", if h == 1 { "" } else { "s" }),
        (h, m) => format!("{h}h {m}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse_the_ways_people_type_them() {
        assert_eq!(parse_duration("45"), Some(Duration::from_secs(45 * 60)));
        assert_eq!(parse_duration("45m"), Some(Duration::from_secs(45 * 60)));
        assert_eq!(parse_duration("2h"), Some(Duration::from_secs(2 * 3600)));
        assert_eq!(parse_duration("1h30m"), Some(Duration::from_secs(90 * 60)));
        assert_eq!(parse_duration("1h30"), Some(Duration::from_secs(90 * 60)));
        assert_eq!(parse_duration("2:30"), Some(Duration::from_secs(150 * 60)));
        assert_eq!(parse_duration("90s"), Some(Duration::from_secs(90)));
    }

    #[test]
    fn nonsense_and_absurd_durations_are_refused() {
        for bad in ["", "0", "abc", "2:75", "-5", "999999999"] {
            assert_eq!(parse_duration(bad), None, "{bad:?} should not parse");
        }
    }

    #[test]
    fn durations_read_back_naturally() {
        assert_eq!(human_duration(Duration::from_secs(60)), "1 minute");
        assert_eq!(human_duration(Duration::from_secs(45 * 60)), "45 minutes");
        assert_eq!(human_duration(Duration::from_secs(3600)), "1 hour");
        assert_eq!(human_duration(Duration::from_secs(90 * 60)), "1h 30m");
    }

    #[test]
    fn a_session_reports_running_then_stops_cleanly() {
        let runtime = AwakeRuntime::new();
        assert!(!runtime.status().active);

        let started = runtime.start(Some(Duration::from_secs(120)), false);
        assert!(started.ok, "{}", started.message);

        let status = runtime.status();
        assert!(status.active);
        let remaining = status.remaining_secs.expect("timed session has a countdown");
        assert!(remaining > 100 && remaining <= 120, "remaining {remaining}");
        assert_eq!(status.total_secs, Some(120));
        assert!(!status.display_may_sleep);

        assert!(runtime.stop().ok);
        assert!(!runtime.status().active);
    }

    #[test]
    fn starting_again_replaces_the_running_session() {
        let runtime = AwakeRuntime::new();
        runtime.start(None, false);
        runtime.start(Some(Duration::from_secs(600)), true);

        let status = runtime.status();
        assert!(status.active);
        assert!(status.display_may_sleep);
        assert!(status.remaining_secs.is_some());

        // Exactly one caffeinate should remain ours; stop() ends it.
        assert!(runtime.stop().ok);
        assert!(!runtime.status().active);
    }

    #[test]
    fn an_indefinite_session_has_no_countdown() {
        let runtime = AwakeRuntime::new();
        runtime.start(None, true);
        let status = runtime.status();
        assert!(status.active);
        assert_eq!(status.remaining_secs, None);
        assert_eq!(status.total_secs, None);
        runtime.stop();
    }

    #[test]
    fn a_caffeinate_killed_from_outside_reads_as_inactive() {
        let runtime = AwakeRuntime::new();
        runtime.start(None, false);
        // Simulate `kill` from Activity Monitor.
        if let Some(session) = runtime.session.lock().as_mut() {
            let _ = session.child.kill();
            let _ = session.child.wait();
        }
        assert!(!runtime.status().active);
    }
}
