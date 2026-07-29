//! World clock, a timezone converter, countdown timers, a stopwatch, and a
//! pomodoro cycle.
//!
//! # Where a running timer lives
//!
//! In Rust, not the React tree — deliberately, and this is the one thing in
//! this file worth getting right before anything else.
//!
//! The Command Center window spends most of its life hidden: `WindowEvent::
//! CloseRequested` in `lib.rs` hides it rather than destroying it, and the
//! whole point of the app is that you summon it, do one thing, and it goes
//! away again. Hiding does not kill the webview process, but a page that is
//! not visible has its `setInterval`/`setTimeout` throttled by WebKit, and
//! that throttling is not documented to stop at any particular bound — it is
//! "as aggressive as the OS feels like being to save power." A pomodoro timer
//! that quietly drifts, or a countdown that simply stops ticking while you are
//! in another app for twenty minutes, is worse than not shipping the feature.
//!
//! So every timer here stores a **deadline** — a `std::time::Instant` — rather
//! than a counting-down number, and "how much is left" is always `deadline -
//! Instant::now()`, computed fresh whenever something asks. Completion is
//! driven by a `tokio::time::sleep` owned by this process's async runtime, the
//! same one `tools::awake::AwakeRuntime` uses to end a timed keep-awake
//! session — it keeps running on Caduceus's own clock regardless of what the
//! webview is doing, and it is what fires the completion notification. The
//! frontend's job is only to *display* the deadline; it polls this module's
//! state on an interval for the ticking digits, but losing a poll or two to
//! throttling costs a stale display for a moment, never a timer that failed to
//! go off.
//!
//! # Timezones without `chrono-tz`
//!
//! Caduceus's `chrono` dependency does not include `chrono-tz` — the IANA
//! database — so there is no lookup table of historical DST rules to draw on.
//! Instead [`ZONES`] is a small fixed table of UTC offsets, each tagged with
//! which of a handful of DST *patterns* (US, EU, southern-hemisphere
//! Australia, New Zealand, or none) the zone follows, and [`offset_minutes`]
//! computes today's offset from the pattern's rule directly. This is right for
//! every zone in the table for the foreseeable future — the US, EU and
//! Australian rules have been stable for years — but it is a hand-maintained
//! approximation, not a database: a government moving a transition date (as
//! several have, historically) would need this file updated, where a real
//! `chrono-tz` install would not. It is also date-grained rather than
//! second-grained — the DST switchover is treated as happening at local
//! midnight rather than at its actual 1am/2am wall-clock moment — which is
//! wrong only during the transition night itself and correct every other hour
//! of the year.
//!
//! # `chrono` is already a dependency
//!
//! `chrono-tz` was deliberately *not* added for this — see the module owner's
//! note on that decision. Everything below uses plain `chrono` (`NaiveDate`,
//! `NaiveDateTime`, `DateTime<Utc>`), which was already a dependency with the
//! `serde` feature on.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike, Duration as ChronoDuration, NaiveDate, NaiveDateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::apple;

// ---------------------------------------------------------------------------
// Notifications
// ---------------------------------------------------------------------------

/// Show a macOS notification banner.
///
/// The same `display notification … with title …` shape `extension_notify`
/// uses in `commands.rs`, reused rather than duplicated with different
/// escaping — both `title` and `body` here can contain a timer name someone
/// typed themselves, and an unescaped quote in either one turns the rest of
/// the AppleScript line into script. `escape_applescript` is the fix; skipping
/// it for "just a timer name" is exactly the kind of shortcut that made this a
/// real bug here before.
fn notify(title: &str, body: &str) {
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        crate::shortcuts::escape_applescript(&truncate(body, 400)),
        crate::shortcuts::escape_applescript(&truncate(title, 100)),
    );
    // Best-effort: a notification that fails to show (no permission granted
    // yet, System Events unavailable) should not take the timer itself down
    // with it — the countdown already reached zero either way.
    let _ = apple::run_script(&script);
}

fn truncate(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        text.to_string()
    } else {
        text.chars().take(chars).collect::<String>() + "…"
    }
}

/// Milliseconds-aware "how many seconds are left", rounded up.
///
/// A plain `Duration::as_secs()` truncates, so a timer with 29.97s left reads
/// as "29" the instant it starts — a countdown that visibly loses a second
/// before anything has actually happened. Rounding up means a fresh 30-second
/// timer shows 30 until the first whole second has actually elapsed.
fn ceil_secs(remaining: Duration) -> u64 {
    (remaining.as_millis() as u64).div_ceil(1000)
}

// ---------------------------------------------------------------------------
// Timezones
// ---------------------------------------------------------------------------

/// Which DST pattern a zone follows. See the module header for why this is a
/// pattern rather than a real historical rule table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DstRule {
    /// No daylight saving — the offset never changes.
    None,
    /// Second Sunday in March to the first Sunday in November.
    UnitedStates,
    /// Last Sunday in March to the last Sunday in October.
    EuropeanUnion,
    /// Southern hemisphere: first Sunday in October to the first Sunday in
    /// April, of the *following* year.
    SouthernAustralia,
    /// Southern hemisphere: last Sunday in September to the first Sunday in
    /// April.
    NewZealand,
}

pub struct ZoneDef {
    /// A stable key, in the IANA-ish shape people expect, even though this
    /// table does not actually read the IANA database. Used as the wire id.
    pub id: &'static str,
    pub label: &'static str,
    /// Standard (non-DST) offset from UTC, in minutes.
    std_offset_minutes: i32,
    dst: DstRule,
}

/// A curated set of zones covering the world's major population centres and
/// financial hubs — not the ~400 IANA has, which is more than anyone
/// searching a "world clock" picker wants to scroll through.
pub static ZONES: &[ZoneDef] = &[
    ZoneDef {
        id: "Pacific/Honolulu",
        label: "Honolulu",
        std_offset_minutes: -600,
        dst: DstRule::None,
    },
    ZoneDef {
        id: "America/Anchorage",
        label: "Anchorage",
        std_offset_minutes: -540,
        dst: DstRule::UnitedStates,
    },
    ZoneDef {
        id: "America/Los_Angeles",
        label: "Los Angeles",
        std_offset_minutes: -480,
        dst: DstRule::UnitedStates,
    },
    ZoneDef {
        id: "America/Denver",
        label: "Denver",
        std_offset_minutes: -420,
        dst: DstRule::UnitedStates,
    },
    ZoneDef {
        id: "America/Chicago",
        label: "Chicago",
        std_offset_minutes: -360,
        dst: DstRule::UnitedStates,
    },
    ZoneDef {
        id: "America/Mexico_City",
        label: "Mexico City",
        std_offset_minutes: -360,
        dst: DstRule::None,
    },
    ZoneDef {
        id: "America/New_York",
        label: "New York",
        std_offset_minutes: -300,
        dst: DstRule::UnitedStates,
    },
    ZoneDef {
        id: "America/Toronto",
        label: "Toronto",
        std_offset_minutes: -300,
        dst: DstRule::UnitedStates,
    },
    ZoneDef {
        id: "America/Sao_Paulo",
        label: "São Paulo",
        std_offset_minutes: -180,
        dst: DstRule::None,
    },
    ZoneDef {
        id: "Atlantic/Azores",
        label: "Azores",
        std_offset_minutes: -60,
        dst: DstRule::EuropeanUnion,
    },
    ZoneDef {
        id: "Etc/UTC",
        label: "UTC",
        std_offset_minutes: 0,
        dst: DstRule::None,
    },
    ZoneDef {
        id: "Europe/London",
        label: "London",
        std_offset_minutes: 0,
        dst: DstRule::EuropeanUnion,
    },
    ZoneDef {
        id: "Europe/Lisbon",
        label: "Lisbon",
        std_offset_minutes: 0,
        dst: DstRule::EuropeanUnion,
    },
    ZoneDef {
        id: "Europe/Paris",
        label: "Paris",
        std_offset_minutes: 60,
        dst: DstRule::EuropeanUnion,
    },
    ZoneDef {
        id: "Europe/Berlin",
        label: "Berlin",
        std_offset_minutes: 60,
        dst: DstRule::EuropeanUnion,
    },
    ZoneDef {
        id: "Europe/Madrid",
        label: "Madrid",
        std_offset_minutes: 60,
        dst: DstRule::EuropeanUnion,
    },
    ZoneDef {
        id: "Europe/Rome",
        label: "Rome",
        std_offset_minutes: 60,
        dst: DstRule::EuropeanUnion,
    },
    ZoneDef {
        id: "Europe/Athens",
        label: "Athens",
        std_offset_minutes: 120,
        dst: DstRule::EuropeanUnion,
    },
    ZoneDef {
        id: "Africa/Cairo",
        label: "Cairo",
        std_offset_minutes: 120,
        dst: DstRule::None,
    },
    ZoneDef {
        id: "Africa/Johannesburg",
        label: "Johannesburg",
        std_offset_minutes: 120,
        dst: DstRule::None,
    },
    ZoneDef {
        id: "Europe/Moscow",
        label: "Moscow",
        std_offset_minutes: 180,
        dst: DstRule::None,
    },
    ZoneDef {
        id: "Asia/Dubai",
        label: "Dubai",
        std_offset_minutes: 240,
        dst: DstRule::None,
    },
    ZoneDef {
        id: "Asia/Karachi",
        label: "Karachi",
        std_offset_minutes: 300,
        dst: DstRule::None,
    },
    ZoneDef {
        id: "Asia/Kolkata",
        label: "Mumbai, New Delhi",
        std_offset_minutes: 330,
        dst: DstRule::None,
    },
    ZoneDef {
        id: "Asia/Dhaka",
        label: "Dhaka",
        std_offset_minutes: 360,
        dst: DstRule::None,
    },
    ZoneDef {
        id: "Asia/Bangkok",
        label: "Bangkok",
        std_offset_minutes: 420,
        dst: DstRule::None,
    },
    ZoneDef {
        id: "Asia/Jakarta",
        label: "Jakarta",
        std_offset_minutes: 420,
        dst: DstRule::None,
    },
    ZoneDef {
        id: "Asia/Shanghai",
        label: "Shanghai, Beijing",
        std_offset_minutes: 480,
        dst: DstRule::None,
    },
    ZoneDef {
        id: "Asia/Singapore",
        label: "Singapore",
        std_offset_minutes: 480,
        dst: DstRule::None,
    },
    ZoneDef {
        id: "Asia/Hong_Kong",
        label: "Hong Kong",
        std_offset_minutes: 480,
        dst: DstRule::None,
    },
    ZoneDef {
        id: "Asia/Tokyo",
        label: "Tokyo",
        std_offset_minutes: 540,
        dst: DstRule::None,
    },
    ZoneDef {
        id: "Asia/Seoul",
        label: "Seoul",
        std_offset_minutes: 540,
        dst: DstRule::None,
    },
    ZoneDef {
        id: "Australia/Sydney",
        label: "Sydney",
        std_offset_minutes: 600,
        dst: DstRule::SouthernAustralia,
    },
    ZoneDef {
        id: "Australia/Melbourne",
        label: "Melbourne",
        std_offset_minutes: 600,
        dst: DstRule::SouthernAustralia,
    },
    ZoneDef {
        id: "Australia/Perth",
        label: "Perth",
        std_offset_minutes: 480,
        dst: DstRule::None,
    },
    ZoneDef {
        id: "Pacific/Auckland",
        label: "Auckland",
        std_offset_minutes: 720,
        dst: DstRule::NewZealand,
    },
];

pub fn find_zone(id: &str) -> Option<&'static ZoneDef> {
    ZONES.iter().find(|z| z.id == id)
}

/// The Sunday that is the `n`th (1-based) Sunday of `month`.
fn nth_sunday(year: i32, month: u32, n: u32) -> NaiveDate {
    let first = NaiveDate::from_ymd_opt(year, month, 1).expect("valid calendar month");
    let days_to_first_sunday = (7 - first.weekday().num_days_from_sunday()) % 7;
    let first_sunday = 1 + days_to_first_sunday;
    NaiveDate::from_ymd_opt(year, month, first_sunday + (n - 1) * 7)
        .expect("Sunday exists in every month")
}

/// The last Sunday of `month`.
fn last_sunday(year: i32, month: u32) -> NaiveDate {
    let next_month_first = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    }
    .expect("valid calendar month");
    let last_day = next_month_first
        .pred_opt()
        .expect("a month always has a previous day");
    last_day - ChronoDuration::days(last_day.weekday().num_days_from_sunday() as i64)
}

/// Whether `date` falls inside the given DST pattern's active window.
///
/// Date-grained, per the module header: this decides "which side of the
/// switchover is this calendar day on", not "is it past 2am yet on the
/// switchover day itself".
fn is_dst_active(rule: DstRule, date: NaiveDate) -> bool {
    match rule {
        DstRule::None => false,
        DstRule::UnitedStates => {
            let start = nth_sunday(date.year(), 3, 2);
            let end = nth_sunday(date.year(), 11, 1);
            date >= start && date < end
        }
        DstRule::EuropeanUnion => {
            let start = last_sunday(date.year(), 3);
            let end = last_sunday(date.year(), 10);
            date >= start && date < end
        }
        // Southern hemisphere: the active window straddles New Year's, so it
        // is "on or after this year's start" OR "before this year's end",
        // rather than a single contiguous `start..end` within one year.
        DstRule::SouthernAustralia => {
            let start = nth_sunday(date.year(), 10, 1);
            let end = nth_sunday(date.year(), 4, 1);
            date >= start || date < end
        }
        DstRule::NewZealand => {
            let start = last_sunday(date.year(), 9);
            let end = nth_sunday(date.year(), 4, 1);
            date >= start || date < end
        }
    }
}

/// The zone's UTC offset, in minutes, on the given calendar date.
pub fn offset_minutes(zone: &ZoneDef, date: NaiveDate) -> i32 {
    zone.std_offset_minutes + if is_dst_active(zone.dst, date) { 60 } else { 0 }
}

fn format_offset(minutes: i32) -> String {
    let sign = if minutes < 0 { "-" } else { "+" };
    let abs = minutes.unsigned_abs();
    format!("UTC{sign}{:02}:{:02}", abs / 60, abs % 60)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZoneClock {
    pub id: String,
    pub label: String,
    pub offset_minutes: i32,
    pub utc_offset_label: String,
    /// `YYYY-MM-DDTHH:MM:SS` wall-clock time in this zone, right now. The
    /// frontend only reads this once per fetch and ticks the seconds forward
    /// itself with the offset — see `TimePage.tsx` — so this does not need to
    /// be re-requested every second.
    pub local_iso: String,
    pub is_dst: bool,
}

fn zone_clock(zone: &ZoneDef, now_utc: DateTime<Utc>) -> ZoneClock {
    let offset = offset_minutes(zone, now_utc.date_naive());
    let local = now_utc.naive_utc() + ChronoDuration::minutes(offset as i64);
    ZoneClock {
        id: zone.id.to_string(),
        label: zone.label.to_string(),
        offset_minutes: offset,
        utc_offset_label: format_offset(offset),
        local_iso: local.format("%Y-%m-%dT%H:%M:%S").to_string(),
        is_dst: offset != zone.std_offset_minutes,
    }
}

/// Every catalogued zone with its current offset and wall-clock time — the
/// data behind both the world clock's rows and its searchable picker.
pub fn world_clock(now_utc: DateTime<Utc>) -> Vec<ZoneClock> {
    ZONES.iter().map(|z| zone_clock(z, now_utc)).collect()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertRequest {
    pub zone_id: String,
    /// `YYYY-MM-DDTHH:MM`, the shape an `<input type="datetime-local">` gives.
    pub local_datetime: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertedTime {
    pub id: String,
    pub label: String,
    /// `YYYY-MM-DDTHH:MM`.
    pub local_iso: String,
    pub utc_offset_label: String,
    /// Days from the source zone's date to this zone's date for the same
    /// instant — `-1`, `0`, or `1` almost always, so "5pm EST in Tokyo" can
    /// say "tomorrow" instead of just a time that looks wrong out of context.
    pub day_offset: i32,
}

/// Read a time in one zone and show it in a set of others.
///
/// The source offset is resolved against the *typed* date, not today's date —
/// converting a March time needs March's DST state, which may differ from
/// whatever the offset would be right now.
pub fn convert(
    request: &ConvertRequest,
    target_ids: &[String],
) -> Result<Vec<ConvertedTime>, String> {
    let source = find_zone(&request.zone_id)
        .ok_or_else(|| format!("“{}” is not a time zone Caduceus knows.", request.zone_id))?;
    let naive = NaiveDateTime::parse_from_str(&request.local_datetime, "%Y-%m-%dT%H:%M")
        .map_err(|_| "That is not a time Caduceus recognises.".to_string())?;

    let source_offset = offset_minutes(source, naive.date());
    let utc = naive - ChronoDuration::minutes(source_offset as i64);

    target_ids
        .iter()
        .map(|id| {
            let zone = find_zone(id)
                .ok_or_else(|| format!("“{id}” is not a time zone Caduceus knows."))?;
            // The offset for the target is resolved from its own local date,
            // guarded against the rare case a conversion lands right on a DST
            // boundary and the first guess is a day off — one correction step
            // is enough because no zone's DST offset ever exceeds an hour.
            let first_guess =
                utc + ChronoDuration::minutes(offset_minutes(zone, utc.date()) as i64);
            let offset = offset_minutes(zone, first_guess.date());
            let local = utc + ChronoDuration::minutes(offset as i64);
            Ok(ConvertedTime {
                id: zone.id.to_string(),
                label: zone.label.to_string(),
                local_iso: local.format("%Y-%m-%dT%H:%M").to_string(),
                utc_offset_label: format_offset(offset),
                day_offset: (local.date() - naive.date()).num_days() as i32,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Countdown timers
// ---------------------------------------------------------------------------

const MAX_TIMER_SECS: u64 = 24 * 3600;
const MAX_CONCURRENT_TIMERS: usize = 20;

struct CountdownTimer {
    id: u64,
    name: String,
    deadline: Instant,
    total: Duration,
    /// Set by the reaper once it fires. Kept in the list (rather than removed)
    /// so the UI can show "Done" until the user dismisses it — a timer that
    /// silently vanishes at zero is easy to miss if you were not looking right
    /// at that moment.
    completed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimerSnapshot {
    pub id: u64,
    pub name: String,
    pub total_secs: u64,
    pub remaining_secs: u64,
    pub completed: bool,
}

fn timer_snapshot(timer: &CountdownTimer, now: Instant) -> TimerSnapshot {
    let remaining = if timer.completed {
        0
    } else {
        ceil_secs(timer.deadline.saturating_duration_since(now))
    };
    TimerSnapshot {
        id: timer.id,
        name: timer.name.clone(),
        total_secs: timer.total.as_secs(),
        remaining_secs: remaining,
        completed: timer.completed || remaining == 0,
    }
}

// ---------------------------------------------------------------------------
// Stopwatch
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Stopwatch {
    running: bool,
    /// Time banked from previous start/stop cycles, not counting the one in
    /// progress.
    accumulated: Duration,
    started_at: Option<Instant>,
    /// Elapsed time *at* each lap, cumulative from the very start — the shape
    /// every stopwatch app shows a lap list in. The frontend derives the
    /// per-lap split by subtracting consecutive entries.
    laps: Vec<Duration>,
}

fn stopwatch_elapsed(sw: &Stopwatch, now: Instant) -> Duration {
    match (sw.running, sw.started_at) {
        (true, Some(started)) => sw.accumulated + now.saturating_duration_since(started),
        _ => sw.accumulated,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StopwatchStatus {
    pub running: bool,
    pub elapsed_ms: u64,
    pub laps_ms: Vec<u64>,
}

fn stopwatch_status(sw: &Stopwatch, now: Instant) -> StopwatchStatus {
    StopwatchStatus {
        running: sw.running,
        elapsed_ms: stopwatch_elapsed(sw, now).as_millis() as u64,
        laps_ms: sw.laps.iter().map(|d| d.as_millis() as u64).collect(),
    }
}

// ---------------------------------------------------------------------------
// Pomodoro
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Work,
    ShortBreak,
    LongBreak,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PomodoroConfig {
    pub work_minutes: u32,
    pub short_break_minutes: u32,
    pub long_break_minutes: u32,
    /// How many work sessions between long breaks — a long break follows
    /// every `cycles_before_long_break`th work session instead of a short one.
    /// `0` means "never", i.e. always a short break.
    pub cycles_before_long_break: u32,
    /// Total work sessions for the whole run. The frontend no longer offers
    /// `0` — see `MAX_TOTAL_CYCLES` for why — but a `0` arriving here anyway
    /// (an older client, a script) is not treated as "run forever": it is
    /// clamped to `MAX_TOTAL_CYCLES` by [`TimekeepingRuntime::pomodoro_start`]
    /// before a session is ever created. `advance`, below, still implements
    /// the literal "0 means no fixed end" rule as a pure function, because
    /// that is the simplest thing to unit-test — the runtime is what makes
    /// sure a live session never actually sees a `0` it would act on.
    pub total_cycles: u32,
}

/// Hard ceiling on work sessions in a single pomodoro run, applied by
/// [`TimekeepingRuntime::pomodoro_start`] regardless of what was requested.
///
/// This exists because "random notifications" turned out to mean exactly one
/// thing: a session started with `total_cycles == 0` ("run until stopped by
/// hand") that nobody remembered to stop, quietly alerting at every phase
/// boundary for as long as Caduceus stayed running. Sixteen work sessions, at
/// even the shortest sensible cadence (25 minutes work, 5 minutes break), is
/// already well over six hours of active-plus-break time — a full working day
/// — and past that point "just one more session" stops being something a
/// person deliberately chose and starts being a session they forgot about.
/// The tray's "Stop pomodoro" item (see `tray.rs`) is the real safety net for
/// day-to-day use; this ceiling is the backstop for when nobody is looking.
const MAX_TOTAL_CYCLES: u32 = 16;

fn phase_minutes(config: &PomodoroConfig, phase: Phase) -> u32 {
    match phase {
        Phase::Work => config.work_minutes,
        Phase::ShortBreak => config.short_break_minutes,
        Phase::LongBreak => config.long_break_minutes,
    }
}

fn phase_message(phase: Phase, cycle: u32) -> String {
    match phase {
        Phase::Work => format!(
            "Pomodoro — break's over; work session {cycle} starting. Stop it from the tray or Caduceus → Time → Pomodoro."
        ),
        Phase::ShortBreak => {
            "Pomodoro — work session done. Take a short break. Stop it from the tray or Caduceus → Time → Pomodoro."
                .to_string()
        }
        Phase::LongBreak => {
            "Pomodoro — nice streak; take a long break. Stop it from the tray or Caduceus → Time → Pomodoro."
                .to_string()
        }
    }
}

/// What happens when the phase currently running reaches zero.
///
/// A pure function of the config and where the session currently is — no
/// `Instant`, no lock, nothing to fake — which is what makes the cycle logic
/// (long break every Nth session, stopping at the configured total) testable
/// without a clock at all. `None` means the whole run is over.
fn advance(config: &PomodoroConfig, phase: Phase, cycle: u32) -> Option<(Phase, u32)> {
    match phase {
        Phase::Work => {
            if config.total_cycles > 0 && cycle >= config.total_cycles {
                return None;
            }
            let long_break =
                config.cycles_before_long_break > 0 && cycle % config.cycles_before_long_break == 0;
            Some((
                if long_break {
                    Phase::LongBreak
                } else {
                    Phase::ShortBreak
                },
                cycle,
            ))
        }
        Phase::ShortBreak | Phase::LongBreak => {
            let next_cycle = cycle + 1;
            if config.total_cycles > 0 && next_cycle > config.total_cycles {
                return None;
            }
            Some((Phase::Work, next_cycle))
        }
    }
}

struct PomodoroSession {
    config: PomodoroConfig,
    phase: Phase,
    /// 1-based: the work session in progress, or the one that just ended.
    cycle: u32,
    deadline: Instant,
    duration: Duration,
    /// Distinguishes this run from a later one when the reaper wakes up — the
    /// same trick `AwakeRuntime` uses, and for the same reason: stopping and
    /// immediately restarting must not let a stale reaper for the old run
    /// mutate the new one.
    generation: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PomodoroStatus {
    pub running: bool,
    pub phase: Option<Phase>,
    pub cycle: u32,
    pub total_cycles: u32,
    pub remaining_secs: u64,
    pub total_secs: u64,
}

fn pomodoro_status_of(session: Option<&PomodoroSession>, now: Instant) -> PomodoroStatus {
    match session {
        Some(s) => PomodoroStatus {
            running: true,
            phase: Some(s.phase),
            cycle: s.cycle,
            total_cycles: s.config.total_cycles,
            remaining_secs: ceil_secs(s.deadline.saturating_duration_since(now)),
            total_secs: s.duration.as_secs(),
        },
        None => PomodoroStatus {
            running: false,
            phase: None,
            cycle: 0,
            total_cycles: 0,
            remaining_secs: 0,
            total_secs: 0,
        },
    }
}

// ---------------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------------

/// The live state behind the Time page: timers, the stopwatch, and a pomodoro
/// session, all managed as Tauri state so they outlive any particular webview
/// render — see the module header for why that matters.
#[derive(Default)]
pub struct TimekeepingRuntime {
    next_timer_id: AtomicU64,
    timers: Arc<Mutex<Vec<CountdownTimer>>>,
    stopwatch: Arc<Mutex<Stopwatch>>,
    pomodoro: Arc<Mutex<Option<PomodoroSession>>>,
    next_pomodoro_generation: AtomicU64,
    /// Fired after every pomodoro start, stop, and phase transition. `lib.rs`
    /// wires this up to `tray::refresh` so a running session — and its "Stop
    /// pomodoro" item — is never more than a menu-bar click away, which is
    /// what actually prevents a forgotten session from feeling like it is
    /// sending "random" notifications. Deliberately just a callback rather
    /// than this module reaching for `tauri::AppHandle` directly: everything
    /// above stays a plain, clock-free unit that tests without any Tauri
    /// runtime at all, and tests can plug in their own callback to observe
    /// exactly when a change happens (see the reaper tests below).
    on_pomodoro_change: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl TimekeepingRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    // --- countdown timers ---------------------------------------------------

    pub fn start_timer(&self, name: String, seconds: u64) -> Result<TimerSnapshot, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Give the timer a name.".into());
        }
        if seconds == 0 {
            return Err("Pick a duration longer than zero.".into());
        }
        if seconds > MAX_TIMER_SECS {
            return Err("Timers top out at 24 hours.".into());
        }
        if self.timers.lock().iter().filter(|t| !t.completed).count() >= MAX_CONCURRENT_TIMERS {
            return Err(format!(
                "That is {MAX_CONCURRENT_TIMERS} running timers already — dismiss one first."
            ));
        }

        let id = self.next_timer_id.fetch_add(1, Ordering::SeqCst);
        let total = Duration::from_secs(seconds);
        let deadline = Instant::now() + total;
        let name = name.to_string();

        let snapshot = {
            let mut guard = self.timers.lock();
            guard.push(CountdownTimer {
                id,
                name: name.clone(),
                deadline,
                total,
                completed: false,
            });
            timer_snapshot(guard.last().expect("just pushed"), Instant::now())
        };

        // The reaper: sleeps for exactly the timer's duration, then marks it
        // done and notifies — unless the timer was dismissed first, in which
        // case it has already vanished from the list and there is nothing to
        // fire for.
        let timers = Arc::clone(&self.timers);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(total).await;
            let fired = {
                let mut guard = timers.lock();
                match guard.iter_mut().find(|t| t.id == id) {
                    Some(t) => {
                        t.completed = true;
                        true
                    }
                    None => false,
                }
            };
            if fired {
                notify("Timer done", &name);
            }
        });

        Ok(snapshot)
    }

    pub fn list_timers(&self) -> Vec<TimerSnapshot> {
        let now = Instant::now();
        self.timers
            .lock()
            .iter()
            .map(|t| timer_snapshot(t, now))
            .collect()
    }

    pub fn dismiss_timer(&self, id: u64) {
        self.timers.lock().retain(|t| t.id != id);
    }

    // --- stopwatch -----------------------------------------------------------

    pub fn stopwatch_start(&self) -> StopwatchStatus {
        let mut sw = self.stopwatch.lock();
        if !sw.running {
            sw.running = true;
            sw.started_at = Some(Instant::now());
        }
        stopwatch_status(&sw, Instant::now())
    }

    pub fn stopwatch_stop(&self) -> StopwatchStatus {
        let mut sw = self.stopwatch.lock();
        if sw.running {
            let now = Instant::now();
            let started_at = sw.started_at.unwrap_or(now);
            sw.accumulated += now.saturating_duration_since(started_at);
            sw.running = false;
            sw.started_at = None;
        }
        stopwatch_status(&sw, Instant::now())
    }

    /// Record a lap. Only meaningful while running — a lap taken on a stopped
    /// watch would just be a duplicate of the last one.
    pub fn stopwatch_lap(&self) -> StopwatchStatus {
        let mut sw = self.stopwatch.lock();
        let now = Instant::now();
        if sw.running {
            let elapsed = stopwatch_elapsed(&sw, now);
            sw.laps.push(elapsed);
        }
        stopwatch_status(&sw, now)
    }

    pub fn stopwatch_reset(&self) -> StopwatchStatus {
        let mut sw = self.stopwatch.lock();
        *sw = Stopwatch::default();
        stopwatch_status(&sw, Instant::now())
    }

    pub fn stopwatch_status(&self) -> StopwatchStatus {
        let sw = self.stopwatch.lock();
        stopwatch_status(&sw, Instant::now())
    }

    // --- pomodoro --------------------------------------------------------------

    /// Wire up a callback for "the pomodoro state changed" — see the field
    /// doc comment on `on_pomodoro_change` for why this exists as a callback
    /// rather than a direct `tray::refresh` call from in here.
    pub fn set_on_pomodoro_change(&self, callback: impl Fn() + Send + Sync + 'static) {
        *self.on_pomodoro_change.lock() = Some(Arc::new(callback));
    }

    fn notify_pomodoro_change(&self) {
        if let Some(callback) = self.on_pomodoro_change.lock().as_ref() {
            callback();
        }
    }

    pub fn pomodoro_start(&self, mut config: PomodoroConfig) -> Result<PomodoroStatus, String> {
        if config.work_minutes == 0 || config.short_break_minutes == 0 {
            return Err("Work and short-break lengths must be at least a minute.".into());
        }
        if config.long_break_minutes == 0 {
            return Err("Long-break length must be at least a minute.".into());
        }
        if config.work_minutes > 180
            || config.short_break_minutes > 180
            || config.long_break_minutes > 180
        {
            return Err("Three hours is the longest a single phase can run.".into());
        }
        if config.total_cycles > MAX_TOTAL_CYCLES {
            return Err(format!(
                "{MAX_TOTAL_CYCLES} work sessions is the longest a single run can be — that's already a full day."
            ));
        }
        // `0` ("run until stopped by hand") is still accepted for
        // compatibility, but never actually honoured as unbounded — see
        // `MAX_TOTAL_CYCLES` for why this is where an "infinite" request
        // actually stops.
        if config.total_cycles == 0 {
            config.total_cycles = MAX_TOTAL_CYCLES;
        }

        let generation = self.next_pomodoro_generation.fetch_add(1, Ordering::SeqCst);
        let duration = Duration::from_secs(config.work_minutes as u64 * 60);
        let deadline = Instant::now() + duration;

        *self.pomodoro.lock() = Some(PomodoroSession {
            config: config.clone(),
            phase: Phase::Work,
            cycle: 1,
            deadline,
            duration,
            generation,
        });

        Self::spawn_pomodoro_reaper(
            Arc::clone(&self.pomodoro),
            generation,
            duration,
            self.on_pomodoro_change.lock().clone(),
        );

        // Announce the start: a session that only notifies at phase boundaries
        // is easy to forget you began — and then every break alert feels random.
        notify(
            "Caduceus",
            &format!(
                "Pomodoro started — {mins} min work session 1. Stop it anytime from the tray or Time → Pomodoro.",
                mins = config.work_minutes
            ),
        );
        self.notify_pomodoro_change();

        Ok(self.pomodoro_status())
    }

    pub fn pomodoro_stop(&self) -> PomodoroStatus {
        // Dropping the session (rather than flagging it stopped) is what the
        // reaper checks: its generation simply will not match whatever runs
        // next, so it fires its notification-less no-op and stops there.
        *self.pomodoro.lock() = None;
        self.notify_pomodoro_change();
        self.pomodoro_status()
    }

    pub fn pomodoro_status(&self) -> PomodoroStatus {
        pomodoro_status_of(self.pomodoro.lock().as_ref(), Instant::now())
    }

    /// Sleep out the current phase, then transition and notify, then arm the
    /// next phase's sleep — a self-perpetuating chain rather than one task
    /// with a loop in it, so a `pomodoro_stop` between two phases has nothing
    /// to cancel: the next iteration simply finds no session, or the wrong
    /// generation, and stops quietly.
    ///
    /// This is the choke point every pomodoro notification passes through —
    /// there is no other path from "phase ended" to `notify()`. A session
    /// that has been stopped (`guard.as_mut()` is `None`) or superseded by a
    /// later one (`session.generation != generation`) returns before either
    /// `notify` or `on_change` runs, which is what makes "a stopped session
    /// can still ping you" structurally impossible rather than just unlikely.
    /// See the `a_stopped_session_…` / `a_superseded_session_…` tests below.
    fn spawn_pomodoro_reaper(
        pomodoro: Arc<Mutex<Option<PomodoroSession>>>,
        generation: u64,
        wait: Duration,
        on_change: Option<Arc<dyn Fn() + Send + Sync>>,
    ) {
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(wait).await;

            let outcome = {
                let mut guard = pomodoro.lock();
                let Some(session) = guard.as_mut() else {
                    return;
                };
                if session.generation != generation {
                    return;
                }
                match advance(&session.config, session.phase, session.cycle) {
                    Some((phase, cycle)) => {
                        let duration =
                            Duration::from_secs(phase_minutes(&session.config, phase) as u64 * 60);
                        session.phase = phase;
                        session.cycle = cycle;
                        session.duration = duration;
                        session.deadline = Instant::now() + duration;
                        Some((phase_message(phase, cycle), Some(duration)))
                    }
                    None => {
                        *guard = None;
                        Some((
                            "Pomodoro complete — nice work. Start another from Caduceus → Time → Pomodoro."
                                .to_string(),
                            None,
                        ))
                    }
                }
            };

            if let Some((message, next_wait)) = outcome {
                notify("Caduceus", &message);
                if let Some(callback) = &on_change {
                    callback();
                }
                if let Some(wait) = next_wait {
                    Self::spawn_pomodoro_reaper(pomodoro, generation, wait, on_change);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- timezone maths ------------------------------------------------------

    #[test]
    fn nth_sunday_finds_the_right_calendar_day() {
        // March 2026: the 1st is a Sunday, so the 2nd Sunday (US DST start) is
        // the 8th.
        assert_eq!(
            nth_sunday(2026, 3, 1),
            NaiveDate::from_ymd_opt(2026, 3, 1).unwrap()
        );
        assert_eq!(
            nth_sunday(2026, 3, 2),
            NaiveDate::from_ymd_opt(2026, 3, 8).unwrap()
        );
    }

    #[test]
    fn last_sunday_finds_the_final_one_even_when_the_month_ends_midweek() {
        // October 2026 ends on a Saturday, so the last Sunday is the 25th.
        assert_eq!(
            last_sunday(2026, 10),
            NaiveDate::from_ymd_opt(2026, 10, 25).unwrap()
        );
    }

    #[test]
    fn us_dst_is_active_in_july_and_not_in_january() {
        assert!(is_dst_active(
            DstRule::UnitedStates,
            NaiveDate::from_ymd_opt(2026, 7, 15).unwrap()
        ));
        assert!(!is_dst_active(
            DstRule::UnitedStates,
            NaiveDate::from_ymd_opt(2026, 1, 15).unwrap()
        ));
    }

    #[test]
    fn us_dst_boundaries_land_on_the_right_sundays() {
        // 2026: US DST starts Sun Mar 8, ends Sun Nov 1.
        assert!(!is_dst_active(
            DstRule::UnitedStates,
            NaiveDate::from_ymd_opt(2026, 3, 7).unwrap()
        ));
        assert!(is_dst_active(
            DstRule::UnitedStates,
            NaiveDate::from_ymd_opt(2026, 3, 8).unwrap()
        ));
        assert!(is_dst_active(
            DstRule::UnitedStates,
            NaiveDate::from_ymd_opt(2026, 10, 31).unwrap()
        ));
        assert!(!is_dst_active(
            DstRule::UnitedStates,
            NaiveDate::from_ymd_opt(2026, 11, 1).unwrap()
        ));
    }

    #[test]
    fn eu_dst_boundaries_land_on_the_last_sundays() {
        // 2026: EU DST starts Sun Mar 29, ends Sun Oct 25.
        assert!(!is_dst_active(
            DstRule::EuropeanUnion,
            NaiveDate::from_ymd_opt(2026, 3, 28).unwrap()
        ));
        assert!(is_dst_active(
            DstRule::EuropeanUnion,
            NaiveDate::from_ymd_opt(2026, 3, 29).unwrap()
        ));
        assert!(is_dst_active(
            DstRule::EuropeanUnion,
            NaiveDate::from_ymd_opt(2026, 10, 24).unwrap()
        ));
        assert!(!is_dst_active(
            DstRule::EuropeanUnion,
            NaiveDate::from_ymd_opt(2026, 10, 25).unwrap()
        ));
    }

    #[test]
    fn southern_hemisphere_dst_wraps_across_new_year() {
        // Sydney: DST from Oct through the following April.
        assert!(is_dst_active(
            DstRule::SouthernAustralia,
            NaiveDate::from_ymd_opt(2026, 12, 25).unwrap()
        ));
        assert!(is_dst_active(
            DstRule::SouthernAustralia,
            NaiveDate::from_ymd_opt(2026, 1, 15).unwrap()
        ));
        assert!(!is_dst_active(
            DstRule::SouthernAustralia,
            NaiveDate::from_ymd_opt(2026, 7, 15).unwrap()
        ));
    }

    #[test]
    fn zones_without_dst_never_move() {
        let tokyo = find_zone("Asia/Tokyo").unwrap();
        let summer = offset_minutes(tokyo, NaiveDate::from_ymd_opt(2026, 7, 1).unwrap());
        let winter = offset_minutes(tokyo, NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
        assert_eq!(summer, winter);
        assert_eq!(summer, 540);
    }

    #[test]
    fn new_york_is_five_hours_behind_utc_in_winter_and_four_in_summer() {
        let ny = find_zone("America/New_York").unwrap();
        assert_eq!(
            offset_minutes(ny, NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
            -300
        );
        assert_eq!(
            offset_minutes(ny, NaiveDate::from_ymd_opt(2026, 7, 1).unwrap()),
            -240
        );
    }

    #[test]
    fn an_unknown_zone_id_is_named_in_the_error() {
        let request = ConvertRequest {
            zone_id: "Nowhere/Imaginary".into(),
            local_datetime: "2026-07-01T12:00".into(),
        };
        let err = convert(&request, &["Etc/UTC".to_string()]).unwrap_err();
        assert!(err.contains("Nowhere/Imaginary"));
    }

    #[test]
    fn a_malformed_datetime_is_refused_rather_than_panicking() {
        let request = ConvertRequest {
            zone_id: "Etc/UTC".into(),
            local_datetime: "not a date".into(),
        };
        assert!(convert(&request, &["Etc/UTC".to_string()]).is_err());
    }

    #[test]
    fn five_pm_new_york_is_next_morning_in_tokyo() {
        // 5pm EDT (summer, UTC-4) is 9pm UTC, which is 6am the next day in
        // Tokyo (UTC+9) — the canonical "5pm EST in Tokyo" example from the
        // feature brief, using EDT since July is inside US DST.
        let request = ConvertRequest {
            zone_id: "America/New_York".into(),
            local_datetime: "2026-07-15T17:00".into(),
        };
        let results = convert(
            &request,
            &["Asia/Tokyo".to_string(), "Europe/London".to_string()],
        )
        .unwrap();

        let tokyo = results.iter().find(|r| r.id == "Asia/Tokyo").unwrap();
        assert_eq!(tokyo.local_iso, "2026-07-16T06:00");
        assert_eq!(tokyo.day_offset, 1);

        // London (BST, UTC+1 in July) is 5 hours ahead of New York in summer.
        let london = results.iter().find(|r| r.id == "Europe/London").unwrap();
        assert_eq!(london.local_iso, "2026-07-15T22:00");
        assert_eq!(london.day_offset, 0);
    }

    #[test]
    fn converting_to_the_same_zone_is_the_identity() {
        let request = ConvertRequest {
            zone_id: "Etc/UTC".into(),
            local_datetime: "2026-07-15T12:00".into(),
        };
        let results = convert(&request, &["Etc/UTC".to_string()]).unwrap();
        assert_eq!(results[0].local_iso, "2026-07-15T12:00");
        assert_eq!(results[0].day_offset, 0);
    }

    // --- countdown timer expiry arithmetic ------------------------------------

    #[test]
    fn remaining_time_counts_down_from_the_deadline() {
        let start = Instant::now();
        let timer = CountdownTimer {
            id: 1,
            name: "Pasta".into(),
            deadline: start + Duration::from_secs(60),
            total: Duration::from_secs(60),
            completed: false,
        };
        let snap = timer_snapshot(&timer, start + Duration::from_secs(21));
        assert_eq!(snap.remaining_secs, 39);
        assert!(!snap.completed);
    }

    #[test]
    fn a_timer_past_its_deadline_reads_as_completed_even_before_the_reaper_marks_it() {
        let start = Instant::now();
        let timer = CountdownTimer {
            id: 1,
            name: "Eggs".into(),
            deadline: start + Duration::from_secs(10),
            total: Duration::from_secs(10),
            completed: false,
        };
        let snap = timer_snapshot(&timer, start + Duration::from_secs(15));
        assert_eq!(snap.remaining_secs, 0);
        assert!(snap.completed);
    }

    #[test]
    fn remaining_seconds_round_up_rather_than_truncate() {
        let start = Instant::now();
        let timer = CountdownTimer {
            id: 1,
            name: "Tea".into(),
            deadline: start + Duration::from_millis(30_500),
            total: Duration::from_secs(31),
            completed: false,
        };
        // 30.5s left should read as 31, not 30 — truncating would make a
        // freshly started timer look like it had already lost half a second.
        let snap = timer_snapshot(&timer, start);
        assert_eq!(snap.remaining_secs, 31);
    }

    #[test]
    fn a_marked_completed_timer_reports_zero_regardless_of_its_deadline() {
        let start = Instant::now();
        let timer = CountdownTimer {
            id: 1,
            name: "Done already".into(),
            deadline: start + Duration::from_secs(3600),
            total: Duration::from_secs(3600),
            completed: true,
        };
        assert_eq!(timer_snapshot(&timer, start).remaining_secs, 0);
    }

    #[test]
    fn starting_a_timer_with_no_name_is_refused() {
        let runtime = TimekeepingRuntime::new();
        assert!(runtime.start_timer("   ".into(), 60).is_err());
    }

    #[test]
    fn starting_a_zero_length_timer_is_refused() {
        let runtime = TimekeepingRuntime::new();
        assert!(runtime.start_timer("Eggs".into(), 0).is_err());
    }

    #[test]
    fn a_timer_over_a_day_is_refused() {
        let runtime = TimekeepingRuntime::new();
        assert!(runtime
            .start_timer("Marathon".into(), MAX_TIMER_SECS + 1)
            .is_err());
    }

    #[test]
    fn a_started_timer_appears_in_the_list_with_the_full_duration_left() {
        let runtime = TimekeepingRuntime::new();
        let started = runtime.start_timer("Pasta".into(), 600).unwrap();
        assert_eq!(started.remaining_secs, 600);
        assert!(!started.completed);

        let listed = runtime.list_timers();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Pasta");
    }

    #[test]
    fn dismissing_a_timer_removes_it_from_the_list() {
        let runtime = TimekeepingRuntime::new();
        let started = runtime.start_timer("Eggs".into(), 60).unwrap();
        runtime.dismiss_timer(started.id);
        assert!(runtime.list_timers().is_empty());
    }

    #[test]
    fn a_real_but_brief_timer_actually_counts_down() {
        // The one place a real (short) sleep is worth it: proving the whole
        // pipeline — spawn, tick, read back — agrees with itself, not just
        // the pure arithmetic above. Well under the 50ms budget.
        let runtime = TimekeepingRuntime::new();
        runtime.start_timer("Blink".into(), 1).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        let remaining = runtime.list_timers()[0].remaining_secs;
        assert!(
            remaining <= 1,
            "expected the countdown to have moved, got {remaining}"
        );
    }

    // --- stopwatch -------------------------------------------------------------

    #[test]
    fn elapsed_time_accrues_while_running() {
        let start = Instant::now();
        let sw = Stopwatch {
            running: true,
            accumulated: Duration::ZERO,
            started_at: Some(start),
            laps: vec![],
        };
        let elapsed = stopwatch_elapsed(&sw, start + Duration::from_secs(5));
        assert_eq!(elapsed, Duration::from_secs(5));
    }

    #[test]
    fn elapsed_time_freezes_once_stopped() {
        let sw = Stopwatch {
            running: false,
            accumulated: Duration::from_secs(12),
            started_at: None,
            laps: vec![],
        };
        // "Now" moving on should not move a stopped watch's elapsed time.
        assert_eq!(
            stopwatch_elapsed(&sw, Instant::now()),
            Duration::from_secs(12)
        );
        assert_eq!(
            stopwatch_elapsed(&sw, Instant::now() + Duration::from_secs(100)),
            Duration::from_secs(12)
        );
    }

    #[test]
    fn stopping_and_restarting_accumulates_across_the_gap() {
        let runtime = TimekeepingRuntime::new();
        runtime.stopwatch_start();
        std::thread::sleep(Duration::from_millis(15));
        let paused = runtime.stopwatch_stop();
        assert!(paused.elapsed_ms >= 15);
        assert!(!paused.running);

        runtime.stopwatch_start();
        std::thread::sleep(Duration::from_millis(15));
        let status = runtime.stopwatch_status();
        // Both stints should have counted, not just the second one.
        assert!(status.elapsed_ms >= paused.elapsed_ms + 10, "{status:?}");
    }

    #[test]
    fn laps_are_only_recorded_while_running() {
        let runtime = TimekeepingRuntime::new();
        // Not running yet: a lap here should not create a bogus zero entry.
        let idle_lap = runtime.stopwatch_lap();
        assert!(idle_lap.laps_ms.is_empty());

        runtime.stopwatch_start();
        std::thread::sleep(Duration::from_millis(10));
        let after_first_lap = runtime.stopwatch_lap();
        std::thread::sleep(Duration::from_millis(10));
        let after_second_lap = runtime.stopwatch_lap();

        assert_eq!(after_first_lap.laps_ms.len(), 1);
        assert_eq!(after_second_lap.laps_ms.len(), 2);
        // Laps are cumulative, so the second should be strictly later.
        assert!(after_second_lap.laps_ms[1] > after_second_lap.laps_ms[0]);
    }

    #[test]
    fn resetting_clears_time_and_laps() {
        let runtime = TimekeepingRuntime::new();
        runtime.stopwatch_start();
        std::thread::sleep(Duration::from_millis(10));
        runtime.stopwatch_lap();
        let status = runtime.stopwatch_reset();
        assert_eq!(status.elapsed_ms, 0);
        assert!(status.laps_ms.is_empty());
        assert!(!status.running);
    }

    // --- pomodoro cycle transitions ---------------------------------------------

    fn config(
        work: u32,
        short: u32,
        long: u32,
        cycles_before_long: u32,
        total: u32,
    ) -> PomodoroConfig {
        PomodoroConfig {
            work_minutes: work,
            short_break_minutes: short,
            long_break_minutes: long,
            cycles_before_long_break: cycles_before_long,
            total_cycles: total,
        }
    }

    #[test]
    fn work_is_followed_by_a_short_break_by_default() {
        let cfg = config(25, 5, 15, 4, 0);
        assert_eq!(advance(&cfg, Phase::Work, 1), Some((Phase::ShortBreak, 1)));
        assert_eq!(advance(&cfg, Phase::Work, 2), Some((Phase::ShortBreak, 2)));
        assert_eq!(advance(&cfg, Phase::Work, 3), Some((Phase::ShortBreak, 3)));
    }

    #[test]
    fn every_nth_work_session_is_followed_by_a_long_break() {
        let cfg = config(25, 5, 15, 4, 0);
        assert_eq!(advance(&cfg, Phase::Work, 4), Some((Phase::LongBreak, 4)));
        assert_eq!(advance(&cfg, Phase::Work, 8), Some((Phase::LongBreak, 8)));
    }

    #[test]
    fn a_break_always_advances_to_the_next_work_cycle() {
        let cfg = config(25, 5, 15, 4, 0);
        assert_eq!(advance(&cfg, Phase::ShortBreak, 2), Some((Phase::Work, 3)));
        assert_eq!(advance(&cfg, Phase::LongBreak, 4), Some((Phase::Work, 5)));
    }

    #[test]
    fn an_unbounded_run_never_ends_on_its_own() {
        let cfg = config(25, 5, 15, 4, 0);
        for cycle in 1..=20 {
            assert!(advance(&cfg, Phase::Work, cycle).is_some());
            assert!(advance(&cfg, Phase::ShortBreak, cycle).is_some());
        }
    }

    #[test]
    fn a_fixed_run_ends_after_its_final_work_session_without_a_trailing_break() {
        let cfg = config(25, 5, 15, 4, 4);
        assert_eq!(advance(&cfg, Phase::Work, 4), None);
        // Earlier work sessions in the same run still get their break.
        assert!(advance(&cfg, Phase::Work, 3).is_some());
    }

    #[test]
    fn a_fixed_run_stops_after_the_last_breaks_next_work_session_would_exceed_the_total() {
        let cfg = config(25, 5, 15, 4, 2);
        // Work 2 ends the run outright (>= total).
        assert_eq!(advance(&cfg, Phase::Work, 2), None);
        // A break after work 1 still leads into work 2, which is in range.
        assert_eq!(advance(&cfg, Phase::ShortBreak, 1), Some((Phase::Work, 2)));
    }

    #[test]
    fn cycles_before_long_break_of_zero_means_always_short() {
        let cfg = config(25, 5, 15, 0, 0);
        for cycle in 1..=10 {
            assert_eq!(
                advance(&cfg, Phase::Work, cycle),
                Some((Phase::ShortBreak, cycle))
            );
        }
    }

    #[test]
    fn starting_with_a_zero_length_phase_is_refused() {
        let runtime = TimekeepingRuntime::new();
        assert!(runtime.pomodoro_start(config(0, 5, 15, 4, 0)).is_err());
        assert!(runtime.pomodoro_start(config(25, 0, 15, 4, 0)).is_err());
        assert!(runtime.pomodoro_start(config(25, 5, 0, 4, 0)).is_err());
    }

    #[test]
    fn an_absurdly_long_phase_is_refused() {
        let runtime = TimekeepingRuntime::new();
        assert!(runtime.pomodoro_start(config(181, 5, 15, 4, 0)).is_err());
    }

    #[test]
    fn starting_a_session_begins_on_work_cycle_one() {
        let runtime = TimekeepingRuntime::new();
        let status = runtime.pomodoro_start(config(25, 5, 15, 4, 8)).unwrap();
        assert!(status.running);
        assert_eq!(status.phase, Some(Phase::Work));
        assert_eq!(status.cycle, 1);
        assert_eq!(status.total_secs, 25 * 60);
        assert_eq!(status.remaining_secs, 25 * 60);
    }

    #[test]
    fn stopping_a_session_clears_it() {
        let runtime = TimekeepingRuntime::new();
        runtime.pomodoro_start(config(25, 5, 15, 4, 0)).unwrap();
        let status = runtime.pomodoro_stop();
        assert!(!status.running);
        assert_eq!(status.phase, None);
    }

    #[test]
    fn starting_again_replaces_the_running_session_rather_than_stacking() {
        let runtime = TimekeepingRuntime::new();
        runtime.pomodoro_start(config(25, 5, 15, 4, 0)).unwrap();
        let restarted = runtime.pomodoro_start(config(50, 10, 20, 4, 0)).unwrap();
        assert_eq!(restarted.total_secs, 50 * 60);
        assert_eq!(restarted.cycle, 1);
    }

    #[test]
    fn idle_status_before_anything_starts_reports_not_running() {
        let runtime = TimekeepingRuntime::new();
        let status = runtime.pomodoro_status();
        assert!(!status.running);
        assert_eq!(status.remaining_secs, 0);
    }

    #[test]
    fn a_request_for_zero_total_cycles_is_clamped_rather_than_run_forever() {
        // `0` is still accepted — old callers may still send it — but a live
        // session must never actually be unbounded. This is the regression
        // test for the bug report itself: "0 = run until stopped by hand"
        // used to mean exactly that.
        let runtime = TimekeepingRuntime::new();
        let status = runtime.pomodoro_start(config(25, 5, 15, 4, 0)).unwrap();
        assert_eq!(status.total_cycles, MAX_TOTAL_CYCLES);
    }

    #[test]
    fn a_total_cycles_request_above_the_ceiling_is_refused_outright() {
        let runtime = TimekeepingRuntime::new();
        assert!(runtime
            .pomodoro_start(config(25, 5, 15, 4, MAX_TOTAL_CYCLES + 1))
            .is_err());
    }

    #[test]
    fn a_total_cycles_request_at_the_ceiling_is_accepted() {
        let runtime = TimekeepingRuntime::new();
        let status = runtime
            .pomodoro_start(config(25, 5, 15, 4, MAX_TOTAL_CYCLES))
            .unwrap();
        assert_eq!(status.total_cycles, MAX_TOTAL_CYCLES);
    }

    #[test]
    fn a_stopped_sessions_pending_reaper_notifies_and_refreshes_nothing() {
        // Models a reaper whose `tokio::time::sleep` was still pending when
        // `pomodoro_stop` ran — the exact race the generation guard exists
        // for. Calling `spawn_pomodoro_reaper` directly (rather than waiting
        // out a real phase) is what makes this expressible without a clock.
        let runtime = TimekeepingRuntime::new();
        let calls = Arc::new(AtomicU64::new(0));
        {
            let calls = Arc::clone(&calls);
            runtime.set_on_pomodoro_change(move || {
                calls.fetch_add(1, Ordering::SeqCst);
            });
        }

        runtime.pomodoro_start(config(25, 5, 15, 4, 0)).unwrap();
        runtime.pomodoro_stop();
        let after_start_and_stop = calls.load(Ordering::SeqCst);
        assert_eq!(after_start_and_stop, 2, "start and stop should each fire once");

        // The session is gone; a reaper for it — any generation — must find
        // nothing and quietly return before notifying or calling back.
        TimekeepingRuntime::spawn_pomodoro_reaper(
            Arc::clone(&runtime.pomodoro),
            0,
            Duration::from_millis(5),
            runtime.on_pomodoro_change.lock().clone(),
        );
        std::thread::sleep(Duration::from_millis(50));

        assert_eq!(
            calls.load(Ordering::SeqCst),
            after_start_and_stop,
            "a reaper for an already-stopped session must not fire the change callback"
        );
    }

    #[test]
    fn a_superseded_sessions_pending_reaper_leaves_the_live_session_untouched() {
        // Same race, but for a restart rather than a stop: the old session's
        // reaper wakes up after a *new* session has already replaced it.
        let runtime = TimekeepingRuntime::new();
        let calls = Arc::new(AtomicU64::new(0));
        {
            let calls = Arc::clone(&calls);
            runtime.set_on_pomodoro_change(move || {
                calls.fetch_add(1, Ordering::SeqCst);
            });
        }

        runtime.pomodoro_start(config(25, 5, 15, 4, 0)).unwrap();
        let live = runtime.pomodoro_start(config(50, 10, 20, 4, 0)).unwrap();
        let after_two_starts = calls.load(Ordering::SeqCst);
        assert_eq!(after_two_starts, 2);

        // `u64::MAX` stands in for "the first session's generation" — the
        // exact value does not matter, only that it cannot match whatever
        // generation the live (second) session actually has.
        TimekeepingRuntime::spawn_pomodoro_reaper(
            Arc::clone(&runtime.pomodoro),
            u64::MAX,
            Duration::from_millis(5),
            runtime.on_pomodoro_change.lock().clone(),
        );
        std::thread::sleep(Duration::from_millis(50));

        assert_eq!(
            calls.load(Ordering::SeqCst),
            after_two_starts,
            "a superseded reaper must not fire the change callback"
        );
        // And the live session's own state must be exactly as it started.
        let status = runtime.pomodoro_status();
        assert_eq!(status.total_secs, live.total_secs);
        assert_eq!(status.cycle, 1);
    }
}
