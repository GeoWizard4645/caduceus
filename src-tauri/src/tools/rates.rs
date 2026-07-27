//! Currency rates — the one conversion that needs the internet.
//!
//! # Why this is separated from every other unit
//!
//! A metre is a metre. An inch has been exactly 25.4mm since 1959, and
//! `shared/units.ts` converts between them with arithmetic on constants that
//! cannot go stale and cannot fail.
//!
//! A euro is worth whatever it is worth this afternoon. So currency is the only
//! conversion in Caduceus that:
//!
//! * needs a network request, in an app whose entire premise is that it does
//!   not need one;
//! * can be **wrong** rather than merely unavailable, if the answer is old;
//! * has to say where its number came from and when.
//!
//! Keeping it in its own module, behind its own setting, means none of that
//! leaks into "convert 10 km to miles" — which must keep working on a plane.
//!
//! # What it talks to
//!
//! [Frankfurter](https://frankfurter.dev), which republishes the European
//! Central Bank's daily reference rates. No account, no API key, no tracking,
//! and rates that are published once a day — which is the right granularity for
//! "roughly how much is that in pounds" and honest about not being a trading
//! feed. The request contains a base currency and nothing else.
//!
//! Nothing here runs unless the user asks for a currency conversion.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// How long a fetched set of rates is reused before going back to the network.
///
/// The source publishes once a working day, so anything under that is asking a
/// question whose answer has not changed. Six hours keeps a long-running app
/// current across a weekend without being chatty.
const CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);

const ENDPOINT: &str = "https://api.frankfurter.dev/v1/latest";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RateTable {
    pub base: String,
    /// Currency code → how many of it one `base` buys.
    pub rates: std::collections::BTreeMap<String, f64>,
    /// The day the source published these, as `YYYY-MM-DD`.
    pub date: String,
    /// Where the numbers came from, shown in the UI. Never blank.
    pub source: String,
    /// Whether this came from the cache rather than the network just now.
    pub cached: bool,
}

#[derive(Debug, Deserialize)]
struct FrankfurterResponse {
    base: String,
    date: String,
    rates: std::collections::BTreeMap<String, f64>,
}

#[derive(Default)]
pub struct RateCache {
    inner: RwLock<Option<(u64, RateTable)>>,
}

impl RateCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn get(&self, base: &str) -> Option<RateTable> {
        let guard = self.inner.read();
        let (fetched_at, table) = guard.as_ref()?;
        if !table.base.eq_ignore_ascii_case(base) {
            return None;
        }
        if now_secs().saturating_sub(*fetched_at) > CACHE_TTL.as_secs() {
            return None;
        }
        Some(RateTable { cached: true, ..table.clone() })
    }

    fn put(&self, table: &RateTable) {
        *self.inner.write() = Some((now_secs(), table.clone()));
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Fetch (or reuse) the rate table for a base currency.
///
/// Errors are written for someone who is offline, because that is the common
/// case and "error sending request" is not an explanation.
pub async fn fetch(cache: &RateCache, base: &str) -> Result<RateTable, String> {
    let base = base.trim().to_uppercase();
    if base.len() != 3 || !base.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(format!("\"{base}\" is not a three-letter currency code."));
    }

    if let Some(hit) = cache.get(&base) {
        return Ok(hit);
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Could not build the HTTP client: {e}"))?;

    // Interpolated rather than `.query(...)`: reqwest is built here with
    // `default-features = false`, which leaves out the query-string encoder.
    // `base` has already been checked to be three ASCII letters, so there is
    // nothing here that needs escaping.
    let response = client
        .get(format!("{ENDPOINT}?base={base}"))
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                "The exchange-rate service did not answer in time. Rates need the internet; \
                 every other conversion in Caduceus works offline."
                    .to_string()
            } else {
                "Could not reach the exchange-rate service. Rates need the internet; every \
                 other conversion in Caduceus works offline."
                    .to_string()
            }
        })?;

    if !response.status().is_success() {
        return Err(format!(
            "The exchange-rate service returned {}. Try again shortly.",
            response.status().as_u16()
        ));
    }

    let parsed: FrankfurterResponse = response
        .json()
        .await
        .map_err(|_| "The exchange-rate service sent something unreadable.".to_string())?;

    let table = RateTable {
        base: parsed.base,
        rates: parsed.rates,
        date: parsed.date,
        source: "European Central Bank, via frankfurter.dev".into(),
        cached: false,
    };
    cache.put(&table);
    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(base: &str) -> RateTable {
        RateTable {
            base: base.into(),
            rates: [("USD".to_string(), 1.08)].into_iter().collect(),
            date: "2026-07-27".into(),
            source: "test".into(),
            cached: false,
        }
    }

    #[test]
    fn a_fresh_entry_is_reused_and_marked_as_cached() {
        let cache = RateCache::new();
        cache.put(&table("EUR"));
        let hit = cache.get("EUR").expect("should hit");
        assert!(hit.cached, "a cache hit must say so, or the UI claims it is live");
        assert_eq!(hit.rates["USD"], 1.08);
    }

    #[test]
    fn the_base_currency_is_part_of_the_key() {
        // Handing back EUR-based rates for a GBP question would be wrong by
        // whatever the exchange rate happens to be — silently.
        let cache = RateCache::new();
        cache.put(&table("EUR"));
        assert!(cache.get("GBP").is_none());
    }

    #[test]
    fn the_base_currency_match_ignores_case() {
        let cache = RateCache::new();
        cache.put(&table("EUR"));
        assert!(cache.get("eur").is_some());
    }

    #[test]
    fn a_stale_entry_is_not_reused() {
        let cache = RateCache::new();
        let old = now_secs() - CACHE_TTL.as_secs() - 1;
        *cache.inner.write() = Some((old, table("EUR")));
        assert!(cache.get("EUR").is_none());
    }
}
