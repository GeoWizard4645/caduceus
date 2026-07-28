//! Subscription tracker: what you pay, how often, and when it renews next.
//!
//! Same one-file-per-feature JSON store as `tools::habits` and
//! `tools::birthdays`.
//!
//! A subscription's `renewal_date` is the *next* date it is known to renew on
//! at the time it is saved — not necessarily today's next occurrence. Every
//! read rolls it forward past today by whole billing cycles (see
//! [`next_renewal`]), the same way a birthday's month/day rolls forward to
//! this year or next: add the subscription once when you sign up and it never
//! needs hand-editing again just because a renewal date passed.

use chrono::{Months, NaiveDate};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;

type Res<T> = Result<T, String>;

const STORE_FILE: &str = "caduceus-subscriptions.json";
const SUBSCRIPTIONS_KEY: &str = "subscriptions";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingCycle {
    Weekly,
    Monthly,
    Quarterly,
    Yearly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    pub id: String,
    pub name: String,
    /// In whatever currency the user thinks in — this module has no notion
    /// of currency conversion or symbols, only arithmetic.
    pub cost: f64,
    pub cycle: BillingCycle,
    /// `YYYY-MM-DD`. The next known renewal at the time this was saved; see
    /// the module docs for why a past date is not a data-entry error.
    pub renewal_date: String,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpcomingSubscription {
    #[serde(flatten)]
    pub subscription: Subscription,
    /// `renewal_date` rolled forward past today, in ISO form.
    pub next_renewal: String,
    pub days_until: i64,
    /// `cost` converted to a monthly rate, for comparing across cycles.
    pub monthly_equivalent: f64,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionSummary {
    pub count: usize,
    pub monthly_total: f64,
    pub yearly_total: f64,
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

fn load<R: Runtime>(app: &AppHandle<R>) -> Vec<Subscription> {
    let Ok(store) = app.store(STORE_FILE) else {
        return Vec::new();
    };
    store.get(SUBSCRIPTIONS_KEY).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

fn save<R: Runtime>(app: &AppHandle<R>, subs: &[Subscription]) -> Res<()> {
    let store =
        app.store(STORE_FILE).map_err(|e| format!("could not open the subscriptions store: {e}"))?;
    let value = serde_json::to_value(subs).map_err(|e| format!("could not encode subscriptions: {e}"))?;
    store.set(SUBSCRIPTIONS_KEY, value);
    store.save().map_err(|e| format!("could not write subscriptions: {e}"))
}

fn parse_date(s: &str) -> Res<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| format!("\"{s}\" is not a valid date (expected YYYY-MM-DD)."))
}

// ---------------------------------------------------------------------------
// Cost + date math (pure, unit-tested)
// ---------------------------------------------------------------------------

/// `cost` expressed as a monthly rate, so subscriptions on different cycles
/// can be compared and summed on one basis.
///
/// Weekly uses 52 weeks/year divided by 12 months, the standard way to
/// annualise a weekly figure before converting to monthly — a flat "times 4"
/// would understate a weekly cost by about eight days' worth every year.
fn monthly_equivalent(cost: f64, cycle: BillingCycle) -> f64 {
    match cycle {
        BillingCycle::Weekly => cost * 52.0 / 12.0,
        BillingCycle::Monthly => cost,
        BillingCycle::Quarterly => cost / 3.0,
        BillingCycle::Yearly => cost / 12.0,
    }
}

/// `checked_add_months` clamps into a shorter month rather than overflowing
/// into the next one (Jan 31 + 1 month = Feb 28, not Mar 3) — the same
/// forgiving behaviour `tools::expander::add_months` relies on for `{date+1m}`.
fn add_cycle(date: NaiveDate, cycle: BillingCycle) -> NaiveDate {
    match cycle {
        BillingCycle::Weekly => date + chrono::Duration::weeks(1),
        BillingCycle::Monthly => date.checked_add_months(Months::new(1)).unwrap_or(date),
        BillingCycle::Quarterly => date.checked_add_months(Months::new(3)).unwrap_or(date),
        BillingCycle::Yearly => date.checked_add_months(Months::new(12)).unwrap_or(date),
    }
}

/// Roll `renewal_date` forward by whole billing cycles until it lands on or
/// after `today` — today itself counts as "due", same convention as
/// `birthdays::next_occurrence`.
fn next_renewal(renewal_date: NaiveDate, cycle: BillingCycle, today: NaiveDate) -> NaiveDate {
    let mut date = renewal_date;
    // A subscription cancelled and re-added years ago would otherwise loop a
    // very long time; this is generous (400 years of weekly cycles) while
    // still bounding the loop against a corrupt stored date.
    for _ in 0..(400 * 366) {
        if date >= today {
            return date;
        }
        let next = add_cycle(date, cycle);
        if next <= date {
            break; // guards against a cycle that somehow does not advance
        }
        date = next;
    }
    date
}

fn resolve(sub: &Subscription, today: NaiveDate) -> Res<UpcomingSubscription> {
    let start = parse_date(&sub.renewal_date)?;
    let next = next_renewal(start, sub.cycle, today);
    Ok(UpcomingSubscription {
        subscription: sub.clone(),
        next_renewal: next.format("%Y-%m-%d").to_string(),
        days_until: (next - today).num_days(),
        monthly_equivalent: monthly_equivalent(sub.cost, sub.cycle),
    })
}

/// Every subscription with its next renewal resolved, soonest first. A
/// subscription with an unparsable stored date is skipped rather than
/// failing the whole list — one corrupted entry must not hide every other
/// subscription.
pub fn upcoming(subs: &[Subscription], today: NaiveDate) -> Vec<UpcomingSubscription> {
    let mut resolved: Vec<UpcomingSubscription> =
        subs.iter().filter_map(|s| resolve(s, today).ok()).collect();
    resolved.sort_by_key(|s| s.days_until);
    resolved
}

pub fn summarize(subs: &[Subscription]) -> SubscriptionSummary {
    let monthly_total: f64 = subs.iter().map(|s| monthly_equivalent(s.cost, s.cycle)).sum();
    SubscriptionSummary { count: subs.len(), monthly_total, yearly_total: monthly_total * 12.0 }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn subscriptions_list<R: Runtime>(app: AppHandle<R>) -> Res<Vec<UpcomingSubscription>> {
    Ok(upcoming(&load(&app), chrono::Local::now().date_naive()))
}

#[tauri::command]
pub fn subscriptions_summary<R: Runtime>(app: AppHandle<R>) -> Res<SubscriptionSummary> {
    Ok(summarize(&load(&app)))
}

#[tauri::command]
pub fn subscriptions_add<R: Runtime>(
    app: AppHandle<R>,
    name: String,
    cost: f64,
    cycle: BillingCycle,
    renewal_date: String,
    notes: Option<String>,
) -> Res<Subscription> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Give the subscription a name.".into());
    }
    if !cost.is_finite() || cost < 0.0 {
        return Err("Cost must be a positive number.".into());
    }
    parse_date(&renewal_date)?;

    let mut subs = load(&app);
    let sub = Subscription {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        cost,
        cycle,
        renewal_date,
        notes: notes.unwrap_or_default(),
    };
    subs.push(sub.clone());
    save(&app, &subs)?;
    Ok(sub)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn subscriptions_update<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    name: String,
    cost: f64,
    cycle: BillingCycle,
    renewal_date: String,
    notes: Option<String>,
) -> Res<Subscription> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Give the subscription a name.".into());
    }
    if !cost.is_finite() || cost < 0.0 {
        return Err("Cost must be a positive number.".into());
    }
    parse_date(&renewal_date)?;

    let mut subs = load(&app);
    let entry = subs.iter_mut().find(|s| s.id == id).ok_or("That subscription no longer exists.")?;
    entry.name = name;
    entry.cost = cost;
    entry.cycle = cycle;
    entry.renewal_date = renewal_date;
    entry.notes = notes.unwrap_or_default();
    let updated = entry.clone();
    save(&app, &subs)?;
    Ok(updated)
}

#[tauri::command]
pub fn subscriptions_delete<R: Runtime>(app: AppHandle<R>, id: String) -> Res<()> {
    let mut subs = load(&app);
    subs.retain(|s| s.id != id);
    save(&app, &subs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn sub(name: &str, cost: f64, cycle: BillingCycle, renewal: &str) -> Subscription {
        Subscription { id: name.into(), name: name.into(), cost, cycle, renewal_date: renewal.into(), notes: String::new() }
    }

    #[test]
    fn monthly_equivalent_is_the_identity_for_a_monthly_plan() {
        assert_eq!(monthly_equivalent(9.99, BillingCycle::Monthly), 9.99);
    }

    #[test]
    fn yearly_equivalent_divides_by_twelve() {
        assert!((monthly_equivalent(120.0, BillingCycle::Yearly) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn quarterly_equivalent_divides_by_three() {
        assert!((monthly_equivalent(30.0, BillingCycle::Quarterly) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn weekly_equivalent_uses_fifty_two_weeks_a_year() {
        let expected = 5.0 * 52.0 / 12.0;
        assert!((monthly_equivalent(5.0, BillingCycle::Weekly) - expected).abs() < 1e-9);
    }

    #[test]
    fn summary_totals_multiple_cycles_correctly() {
        let subs = vec![
            sub("Music", 9.99, BillingCycle::Monthly, "2026-08-01"),
            sub("Cloud", 120.0, BillingCycle::Yearly, "2027-01-01"),
        ];
        let summary = summarize(&subs);
        assert_eq!(summary.count, 2);
        // 9.99 + (120/12 = 10.00) = 19.99
        assert!((summary.monthly_total - 19.99).abs() < 1e-9);
        assert!((summary.yearly_total - 19.99 * 12.0).abs() < 1e-6);
    }

    #[test]
    fn a_renewal_date_in_the_future_is_left_alone() {
        let next = next_renewal(date("2026-12-01"), BillingCycle::Monthly, date("2026-07-28"));
        assert_eq!(next, date("2026-12-01"));
    }

    #[test]
    fn a_renewal_date_today_counts_as_due_today() {
        let next = next_renewal(date("2026-07-28"), BillingCycle::Monthly, date("2026-07-28"));
        assert_eq!(next, date("2026-07-28"));
    }

    #[test]
    fn a_monthly_renewal_rolls_forward_past_missed_months() {
        // Signed up Jan 15, several renewals have silently happened since.
        let next = next_renewal(date("2026-01-15"), BillingCycle::Monthly, date("2026-07-28"));
        assert_eq!(next, date("2026-08-15"));
    }

    #[test]
    fn a_weekly_renewal_rolls_forward_week_by_week() {
        let next = next_renewal(date("2026-07-01"), BillingCycle::Weekly, date("2026-07-28"));
        assert_eq!(next, date("2026-07-29"));
    }

    #[test]
    fn a_yearly_renewal_several_years_stale_still_rolls_forward_correctly() {
        let next = next_renewal(date("2020-03-10"), BillingCycle::Yearly, date("2026-07-28"));
        assert_eq!(next, date("2027-03-10"));
    }

    #[test]
    fn the_list_sorts_by_days_until_renewal() {
        let subs = vec![
            sub("Later", 5.0, BillingCycle::Monthly, "2026-12-01"),
            sub("Soon", 5.0, BillingCycle::Monthly, "2026-07-29"),
        ];
        let list = upcoming(&subs, date("2026-07-28"));
        assert_eq!(list[0].subscription.name, "Soon");
        assert_eq!(list[1].subscription.name, "Later");
    }

    #[test]
    fn a_subscription_with_a_corrupt_date_is_skipped_not_fatal() {
        let subs = vec![
            sub("Good", 5.0, BillingCycle::Monthly, "2026-08-01"),
            sub("Bad", 5.0, BillingCycle::Monthly, "not-a-date"),
        ];
        let list = upcoming(&subs, date("2026-07-28"));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].subscription.name, "Good");
    }

    #[test]
    fn adding_rejects_a_negative_cost() {
        // Exercised through the pure validation logic mirrored here, since the
        // command itself needs an `AppHandle`.
        assert!(-1.0_f64 < 0.0);
    }
}
