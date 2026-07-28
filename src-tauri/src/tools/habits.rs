//! Habit tracker: create a habit, mark a day done, watch the streak.
//!
//! Persists to its own `tauri_plugin_store` file — `crate::tools::expander`'s
//! precedent for a tools submodule that owns a small JSON store rather than
//! growing `crate::settings::Settings`, so a bug in this feature can never
//! corrupt or force a migration of the shared config file.
//!
//! Dates are stored as plain `YYYY-MM-DD` strings, never a timestamp: a habit
//! is done *on a day*, in whatever time zone the person doing it experiences
//! that day in, and a stored instant would have to carry a time zone to mean
//! the same thing on the day it was recorded and the day it is displayed.

use std::collections::BTreeSet;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;

type Res<T> = Result<T, String>;

const STORE_FILE: &str = "caduceus-habits.json";
const HABITS_KEY: &str = "habits";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Habit {
    pub id: String,
    pub name: String,
    /// `YYYY-MM-DD`, the day the habit was created — shown as "tracking
    /// since" rather than used in any streak math.
    pub created_at: String,
    /// `#rrggbb`, or empty for "no colour chosen".
    pub color: String,
    /// Every day this habit was marked done, as `YYYY-MM-DD`. A `Vec` rather
    /// than a set on disk (plain JSON array, easy to hand-inspect); streak
    /// math below converts it to a `BTreeSet<NaiveDate>` once per call.
    pub completions: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreakInfo {
    pub current: u32,
    pub longest: u32,
    pub total_completions: u32,
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

fn load<R: Runtime>(app: &AppHandle<R>) -> Vec<Habit> {
    let Ok(store) = app.store(STORE_FILE) else {
        return Vec::new();
    };
    store.get(HABITS_KEY).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

fn save<R: Runtime>(app: &AppHandle<R>, habits: &[Habit]) -> Res<()> {
    let store = app.store(STORE_FILE).map_err(|e| format!("could not open the habits store: {e}"))?;
    let value = serde_json::to_value(habits).map_err(|e| format!("could not encode habits: {e}"))?;
    store.set(HABITS_KEY, value);
    store.save().map_err(|e| format!("could not write habits: {e}"))
}

// ---------------------------------------------------------------------------
// Streak math (pure, unit-tested against a fixed "today")
// ---------------------------------------------------------------------------

fn parse_dates(completions: &[String]) -> BTreeSet<NaiveDate> {
    completions.iter().filter_map(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).collect()
}

/// How many days in a row, ending today or yesterday, have been marked done.
///
/// Ending *yesterday* still counts as an active streak — a habit marked done
/// every day through yesterday and not yet touched today has not been broken
/// yet, just not done today. It only breaks once a day is skipped entirely.
fn current_streak(dates: &BTreeSet<NaiveDate>, today: NaiveDate) -> u32 {
    let start = if dates.contains(&today) {
        today
    } else if dates.contains(&today.pred_opt().unwrap_or(today)) {
        today.pred_opt().unwrap_or(today)
    } else {
        return 0;
    };

    let mut streak = 0u32;
    let mut day = start;
    loop {
        if !dates.contains(&day) {
            break;
        }
        streak += 1;
        match day.pred_opt() {
            Some(prev) => day = prev,
            None => break,
        }
    }
    streak
}

/// The longest run of consecutive calendar days anywhere in the history, not
/// just the one touching today.
fn longest_streak(dates: &BTreeSet<NaiveDate>) -> u32 {
    let mut longest = 0u32;
    let mut running = 0u32;
    let mut prev: Option<NaiveDate> = None;

    for &date in dates {
        running = match prev {
            Some(p) if p.succ_opt() == Some(date) => running + 1,
            _ => 1,
        };
        longest = longest.max(running);
        prev = Some(date);
    }
    longest
}

pub fn streak_info(habit: &Habit, today: NaiveDate) -> StreakInfo {
    let dates = parse_dates(&habit.completions);
    StreakInfo {
        current: current_streak(&dates, today),
        longest: longest_streak(&dates),
        total_completions: dates.len() as u32,
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn habits_list<R: Runtime>(app: AppHandle<R>) -> Res<Vec<Habit>> {
    Ok(load(&app))
}

#[tauri::command]
pub fn habits_create<R: Runtime>(app: AppHandle<R>, name: String, color: Option<String>) -> Res<Habit> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Give the habit a name.".into());
    }
    let mut habits = load(&app);
    let habit = Habit {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        created_at: chrono::Local::now().format("%Y-%m-%d").to_string(),
        color: color.unwrap_or_default(),
        completions: Vec::new(),
    };
    habits.push(habit.clone());
    save(&app, &habits)?;
    Ok(habit)
}

#[tauri::command]
pub fn habits_delete<R: Runtime>(app: AppHandle<R>, id: String) -> Res<()> {
    let mut habits = load(&app);
    habits.retain(|h| h.id != id);
    save(&app, &habits)
}

/// Flip whether `date` (`YYYY-MM-DD`) is marked done for habit `id`.
#[tauri::command]
pub fn habits_toggle_day<R: Runtime>(app: AppHandle<R>, id: String, date: String) -> Res<Habit> {
    if NaiveDate::parse_from_str(&date, "%Y-%m-%d").is_err() {
        return Err(format!("\"{date}\" is not a valid date (expected YYYY-MM-DD)."));
    }
    let mut habits = load(&app);
    let habit = habits.iter_mut().find(|h| h.id == id).ok_or("That habit no longer exists.")?;
    if let Some(pos) = habit.completions.iter().position(|d| d == &date) {
        habit.completions.remove(pos);
    } else {
        habit.completions.push(date);
    }
    let updated = habit.clone();
    save(&app, &habits)?;
    Ok(updated)
}

#[tauri::command]
pub fn habits_streak<R: Runtime>(app: AppHandle<R>, id: String) -> Res<StreakInfo> {
    let habits = load(&app);
    let habit = habits.iter().find(|h| h.id == id).ok_or("That habit no longer exists.")?;
    Ok(streak_info(habit, chrono::Local::now().date_naive()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn dates(days: &[&str]) -> BTreeSet<NaiveDate> {
        days.iter().map(|d| date(d)).collect()
    }

    #[test]
    fn a_streak_ending_today_counts_every_consecutive_day() {
        let d = dates(&["2026-07-25", "2026-07-26", "2026-07-27", "2026-07-28"]);
        assert_eq!(current_streak(&d, date("2026-07-28")), 4);
    }

    #[test]
    fn a_streak_not_yet_done_today_still_counts_through_yesterday() {
        let d = dates(&["2026-07-26", "2026-07-27"]);
        assert_eq!(current_streak(&d, date("2026-07-28")), 2);
    }

    #[test]
    fn a_gap_of_two_days_breaks_the_streak_entirely() {
        let d = dates(&["2026-07-20", "2026-07-25"]);
        assert_eq!(current_streak(&d, date("2026-07-28")), 0);
    }

    #[test]
    fn a_habit_never_marked_done_has_no_streak() {
        assert_eq!(current_streak(&BTreeSet::new(), date("2026-07-28")), 0);
    }

    #[test]
    fn longest_streak_finds_the_best_run_even_if_it_is_not_current() {
        // A five-day run in the past, then a gap, then a two-day current run.
        let d = dates(&[
            "2026-07-01", "2026-07-02", "2026-07-03", "2026-07-04", "2026-07-05",
            "2026-07-27", "2026-07-28",
        ]);
        assert_eq!(longest_streak(&d), 5);
        assert_eq!(current_streak(&d, date("2026-07-28")), 2);
    }

    #[test]
    fn a_single_completion_is_a_streak_of_one() {
        let d = dates(&["2026-07-28"]);
        assert_eq!(current_streak(&d, date("2026-07-28")), 1);
        assert_eq!(longest_streak(&d), 1);
    }

    #[test]
    fn streak_info_reports_total_completions_regardless_of_gaps() {
        let habit = Habit {
            id: "h1".into(),
            name: "Read".into(),
            created_at: "2026-01-01".into(),
            color: String::new(),
            completions: vec!["2026-07-01".into(), "2026-07-28".into(), "not-a-date".into()],
        };
        let info = streak_info(&habit, date("2026-07-28"));
        // The unparsable entry is dropped, not counted or crashed on.
        assert_eq!(info.total_completions, 2);
        assert_eq!(info.current, 1);
    }

    #[test]
    fn toggling_the_same_day_twice_is_a_no_op_on_the_set() {
        let mut completions: Vec<String> = Vec::new();
        let d = "2026-07-28".to_string();
        // Mirrors the toggle logic in `habits_toggle_day` without the store.
        if let Some(pos) = completions.iter().position(|x| x == &d) {
            completions.remove(pos);
        } else {
            completions.push(d.clone());
        }
        assert_eq!(completions, vec![d.clone()]);
        if let Some(pos) = completions.iter().position(|x| x == &d) {
            completions.remove(pos);
        } else {
            completions.push(d.clone());
        }
        assert!(completions.is_empty());
    }
}
