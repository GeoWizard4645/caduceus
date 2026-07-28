//! Parsing a 5-field cron expression, describing it in English, and listing
//! its next runs.
//!
//! Five fields, not six or seven: no seconds field and no year field, which is
//! what every crontab(5) on a Mac or Linux box actually reads, as opposed to
//! the various six-field dialects other schedulers invented later. Someone
//! pasting a line out of their actual crontab should get an answer, not a
//! field-count error because this tool assumed a different flavour.
//!
//! Cron carries no time zone of its own — a crontab just runs in whatever zone
//! the machine is set to — so "next run" here means next run in *this* Mac's
//! local time zone. That is the only reading that means anything without a
//! zone to ask for.

use chrono::{Datelike, Duration, NaiveDateTime, Timelike};
use serde::Serialize;

// ---------------------------------------------------------------------------
// Field names
// ---------------------------------------------------------------------------

const MONTH_NAMES: &[(&str, u32)] = &[
    ("JAN", 1),
    ("FEB", 2),
    ("MAR", 3),
    ("APR", 4),
    ("MAY", 5),
    ("JUN", 6),
    ("JUL", 7),
    ("AUG", 8),
    ("SEP", 9),
    ("OCT", 10),
    ("NOV", 11),
    ("DEC", 12),
];

const WEEKDAY_NAMES: &[(&str, u32)] =
    &[("SUN", 0), ("MON", 1), ("TUE", 2), ("WED", 3), ("THU", 4), ("FRI", 5), ("SAT", 6)];

const MONTH_FULL: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September",
    "October", "November", "December",
];

/// Index 0 is Sunday, matching cron's own numbering (`0` and `7` both mean
/// Sunday; see [`parse`]), not `chrono`'s Monday-first `Weekday`.
const WEEKDAY_FULL: [&str; 7] =
    ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// A cron expression, broken into the set of values each field allows.
///
/// `dom_is_wild` and `dow_is_wild` record whether the *raw text* of those two
/// fields was exactly `*`, separately from what values ended up in the sets —
/// see [`day_matches`] for why that distinction, and not just "is the set
/// full", is the one cron itself makes.
#[derive(Debug, Clone)]
pub struct ParsedCron {
    pub minutes: Vec<u32>,
    pub hours: Vec<u32>,
    pub days_of_month: Vec<u32>,
    pub months: Vec<u32>,
    /// Always 0–6, Sunday-first — `7` (cron's alternate spelling of Sunday)
    /// is folded into `0` during parsing.
    pub days_of_week: Vec<u32>,
    pub dom_is_wild: bool,
    pub dow_is_wild: bool,
}

/// Parse a 5-field cron expression: minute, hour, day of month, month, day of
/// week.
pub fn parse(expr: &str) -> Result<ParsedCron, String> {
    let expr = expr.trim();
    if expr.is_empty() {
        return Err("There is no cron expression to parse yet.".into());
    }

    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(format!(
            "A cron expression needs 5 fields — minute, hour, day of month, month, day of week — \
             separated by spaces. This one has {}.",
            fields.len()
        ));
    }

    let minutes = parse_field("minute", fields[0], 0, 59, None)?;
    let hours = parse_field("hour", fields[1], 0, 23, None)?;
    let days_of_month = parse_field("day of month", fields[2], 1, 31, None)?;
    let months = parse_field("month", fields[3], 1, 12, Some(MONTH_NAMES))?;
    let mut days_of_week = parse_field("day of week", fields[4], 0, 7, Some(WEEKDAY_NAMES))?;
    for d in days_of_week.iter_mut() {
        if *d == 7 {
            *d = 0; // cron's one wart of an alias: 0 and 7 both mean Sunday
        }
    }
    days_of_week.sort_unstable();
    days_of_week.dedup();

    Ok(ParsedCron {
        minutes,
        hours,
        days_of_month,
        months,
        days_of_week,
        dom_is_wild: fields[2] == "*",
        dow_is_wild: fields[4] == "*",
    })
}

fn parse_field(
    label: &str,
    field: &str,
    min: u32,
    max: u32,
    names: Option<&[(&str, u32)]>,
) -> Result<Vec<u32>, String> {
    let mut values = Vec::new();
    for part in field.split(',') {
        values.extend(parse_range_part(label, part, min, max, names)?);
    }
    values.sort_unstable();
    values.dedup();
    Ok(values)
}

fn parse_range_part(
    label: &str,
    part: &str,
    min: u32,
    max: u32,
    names: Option<&[(&str, u32)]>,
) -> Result<Vec<u32>, String> {
    if part.is_empty() {
        return Err(format!("The {label} field has an empty entry — check for a stray comma."));
    }

    let (base, step) = match part.split_once('/') {
        Some((b, s)) => {
            let step: u32 =
                s.parse().map_err(|_| format!("\"{s}\" is not a valid step for {label}."))?;
            if step == 0 {
                return Err(format!("A step of 0 in the {label} field would never advance."));
            }
            (b, Some(step))
        }
        None => (part, None),
    };

    let (start, end) = if base == "*" {
        (min, max)
    } else if let Some((a, b)) = base.split_once('-') {
        let a = resolve_token(label, a, min, max, names)?;
        let b = resolve_token(label, b, min, max, names)?;
        if a > b {
            return Err(format!(
                "\"{base}\" in the {label} field runs backwards — {a} comes after {b}."
            ));
        }
        (a, b)
    } else {
        let v = resolve_token(label, base, min, max, names)?;
        (v, v)
    };

    Ok(match step {
        Some(step) => (start..=end).step_by(step as usize).collect(),
        None => (start..=end).collect(),
    })
}

fn resolve_token(
    label: &str,
    token: &str,
    min: u32,
    max: u32,
    names: Option<&[(&str, u32)]>,
) -> Result<u32, String> {
    let value = if let Ok(n) = token.parse::<u32>() {
        n
    } else if let Some(names) = names {
        let upper = token.to_ascii_uppercase();
        names
            .iter()
            .find(|(name, _)| *name == upper)
            .map(|(_, v)| *v)
            .ok_or_else(|| format!("\"{token}\" in the {label} field is not a number or a name."))?
    } else {
        return Err(format!("\"{token}\" in the {label} field is not a number."));
    };
    if value < min || value > max {
        return Err(format!(
            "{value} is out of range for {label} — this field takes {min} to {max}."
        ));
    }
    Ok(value)
}

// ---------------------------------------------------------------------------
// Matching a day
// ---------------------------------------------------------------------------

/// Whether `day_of_month`/`weekday` satisfy the day-of-month and day-of-week
/// fields together.
///
/// This is cron's best-known quirk, and skipping it would make every "run on
/// the 1st, or on Monday" expression wrong: when *both* fields are restricted
/// (neither is a bare `*`), a day matches if it satisfies *either* one, not
/// both. Restrict only one and it behaves the way it looks like it should.
fn day_matches(p: &ParsedCron, day_of_month: u32, weekday: u32) -> bool {
    if p.dom_is_wild && p.dow_is_wild {
        true
    } else {
        (!p.dom_is_wild && p.days_of_month.contains(&day_of_month))
            || (!p.dow_is_wild && p.days_of_week.contains(&weekday))
    }
}

// ---------------------------------------------------------------------------
// Next occurrences
// ---------------------------------------------------------------------------

/// How far ahead to search before giving up.
///
/// Generous enough to always find a `29 FEB` occurrence (leap years are at
/// most 4 years apart, plus a margin) without scanning forever for an
/// expression that can truly never fire, like day 31 of every month combined
/// with a month that never has one — chrono simply never produces that
/// calendar date, so the scan below would otherwise run unbounded.
const MAX_LOOKAHEAD_MINUTES: i64 = 5 * 366 * 24 * 60;

/// The next `count` times `p` fires, strictly after `from`.
///
/// Walks forward one real minute at a time using `chrono`'s calendar
/// arithmetic rather than constructing candidate dates by hand, which is what
/// makes invalid dates (Feb 30, day 31 in April) simply never come up instead
/// of needing to be filtered out.
pub fn next_occurrences(p: &ParsedCron, from: NaiveDateTime, count: usize) -> Vec<NaiveDateTime> {
    let mut results = Vec::with_capacity(count);
    let mut candidate = (from + Duration::minutes(1))
        .with_second(0)
        .expect("0 seconds is always a valid time")
        .with_nanosecond(0)
        .expect("0 nanoseconds is always a valid time");

    let mut steps = 0i64;
    while results.len() < count && steps < MAX_LOOKAHEAD_MINUTES {
        let matches = p.minutes.contains(&candidate.minute())
            && p.hours.contains(&candidate.hour())
            && p.months.contains(&candidate.month())
            && day_matches(p, candidate.day(), candidate.weekday().num_days_from_sunday());

        if matches {
            results.push(candidate);
        }
        candidate += Duration::minutes(1);
        steps += 1;
    }
    results
}

// ---------------------------------------------------------------------------
// Describing in English
// ---------------------------------------------------------------------------

/// Describe a minute or hour field: "every minute", "every 15 minutes", "hour
/// 9", or a plain list — whichever reads most like how a person would say it.
fn describe_minute_or_hour(values: &[u32], min: u32, max: u32, singular: &str, plural: &str) -> (String, bool) {
    let is_every = values.len() as u32 == max - min + 1 && values[0] == min && *values.last().unwrap() == max;
    if is_every {
        return (format!("every {singular}"), true);
    }

    if values.len() >= 2 {
        let step = values[1] - values[0];
        let is_arithmetic = step > 0 && values.windows(2).all(|w| w[1] - w[0] == step);
        if is_arithmetic {
            return if values[0] == min {
                (format!("every {step} {plural}"), false)
            } else {
                (format!("every {step} {plural} starting at {singular} {}", values[0]), false)
            };
        }
    }

    if values.len() == 1 {
        return (format!("{singular} {}", values[0]), false);
    }

    let named: Vec<String> = values.iter().map(u32::to_string).collect();
    (format!("{plural} {}", join_and(&named)), false)
}

/// Describe a day-of-month field. Never called when the field is `*`.
fn describe_dom(values: &[u32]) -> String {
    if values.len() >= 2 {
        let step = values[1] - values[0];
        if step > 0 && values.windows(2).all(|w| w[1] - w[0] == step) {
            return if step == 1 {
                format!("on days {} to {} of the month", values[0], values.last().unwrap())
            } else {
                format!("every {step} days of the month, starting on day {}", values[0])
            };
        }
    }
    if values.len() == 1 {
        return format!("on day {} of the month", values[0]);
    }
    let named: Vec<String> = values.iter().map(u32::to_string).collect();
    format!("on days {} of the month", join_and(&named))
}

/// Describe a day-of-week field. Never called when the field is `*`.
fn describe_weekday(values: &[u32]) -> String {
    if values.len() >= 2 {
        let step = values[1] - values[0];
        if step == 1 && values.windows(2).all(|w| w[1] - w[0] == 1) {
            return format!(
                "{} to {}",
                WEEKDAY_FULL[values[0] as usize],
                WEEKDAY_FULL[*values.last().unwrap() as usize]
            );
        }
    }
    if values.len() == 1 {
        return WEEKDAY_FULL[values[0] as usize].to_string();
    }
    let named: Vec<String> = values.iter().map(|v| WEEKDAY_FULL[*v as usize].to_string()).collect();
    join_and(&named)
}

/// Describe a month field, or `None` when every month is allowed — the
/// sentence reads better with that clause dropped entirely than with "every
/// month" bolted on.
fn describe_month(values: &[u32]) -> Option<String> {
    if values.len() == 12 {
        return None;
    }
    if values.len() >= 2 {
        let step = values[1] - values[0];
        if step == 1 && values.windows(2).all(|w| w[1] - w[0] == 1) {
            return Some(format!(
                "in {} through {}",
                MONTH_FULL[(values[0] - 1) as usize],
                MONTH_FULL[(*values.last().unwrap() - 1) as usize]
            ));
        }
    }
    if values.len() == 1 {
        return Some(format!("in {}", MONTH_FULL[(values[0] - 1) as usize]));
    }
    let named: Vec<String> = values.iter().map(|v| MONTH_FULL[(*v - 1) as usize].to_string()).collect();
    Some(format!("in {}", join_and(&named)))
}

fn join_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} and {second}"),
        _ => {
            let (last, rest) = items.split_last().expect("checked non-empty above");
            format!("{}, and {last}", rest.join(", "))
        }
    }
}

/// Describe `p` as a sentence, e.g. "Every 15 minutes, Monday to Friday".
pub fn describe(p: &ParsedCron) -> String {
    let (minute_desc, minute_is_every) = describe_minute_or_hour(&p.minutes, 0, 59, "minute", "minutes");
    let (hour_desc, hour_is_every) = describe_minute_or_hour(&p.hours, 0, 23, "hour", "hours");

    let time_part = if minute_is_every && hour_is_every {
        "every minute".to_string()
    } else if p.minutes.len() == 1 && p.hours.len() == 1 {
        format!("at {:02}:{:02}", p.hours[0], p.minutes[0])
    } else if hour_is_every {
        minute_desc
    } else if minute_is_every {
        format!("every minute during the {hour_desc}")
    } else {
        format!("{minute_desc}, {hour_desc}")
    };

    let mut parts = vec![time_part];

    let dom_desc = (!p.dom_is_wild).then(|| describe_dom(&p.days_of_month));
    let dow_desc = (!p.dow_is_wild).then(|| describe_weekday(&p.days_of_week));

    // Both restricted: cron ORs them (see `day_matches`), so the sentence has
    // to say "or" too, or it would claim a stricter schedule than the
    // expression actually runs on.
    match (dom_desc, dow_desc) {
        (Some(d), Some(w)) => parts.push(format!("{d}, or on {w}")),
        (Some(d), None) => parts.push(d),
        (None, Some(w)) => parts.push(w),
        (None, None) => {}
    }

    if let Some(m) = describe_month(&p.months) {
        parts.push(m);
    }

    let sentence = parts.join(", ");
    let mut chars = sentence.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => sentence,
    }
}

// ---------------------------------------------------------------------------
// Putting it together
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CronAnalysis {
    pub description: String,
    /// This machine's local time — see the module doc for why there is no
    /// time zone to be more specific than that.
    pub next_runs: Vec<NaiveDateTime>,
}

/// Parse, describe, and find the next `count` runs of a cron expression, all
/// in one call — what the tool page actually needs from a single keystroke.
pub fn analyze(expr: &str, from: NaiveDateTime, count: usize) -> Result<CronAnalysis, String> {
    let parsed = parse(expr)?;
    Ok(CronAnalysis {
        description: describe(&parsed),
        next_runs: next_occurrences(&parsed, from, count),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, m: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
    }

    // --- parse() ---------------------------------------------------------

    #[test]
    fn parses_every_field_kind_in_one_expression() {
        let p = parse("*/15 9-17 1,15 JAN,JUL MON-FRI").unwrap();
        assert_eq!(p.minutes, vec![0, 15, 30, 45]);
        assert_eq!(p.hours, (9..=17).collect::<Vec<_>>());
        assert_eq!(p.days_of_month, vec![1, 15]);
        assert_eq!(p.months, vec![1, 7]);
        assert_eq!(p.days_of_week, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn seven_and_zero_both_mean_sunday() {
        assert_eq!(parse("0 0 * * 7").unwrap().days_of_week, vec![0]);
        assert_eq!(parse("0 0 * * 0,7").unwrap().days_of_week, vec![0]);
    }

    #[test]
    fn wrong_field_count_is_refused_with_the_count() {
        let err = parse("* * * *").unwrap_err();
        assert!(err.contains("5 fields"));
        assert!(err.contains('4'));
    }

    #[test]
    fn an_out_of_range_value_is_refused_by_name() {
        let err = parse("0 25 * * *").unwrap_err();
        assert!(err.contains("25"));
        assert!(err.contains("hour"));
    }

    #[test]
    fn a_backwards_range_is_refused() {
        assert!(parse("0 0 * * 5-1").unwrap_err().contains("backwards"));
    }

    #[test]
    fn an_unrecognised_name_is_refused() {
        assert!(parse("0 0 * FOO *").unwrap_err().contains("FOO"));
    }

    #[test]
    fn an_empty_expression_is_refused_before_splitting() {
        assert!(parse("   ").unwrap_err().contains("no cron expression"));
    }

    // --- describe() --------------------------------------------------------

    #[test]
    fn every_fifteen_minutes_on_weekdays_reads_as_the_readme_promises() {
        let p = parse("*/15 * * * 1-5").unwrap();
        assert_eq!(describe(&p), "Every 15 minutes, Monday to Friday");
    }

    #[test]
    fn a_single_time_reads_as_a_clock_time() {
        let p = parse("30 2 * * *").unwrap();
        assert_eq!(describe(&p), "At 02:30");
    }

    #[test]
    fn every_minute_with_nothing_else_restricted() {
        let p = parse("* * * * *").unwrap();
        assert_eq!(describe(&p), "Every minute");
    }

    #[test]
    fn both_day_fields_restricted_reads_as_an_or() {
        let p = parse("0 0 1 * MON").unwrap();
        assert_eq!(describe(&p), "At 00:00, on day 1 of the month, or on Monday");
    }

    // --- day_matches / cron's OR quirk --------------------------------------

    #[test]
    fn restricting_only_day_of_month_behaves_like_an_and() {
        // "at midnight on the 1st" should not also fire on every Monday.
        let p = parse("0 0 1 * *").unwrap();
        let runs = next_occurrences(&p, at(2024, 1, 1, 0, 0), 3);
        for run in &runs {
            assert_eq!(run.day(), 1);
        }
    }

    #[test]
    fn restricting_both_day_fields_fires_on_either() {
        // Midnight on the 1st OR on any Monday — the classic cron quirk.
        let p = parse("0 0 1 * MON").unwrap();
        let runs = next_occurrences(&p, at(2024, 3, 1, 0, 30), 6);
        assert!(runs.iter().all(|r| r.day() == 1 || r.weekday() == chrono::Weekday::Mon));
        // And at least one of each kind should show up within six hits.
        assert!(runs.iter().any(|r| r.day() == 1));
        assert!(runs.iter().any(|r| r.weekday() == chrono::Weekday::Mon));
    }

    // --- next_occurrences() -------------------------------------------------

    #[test]
    fn every_result_is_strictly_after_the_starting_point() {
        let p = parse("*/10 * * * *").unwrap();
        let from = at(2024, 6, 1, 12, 5);
        let runs = next_occurrences(&p, from, 5);
        assert_eq!(runs.len(), 5);
        for run in &runs {
            assert!(*run > from);
        }
        assert_eq!(runs[0], at(2024, 6, 1, 12, 10));
    }

    #[test]
    fn a_calendar_date_that_never_occurs_yields_nothing_within_the_lookahead() {
        // April has 30 days; day 31 of April never exists on any calendar.
        let p = parse("0 0 31 4 *").unwrap();
        let runs = next_occurrences(&p, at(2024, 1, 1, 0, 0), 10);
        assert!(runs.is_empty());
    }

    #[test]
    fn a_leap_day_expression_still_finds_its_next_occurrence() {
        let p = parse("0 0 29 2 *").unwrap();
        let runs = next_occurrences(&p, at(2024, 3, 1, 0, 0), 1);
        assert_eq!(runs, vec![at(2028, 2, 29, 0, 0)]);
    }

    #[test]
    fn analyze_combines_parsing_describing_and_scheduling() {
        let analysis = analyze("0 9 * * 1-5", at(2024, 1, 1, 0, 0), 2).unwrap();
        assert_eq!(analysis.description, "At 09:00, Monday to Friday");
        assert_eq!(analysis.next_runs.len(), 2);
    }

    #[test]
    fn analyze_propagates_a_parse_error() {
        assert!(analyze("not a cron", at(2024, 1, 1, 0, 0), 1).is_err());
    }
}
