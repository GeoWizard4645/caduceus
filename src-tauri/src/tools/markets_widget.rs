//! Tauri commands that let a floating widget ask for live market and sports
//! data — the glue between the pure library functions in `markets.rs` /
//! `sports.rs` and a mounted `MarketWidget` / `SportsWidget` component.
//!
//! # Why this file exists instead of adding to `commands.rs`
//!
//! Every other tool in this crate is wired up by the crate owner: a command
//! in `commands.rs` takes a `tauri::State<'_, SomeCache>`, and `lib.rs::setup`
//! calls `app.manage(SomeCache::new())` once at launch so that state exists
//! by the time the first command runs (see `rates::fetch` / `RateCache` for
//! the canonical example). Neither `markets.rs` nor `sports.rs` had that
//! wiring anywhere in the crate when this file was written, and the brief for
//! this change was explicit: touch neither `commands.rs` nor `lib.rs`. Rather
//! than block on someone else's edit landing first, this file is
//! self-sufficient — it manages its own cache state the same way
//! `widgets.rs::ensure_managed` already does for `WidgetRuntime`: lazily,
//! from inside the command itself, the first time a widget actually asks for
//! data. See `ensure_managed` below. (`lib.rs` can still be given
//! `app.manage(...)` calls for these same cache types later purely as an
//! optimization — nothing here depends on it, since `ensure_managed` no-ops
//! once something else has already registered the state.)
//!
//! That laziness is not just a workaround for the file boundary — it is
//! also exactly what the product requires. A widget that has never been
//! created must never cause a network request, a cache allocation, or a poll
//! of any kind, and the app must not prefetch anything at startup. Managing
//! the caches from inside the command rather than at app startup means "no
//! widget has asked yet" and "no market data has ever been fetched" are the
//! same state, by construction — there is nothing for anyone to remember to
//! gate.
//!
//! # What is deliberately *not* here
//!
//! No polling. Every command below is a single fetch-or-reuse-from-cache
//! call; the interval on which a widget calls them lives entirely in
//! `src/widgets/MarketWidget.tsx` and `SportsWidget.tsx`, started in a
//! `useEffect` on mount and torn down on unmount. That split keeps the TTLs
//! `markets.rs`/`sports.rs` already chose (crypto/stocks 30s, sports 15s,
//! prediction markets 60s) as the one place they are defined — this file and
//! the frontend both just ask "now", as often as the frontend decides to, and
//! the cache underneath answers `cached: true` instead of re-fetching for
//! anything asked again before its TTL is up.

use tauri::{AppHandle, Manager, Runtime};

use super::markets;
use super::sports;

type Res<T> = Result<T, String>;

/// Managed state for the five caches this module's commands share, created
/// on first use rather than at app startup — see the module docs. Mirrors
/// `widgets::ensure_managed` exactly: check, then `manage()` only if absent.
/// `app.manage` for a type that is already managed silently keeps the
/// existing instance, but the `try_state` check avoids even that surprise and
/// documents the intent: at most one instance of each cache ever exists.
fn ensure_managed<R: Runtime>(app: &AppHandle<R>) {
    if app.try_state::<markets::CryptoCache>().is_none() {
        app.manage(markets::CryptoCache::new());
    }
    if app.try_state::<markets::StockCache>().is_none() {
        app.manage(markets::StockCache::new());
    }
    if app.try_state::<markets::KalshiCache>().is_none() {
        app.manage(markets::KalshiCache::new());
    }
    if app.try_state::<markets::PolymarketCache>().is_none() {
        app.manage(markets::PolymarketCache::new());
    }
    if app.try_state::<sports::SportsCache>().is_none() {
        app.manage(sports::SportsCache::new());
    }
}

// ---------------------------------------------------------------------------
// Crypto / stocks / prediction markets
// ---------------------------------------------------------------------------

/// Fetch crypto quotes for a widget's configured coin list. `ids` may be
/// tickers ("BTC") or CoinGecko ids ("bitcoin") — each one is resolved with
/// [`markets::resolve_crypto_id`] the same way a hand-typed search box would
/// be, so a widget's saved config can hold whatever the user originally typed
/// instead of forcing it to already be a canonical CoinGecko id.
#[tauri::command]
pub async fn widgets_fetch_crypto<R: Runtime>(
    app: AppHandle<R>,
    ids: Vec<String>,
    vs_currency: Option<String>,
) -> Res<Vec<markets::CryptoQuote>> {
    ensure_managed(&app);
    let cache = app.state::<markets::CryptoCache>();
    let resolved: Vec<String> = ids.iter().map(|s| markets::resolve_crypto_id(s)).collect();
    markets::fetch_crypto(&cache, &resolved, vs_currency.as_deref().unwrap_or("usd")).await
}

/// Fetch stock quotes for a widget's configured ticker list.
#[tauri::command]
pub async fn widgets_fetch_stocks<R: Runtime>(
    app: AppHandle<R>,
    symbols: Vec<String>,
) -> Res<markets::StockQuotesResult> {
    ensure_managed(&app);
    let cache = app.state::<markets::StockCache>();
    markets::fetch_stocks(&cache, &symbols).await
}

/// Fetch the current highest-volume open Kalshi markets. `limit` defaults to
/// a handful of rows — plenty for a widget's own small list, and far under
/// the cap `markets::fetch_kalshi_markets` itself clamps to.
#[tauri::command]
pub async fn widgets_fetch_kalshi<R: Runtime>(
    app: AppHandle<R>,
    limit: Option<usize>,
) -> Res<Vec<markets::KalshiMarket>> {
    ensure_managed(&app);
    let cache = app.state::<markets::KalshiCache>();
    markets::fetch_kalshi_markets(&cache, limit.unwrap_or(10)).await
}

/// Fetch the current highest-volume open Polymarket markets.
#[tauri::command]
pub async fn widgets_fetch_polymarket<R: Runtime>(
    app: AppHandle<R>,
    limit: Option<usize>,
) -> Res<Vec<markets::PolymarketMarket>> {
    ensure_managed(&app);
    let cache = app.state::<markets::PolymarketCache>();
    markets::fetch_polymarket_markets(&cache, limit.unwrap_or(10)).await
}

// ---------------------------------------------------------------------------
// Sports
// ---------------------------------------------------------------------------

/// The league names a widget's saved config may hold, matched
/// case-insensitively so "NFL", "nfl" and "Nfl" all resolve the same way —
/// nothing about a widget's persisted `kind` string is validated at save
/// time (see `MarketWidget`/`SportsWidget`'s kind-encoding in
/// `src/widgets/marketApi.ts`), so parsing here has to be as forgiving as
/// `resolve_crypto_id` is for coin ids.
fn parse_league(input: &str) -> Res<sports::League> {
    match input.trim().to_ascii_lowercase().as_str() {
        "nfl" => Ok(sports::League::Nfl),
        "nba" => Ok(sports::League::Nba),
        "mlb" => Ok(sports::League::Mlb),
        "f1" => Ok(sports::League::F1),
        "worldcup" | "world-cup" | "world_cup" => Ok(sports::League::WorldCup),
        other => Err(format!("Unknown league \"{other}\".")),
    }
}

/// Fetch the current scoreboard for a team sport. `league` is one of "nfl",
/// "nba", "mlb", or "worldcup" (case-insensitive) — use [`widgets_fetch_f1`]
/// for F1, whose scoreboard has a different shape, the same split
/// `sports::fetch_scoreboard` itself enforces one layer down.
#[tauri::command]
pub async fn widgets_fetch_scoreboard<R: Runtime>(
    app: AppHandle<R>,
    league: String,
) -> Res<sports::Scoreboard> {
    ensure_managed(&app);
    let league = parse_league(&league)?;
    let cache = app.state::<sports::SportsCache>();
    sports::fetch_scoreboard(&cache, league).await
}

/// Fetch the current Formula 1 race weekend(s): practice, qualifying, and
/// race sessions, each with its top finishers.
#[tauri::command]
pub async fn widgets_fetch_f1<R: Runtime>(app: AppHandle<R>) -> Res<sports::RaceScoreboard> {
    ensure_managed(&app);
    let cache = app.state::<sports::SportsCache>();
    sports::fetch_f1(&cache).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn league_names_parse_case_insensitively() {
        assert!(matches!(parse_league("NFL"), Ok(sports::League::Nfl)));
        assert!(matches!(parse_league("worldcup"), Ok(sports::League::WorldCup)));
        assert!(matches!(parse_league("World-Cup"), Ok(sports::League::WorldCup)));
        assert!(matches!(parse_league("f1"), Ok(sports::League::F1)));
    }

    #[test]
    fn an_unknown_league_is_rejected_with_a_readable_message_naming_it() {
        let err = parse_league("nhl").unwrap_err();
        assert!(err.contains("nhl"));
    }
}
