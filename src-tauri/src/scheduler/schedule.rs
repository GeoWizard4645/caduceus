//! The three schedule formats a job can run on, and computing when one next
//! fires.
//!
//! Matches what Hermes Agent's `cron.jobs.parse_schedule` accepts (see the
//! `scheduler` module doc for why that project is the reference here) —
//! deliberately not just 5-field cron:
//!
//! - `once` — an ISO-8601 timestamp, or a bare duration ("30m", "2h", "1d")
//!   meaning "once, that far from now".
//! - `interval` — `"every 30m"`, `"every 2h"`: fires repeatedly on a fixed
//!   cadence.
//! - `cron` — a literal 5-field crontab expression, parsed and walked by
//!   [`crate::tools::cron`] — that module is a describer/analyzer for a
//!   settings page, not a scheduler, but its field parser and its
//!   calendar-correct "next occurrence" walk are exactly what this needs, so
//!   they are reused rather than reimplemented. See that module's doc for why
//!   it operates in this machine's local time with no time zone of its own —
//!   this module makes the same choice, for the same reason.
//!
//! # Why every timestamp here is `DateTime<Local>`, not `NaiveDateTime`
//!
//! A stored instant (`next_run_at`, `last_run_at`, a `once` schedule's
//! `run_at`) needs to survive round-tripping through JSON and remain the same
//! real-world moment even if this machine's UTC offset changes in between —
//! daylight saving, or a laptop that travels. `DateTime<Local>` (an
//! unambiguous instant, serialized as RFC3339 with its offset) gives that.
//! Cron *matching* itself is necessarily naive-local-clock math (a crontab
//! entry means "9am", not "9am, but specifically UTC-4") — so
//! [`Schedule::compute_next_run`] converts to [`NaiveDateTime`] only for the
//! duration of that one calculation, then converts back via
//! [`local_from_naive_forward`], which is also where this module's DST
//! handling lives (see that function's doc).

use chrono::{DateTime, Duration, Local, LocalResult, NaiveDate, NaiveDateTime, TimeZone};
use serde::{Deserialize, Serialize};

use crate::tools::cron as cron_parser;

/// How late a `once` schedule's `run_at` may already be and still be allowed
/// to fire on the next tick, rather than being treated as missed. Covers the
/// ordinary case of "created a few seconds after the requested minute" —
/// e.g. a `run_now`-adjacent creation, or a tick that was a few seconds late
/// — without letting a schedule genuinely left for hours/days silently fire
/// the moment the app is reopened. Matches Hermes'
/// `cron.jobs.ONESHOT_GRACE_SECONDS`.
const ONCE_GRACE_SECONDS: i64 = 120;

/// A job's schedule, already parsed out of whatever string the user typed.
/// See the module doc for the three forms and [`parse`] for how a string
/// becomes one of these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Schedule {
    /// Fires once, at (or shortly after — see [`ONCE_GRACE_SECONDS`])
    /// `run_at`, then never again.
    Once { run_at: DateTime<Local> },
    /// Fires every `minutes` minutes, anchored to the previous firing (or to
    /// "now" for the very first one) rather than to a wall-clock grid — see
    /// [`Schedule::compute_next_run`].
    Interval { minutes: i64 },
    /// A literal 5-field crontab expression, validated by
    /// [`crate::tools::cron::parse`] at parse time so a broken expression is
    /// rejected when the job is created, not the first time the ticker tries
    /// to use it.
    Cron { expr: String },
}

impl Schedule {
    /// Compute the next time this schedule should fire, given `now` and —
    /// once the job has run at least once — `last_run_at`.
    ///
    /// `None` means "never again": a `Once` schedule that already ran, or
    /// whose `run_at` is more than [`ONCE_GRACE_SECONDS`] in the past. An
    /// `Interval` or `Cron` schedule always returns `Some` — a crontab
    /// expression that can truly never match again (day 31 of a month that
    /// never has one, combined with month-locking that rules out every month
    /// that does) is the one case [`cron_parser::next_occurrences`] itself
    /// can return empty for, which is treated the same as "never again"
    /// here rather than panicking.
    ///
    /// A pure function of its arguments — `now`/`last_run_at` are passed in
    /// rather than read from the wall clock — so every edge case (a DST
    /// transition, a month or year boundary, a leap day) is a deterministic
    /// unit test rather than something that only reproduces at a particular
    /// moment in real time.
    pub fn compute_next_run(
        &self,
        now: DateTime<Local>,
        last_run_at: Option<DateTime<Local>>,
    ) -> Option<DateTime<Local>> {
        match self {
            Schedule::Once { run_at } => {
                if last_run_at.is_some() {
                    return None; // one-shot, already spent
                }
                if *run_at >= now - Duration::seconds(ONCE_GRACE_SECONDS) {
                    Some(*run_at)
                } else {
                    None
                }
            }
            Schedule::Interval { minutes } => {
                // Anchored to the last *scheduled* firing, not to whenever
                // the run actually finished — otherwise a job whose run
                // takes a few seconds would drift a few seconds later every
                // time. First run (no last_run_at yet) is "now + interval":
                // a job is never due the instant it is created.
                let base = last_run_at.unwrap_or(now);
                Some(base + Duration::minutes(*minutes))
            }
            Schedule::Cron { expr } => {
                // Re-validated rather than assumed: `expr` was checked by
                // `parse` when the job was created, but this fn must still
                // degrade to "never again" rather than panic if a stored
                // record was ever hand-edited into something invalid.
                let parsed = cron_parser::parse(expr).ok()?;
                let base = last_run_at.unwrap_or(now);
                let next_naive = cron_parser::next_occurrences(&parsed, base.naive_local(), 1)
                    .into_iter()
                    .next()?;
                local_from_naive_forward(next_naive)
            }
        }
    }

    /// A human-readable sentence for the UI — "every 30 minutes", "once at
    /// 2026-08-10 14:00", or [`crate::tools::cron::describe`]'s sentence for
    /// a cron expression. Stored on the job as `schedule_display` at create
    /// / update time rather than recomputed on every read, so a job whose
    /// stored `Schedule` somehow becomes unparsable (a hand-edited file)
    /// still has *something* to show.
    pub fn describe(&self) -> String {
        match self {
            Schedule::Once { run_at } => format!("once at {}", run_at.format("%Y-%m-%d %H:%M")),
            Schedule::Interval { minutes } => describe_interval(*minutes),
            Schedule::Cron { expr } => cron_parser::parse(expr)
                .map(|p| cron_parser::describe(&p))
                .unwrap_or_else(|_| expr.clone()),
        }
    }
}

fn describe_interval(minutes: i64) -> String {
    let plural = |n: i64| if n == 1 { "" } else { "s" };
    if minutes > 0 && minutes % 1440 == 0 {
        let d = minutes / 1440;
        format!("every {d} day{}", plural(d))
    } else if minutes > 0 && minutes % 60 == 0 {
        let h = minutes / 60;
        format!("every {h} hour{}", plural(h))
    } else {
        format!("every {minutes} minute{}", plural(minutes))
    }
}

// ---------------------------------------------------------------------------
// Local-time reconstruction, and its DST edge cases
// ---------------------------------------------------------------------------

/// Turn a naive (zone-less) calendar moment back into a real instant in this
/// machine's local time zone, choosing a sane answer on both of daylight
/// saving's edge cases rather than the `None`/ambiguous result
/// [`chrono::TimeZone::from_local_datetime`] hands back by default:
///
/// - **Fall back** (a wall-clock hour repeats, e.g. 1:30am happens twice):
///   [`LocalResult::Ambiguous`] carries both instants, and the chronologically
///   earlier one is used (found by direct comparison, not by trusting either
///   position in the tuple — see the inline comment), so a job scheduled for
///   that time fires at the first opportunity rather than the second.
/// - **Spring forward** (a wall-clock hour is skipped, e.g. 2:30am never
///   happens on the day clocks jump from 2:00 to 3:00): naive-time cron
///   matching can still land on that nonexistent moment, and
///   [`LocalResult::None`] is chrono's honest answer that no such instant
///   exists. Rather than silently dropping the job's next run, this nudges
///   forward a minute at a time until it finds real wall-clock time again —
///   which, for a spring-forward gap, is exactly the jump itself (e.g.
///   2:30am → 3:00am). Bounded at two hours of nudging, comfortably past the
///   largest DST jump any real time zone uses, so a pathological input can
///   never turn this into an infinite loop.
fn local_from_naive_forward(naive: NaiveDateTime) -> Option<DateTime<Local>> {
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(dt) => Some(dt),
        // chrono documents this tuple as `(earliest, latest)`, but verified
        // against this crate's actual `Local` (tz-database-backed) behaviour
        // that ordering did not hold — a spot check at a real fall-back
        // transition came back with the *later* instant first. Comparing
        // directly rather than trusting either position sidesteps the
        // question of whether that was this platform, this chrono version,
        // or a misreading, and is correct regardless of the answer.
        LocalResult::Ambiguous(a, b) => Some(if a <= b { a } else { b }),
        LocalResult::None => {
            let mut candidate = naive;
            for _ in 0..120 {
                candidate += Duration::minutes(1);
                if let LocalResult::Single(dt) = Local.from_local_datetime(&candidate) {
                    return Some(dt);
                }
            }
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Parsing a schedule string
// ---------------------------------------------------------------------------

/// Parse whatever a user typed into one of the three [`Schedule`] forms.
///
/// Tried in this order:
///
/// 1. `"every <duration>"` → [`Schedule::Interval`].
/// 2. Five whitespace-separated fields, each built only from the characters
///    a cron field can use → [`Schedule::Cron`], validated immediately via
///    [`cron_parser::parse`] so a typo is rejected here rather than the
///    first time the ticker tries to use it. Unlike Hermes' own heuristic
///    (which only recognises a cron candidate when every field is digits/
///    `*-,/` and therefore never routes a name-bearing expression like
///    `"0 9 * * MON-FRI"` to its cron branch at all), this step's character
///    check allows letters too, because `cron_parser` — unlike Hermes'
///    `croniter` — supports month/weekday names as a first-class feature
///    and there is no reason to hide that. The trade-off: five
///    space-separated words that are not actually meant as cron (and happen
///    to contain nothing but letters, digits, `*-,/`) are reported as an
///    invalid cron expression rather than "not a schedule". In practice
///    nobody types five bare words as a schedule, so this reads better than
///    it costs.
/// 3. Something that looks like an ISO-8601 timestamp (contains `T`, or
///    starts `YYYY-MM-DD`) → [`Schedule::Once`] at that instant.
/// 4. A bare duration ("30m", "2h", "1d") → [`Schedule::Once`], that far
///    from `now`.
///
/// `now` is a parameter (not read from the wall clock) purely for
/// testability — see [`Schedule::compute_next_run`]'s doc for the same
/// reasoning.
pub fn parse(input: &str, now: DateTime<Local>) -> Result<Schedule, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(
            "Give the job a schedule — try \"every 30m\", a cron expression, or a time.".into(),
        );
    }

    if let Some(rest) = strip_every_prefix(trimmed) {
        let minutes = parse_duration_minutes(rest.trim())?;
        return Ok(Schedule::Interval { minutes });
    }

    let fields: Vec<&str> = trimmed.split_whitespace().collect();
    if fields.len() == 5 && fields.iter().all(|f| is_cron_field(f)) {
        cron_parser::parse(trimmed).map_err(|e| format!("Invalid cron expression: {e}"))?;
        return Ok(Schedule::Cron {
            expr: trimmed.to_string(),
        });
    }

    if looks_like_timestamp(trimmed) {
        let run_at = parse_timestamp(trimmed)?;
        return Ok(Schedule::Once { run_at });
    }

    match parse_duration_minutes(trimmed) {
        Ok(minutes) => Ok(Schedule::Once {
            run_at: now + Duration::minutes(minutes),
        }),
        Err(_) => Err(format!(
            "\"{trimmed}\" is not a schedule Caduceus understands. Try a duration (\"30m\", \
             \"2h\", \"1d\"), \"every 30m\" for a recurring interval, a 5-field cron expression \
             (\"0 9 * * *\"), or an ISO timestamp (\"2026-08-10T14:00\")."
        )),
    }
}

fn strip_every_prefix(s: &str) -> Option<&str> {
    // Case-insensitive on just the one keyword, not the whole string — a
    // duration unit like "M" for minutes is deliberately still
    // case-sensitive-agnostic further down in `parse_duration_minutes`, but
    // "every" itself reads fine typed either way.
    if s.len() < 6 || !s.is_char_boundary(6) {
        return None;
    }
    let (head, tail) = s.split_at(6);
    if head.eq_ignore_ascii_case("every ") {
        Some(tail)
    } else {
        None
    }
}

/// Cron's own character set, widened to letters so month/weekday *names*
/// (`JAN`, `MON`) are recognised as cron rather than falling through — see
/// [`parse`]'s doc.
fn is_cron_field(f: &str) -> bool {
    !f.is_empty()
        && f.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '*' | '-' | ',' | '/'))
}

fn looks_like_timestamp(s: &str) -> bool {
    if s.contains('T') || s.contains('t') {
        return true;
    }
    let b = s.as_bytes();
    b.len() >= 5 && b[..4].iter().all(u8::is_ascii_digit) && b[4] == b'-'
}

/// Parse an ISO-8601-ish timestamp. Not a general ISO-8601 parser — it
/// covers the shapes someone actually types or a `<input type="datetime-local">`
/// actually sends: an offset/`Z`-qualified instant, or a naive
/// `YYYY-MM-DD[T ]HH:MM[:SS]` / bare `YYYY-MM-DD` interpreted in this
/// machine's local zone (see [`local_from_naive_forward`] for how that
/// conversion handles a DST edge).
fn parse_timestamp(s: &str) -> Result<DateTime<Local>, String> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Local));
    }

    let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M"))
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S"))
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M"))
        .or_else(|_| {
            NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .map(|d| d.and_hms_opt(0, 0, 0).expect("midnight is always valid"))
        })
        .map_err(|_| format!("\"{s}\" is not a timestamp Caduceus understands."))?;

    local_from_naive_forward(naive)
        .ok_or_else(|| format!("\"{s}\" falls in a daylight-saving gap and never occurs."))
}

/// Parse a duration like `"30m"`, `"2h"`, `"1d"` into a minute count.
/// Mirrors Hermes' `cron.jobs.parse_duration`'s accepted unit spellings.
fn parse_duration_minutes(s: &str) -> Result<i64, String> {
    let lower = s.trim().to_ascii_lowercase();
    let split_at = lower
        .find(|c: char| !c.is_ascii_digit())
        .filter(|&i| i > 0) // must have at least one digit
        .ok_or_else(|| format!("\"{s}\" is not a duration — try \"30m\", \"2h\", or \"1d\"."))?;
    let (digits, unit) = lower.split_at(split_at);
    let value: i64 = digits
        .parse()
        .map_err(|_| format!("\"{s}\" is not a duration — try \"30m\", \"2h\", or \"1d\"."))?;
    if value <= 0 {
        return Err("A duration must be a positive number.".into());
    }
    let minutes_per = match unit.trim() {
        "m" | "min" | "mins" | "minute" | "minutes" => 1,
        "h" | "hr" | "hrs" | "hour" | "hours" => 60,
        "d" | "day" | "days" => 1440,
        other => {
            return Err(format!(
                "\"{other}\" is not a duration unit Caduceus understands — use m, h, or d, e.g. \
                 \"30m\", \"2h\", \"1d\"."
            ))
        }
    };
    Ok(value * minutes_per)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        local_from_naive_forward(
            NaiveDate::from_ymd_opt(y, mo, d)
                .unwrap()
                .and_hms_opt(h, mi, 0)
                .unwrap(),
        )
        .expect("test fixture times must be real local instants")
    }

    // -----------------------------------------------------------------
    // parse() — all three forms, plus rejection
    // -----------------------------------------------------------------

    #[test]
    fn a_bare_duration_is_a_one_shot_that_many_minutes_from_now() {
        let now = local(2026, 6, 15, 12, 0);
        let s = parse("30m", now).unwrap();
        assert_eq!(s, Schedule::Once { run_at: now + Duration::minutes(30) });
    }

    #[test]
    fn duration_units_hours_and_days_both_convert_to_minutes() {
        let now = local(2026, 6, 15, 12, 0);
        assert_eq!(parse("2h", now).unwrap(), Schedule::Once { run_at: now + Duration::minutes(120) });
        assert_eq!(parse("1d", now).unwrap(), Schedule::Once { run_at: now + Duration::minutes(1440) });
    }

    #[test]
    fn every_prefix_is_a_recurring_interval() {
        let now = local(2026, 6, 15, 12, 0);
        assert_eq!(parse("every 30m", now).unwrap(), Schedule::Interval { minutes: 30 });
        assert_eq!(parse("Every 2h", now).unwrap(), Schedule::Interval { minutes: 120 });
    }

    #[test]
    fn five_numeric_fields_are_parsed_as_cron() {
        let now = local(2026, 6, 15, 12, 0);
        assert_eq!(
            parse("0 9 * * 1-5", now).unwrap(),
            Schedule::Cron { expr: "0 9 * * 1-5".into() }
        );
    }

    #[test]
    fn cron_fields_using_month_and_weekday_names_are_still_recognised_as_cron() {
        // Deliberately wider than Hermes' own digit-only heuristic — see
        // `parse`'s doc.
        let now = local(2026, 6, 15, 12, 0);
        assert_eq!(
            parse("0 9 1 JAN MON", now).unwrap(),
            Schedule::Cron { expr: "0 9 1 JAN MON".into() }
        );
    }

    #[test]
    fn an_invalid_cron_expression_is_rejected_at_parse_time() {
        let now = local(2026, 6, 15, 12, 0);
        let err = parse("99 99 * * *", now).unwrap_err();
        assert!(err.contains("Invalid cron expression"));
    }

    #[test]
    fn an_iso_timestamp_with_t_is_a_one_shot() {
        let now = local(2026, 1, 1, 0, 0);
        let s = parse("2026-08-10T14:00:00", now).unwrap();
        assert_eq!(s, Schedule::Once { run_at: local(2026, 8, 10, 14, 0) });
    }

    #[test]
    fn a_bare_date_is_midnight_that_day() {
        let now = local(2026, 1, 1, 0, 0);
        let s = parse("2026-08-10", now).unwrap();
        assert_eq!(s, Schedule::Once { run_at: local(2026, 8, 10, 0, 0) });
    }

    #[test]
    fn an_offset_qualified_timestamp_converts_into_local_time() {
        let now = local(2026, 1, 1, 0, 0);
        let s = parse("2026-08-10T14:00:00Z", now).unwrap();
        let Schedule::Once { run_at } = s else { panic!("expected Once") };
        // Whatever this machine's local offset is, the UTC instant must match.
        assert_eq!(run_at.with_timezone(&chrono::Utc).to_rfc3339(), "2026-08-10T14:00:00+00:00");
    }

    #[test]
    fn garbage_input_is_rejected_with_a_helpful_message() {
        let now = local(2026, 6, 15, 12, 0);
        let err = parse("banana", now).unwrap_err();
        assert!(err.contains("not a schedule Caduceus understands"));
    }

    #[test]
    fn five_bare_words_are_reported_as_invalid_cron_not_as_not_a_schedule() {
        // Pins the documented trade-off in `parse`'s doc: because the cron
        // branch's field check allows letters (so JAN/MON are recognised),
        // five whitespace-separated all-alphabetic words that were never
        // meant as cron still route there and come back as an invalid-cron
        // error rather than the generic "not a schedule" message. Written
        // as its own test — rather than left purely as prose in the doc
        // comment — specifically because a previous version of *this test
        // file* picked exactly this shape of string
        // (`"whenever I feel like it"`) for the *generic* garbage-input
        // test above and failed for this exact reason; pinning the
        // behaviour here means the next surprise is a clear test name, not
        // a confusing assertion failure in an unrelated test.
        let now = local(2026, 6, 15, 12, 0);
        let err = parse("whenever I feel like it", now).unwrap_err();
        assert!(err.contains("Invalid cron expression"), "got: {err}");
    }

    #[test]
    fn an_empty_schedule_is_rejected_before_anything_else() {
        let now = local(2026, 6, 15, 12, 0);
        assert!(parse("   ", now).unwrap_err().contains("Give the job a schedule"));
    }

    // -----------------------------------------------------------------
    // compute_next_run — Once
    // -----------------------------------------------------------------

    #[test]
    fn a_once_schedule_that_has_not_run_yet_fires_at_run_at() {
        let run_at = local(2026, 8, 10, 14, 0);
        let s = Schedule::Once { run_at };
        assert_eq!(s.compute_next_run(local(2026, 8, 10, 13, 0), None), Some(run_at));
    }

    #[test]
    fn a_once_schedule_that_already_ran_never_runs_again() {
        let run_at = local(2026, 8, 10, 14, 0);
        let s = Schedule::Once { run_at };
        let last_run = local(2026, 8, 10, 14, 0);
        assert_eq!(s.compute_next_run(local(2026, 8, 10, 15, 0), Some(last_run)), None);
    }

    #[test]
    fn a_once_schedule_within_the_grace_window_still_fires() {
        let run_at = local(2026, 8, 10, 14, 0);
        let s = Schedule::Once { run_at };
        let now = run_at + Duration::seconds(90); // inside the 120s grace window
        assert_eq!(s.compute_next_run(now, None), Some(run_at));
    }

    #[test]
    fn a_once_schedule_long_past_is_abandoned_not_fast_forwarded() {
        let run_at = local(2026, 8, 10, 14, 0);
        let s = Schedule::Once { run_at };
        let now = run_at + Duration::hours(3);
        assert_eq!(s.compute_next_run(now, None), None);
    }

    // -----------------------------------------------------------------
    // compute_next_run — Interval, including a month boundary
    // -----------------------------------------------------------------

    #[test]
    fn an_interval_schedules_first_run_relative_to_now_not_immediately() {
        let s = Schedule::Interval { minutes: 30 };
        let now = local(2026, 8, 10, 14, 0);
        assert_eq!(s.compute_next_run(now, None), Some(now + Duration::minutes(30)));
    }

    #[test]
    fn an_interval_schedules_off_the_last_run_not_off_now() {
        // Anchoring to last_run_at (not "now") means a slow run does not
        // shrink the gap before the following one.
        let s = Schedule::Interval { minutes: 30 };
        let last_run = local(2026, 8, 10, 14, 0);
        let now = last_run + Duration::minutes(5); // the run took 5 minutes
        assert_eq!(s.compute_next_run(now, Some(last_run)), Some(last_run + Duration::minutes(30)));
    }

    #[test]
    fn an_interval_correctly_crosses_a_month_boundary() {
        let s = Schedule::Interval { minutes: 20 };
        let last_run = local(2026, 1, 31, 23, 50);
        let next = s.compute_next_run(last_run, Some(last_run)).unwrap();
        assert_eq!(next, local(2026, 2, 1, 0, 10));
    }

    #[test]
    fn an_interval_correctly_crosses_a_year_boundary() {
        let s = Schedule::Interval { minutes: 30 };
        let last_run = local(2026, 12, 31, 23, 45);
        let next = s.compute_next_run(last_run, Some(last_run)).unwrap();
        assert_eq!(next, local(2027, 1, 1, 0, 15));
    }

    // -----------------------------------------------------------------
    // compute_next_run — Cron, including DST and a leap day
    // -----------------------------------------------------------------

    #[test]
    fn a_cron_schedule_crosses_a_month_boundary_via_the_shared_walker() {
        let s = Schedule::Cron { expr: "0 9 1 * *".into() }; // 9am on the 1st of every month
        let now = local(2026, 1, 15, 0, 0);
        assert_eq!(s.compute_next_run(now, None), Some(local(2026, 2, 1, 9, 0)));
    }

    #[test]
    fn a_cron_schedule_finds_the_next_leap_day() {
        let s = Schedule::Cron { expr: "0 0 29 2 *".into() };
        let now = local(2024, 3, 1, 0, 0);
        assert_eq!(s.compute_next_run(now, None), Some(local(2028, 2, 29, 0, 0)));
    }

    #[test]
    fn a_cron_schedule_lands_correctly_across_the_spring_forward_gap() {
        // This machine's local zone loses the 2:00-3:00am hour on 2024-03-10
        // (verified against this exact zone before writing this test).
        // A job due for a naive time inside the gap must land on a real
        // instant on or after it, not silently vanish.
        let s = Schedule::Cron { expr: "30 2 10 3 *".into() }; // 2:30am on March 10th
        let now = local(2024, 1, 1, 0, 0);
        let next = s.compute_next_run(now, None);
        assert!(next.is_some(), "a DST gap must not make the job vanish");
        let next = next.unwrap();
        assert_eq!(next.naive_local().date(), NaiveDate::from_ymd_opt(2024, 3, 10).unwrap());
        assert!(
            next.naive_local().time() >= chrono::NaiveTime::from_hms_opt(3, 0, 0).unwrap(),
            "2:30am does not exist that day; expected the first real instant at/after it, got {next}"
        );
    }

    #[test]
    fn a_cron_schedule_across_the_fall_back_overlap_picks_the_earlier_instant() {
        // This machine's local zone repeats 1:00-2:00am on 2024-11-03.
        let s = Schedule::Cron { expr: "30 1 3 11 *".into() }; // 1:30am on November 3rd
        let now = local(2024, 1, 1, 0, 0);
        let next = s.compute_next_run(now, None).unwrap();
        assert_eq!(next.naive_local(), NaiveDate::from_ymd_opt(2024, 11, 3).unwrap().and_hms_opt(1, 30, 0).unwrap());
        // The earlier of the two real instants that share that wall-clock time.
        assert_eq!(next.offset().local_minus_utc(), -4 * 3600);
    }

    // -----------------------------------------------------------------
    // describe()
    // -----------------------------------------------------------------

    #[test]
    fn describe_reads_naturally_for_every_kind() {
        assert_eq!(Schedule::Interval { minutes: 90 }.describe(), "every 90 minutes");
        assert_eq!(Schedule::Interval { minutes: 60 }.describe(), "every 1 hour");
        assert_eq!(Schedule::Interval { minutes: 1440 }.describe(), "every 1 day");
        assert_eq!(
            Schedule::Once { run_at: local(2026, 8, 10, 14, 0) }.describe(),
            "once at 2026-08-10 14:00"
        );
        assert_eq!(Schedule::Cron { expr: "30 2 * * *".into() }.describe(), "At 02:30");
    }

    // -----------------------------------------------------------------
    // parse_duration_minutes / unit coverage
    // -----------------------------------------------------------------

    #[test]
    fn duration_unit_spellings_all_resolve() {
        for (s, expected) in [
            ("5min", 5), ("5mins", 5), ("5minute", 5), ("5minutes", 5),
            ("2hr", 120), ("2hrs", 120), ("2hour", 120), ("2hours", 120),
            ("1day", 1440), ("1days", 1440),
        ] {
            assert_eq!(parse_duration_minutes(s).unwrap(), expected, "input was {s:?}");
        }
    }

    #[test]
    fn a_zero_or_negative_duration_is_rejected() {
        assert!(parse_duration_minutes("0m").is_err());
    }

    #[test]
    fn an_unknown_duration_unit_is_rejected() {
        assert!(parse_duration_minutes("5weeks").is_err());
    }
}
