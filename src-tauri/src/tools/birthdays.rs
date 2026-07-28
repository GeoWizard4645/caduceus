//! Birthdays: who, and when they next come around.
//!
//! Storage follows the same one-file-per-feature `tauri_plugin_store` pattern
//! as `tools::habits` and `tools::expander` — its own JSON file, never
//! `crate::settings::Settings`.
//!
//! Only month and day are ever required. The birth *year* is optional: it is
//! enough to know someone's birthday to wish them a happy one, and a lot of
//! people you'd add here (a coworker, an acquaintance) never told you the
//! year. When it is known, it drives the "turning N" label; when it isn't,
//! the list still sorts and counts down exactly the same way.

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;

type Res<T> = Result<T, String>;

const STORE_FILE: &str = "caduceus-birthdays.json";
const BIRTHDAYS_KEY: &str = "birthdays";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Birthday {
    pub id: String,
    pub name: String,
    pub month: u32,
    pub day: u32,
    /// The birth year, if known — drives "turning N"; omit it and the entry
    /// still sorts and counts down correctly.
    pub year: Option<i32>,
    pub notes: String,
}

/// A birthday with its next occurrence resolved, for the sorted list view.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpcomingBirthday {
    #[serde(flatten)]
    pub birthday: Birthday,
    /// ISO date of the next time this birthday occurs, today included.
    pub next_occurrence: String,
    pub days_until: i64,
    /// The age they turn on `next_occurrence`, if `year` is known.
    pub turning: Option<i32>,
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

fn load<R: Runtime>(app: &AppHandle<R>) -> Vec<Birthday> {
    let Ok(store) = app.store(STORE_FILE) else {
        return Vec::new();
    };
    store.get(BIRTHDAYS_KEY).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

fn save<R: Runtime>(app: &AppHandle<R>, birthdays: &[Birthday]) -> Res<()> {
    let store = app.store(STORE_FILE).map_err(|e| format!("could not open the birthdays store: {e}"))?;
    let value = serde_json::to_value(birthdays).map_err(|e| format!("could not encode birthdays: {e}"))?;
    store.set(BIRTHDAYS_KEY, value);
    store.save().map_err(|e| format!("could not write birthdays: {e}"))
}

fn validate(month: u32, day: u32) -> Res<()> {
    if !(1..=12).contains(&month) {
        return Err(format!("{month} is not a valid month (1-12)."));
    }
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        // February: 29 is allowed year-round, since a birthday on the 29th is
        // a real thing to record even though [`safe_date`] shows it on the
        // 28th in a non-leap year.
        2 => 29,
        _ => unreachable!(),
    };
    if day < 1 || day > max_day {
        return Err(format!("{day} is not a valid day for that month."));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Date math (pure, unit-tested against a fixed "today")
// ---------------------------------------------------------------------------

/// `month`/`day` in `year`, falling back to February 28th when `year` is not
/// a leap year and the birthday is on the 29th — the same convention most
/// calendar apps use, rather than either erroring or jumping to March 1st.
fn safe_date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day)
        .or_else(|| NaiveDate::from_ymd_opt(year, month, day.saturating_sub(1)))
        .expect("month/day were validated on the way in")
}

/// The next time this birthday occurs on or after `today` — today itself
/// counts, so someone's birthday shows "0 days" on the day itself rather than
/// jumping straight to next year.
fn next_occurrence(month: u32, day: u32, today: NaiveDate) -> NaiveDate {
    let this_year = safe_date(today.year(), month, day);
    if this_year >= today {
        this_year
    } else {
        safe_date(today.year() + 1, month, day)
    }
}

fn resolve(birthday: &Birthday, today: NaiveDate) -> UpcomingBirthday {
    let next = next_occurrence(birthday.month, birthday.day, today);
    let turning = birthday.year.map(|y| next.year() - y);
    UpcomingBirthday {
        birthday: birthday.clone(),
        next_occurrence: next.format("%Y-%m-%d").to_string(),
        days_until: (next - today).num_days(),
        turning,
    }
}

/// Every birthday, soonest first. Ties (two people with the same next
/// occurrence) keep their original relative order — a stable sort, so the
/// list does not reshuffle itself for no reason on every reload.
pub fn upcoming(birthdays: &[Birthday], today: NaiveDate) -> Vec<UpcomingBirthday> {
    let mut resolved: Vec<UpcomingBirthday> = birthdays.iter().map(|b| resolve(b, today)).collect();
    resolved.sort_by_key(|b| b.days_until);
    resolved
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn birthdays_list<R: Runtime>(app: AppHandle<R>) -> Res<Vec<UpcomingBirthday>> {
    Ok(upcoming(&load(&app), chrono::Local::now().date_naive()))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn birthdays_add<R: Runtime>(
    app: AppHandle<R>,
    name: String,
    month: u32,
    day: u32,
    year: Option<i32>,
    notes: Option<String>,
) -> Res<Birthday> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Give this birthday a name.".into());
    }
    validate(month, day)?;

    let mut birthdays = load(&app);
    let birthday = Birthday {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        month,
        day,
        year,
        notes: notes.unwrap_or_default(),
    };
    birthdays.push(birthday.clone());
    save(&app, &birthdays)?;
    Ok(birthday)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn birthdays_update<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    name: String,
    month: u32,
    day: u32,
    year: Option<i32>,
    notes: Option<String>,
) -> Res<Birthday> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Give this birthday a name.".into());
    }
    validate(month, day)?;

    let mut birthdays = load(&app);
    let entry = birthdays.iter_mut().find(|b| b.id == id).ok_or("That birthday no longer exists.")?;
    entry.name = name;
    entry.month = month;
    entry.day = day;
    entry.year = year;
    entry.notes = notes.unwrap_or_default();
    let updated = entry.clone();
    save(&app, &birthdays)?;
    Ok(updated)
}

#[tauri::command]
pub fn birthdays_delete<R: Runtime>(app: AppHandle<R>, id: String) -> Res<()> {
    let mut birthdays = load(&app);
    birthdays.retain(|b| b.id != id);
    save(&app, &birthdays)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn bday(name: &str, month: u32, day: u32, year: Option<i32>) -> Birthday {
        Birthday { id: name.into(), name: name.into(), month, day, year, notes: String::new() }
    }

    #[test]
    fn a_birthday_today_is_zero_days_away() {
        let next = next_occurrence(7, 28, date("2026-07-28"));
        assert_eq!(next, date("2026-07-28"));
    }

    #[test]
    fn a_birthday_earlier_this_year_rolls_to_next_year() {
        let next = next_occurrence(1, 15, date("2026-07-28"));
        assert_eq!(next, date("2027-01-15"));
    }

    #[test]
    fn a_birthday_later_this_year_stays_in_this_year() {
        let next = next_occurrence(12, 25, date("2026-07-28"));
        assert_eq!(next, date("2026-12-25"));
    }

    #[test]
    fn a_leap_day_birthday_shows_on_february_28th_in_a_non_leap_year() {
        // 2026 is not a leap year.
        let next = next_occurrence(2, 29, date("2026-01-01"));
        assert_eq!(next, date("2026-02-28"));
    }

    #[test]
    fn a_leap_day_birthday_lands_on_the_29th_in_a_leap_year() {
        // 2028 is a leap year.
        let next = next_occurrence(2, 29, date("2028-01-01"));
        assert_eq!(next, date("2028-02-29"));
    }

    #[test]
    fn turning_is_computed_from_the_next_occurrence_year() {
        let list = upcoming(&[bday("Ada", 12, 10, Some(1990))], date("2026-07-28"));
        assert_eq!(list[0].turning, Some(2026 - 1990));
    }

    #[test]
    fn turning_is_none_without_a_birth_year() {
        let list = upcoming(&[bday("Grace", 3, 1, None)], date("2026-07-28"));
        assert_eq!(list[0].turning, None);
    }

    #[test]
    fn the_list_sorts_soonest_first() {
        let list = upcoming(
            &[bday("December", 12, 25, None), bday("Tomorrow", 7, 29, None), bday("Today", 7, 28, None)],
            date("2026-07-28"),
        );
        let names: Vec<&str> = list.iter().map(|b| b.birthday.name.as_str()).collect();
        assert_eq!(names, vec!["Today", "Tomorrow", "December"]);
    }

    #[test]
    fn days_until_is_computed_correctly_across_a_year_boundary() {
        let list = upcoming(&[bday("NewYear", 1, 1, None)], date("2026-12-30"));
        assert_eq!(list[0].days_until, 2);
    }

    #[test]
    fn validate_rejects_an_impossible_month() {
        assert!(validate(13, 1).is_err());
        assert!(validate(0, 1).is_err());
    }

    #[test]
    fn validate_rejects_a_day_that_does_not_exist_in_that_month() {
        assert!(validate(4, 31).is_err()); // April has 30 days
        assert!(validate(2, 30).is_err());
    }

    #[test]
    fn validate_accepts_february_29th() {
        assert!(validate(2, 29).is_ok());
    }
}
