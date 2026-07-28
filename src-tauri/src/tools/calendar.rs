//! Calendar events and Reminders, via AppleScript.
//!
//! Same shape as `notes.rs` and `media.rs`: the only supported way into
//! Calendar.app and Reminders.app is their scripting dictionary, so every
//! write here is an AppleScript `make new event`/`make new reminder`, and
//! every read is an AppleScript query. All of it goes through
//! [`super::apple::run_script`] rather than a fresh `osascript` call, which
//! buys three things for free: the piped-stdin transport (so a note body or
//! event title containing a literal `"` never has to survive a shell's
//! quoting rules on top of AppleScript's), the wedged-app timeout, and —
//! critically for requirement 5 of this module — the translated Automation
//! error. The very first time Caduceus asks Calendar or Reminders to do
//! anything, macOS refuses with `-1743` and a prompt appears; `apple::
//! run_script` turns that into the exact sentence
//! `src-tauri/src/tools/apple.rs::translate` produces for every other tool in
//! this crate, which is what lets the app's permission-gate machinery (see
//! that module's doc comment on `translate`) recognise the failure and offer
//! the System Settings walkthrough instead of a bare error number.
//!
//! # AppleScript injection
//!
//! Every user-supplied string that ends up inside a `"…"` AppleScript literal
//! — an event title, a location, notes, a reminder's text — is passed through
//! [`crate::shortcuts::escape_applescript`] first. Skipping that for "just a
//! title" is exactly the mistake this codebase has fixed before (see
//! `notes.rs` and `timekeeping.rs`, which carry the same warning): a title of
//! `Lunch" & (do shell script "rm -rf ~") & "` closes the literal early and
//! the rest of it parses as AppleScript source, and `do shell script` makes
//! that arbitrary command execution, not just a garbled note. The tests below
//! (`a_title_with_quotes_and_backslashes_cannot_break_out_of_the_applescript_literal`
//! and friends) build the real script text — not just the escaper — and
//! assert the malicious payload never appears unescaped in it.
//!
//! # Dates: numeric fields, never a `date "…"` string literal
//!
//! AppleScript's `date "July 28, 2026 3:00 PM"` literal is parsed against
//! *the Mac's own* system date/time format preference at script-run time, not
//! against a fixed grammar. The identical script produces a different day on
//! a Mac configured for day-first dates, and silently misreads on some
//! locales rather than erroring — which is precisely the "silently
//! mis-scheduled meeting" this feature was told to avoid. Every date this
//! module hands to Calendar or Reminders is instead built by taking a fresh
//! `current date` and overwriting its `year`/`month`/`day`/`hours`/`minutes`
//! fields one at a time (see [`applescript_set_date`]) — those are plain
//! numeric property sets, immune to locale, and the technique is the
//! standard workaround for exactly this AppleScript footgun.
//!
//! # Reading without launching
//!
//! Calendar.app must not be *launched* just to answer "what's on my agenda
//! today" — a read that pops the Dock icon and spins up a whole GUI app the
//! user never asked to open is a worse experience than the read failing. The
//! `it is running` test AppleScript exposes (used the same way `qr.rs` uses
//! it for Safari/Chromium, and `media.rs` uses `System Events`'s process list
//! for) is special-cased by AppleScript's own dispatch to answer without
//! starting the target process, so [`events_between`] wraps its whole query in
//! `if it is running`. When Calendar is not already open, the read comes back
//! as [`Err`] rather than a silently empty agenda — an empty list and "I
//! didn't check" are different facts, and only one of them is true here.
//!
//! This guard applies **only** to reads. [`create_event`] and
//! [`create_reminder`] are writes: `make new event`/`make new reminder` is an
//! Apple Event that has to reach a running process, so AppleScript starts
//! Calendar/Reminders if needed the same way opening either app from the Dock
//! would. That is not "launched merely to read" — it is launched to actually
//! do the thing the user asked for, which is the one case launching is the
//! right call.

use chrono::{Datelike, Duration as ChronoDuration, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Weekday};
use serde::Serialize;

use super::apple;
use crate::shortcuts::escape_applescript;

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// What `create_event` actually filed, echoed back so the UI can confirm it
/// rather than just trusting the request succeeded.
///
/// `start`/`end` are `YYYY-MM-DDTHH:MM` in the Mac's local time zone — the
/// same wall-clock instant that was written into Calendar, not re-derived
/// from anything AppleScript sent back. Calendar's own `date ... as string`
/// formatting is exactly the locale trap documented on [`applescript_set_date`],
/// so this module never round-trips a date through it; only `calendar` (a
/// plain name) comes back from the script.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedEvent {
    pub title: String,
    pub start: String,
    pub end: String,
    pub location: String,
    pub notes: String,
    /// The name of the calendar the event actually landed in — see
    /// [`build_create_event_script`] for why this is "the first writable
    /// calendar" rather than a calendar this module lets the caller name.
    pub calendar: String,
}

/// One event as read back from Calendar for an agenda view.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEvent {
    pub title: String,
    pub start: String,
    pub end: String,
    pub location: String,
    pub calendar: String,
    pub all_day: bool,
}

/// What `create_reminder` actually filed.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedReminder {
    pub text: String,
    /// `YYYY-MM-DDTHH:MM`, or `None` for a reminder with no due date.
    pub due: Option<String>,
    pub list: String,
}

// ---------------------------------------------------------------------------
// AppleScript date construction
// ---------------------------------------------------------------------------

/// AppleScript statements that turn a fresh `current date` into an exact,
/// locale-proof wall-clock instant.
///
/// `day` is force-set to `1` *before* the year and month are written, and
/// only set to the real target day afterward. Without that first step,
/// writing a new month onto a `current date` that currently sits on, say,
/// the 31st can ask AppleScript for a day that does not exist in the
/// in-between state (setting the month to February while `day` is still 31)
/// and it errors instead of just landing wherever "the 31st of February"
/// would round to. Pinning `day` to `1` first means every intermediate state
/// is a real calendar date.
///
/// `var` is always one of this module's own script-local identifiers
/// (`startDate`, `rangeStart`, …), never user text, so it is interpolated
/// as-is.
fn applescript_set_date(var: &str, dt: NaiveDateTime) -> String {
    format!(
        "set {var} to current date\n\
         set day of {var} to 1\n\
         set year of {var} to {y}\n\
         set month of {var} to {mo}\n\
         set day of {var} to {d}\n\
         set hours of {var} to {h}\n\
         set minutes of {var} to {mi}\n\
         set seconds of {var} to 0\n",
        var = var,
        y = dt.year(),
        mo = dt.month(),
        d = dt.day(),
        h = dt.hour(),
        mi = dt.minute(),
    )
}

/// An AppleScript handler that turns one of the date objects Calendar hands
/// back into `YYYY-MM-DDTHH:MM`, computed from its numeric fields.
///
/// Reusing [`applescript_set_date`]'s reasoning in reverse: `date ... as
/// string` is locale-formatted text, not a fixed shape, so parsing it back in
/// Rust would be exactly the same footgun this module avoids on the way in.
/// Reading `year`/`month`/`day`/`hours`/`minutes` as numbers and formatting
/// them by hand sidesteps that entirely. `text -2 thru -1 of ("0" & n)` is
/// AppleScript's idiom for zero-padding to two digits: prefixing a one- or
/// two-digit number with `"0"` and keeping only the last two characters
/// always yields two digits either way.
const ISO_DATE_HANDLER: &str = r#"on isoDate(d)
    set y to year of d
    set mo to (month of d) as integer
    set dy to day of d
    set h to hours of d
    set mi to minutes of d
    set moStr to text -2 thru -1 of ("0" & mo)
    set dyStr to text -2 thru -1 of ("0" & dy)
    set hStr to text -2 thru -1 of ("0" & h)
    set miStr to text -2 thru -1 of ("0" & mi)
    return (y as string) & "-" & moStr & "-" & dyStr & "T" & hStr & ":" & miStr
end isoDate
"#;

// ---------------------------------------------------------------------------
// Creating an event
// ---------------------------------------------------------------------------

const DEFAULT_DURATION_MINUTES: i64 = 60;

/// Resolve the caller's requested duration, or the default, rejecting
/// anything that would make `end <= start`.
///
/// Pulled out of `create_event` as its own pure function so the "no duration
/// given" and "zero or negative duration" behaviour is testable without
/// touching AppleScript at all.
fn resolve_duration(duration_minutes: Option<i64>) -> Result<i64, String> {
    let minutes = duration_minutes.unwrap_or(DEFAULT_DURATION_MINUTES);
    if minutes <= 0 {
        return Err("An event's duration has to be longer than zero minutes.".into());
    }
    Ok(minutes)
}

/// Build the `osascript` source for [`create_event`], without running it.
///
/// Separated out so the injection tests can inspect the exact script text a
/// hostile title produces, rather than only exercising the escaper in
/// isolation.
///
/// The target calendar is `first calendar whose writable is true`, not a
/// `default calendar` property — Calendar's own scripting dictionary (dumped
/// with `sdef /System/Applications/Calendar.app` while writing this against
/// a real Mac) has no such property; that name only ever existed for
/// Reminders' `default list`, and assuming Calendar had the same shape by
/// analogy was wrong. "First writable calendar" is the closest available
/// stand-in for "the calendar new events should go in" — it deliberately
/// skips read-only subscribed calendars (holidays, a shared calendar someone
/// else owns), which a real "make new event" would refuse to write into
/// anyway.
fn build_create_event_script(
    title: &str,
    location: &str,
    notes: &str,
    start: NaiveDateTime,
    duration_minutes: i64,
) -> String {
    format!(
        r#"{start_setup}set endDate to startDate + ({duration_minutes} * minutes)
tell application "Calendar"
    set targetCal to first calendar whose writable is true
    tell targetCal
        make new event with properties {{summary:"{title}", start date:startDate, end date:endDate, location:"{location}", description:"{notes}"}}
    end tell
    return name of targetCal
end tell"#,
        start_setup = applescript_set_date("startDate", start),
        duration_minutes = duration_minutes,
        title = escape_applescript(title),
        location = escape_applescript(location),
        notes = escape_applescript(notes),
    )
}

/// Create a Calendar event from a natural-language `when`, returning what was
/// actually scheduled so the caller can show a confirmation instead of just
/// trusting the request went through.
///
/// `when` is parsed by [`parse_when`] — offline, instantly, no model call —
/// against the Mac's current local time. `duration_minutes` defaults to 60.
/// `location`/`notes` may be empty strings or `None`; both are optional in
/// Calendar's own dictionary.
pub fn create_event(
    title: &str,
    when: &str,
    duration_minutes: Option<i64>,
    location: Option<&str>,
    notes: Option<&str>,
) -> Result<CreatedEvent, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("Give the event a title.".into());
    }

    let minutes = resolve_duration(duration_minutes)?;
    let now = chrono::Local::now().naive_local();
    let start = parse_when(when, now)?;
    let end = start + ChronoDuration::minutes(minutes);

    let location = location.unwrap_or("").trim();
    let notes = notes.unwrap_or("").trim();

    let script = build_create_event_script(title, location, notes, start, minutes);
    let calendar_name = apple::run_script(&script)?;

    Ok(CreatedEvent {
        title: title.to_string(),
        start: start.format("%Y-%m-%dT%H:%M").to_string(),
        end: end.format("%Y-%m-%dT%H:%M").to_string(),
        location: location.to_string(),
        notes: notes.to_string(),
        calendar: calendar_name.trim().to_string(),
    })
}

// ---------------------------------------------------------------------------
// Reading the agenda
// ---------------------------------------------------------------------------

/// Field/record separators for the one blob of text `osascript` hands back
/// for a whole agenda query.
///
/// Control characters that are vanishingly unlikely to appear in a real event
/// title (unlike a comma, pipe, or tab, all of which people genuinely type),
/// so no escaping scheme is needed on the way out — Calendar's own text
/// fields are simply never going to contain `\u{1f}` or `\u{1e}`.
const FIELD_SEP: char = '\u{1f}';
const RECORD_SEP: char = '\u{1e}';

/// Build the `osascript` source for an agenda query over `[start, end)`.
fn build_events_between_script(start: NaiveDateTime, end: NaiveDateTime) -> String {
    format!(
        r#"{iso_handler}{range_start}{range_end}tell application "Calendar"
    if it is running then
        set out to ""
        repeat with cal in calendars
            set evts to (every event of cal whose start date ≥ rangeStart and start date < rangeEnd)
            repeat with e in evts
                set evtLoc to location of e
                if evtLoc is missing value then set evtLoc to ""
                set out to out & (summary of e) & "{fs}" & my isoDate(start date of e) & "{fs}" & my isoDate(end date of e) & "{fs}" & evtLoc & "{fs}" & (name of cal) & "{fs}" & (allday event of e as string) & "{rs}"
            end repeat
        end repeat
        return out
    else
        return "NOT_RUNNING"
    end if
end tell"#,
        iso_handler = ISO_DATE_HANDLER,
        range_start = applescript_set_date("rangeStart", start),
        range_end = applescript_set_date("rangeEnd", end),
        fs = FIELD_SEP,
        rs = RECORD_SEP,
    )
}

/// Parse the field/record-separated blob [`build_events_between_script`]'s
/// script returns into [`CalendarEvent`]s.
///
/// A pure function on purpose: it is the part of the read path most worth
/// unit-testing directly, since nothing about it depends on AppleScript
/// actually running.
fn parse_events(raw: &str) -> Vec<CalendarEvent> {
    raw.split(RECORD_SEP)
        .map(str::trim)
        .filter(|record| !record.is_empty())
        .filter_map(|record| {
            let fields: Vec<&str> = record.split(FIELD_SEP).collect();
            if fields.len() < 6 {
                return None;
            }
            Some(CalendarEvent {
                title: fields[0].to_string(),
                start: fields[1].to_string(),
                end: fields[2].to_string(),
                location: fields[3].to_string(),
                calendar: fields[4].to_string(),
                all_day: fields[5].trim() == "true",
            })
        })
        .collect()
}

/// Every event across every subscribed calendar (including read-only and
/// holiday calendars) that starts in `[start, end)`.
///
/// Calendar.app is never launched to serve this — see the module doc's
/// "Reading without launching" section. When Calendar is not already
/// running, this returns [`Err`] rather than an empty [`Vec`], because an
/// agenda that silently reads as "nothing today" when the real answer is "I
/// didn't check" is worse than telling the caller why.
pub fn events_between(start: NaiveDateTime, end: NaiveDateTime) -> Result<Vec<CalendarEvent>, String> {
    if end <= start {
        return Err("The end of the range has to be after its start.".into());
    }

    let script = build_events_between_script(start, end);
    let raw = apple::run_script(&script)?;

    if raw.trim() == "NOT_RUNNING" {
        return Err(
            "Calendar isn't open, so there is nothing to read without launching it. Open \
             Calendar once and Caduceus will read straight from it after that."
                .into(),
        );
    }

    Ok(parse_events(&raw))
}

/// Every event that starts today, in the Mac's local time zone.
pub fn events_today() -> Result<Vec<CalendarEvent>, String> {
    let today = chrono::Local::now().date_naive();
    let start = today.and_hms_opt(0, 0, 0).expect("midnight is always valid");
    let end = start + ChronoDuration::days(1);
    events_between(start, end)
}

// ---------------------------------------------------------------------------
// Reminders
// ---------------------------------------------------------------------------

/// Build the `osascript` source for [`create_reminder`].
fn build_create_reminder_script(text: &str, due: Option<NaiveDateTime>) -> String {
    let (date_setup, due_property) = match due {
        Some(dt) => (applescript_set_date("dueDate", dt), ", due date:dueDate"),
        None => (String::new(), ""),
    };

    format!(
        r#"{date_setup}tell application "Reminders"
    set targetList to default list
    tell targetList
        make new reminder with properties {{name:"{name}"{due_property}}}
    end tell
    return name of targetList
end tell"#,
        date_setup = date_setup,
        name = escape_applescript(text),
        due_property = due_property,
    )
}

/// Create a Reminders item, with an optional natural-language due date.
///
/// `due` goes through the same [`parse_when`] as `create_event`'s `when` — a
/// due date is just a point in time like an event's start, so there is no
/// reason for it to accept a different grammar.
pub fn create_reminder(text: &str, due: Option<&str>) -> Result<CreatedReminder, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Give the reminder some text.".into());
    }

    let due_dt = match due.map(str::trim).filter(|d| !d.is_empty()) {
        Some(when) => {
            let now = chrono::Local::now().naive_local();
            Some(parse_when(when, now)?)
        }
        None => None,
    };

    let script = build_create_reminder_script(text, due_dt);
    let list_name = apple::run_script(&script)?;

    Ok(CreatedReminder {
        text: text.to_string(),
        due: due_dt.map(|dt| dt.format("%Y-%m-%dT%H:%M").to_string()),
        list: list_name.trim().to_string(),
    })
}

// ---------------------------------------------------------------------------
// Natural-language date/time parsing
// ---------------------------------------------------------------------------
//
// Everything below is pure — no I/O, no clock reads except through the `now`
// parameter callers pass in — which is what makes it exhaustively testable
// and is also why it can run instantly and offline: there is no model in this
// loop, just string matching against a fixed, documented grammar. Anything
// outside that grammar is a refusal, never a guess, per the feature brief:
// a silently mis-scheduled meeting is worse than Caduceus saying "I don't
// understand that."

/// What a bare date with no clock time attached resolves to.
///
/// "next Tuesday" has to mean *something*, and the feature brief's own
/// examples include dates with no time component — refusing them outright
/// would make half the supported grammar unusable. 9 AM is unambiguous
/// across every time zone Caduceus runs in (never "the middle of the
/// night") and matches what quick-add in most calendar apps defaults an
/// untimed entry to.
const DEFAULT_HOUR: u32 = 9;

/// Parse a natural-language date/time expression against `now`.
///
/// # Supported grammar (case-insensitive)
///
/// - `today [at <time>]`, `tomorrow [at <time>]`
/// - `<weekday> [at <time>]` — the nearest upcoming occurrence, **including
///   today** if today is that weekday (e.g. saying "Tuesday" on a Tuesday
///   means today).
/// - `this <weekday> [at <time>]` — identical to the bare weekday form above.
/// - `next <weekday> [at <time>]` — the nearest upcoming occurrence, same as
///   the bare form, **except** it is never today: if today already is that
///   weekday, "next" jumps a full week ahead instead of landing on a
///   same-day zero offset. ("Next Tuesday" said on a Tuesday means the
///   Tuesday a week from now, not today — the reading most people expect
///   from "next".) For every other weekday, "next X" and bare "X" agree.
/// - `in <n> minute(s)|hour(s)|day(s)|week(s)` — relative to `now`, not to
///   midnight.
/// - `<month> <day>[, <year>]` or `<day> <month>[, <year>]` — month names or
///   3-letter abbreviations (`jan`…`dec`), ordinal suffixes on the day
///   accepted (`5th`). A year that is omitted and would put the date in the
///   past rolls forward to next year — "Jan 5" said in December means the
///   *next* January 5th, not one that already happened.
/// - `<month>/<day>[/<year>]` — numeric, US month-first order. Same
///   past-date roll-forward as the month-name form.
/// - `<year>-<month>-<day>[T<hour>:<minute>]` or with a space instead of
///   `T` — ISO 8601-ish, tried before anything else since it is completely
///   unambiguous.
///
/// Clock times: `3pm`, `3:30pm`, `14:00` (24-hour, requires a colon),
/// `noon`, `midnight`. A date given with no clock time defaults to
/// [`DEFAULT_HOUR`]:00.
///
/// Anything that does not match one of the above is an [`Err`] naming a few
/// examples, not a best-effort guess.
pub fn parse_when(input: &str, now: NaiveDateTime) -> Result<NaiveDateTime, String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(unparseable(raw));
    }

    // ISO forms are tried first, against the untouched original text: they
    // are locale-proof and unambiguous, so there is no reason to risk a
    // false match against the looser word-based grammar below.
    if let Ok(dt) = NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M") {
        return Ok(dt);
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M") {
        return Ok(dt);
    }
    if let Ok(date) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        return Ok(default_time_on(date));
    }

    let lower = raw.to_lowercase().replace(',', " ");
    let tokens: Vec<&str> = lower.split_whitespace().collect();
    if tokens.is_empty() {
        return Err(unparseable(raw));
    }

    // "in <n> <unit>" is not a date-plus-optional-time phrase at all, so it
    // is handled before the date/time split below would get a chance to
    // misread "hours" as some kind of trailing date token.
    if tokens[0] == "in" {
        return parse_relative_offset(&tokens, now).ok_or_else(|| unparseable(raw));
    }

    let (date_tokens, time_tokens) = split_time(&tokens);
    let today = now.date();
    let date = parse_date_tokens(&date_tokens, today).ok_or_else(|| unparseable(raw))?;

    let time = if time_tokens.is_empty() {
        NaiveTime::from_hms_opt(DEFAULT_HOUR, 0, 0).expect("DEFAULT_HOUR is a valid hour")
    } else {
        let (hour, minute) = parse_time_tokens(&time_tokens).ok_or_else(|| {
            format!(
                "“{}” is not a time Caduceus recognises — try “3pm”, “3:30pm”, or “14:00”.",
                time_tokens.join(" ")
            )
        })?;
        NaiveTime::from_hms_opt(hour, minute, 0)
            .ok_or_else(|| format!("“{}” is not a valid time.", time_tokens.join(" ")))?
    };

    Ok(NaiveDateTime::new(date, time))
}

fn default_time_on(date: NaiveDate) -> NaiveDateTime {
    NaiveDateTime::new(
        date,
        NaiveTime::from_hms_opt(DEFAULT_HOUR, 0, 0).expect("DEFAULT_HOUR is a valid hour"),
    )
}

fn unparseable(raw: &str) -> String {
    format!(
        "Could not work out when “{raw}” means. Try things like “tomorrow at 3pm”, “next \
         Tuesday”, “Friday 14:00”, “in 2 hours”, “Jan 5”, or an exact date like \
         “2026-08-04 15:00”."
    )
}

/// Split tokens into a date part and a time part.
///
/// Two shapes are recognised: an explicit `at` separator (`tomorrow at
/// 3pm`), or — when there is no `at` — a trailing token that is itself a
/// valid time (`Friday 14:00`). Neither applies, the whole phrase is the date
/// part and the caller falls back to [`DEFAULT_HOUR`].
fn split_time<'t>(tokens: &[&'t str]) -> (Vec<&'t str>, Vec<&'t str>) {
    if let Some(pos) = tokens.iter().position(|t| *t == "at") {
        return (tokens[..pos].to_vec(), tokens[pos + 1..].to_vec());
    }
    if let Some((&last, rest)) = tokens.split_last() {
        if parse_time_str(last).is_some() {
            return (rest.to_vec(), vec![last]);
        }
    }
    (tokens.to_vec(), Vec::new())
}

fn parse_time_tokens(tokens: &[&str]) -> Option<(u32, u32)> {
    // "3 pm" (a stray space before am/pm) reads the same as "3pm" — joining
    // the tokens with nothing between them costs nothing and is one less way
    // for an otherwise-valid time to be refused.
    let joined = tokens.concat();
    parse_time_str(&joined)
}

fn parse_time_str(s: &str) -> Option<(u32, u32)> {
    match s {
        "noon" => return Some((12, 0)),
        "midnight" => return Some((0, 0)),
        _ => {}
    }
    if let Some(core) = s.strip_suffix("am") {
        return parse_12h(core, false);
    }
    if let Some(core) = s.strip_suffix("pm") {
        return parse_12h(core, true);
    }
    if s.contains(':') {
        return parse_24h(s);
    }
    None
}

fn parse_12h(core: &str, is_pm: bool) -> Option<(u32, u32)> {
    let (h_str, m_str) = core.split_once(':').unwrap_or((core, "0"));
    let hour: u32 = h_str.parse().ok()?;
    let minute: u32 = m_str.parse().ok()?;
    if !(1..=12).contains(&hour) || minute > 59 {
        return None;
    }
    let hour24 = match (hour, is_pm) {
        (12, true) => 12,
        (12, false) => 0,
        (h, true) => h + 12,
        (h, false) => h,
    };
    Some((hour24, minute))
}

fn parse_24h(s: &str) -> Option<(u32, u32)> {
    let (h_str, m_str) = s.split_once(':')?;
    let hour: u32 = h_str.parse().ok()?;
    let minute: u32 = m_str.parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some((hour, minute))
}

fn parse_date_tokens(tokens: &[&str], today: NaiveDate) -> Option<NaiveDate> {
    match tokens {
        ["today"] => return Some(today),
        ["tomorrow"] => return Some(today + ChronoDuration::days(1)),
        [qualifier, day] if *qualifier == "next" || *qualifier == "this" => {
            let weekday = parse_weekday(day)?;
            return Some(next_weekday(today, weekday, *qualifier == "this"));
        }
        [day] => {
            if let Some(weekday) = parse_weekday(day) {
                return Some(next_weekday(today, weekday, true));
            }
        }
        _ => {}
    }
    parse_explicit_date(tokens, today)
}

/// The weekday form's "next" vs. bare/"this" distinction, as actual maths.
///
/// `inclusive_of_today = true` allows a zero-day offset (today counts);
/// `false` forces at least a week out, which is what "next" means here — see
/// the grammar note on [`parse_when`].
fn next_weekday(today: NaiveDate, weekday: Weekday, inclusive_of_today: bool) -> NaiveDate {
    let today_idx = today.weekday().num_days_from_monday() as i64;
    let target_idx = weekday.num_days_from_monday() as i64;
    let mut diff = (target_idx - today_idx).rem_euclid(7);
    if diff == 0 && !inclusive_of_today {
        diff = 7;
    }
    today + ChronoDuration::days(diff)
}

fn parse_weekday(tok: &str) -> Option<Weekday> {
    Some(match tok {
        "mon" | "monday" => Weekday::Mon,
        "tue" | "tues" | "tuesday" => Weekday::Tue,
        "wed" | "weds" | "wednesday" => Weekday::Wed,
        "thu" | "thur" | "thurs" | "thursday" => Weekday::Thu,
        "fri" | "friday" => Weekday::Fri,
        "sat" | "saturday" => Weekday::Sat,
        "sun" | "sunday" => Weekday::Sun,
        _ => return None,
    })
}

fn parse_relative_offset(tokens: &[&str], now: NaiveDateTime) -> Option<NaiveDateTime> {
    let [_, amount, unit] = tokens else { return None };
    let amount: i64 = amount.parse().ok()?;
    if amount <= 0 {
        return None;
    }
    // Tolerates both "hour" and "hours" by trimming a trailing 's' — cheaper
    // than listing every plural explicitly and there is no unit here whose
    // singular already ends in 's'.
    let delta = match unit.trim_end_matches('s') {
        "minute" => ChronoDuration::minutes(amount),
        "hour" => ChronoDuration::hours(amount),
        "day" => ChronoDuration::days(amount),
        "week" => ChronoDuration::weeks(amount),
        _ => return None,
    };
    Some(now + delta)
}

fn parse_explicit_date(tokens: &[&str], today: NaiveDate) -> Option<NaiveDate> {
    if tokens.len() == 1 {
        if let Some(date) = parse_slash_date(tokens[0], today) {
            return Some(date);
        }
    }
    parse_month_name_date(tokens, today)
}

/// `mm/dd` or `mm/dd/yyyy`, US month-first order — the order "Jan 5" already
/// commits this grammar to.
fn parse_slash_date(tok: &str, today: NaiveDate) -> Option<NaiveDate> {
    let parts: Vec<&str> = tok.split('/').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return None;
    }
    let month: u32 = parts[0].parse().ok()?;
    let day: u32 = parts[1].parse().ok()?;
    let had_year = parts.len() == 3;
    let year = match parts.get(2) {
        Some(y) => {
            let y: i32 = y.parse().ok()?;
            if y < 100 {
                2000 + y
            } else {
                y
            }
        }
        None => today.year(),
    };

    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    Some(if !had_year && date < today { roll_year_forward(date) } else { date })
}

fn parse_month_name_date(tokens: &[&str], today: NaiveDate) -> Option<NaiveDate> {
    if tokens.len() < 2 || tokens.len() > 3 {
        return None;
    }

    let (month, day_token, year_token) = if let Some(month) = month_number(tokens[0]) {
        (month, tokens[1], tokens.get(2).copied())
    } else if let Some(month) = month_number(tokens[1]) {
        (month, tokens[0], tokens.get(2).copied())
    } else {
        return None;
    };

    let day = parse_ordinal_day(day_token)?;
    let had_year = year_token.is_some();
    let year = match year_token {
        Some(y) => y.parse::<i32>().ok()?,
        None => today.year(),
    };

    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    Some(if !had_year && date < today { roll_year_forward(date) } else { date })
}

fn roll_year_forward(date: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(date.year() + 1, date.month(), date.day()).unwrap_or(date)
}

fn month_number(tok: &str) -> Option<u32> {
    Some(match tok {
        "jan" | "january" => 1,
        "feb" | "february" => 2,
        "mar" | "march" => 3,
        "apr" | "april" => 4,
        "may" => 5,
        "jun" | "june" => 6,
        "jul" | "july" => 7,
        "aug" | "august" => 8,
        "sep" | "sept" | "september" => 9,
        "oct" | "october" => 10,
        "nov" | "november" => 11,
        "dec" | "december" => 12,
        _ => return None,
    })
}

/// `5`, `5th`, `1st`, `2nd`, `3rd` — a bare day-of-month, with or without its
/// ordinal suffix.
fn parse_ordinal_day(tok: &str) -> Option<u32> {
    let core = tok
        .strip_suffix("st")
        .or_else(|| tok.strip_suffix("nd"))
        .or_else(|| tok.strip_suffix("rd"))
        .or_else(|| tok.strip_suffix("th"))
        .unwrap_or(tok);
    let day: u32 = core.parse().ok()?;
    if (1..=31).contains(&day) {
        Some(day)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
//
// No test here ever creates a real Calendar/Reminders item — every AppleScript
// entry point is exercised only through its `build_*_script` function, which
// is pure text generation. The date parser and the escaping it relies on are
// otherwise tested exhaustively, since the brief calls out the parser as the
// part most likely to be subtly wrong.

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d).unwrap().and_hms_opt(h, mi, 0).unwrap()
    }

    // A fixed "now" for every relative test: Tuesday 2026-07-28, 10:00 local.
    fn now() -> NaiveDateTime {
        dt(2026, 7, 28, 10, 0)
    }

    // --- AppleScript escaping / injection ------------------------------------

    #[test]
    fn a_title_with_quotes_and_backslashes_cannot_break_out_of_the_applescript_literal() {
        let evil = r#"Lunch" & (do shell script "rm -rf ~") & ""#;
        let script = build_create_event_script(evil, "", "", dt(2026, 7, 28, 12, 0), 60);

        // The raw payload — with its unescaped quotes — must never appear in
        // the generated script: if it did, the `"` in `"rm -rf ~"` would
        // close the AppleScript string literal early and the rest would be
        // read as source, not text.
        assert!(!script.contains(evil));

        // What must appear instead is the escaped form: every `"` preceded
        // by a `\`, so AppleScript's tokenizer treats the whole thing as one
        // string.
        let escaped = escape_applescript(evil);
        assert!(script.contains(&escaped));

        // Belt and suspenders: an empty `""` immediately inside the summary
        // literal (rather than escaped `\"\"`) would be the signature of a
        // successful break-out, since it would mean the literal closed and
        // reopened around injected source.
        assert!(!script.contains("summary:\"\"") || evil.is_empty());
    }

    #[test]
    fn a_reminder_with_a_backslash_and_quote_is_neutralised_the_same_way() {
        let evil = r#"call home\" then do shell script "id"#;
        let script = build_create_reminder_script(evil, None);
        assert!(!script.contains(evil));
        assert!(script.contains(&escape_applescript(evil)));
    }

    #[test]
    fn location_and_notes_are_escaped_independently_of_the_title() {
        let script = build_create_event_script(
            "Lunch",
            r#"Panera" & (do shell script "whoami") & ""#,
            r#"bring "notes""#,
            dt(2026, 7, 28, 12, 0),
            60,
        );
        assert!(script.contains(&escape_applescript(r#"Panera" & (do shell script "whoami") & ""#)));
        assert!(script.contains(&escape_applescript(r#"bring "notes""#)));
    }

    // --- duration ---------------------------------------------------------

    #[test]
    fn no_duration_defaults_to_an_hour() {
        assert_eq!(resolve_duration(None).unwrap(), 60);
    }

    #[test]
    fn a_specific_duration_is_honoured() {
        assert_eq!(resolve_duration(Some(30)).unwrap(), 30);
    }

    #[test]
    fn a_zero_or_negative_duration_is_refused() {
        assert!(resolve_duration(Some(0)).is_err());
        assert!(resolve_duration(Some(-15)).is_err());
    }

    // --- date parser: relative words --------------------------------------

    #[test]
    fn tomorrow_at_a_time() {
        assert_eq!(parse_when("tomorrow at 3pm", now()).unwrap(), dt(2026, 7, 29, 15, 0));
    }

    #[test]
    fn today_with_no_time_defaults_to_nine_am() {
        assert_eq!(parse_when("today", now()).unwrap(), dt(2026, 7, 28, 9, 0));
    }

    #[test]
    fn tomorrow_with_no_time_defaults_to_nine_am() {
        assert_eq!(parse_when("tomorrow", now()).unwrap(), dt(2026, 7, 29, 9, 0));
    }

    #[test]
    fn a_bare_weekday_means_the_nearest_upcoming_occurrence_including_today() {
        // now() is a Tuesday, so a bare "tuesday" means today.
        assert_eq!(parse_when("tuesday", now()).unwrap(), dt(2026, 7, 28, 9, 0));
        // "friday" is three days out.
        assert_eq!(parse_when("friday", now()).unwrap(), dt(2026, 7, 31, 9, 0));
    }

    #[test]
    fn this_weekday_behaves_like_the_bare_form() {
        assert_eq!(parse_when("this tuesday", now()).unwrap(), dt(2026, 7, 28, 9, 0));
        assert_eq!(parse_when("this friday", now()).unwrap(), dt(2026, 7, 31, 9, 0));
    }

    #[test]
    fn next_weekday_never_means_today_even_when_today_is_a_match() {
        // Said on a Tuesday, "next Tuesday" cannot mean today, so it jumps a
        // full week rather than landing on a same-day zero offset.
        assert_eq!(parse_when("next tuesday", now()).unwrap(), dt(2026, 8, 4, 9, 0));
    }

    #[test]
    fn next_weekday_otherwise_matches_the_nearest_upcoming_occurrence() {
        // For any weekday that is not today, "next X" and bare "X" agree:
        // both mean the closest upcoming X, which is still within this week.
        assert_eq!(parse_when("next friday", now()).unwrap(), dt(2026, 7, 31, 9, 0));
        assert_eq!(parse_when("friday", now()).unwrap(), dt(2026, 7, 31, 9, 0));
    }

    #[test]
    fn friday_with_a_24_hour_time_and_no_at() {
        assert_eq!(parse_when("Friday 14:00", now()).unwrap(), dt(2026, 7, 31, 14, 0));
    }

    #[test]
    fn weekday_and_time_parsing_is_case_insensitive() {
        assert_eq!(parse_when("NEXT Tuesday AT 3PM", now()).unwrap(), dt(2026, 8, 4, 15, 0));
    }

    #[test]
    fn noon_and_midnight_are_understood() {
        assert_eq!(parse_when("tomorrow at noon", now()).unwrap(), dt(2026, 7, 29, 12, 0));
        assert_eq!(parse_when("tomorrow at midnight", now()).unwrap(), dt(2026, 7, 29, 0, 0));
    }

    #[test]
    fn twelve_am_and_twelve_pm_land_on_the_right_side_of_noon() {
        assert_eq!(parse_when("today at 12am", now()).unwrap(), dt(2026, 7, 28, 0, 0));
        assert_eq!(parse_when("today at 12pm", now()).unwrap(), dt(2026, 7, 28, 12, 0));
    }

    // --- date parser: relative offsets -------------------------------------

    #[test]
    fn in_n_hours_adds_from_now_not_from_midnight() {
        assert_eq!(parse_when("in 2 hours", now()).unwrap(), dt(2026, 7, 28, 12, 0));
    }

    #[test]
    fn in_n_minutes_and_days_and_weeks() {
        assert_eq!(parse_when("in 45 minutes", now()).unwrap(), dt(2026, 7, 28, 10, 45));
        assert_eq!(parse_when("in 3 days", now()).unwrap(), dt(2026, 7, 31, 10, 0));
        assert_eq!(parse_when("in 1 week", now()).unwrap(), dt(2026, 8, 4, 10, 0));
    }

    #[test]
    fn a_zero_or_negative_relative_offset_is_refused() {
        assert!(parse_when("in 0 minutes", now()).is_err());
        assert!(parse_when("in -3 hours", now()).is_err());
    }

    // --- date parser: explicit dates ---------------------------------------

    #[test]
    fn month_name_then_day_rolls_to_next_year_when_already_past() {
        // now() is July 2026; "Jan 5" with no year has already happened this
        // year, so it should mean January 5th, 2027.
        assert_eq!(parse_when("Jan 5", now()).unwrap(), dt(2027, 1, 5, 9, 0));
    }

    #[test]
    fn month_name_then_day_stays_this_year_when_still_upcoming() {
        assert_eq!(parse_when("Dec 25", now()).unwrap(), dt(2026, 12, 25, 9, 0));
    }

    #[test]
    fn an_explicit_year_is_never_rolled_forward() {
        assert_eq!(parse_when("Jan 5 2026", now()).unwrap(), dt(2026, 1, 5, 9, 0));
    }

    #[test]
    fn day_then_month_name_is_also_accepted() {
        assert_eq!(parse_when("5 January 2027", now()).unwrap(), dt(2027, 1, 5, 9, 0));
    }

    #[test]
    fn an_ordinal_suffix_on_the_day_is_accepted() {
        assert_eq!(parse_when("Jan 5th 2027", now()).unwrap(), dt(2027, 1, 5, 9, 0));
    }

    #[test]
    fn month_name_date_with_a_time() {
        assert_eq!(parse_when("Dec 25 at 6pm", now()).unwrap(), dt(2026, 12, 25, 18, 0));
    }

    #[test]
    fn slash_dates_roll_forward_past_dates_and_respect_an_explicit_year() {
        assert_eq!(parse_when("1/5", now()).unwrap(), dt(2027, 1, 5, 9, 0));
        assert_eq!(parse_when("12/25", now()).unwrap(), dt(2026, 12, 25, 9, 0));
        assert_eq!(parse_when("1/5/2026", now()).unwrap(), dt(2026, 1, 5, 9, 0));
    }

    #[test]
    fn iso_date_and_datetime_forms() {
        assert_eq!(parse_when("2026-08-04", now()).unwrap(), dt(2026, 8, 4, 9, 0));
        assert_eq!(parse_when("2026-08-04T15:00", now()).unwrap(), dt(2026, 8, 4, 15, 0));
        assert_eq!(parse_when("2026-08-04 15:00", now()).unwrap(), dt(2026, 8, 4, 15, 0));
    }

    // --- date parser: refusals ----------------------------------------------

    #[test]
    fn empty_input_is_refused() {
        assert!(parse_when("", now()).is_err());
        assert!(parse_when("   ", now()).is_err());
    }

    #[test]
    fn nonsense_is_refused_rather_than_guessed_at() {
        assert!(parse_when("elephant", now()).is_err());
        assert!(parse_when("sometime soonish", now()).is_err());
    }

    #[test]
    fn an_impossible_calendar_date_is_refused() {
        assert!(parse_when("Feb 30", now()).is_err());
        assert!(parse_when("2026-02-30", now()).is_err());
    }

    #[test]
    fn an_impossible_time_is_refused_rather_than_silently_clamped() {
        assert!(parse_when("today at 25:00", now()).is_err());
        assert!(parse_when("today at 14:99", now()).is_err());
        assert!(parse_when("today at 13pm", now()).is_err());
    }

    #[test]
    fn a_date_with_an_unparseable_trailing_word_is_refused_rather_than_defaulting_the_time() {
        // "Jan 5 whenever" is not "Jan 5" with an assumed default time — the
        // trailing garbage should not be silently dropped.
        assert!(parse_when("Jan 5 whenever", now()).is_err());
    }

    // --- events_between parsing ----------------------------------------------

    #[test]
    fn parsing_a_well_formed_agenda_blob() {
        let raw = format!(
            "Standup{fs}2026-07-28T09:00{fs}2026-07-28T09:15{fs}Zoom{fs}Work{fs}false{rs}\
             Dentist{fs}2026-07-28T14:00{fs}2026-07-28T15:00{fs}{fs}Personal{fs}false{rs}",
            fs = FIELD_SEP,
            rs = RECORD_SEP,
        );
        let events = parse_events(&raw);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].title, "Standup");
        assert_eq!(events[0].location, "Zoom");
        assert_eq!(events[0].calendar, "Work");
        assert!(!events[0].all_day);
        assert_eq!(events[1].title, "Dentist");
        assert_eq!(events[1].location, "");
    }

    #[test]
    fn an_empty_agenda_blob_parses_to_no_events() {
        assert!(parse_events("").is_empty());
    }

    #[test]
    fn an_all_day_flag_round_trips() {
        let raw = format!(
            "Birthday{fs}2026-07-28T09:00{fs}2026-07-28T09:00{fs}{fs}Family{fs}true{rs}",
            fs = FIELD_SEP,
            rs = RECORD_SEP,
        );
        assert!(parse_events(&raw)[0].all_day);
    }

    #[test]
    fn a_malformed_record_with_too_few_fields_is_dropped_not_panicked_on() {
        let raw = format!("Broken{fs}only two fields{rs}", fs = FIELD_SEP, rs = RECORD_SEP);
        assert!(parse_events(&raw).is_empty());
    }

    #[test]
    fn events_between_refuses_an_inverted_range() {
        let start = dt(2026, 7, 28, 10, 0);
        let end = dt(2026, 7, 28, 9, 0);
        assert!(events_between(start, end).is_err());
    }
}
