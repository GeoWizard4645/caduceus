//! Live stocks, crypto, and prediction-market quotes — read-only, no account required.
//!
//! # Why this can exist in an app that is proud of not phoning home
//!
//! Every other tool in this crate either runs entirely on-device or, in
//! `rates.rs`, makes one anonymous request for a number that is the same for
//! everyone who asks it that day. A stock quote is different: it changes by
//! the second, and a floating widget wants to poll it. That is a genuinely
//! new kind of request for this app to be making, so it gets a genuinely
//! narrow contract:
//!
//! * every request here carries a ticker or a coin id and nothing that
//!   identifies the person asking — no account, no cookie jar, no session;
//! * nothing the user types, owns, or configures elsewhere in Caduceus is
//!   ever attached to these requests;
//! * every source is usable with zero signup, so "free" is not a plan tier
//!   that a build can quietly regress out of.
//!
//! # Sources, and why each one
//!
//! * **Crypto** — CoinGecko's public `coins/markets` endpoint. Confirmed
//!   working with no key: `curl "https://api.coingecko.com/api/v3/coins/markets\
//!   ?vs_currency=usd&ids=bitcoin"` returns 200 with `cache-control:
//!   max-age=30, public` — the source itself says 30 seconds is the right
//!   granularity, which is where [`CRYPTO_TTL`] comes from. An invalid id is
//!   silently dropped from the array (still 200); an invalid `vs_currency`
//!   is a 400 with `{"error": "..."}`.
//! * **Stocks** — Yahoo Finance's undocumented but longstanding `v7/finance/
//!   spark` endpoint, which is the one Yahoo Finance's own site uses to
//!   sparkline a watchlist and, unlike `v7/finance/quote`, does not demand a
//!   crumb/cookie handshake (confirmed: `quote` returns 401 `Unauthorized`
//!   with no key path available at all; `spark` returns 200 for the same
//!   symbols with no auth of any kind). It batches many tickers in one
//!   request, which matters more than usual here — a widget polling every
//!   few seconds must not turn into one request per symbol. Stooq's CSV
//!   endpoint, the other free-and-keyless candidate, no longer resolves
//!   (`stooq.com/q/l/...` is a 404 as of this writing) and was dropped
//!   rather than shipped against a guess.
//! * **Prediction markets** — Kalshi's `/events` endpoint and Polymarket's
//!   Gamma `/markets` endpoint. Both are genuinely public: no key, no
//!   handshake, plain `curl` returns real order-book prices. Kalshi's flat
//!   `/markets` list is dominated by auto-generated multi-leg parlay
//!   markets with unreadable titles (`"yes Chicago C wins by over 1.5
//!   runs,yes Los Angeles A..."`); `/events?with_nested_markets=true` gives
//!   the human-written event title once and the tradeable legs underneath
//!   it, which is what [`fetch_kalshi_markets`] flattens. Both venues keep
//!   Polymarket's `outcomes`/`outcomePrices` fields as JSON *encoded as a
//!   string* (`"[\"Yes\", \"No\"]"`, not an array) — confirmed by
//!   inspecting the raw response, not assumed — hence the manual
//!   `serde_json::from_str` in [`PolymarketRaw`]'s conversion rather than a
//!   derived `Vec<String>` field.
//!
//! # What was deliberately left out
//!
//! Binance's public ticker was evaluated and dropped: `curl
//! "https://api.binance.com/api/v3/ticker/24hr?symbol=BTCUSDT"` returns
//! HTTP 451 ("Service unavailable from a restricted location") from the
//! network this was built on, which is Binance's standard response to US
//! and several other jurisdictions. Shipping it as the crypto source would
//! mean the flagship feature 451s for a chunk of users on day one; CoinGecko
//! has no such restriction and already covers the same ground.
//!
//! # Optional keys
//!
//! None of the four sources require one. CoinGecko's Demo plan accepts an
//! optional `x-cg-demo-api-key` header on this same public host for a higher
//! rate limit; [`fetch_crypto`] attaches it when
//! `secrets::get_backend_api_key_opt("coingecko")` returns one, and omits
//! the header entirely otherwise — which is also the tested, working path.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::settings::secrets;

/// Applies to every request this module makes. A quote that takes longer than
/// this to arrive is not worth making a poller wait for; the cache (or the
/// prior value the frontend already has) is a better answer than a spinner.
const TIMEOUT: Duration = Duration::from_secs(10);

/// How long a fetched crypto quote is reused. CoinGecko's own response
/// headers advertise `max-age=30` on this endpoint — matching it means this
/// module never asks a question the source has already told it the answer
/// will not change for.
const CRYPTO_TTL: Duration = Duration::from_secs(30);

/// Stock prices move at the same real-world pace as crypto during market
/// hours; the shorter interval below is not about freshness, it is about not
/// re-fetching a whole watchlist every time a widget redraws.
const STOCK_TTL: Duration = Duration::from_secs(30);

/// Prediction markets settle over days or months; a minute of staleness on
/// "will the Fed cut 50bps" costs nothing a viewer would notice.
const PREDICTION_TTL: Duration = Duration::from_secs(60);

/// Bounds both the request (fewer symbols means a smaller response) and the
/// batch endpoints' own practical limits, which are undocumented and not
/// worth finding by accident in production.
const MAX_SYMBOLS: usize = 25;

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(concat!("Caduceus/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("Could not start the request: {e}"))
}

/// Turns a transport failure into something a widget can show instead of a
/// blank slot. The offline case is the common one worth naming explicitly.
fn describe_transport_error(e: &reqwest::Error, what: &str) -> String {
    if e.is_timeout() {
        format!("{what} did not answer in time.")
    } else if e.is_connect() {
        format!("Could not reach {what}. Check that you are online.")
    } else {
        format!("{what} could not be read: {e}")
    }
}

// ---------------------------------------------------------------------------
// Crypto — CoinGecko
// ---------------------------------------------------------------------------

const COINGECKO_ENDPOINT: &str = "https://api.coingecko.com/api/v3/coins/markets";

/// Best-effort ticker → CoinGecko id table for the coins someone is likeliest
/// to type into a search box. CoinGecko's id space has tens of thousands of
/// entries and changes as coins list; this is deliberately not an attempt to
/// cover all of it. Anything not in the table is passed through lowercased on
/// the assumption the caller already has a real CoinGecko id (which is what
/// its own `/coins/list` endpoint hands out) — that keeps the table a
/// convenience rather than a silent ceiling on what can be looked up.
const TICKER_ALIASES: &[(&str, &str)] = &[
    ("BTC", "bitcoin"),
    ("ETH", "ethereum"),
    ("USDT", "tether"),
    ("XRP", "ripple"),
    ("BNB", "binancecoin"),
    ("SOL", "solana"),
    ("USDC", "usd-coin"),
    ("DOGE", "dogecoin"),
    ("ADA", "cardano"),
    ("TRX", "tron"),
    ("AVAX", "avalanche-2"),
    ("SHIB", "shiba-inu"),
    ("DOT", "polkadot"),
    ("LINK", "chainlink"),
    ("LTC", "litecoin"),
    ("MATIC", "matic-network"),
    ("POL", "polygon-ecosystem-token"),
    ("UNI", "uniswap"),
    ("BCH", "bitcoin-cash"),
    ("XLM", "stellar"),
    ("ATOM", "cosmos"),
    ("XMR", "monero"),
];

/// The ids used when a caller does not specify any — a reasonable default
/// watchlist for a widget's first paint before the user has picked anything.
pub const DEFAULT_CRYPTO_IDS: &[&str] =
    &["bitcoin", "ethereum", "solana", "ripple", "dogecoin"];

/// Resolve a user-typed symbol or id into a CoinGecko id. Case-insensitive on
/// the alias table; anything unrecognized is lowercased and returned as-is.
pub fn resolve_crypto_id(input: &str) -> String {
    let trimmed = input.trim();
    let upper = trimmed.to_ascii_uppercase();
    for (ticker, id) in TICKER_ALIASES {
        if *ticker == upper {
            return id.to_string();
        }
    }
    trimmed.to_ascii_lowercase()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CryptoQuote {
    pub id: String,
    pub symbol: String,
    pub name: String,
    pub price: f64,
    pub market_cap: Option<f64>,
    pub market_cap_rank: Option<u32>,
    pub volume_24h: Option<f64>,
    pub high_24h: Option<f64>,
    pub low_24h: Option<f64>,
    pub change_24h: Option<f64>,
    pub change_percent_24h: Option<f64>,
    pub image: Option<String>,
    pub last_updated: Option<String>,
    /// Whether this came from the cache rather than the network just now.
    pub cached: bool,
}

#[derive(Debug, Deserialize)]
struct CoinGeckoRaw {
    id: String,
    symbol: String,
    name: String,
    current_price: Option<f64>,
    market_cap: Option<f64>,
    market_cap_rank: Option<u32>,
    total_volume: Option<f64>,
    high_24h: Option<f64>,
    low_24h: Option<f64>,
    price_change_24h: Option<f64>,
    price_change_percentage_24h: Option<f64>,
    image: Option<String>,
    last_updated: Option<String>,
}

impl From<CoinGeckoRaw> for CryptoQuote {
    fn from(r: CoinGeckoRaw) -> Self {
        CryptoQuote {
            id: r.id,
            symbol: r.symbol.to_uppercase(),
            name: r.name,
            price: r.current_price.unwrap_or(0.0),
            market_cap: r.market_cap,
            market_cap_rank: r.market_cap_rank,
            volume_24h: r.total_volume,
            high_24h: r.high_24h,
            low_24h: r.low_24h,
            change_24h: r.price_change_24h,
            change_percent_24h: r.price_change_percentage_24h,
            image: r.image,
            last_updated: r.last_updated,
            cached: false,
        }
    }
}

#[derive(Deserialize)]
struct CoinGeckoError {
    error: String,
}

#[derive(Default)]
pub struct CryptoCache {
    inner: RwLock<HashMap<String, (u64, Vec<CryptoQuote>)>>,
}

impl CryptoCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(vs_currency: &str, ids: &[String]) -> String {
        let mut sorted = ids.to_vec();
        sorted.sort();
        sorted.dedup();
        format!("{}:{}", vs_currency.to_ascii_lowercase(), sorted.join(","))
    }

    fn get(&self, key: &str) -> Option<Vec<CryptoQuote>> {
        let guard = self.inner.read();
        let (fetched_at, quotes) = guard.get(key)?;
        if now_secs().saturating_sub(*fetched_at) > CRYPTO_TTL.as_secs() {
            return None;
        }
        Some(quotes.iter().cloned().map(|q| CryptoQuote { cached: true, ..q }).collect())
    }

    fn put(&self, key: String, quotes: &[CryptoQuote]) {
        self.inner.write().insert(key, (now_secs(), quotes.to_vec()));
    }
}

/// Fetch (or reuse) quotes for a set of CoinGecko coin ids.
///
/// `ids` should already be CoinGecko ids — pass user input through
/// [`resolve_crypto_id`] first. Ids CoinGecko does not recognize are simply
/// absent from the result rather than erroring the whole call, matching what
/// the API itself does (an unknown id returns `200 []`, not a 404).
pub async fn fetch_crypto(
    cache: &CryptoCache,
    ids: &[String],
    vs_currency: &str,
) -> Result<Vec<CryptoQuote>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let vs_currency = vs_currency.trim();
    let vs_currency = if vs_currency.is_empty() { "usd" } else { vs_currency };
    let ids: Vec<String> = ids.iter().take(MAX_SYMBOLS).cloned().collect();

    let key = CryptoCache::key(vs_currency, &ids);
    if let Some(hit) = cache.get(&key) {
        return Ok(hit);
    }

    let url = format!(
        "{COINGECKO_ENDPOINT}?vs_currency={}&ids={}&price_change_percentage=24h",
        urlencode(vs_currency),
        ids.join(",")
    );

    let mut req = client()?.get(&url);
    if let Some(api_key) = secrets::get_backend_api_key_opt("coingecko") {
        req = req.header("x-cg-demo-api-key", api_key);
    }

    let response = req
        .send()
        .await
        .map_err(|e| describe_transport_error(&e, "CoinGecko"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if let Ok(err) = serde_json::from_str::<CoinGeckoError>(&body) {
            return Err(format!("CoinGecko said: {}", err.error));
        }
        if status.as_u16() == 429 {
            return Err("CoinGecko is rate-limiting this app right now. Try again shortly.".into());
        }
        return Err(format!("CoinGecko returned {}.", status.as_u16()));
    }

    let raw: Vec<CoinGeckoRaw> = response
        .json()
        .await
        .map_err(|_| "CoinGecko sent something unreadable.".to_string())?;

    let quotes: Vec<CryptoQuote> = raw.into_iter().map(CryptoQuote::from).collect();
    cache.put(key, &quotes);
    Ok(quotes)
}

// ---------------------------------------------------------------------------
// Stocks — Yahoo Finance
// ---------------------------------------------------------------------------

const YAHOO_SPARK_ENDPOINT: &str = "https://query1.finance.yahoo.com/v7/finance/spark";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StockQuote {
    pub symbol: String,
    pub name: String,
    pub price: f64,
    pub previous_close: f64,
    pub change: f64,
    pub change_percent: f64,
    pub day_high: Option<f64>,
    pub day_low: Option<f64>,
    pub volume: Option<u64>,
    pub currency: String,
    pub exchange: Option<String>,
    pub cached: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StockQuotesResult {
    pub quotes: Vec<StockQuote>,
    /// Symbols that were asked for but Yahoo did not return — typically a
    /// typo or a delisted ticker, surfaced instead of silently dropped.
    pub not_found: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct YahooSparkResponse {
    spark: YahooSparkOuter,
}

#[derive(Debug, Deserialize)]
struct YahooSparkOuter {
    result: Option<Vec<YahooSparkEntry>>,
}

#[derive(Debug, Deserialize)]
struct YahooSparkEntry {
    symbol: String,
    response: Vec<YahooSparkResponseItem>,
}

#[derive(Debug, Deserialize)]
struct YahooSparkResponseItem {
    meta: YahooMeta,
}

#[derive(Debug, Deserialize)]
struct YahooMeta {
    symbol: String,
    currency: Option<String>,
    #[serde(rename = "fullExchangeName")]
    full_exchange_name: Option<String>,
    #[serde(rename = "longName")]
    long_name: Option<String>,
    #[serde(rename = "shortName")]
    short_name: Option<String>,
    #[serde(rename = "regularMarketPrice")]
    regular_market_price: Option<f64>,
    #[serde(rename = "previousClose")]
    previous_close: Option<f64>,
    #[serde(rename = "chartPreviousClose")]
    chart_previous_close: Option<f64>,
    #[serde(rename = "regularMarketDayHigh")]
    regular_market_day_high: Option<f64>,
    #[serde(rename = "regularMarketDayLow")]
    regular_market_day_low: Option<f64>,
    #[serde(rename = "regularMarketVolume")]
    regular_market_volume: Option<u64>,
}

/// Turn one Yahoo meta block into a quote, or `None` if it lacks the one
/// field ("what is the price") that makes it worth showing at all.
fn meta_to_quote(meta: YahooMeta) -> Option<StockQuote> {
    let price = meta.regular_market_price?;
    let previous_close = meta.previous_close.or(meta.chart_previous_close).unwrap_or(price);
    let change = price - previous_close;
    let change_percent = if previous_close != 0.0 { (change / previous_close) * 100.0 } else { 0.0 };
    Some(StockQuote {
        symbol: meta.symbol,
        name: meta.long_name.or(meta.short_name).unwrap_or_default(),
        price,
        previous_close,
        change,
        change_percent,
        day_high: meta.regular_market_day_high,
        day_low: meta.regular_market_day_low,
        volume: meta.regular_market_volume,
        currency: meta.currency.unwrap_or_else(|| "USD".into()),
        exchange: meta.full_exchange_name,
        cached: false,
    })
}

#[derive(Default)]
pub struct StockCache {
    inner: RwLock<HashMap<String, (u64, StockQuotesResult)>>,
}

impl StockCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(symbols: &[String]) -> String {
        let mut sorted: Vec<String> = symbols.iter().map(|s| s.to_ascii_uppercase()).collect();
        sorted.sort();
        sorted.dedup();
        sorted.join(",")
    }

    fn get(&self, key: &str) -> Option<StockQuotesResult> {
        let guard = self.inner.read();
        let (fetched_at, result) = guard.get(key)?;
        if now_secs().saturating_sub(*fetched_at) > STOCK_TTL.as_secs() {
            return None;
        }
        Some(StockQuotesResult {
            quotes: result.quotes.iter().cloned().map(|q| StockQuote { cached: true, ..q }).collect(),
            not_found: result.not_found.clone(),
        })
    }

    fn put(&self, key: String, result: &StockQuotesResult) {
        self.inner.write().insert(key, (now_secs(), result.clone()));
    }
}

/// Fetch (or reuse) quotes for up to [`MAX_SYMBOLS`] stock tickers in one
/// request. Symbols are matched case-insensitively against what Yahoo
/// returns; anything requested but absent from the response ends up in
/// [`StockQuotesResult::not_found`] rather than being dropped silently.
pub async fn fetch_stocks(cache: &StockCache, symbols: &[String]) -> Result<StockQuotesResult, String> {
    let symbols: Vec<String> = symbols
        .iter()
        .map(|s| s.trim().to_ascii_uppercase())
        .filter(|s| !s.is_empty())
        .take(MAX_SYMBOLS)
        .collect();
    if symbols.is_empty() {
        return Ok(StockQuotesResult { quotes: Vec::new(), not_found: Vec::new() });
    }

    let key = StockCache::key(&symbols);
    if let Some(hit) = cache.get(&key) {
        return Ok(hit);
    }

    let url = format!(
        "{YAHOO_SPARK_ENDPOINT}?symbols={}&range=1d&interval=15m",
        symbols.iter().map(|s| urlencode(s)).collect::<Vec<_>>().join(",")
    );

    let response = client()?
        .get(&url)
        .send()
        .await
        .map_err(|e| describe_transport_error(&e, "Yahoo Finance"))?;

    if !response.status().is_success() {
        return Err(format!("Yahoo Finance returned {}.", response.status().as_u16()));
    }

    let parsed: YahooSparkResponse = response
        .json()
        .await
        .map_err(|_| "Yahoo Finance sent something unreadable.".to_string())?;

    let entries = parsed.spark.result.unwrap_or_default();
    let mut found: HashMap<String, StockQuote> = HashMap::new();
    for entry in entries {
        let symbol_key = entry.symbol.to_ascii_uppercase();
        if let Some(item) = entry.response.into_iter().next() {
            if let Some(quote) = meta_to_quote(item.meta) {
                found.insert(symbol_key, quote);
            }
        }
    }

    let mut quotes = Vec::with_capacity(found.len());
    let mut not_found = Vec::new();
    for symbol in &symbols {
        match found.remove(symbol) {
            Some(q) => quotes.push(q),
            None => not_found.push(symbol.clone()),
        }
    }

    let result = StockQuotesResult { quotes, not_found };
    cache.put(key, &result);
    Ok(result)
}

// ---------------------------------------------------------------------------
// Prediction markets — Kalshi
// ---------------------------------------------------------------------------

const KALSHI_EVENTS_ENDPOINT: &str = "https://api.elections.kalshi.com/trade-api/v2/events";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KalshiMarket {
    pub event_ticker: String,
    pub market_ticker: String,
    pub category: String,
    /// The event's human-written title — e.g. "Who will the next Pope be?" —
    /// which is what a person searching Kalshi's site would recognize,
    /// unlike the flat `/markets` endpoint's auto-generated combo titles.
    pub title: String,
    /// The specific outcome this market prices, when the event has more than
    /// one leg (e.g. "Pope Pietro Parolin" under the Pope event). `None` when
    /// the event has exactly one market and the title already says it all.
    pub subtitle: Option<String>,
    pub yes_bid: f64,
    pub yes_ask: f64,
    pub no_bid: f64,
    pub no_ask: f64,
    pub last_price: f64,
    pub volume: f64,
    pub status: String,
    pub close_time: String,
    pub cached: bool,
}

#[derive(Debug, Deserialize)]
struct KalshiEventsResponse {
    events: Vec<KalshiEventRaw>,
}

#[derive(Debug, Deserialize)]
struct KalshiEventRaw {
    event_ticker: String,
    #[serde(default)]
    category: String,
    title: String,
    #[serde(default)]
    markets: Vec<KalshiMarketRaw>,
}

#[derive(Debug, Deserialize)]
struct KalshiMarketRaw {
    ticker: String,
    #[serde(default)]
    yes_sub_title: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    close_time: String,
    // Kalshi sends every price as a decimal-dollar *string* (e.g. "0.1300"),
    // not a JSON number — matched by curling the endpoint, not guessed.
    yes_bid_dollars: String,
    yes_ask_dollars: String,
    no_bid_dollars: String,
    no_ask_dollars: String,
    last_price_dollars: String,
    #[serde(default)]
    volume_fp: String,
}

fn parse_kalshi_dollars(s: &str) -> f64 {
    s.parse().unwrap_or(0.0)
}

#[derive(Default)]
pub struct KalshiCache {
    inner: RwLock<Option<(u64, Vec<KalshiMarket>)>>,
}

impl KalshiCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn get(&self) -> Option<Vec<KalshiMarket>> {
        let guard = self.inner.read();
        let (fetched_at, markets) = guard.as_ref()?;
        if now_secs().saturating_sub(*fetched_at) > PREDICTION_TTL.as_secs() {
            return None;
        }
        Some(markets.iter().cloned().map(|m| KalshiMarket { cached: true, ..m }).collect())
    }

    fn put(&self, markets: &[KalshiMarket]) {
        *self.inner.write() = Some((now_secs(), markets.to_vec()));
    }
}

/// Fetch (or reuse) the currently open Kalshi markets, flattened one row per
/// tradeable leg, capped at `limit` events (each of which may contribute more
/// than one row).
pub async fn fetch_kalshi_markets(cache: &KalshiCache, limit: usize) -> Result<Vec<KalshiMarket>, String> {
    if let Some(hit) = cache.get() {
        return Ok(hit);
    }

    let limit = limit.clamp(1, 100);
    let url = format!(
        "{KALSHI_EVENTS_ENDPOINT}?limit={limit}&status=open&with_nested_markets=true"
    );

    let response = client()?
        .get(&url)
        .send()
        .await
        .map_err(|e| describe_transport_error(&e, "Kalshi"))?;

    if !response.status().is_success() {
        return Err(format!("Kalshi returned {}.", response.status().as_u16()));
    }

    let parsed: KalshiEventsResponse = response
        .json()
        .await
        .map_err(|_| "Kalshi sent something unreadable.".to_string())?;

    let mut markets = Vec::new();
    for event in parsed.events {
        let single_market = event.markets.len() == 1;
        for m in event.markets {
            markets.push(KalshiMarket {
                event_ticker: event.event_ticker.clone(),
                market_ticker: m.ticker,
                category: event.category.clone(),
                title: event.title.clone(),
                subtitle: if single_market || m.yes_sub_title.is_empty() {
                    None
                } else {
                    Some(m.yes_sub_title)
                },
                yes_bid: parse_kalshi_dollars(&m.yes_bid_dollars),
                yes_ask: parse_kalshi_dollars(&m.yes_ask_dollars),
                no_bid: parse_kalshi_dollars(&m.no_bid_dollars),
                no_ask: parse_kalshi_dollars(&m.no_ask_dollars),
                last_price: parse_kalshi_dollars(&m.last_price_dollars),
                volume: parse_kalshi_dollars(&m.volume_fp),
                status: m.status,
                close_time: m.close_time,
                cached: false,
            });
        }
    }

    cache.put(&markets);
    Ok(markets)
}

// ---------------------------------------------------------------------------
// Prediction markets — Polymarket
// ---------------------------------------------------------------------------

const POLYMARKET_ENDPOINT: &str = "https://gamma-api.polymarket.com/markets";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolymarketMarket {
    pub id: String,
    pub question: String,
    pub slug: String,
    pub outcomes: Vec<String>,
    pub outcome_prices: Vec<f64>,
    pub volume_24h: f64,
    pub liquidity: f64,
    pub end_date: Option<String>,
    pub cached: bool,
}

#[derive(Debug, Deserialize)]
struct PolymarketRaw {
    id: String,
    question: String,
    slug: String,
    // Confirmed live: Polymarket's Gamma API encodes these two array fields
    // as JSON *inside a string* (`"outcomes":"[\"Yes\", \"No\"]"`), not as a
    // real JSON array — so they cannot be derived as `Vec<String>` directly.
    #[serde(default)]
    outcomes: String,
    #[serde(rename = "outcomePrices", default)]
    outcome_prices: String,
    #[serde(rename = "volume24hr", default)]
    volume_24hr: f64,
    #[serde(default)]
    liquidity: String,
    #[serde(rename = "endDate", default)]
    end_date: Option<String>,
}

impl From<PolymarketRaw> for PolymarketMarket {
    fn from(r: PolymarketRaw) -> Self {
        let outcomes: Vec<String> = serde_json::from_str(&r.outcomes).unwrap_or_default();
        let outcome_prices: Vec<f64> = serde_json::from_str::<Vec<String>>(&r.outcome_prices)
            .unwrap_or_default()
            .iter()
            .filter_map(|s| s.parse().ok())
            .collect();
        PolymarketMarket {
            id: r.id,
            question: r.question,
            slug: r.slug,
            outcomes,
            outcome_prices,
            volume_24h: r.volume_24hr,
            liquidity: r.liquidity.parse().unwrap_or(0.0),
            end_date: r.end_date,
            cached: false,
        }
    }
}

#[derive(Default)]
pub struct PolymarketCache {
    inner: RwLock<Option<(u64, Vec<PolymarketMarket>)>>,
}

impl PolymarketCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn get(&self) -> Option<Vec<PolymarketMarket>> {
        let guard = self.inner.read();
        let (fetched_at, markets) = guard.as_ref()?;
        if now_secs().saturating_sub(*fetched_at) > PREDICTION_TTL.as_secs() {
            return None;
        }
        Some(markets.iter().cloned().map(|m| PolymarketMarket { cached: true, ..m }).collect())
    }

    fn put(&self, markets: &[PolymarketMarket]) {
        *self.inner.write() = Some((now_secs(), markets.to_vec()));
    }
}

/// Fetch (or reuse) the highest-volume open Polymarket markets.
pub async fn fetch_polymarket_markets(
    cache: &PolymarketCache,
    limit: usize,
) -> Result<Vec<PolymarketMarket>, String> {
    if let Some(hit) = cache.get() {
        return Ok(hit);
    }

    let limit = limit.clamp(1, 100);
    let url = format!(
        "{POLYMARKET_ENDPOINT}?limit={limit}&closed=false&order=volume24hr&ascending=false"
    );

    let response = client()?
        .get(&url)
        .send()
        .await
        .map_err(|e| describe_transport_error(&e, "Polymarket"))?;

    if !response.status().is_success() {
        return Err(format!("Polymarket returned {}.", response.status().as_u16()));
    }

    let raw: Vec<PolymarketRaw> = response
        .json()
        .await
        .map_err(|_| "Polymarket sent something unreadable.".to_string())?;

    let markets: Vec<PolymarketMarket> = raw.into_iter().map(PolymarketMarket::from).collect();
    cache.put(&markets);
    Ok(markets)
}

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Recorded payloads -------------------------------------------------
    //
    // Each of these is a real response body, fetched with `curl` against the
    // live endpoint while writing this module (see the module doc comment for
    // the exact commands). None of these tests touch the network.

    const COINGECKO_SAMPLE: &str = r#"[{"id":"bitcoin","symbol":"btc","name":"Bitcoin","image":"https://coin-images.coingecko.com/coins/images/1/large/bitcoin.png?1696501400","current_price":63236,"market_cap":1268723428054,"market_cap_rank":1,"fully_diluted_valuation":1268723428054,"total_volume":26668820367,"high_24h":65618,"low_24h":63038,"price_change_24h":-2075.618004914417,"price_change_percentage_24h":-3.1,"market_cap_change_24h":-41549339708.648926,"market_cap_change_percentage_24h":-3.17105,"circulating_supply":20062193.0,"total_supply":20062228.0,"max_supply":21000000.0,"ath":126080,"ath_change_percentage":-49.84429,"ath_date":"2025-10-06T10:57:42.000Z","atl":67.81,"atl_change_percentage":93156.43931,"atl_date":"2013-07-05T16:00:00.000Z","roi":null,"last_updated":"2026-07-28T03:36:30.000Z","price_change_percentage_24h_in_currency":-3.1},{"id":"ethereum","symbol":"eth","name":"Ethereum","image":"https://coin-images.coingecko.com/coins/images/279/large/ethereum.png?1696501628","current_price":1876.28,"market_cap":226428787410,"market_cap_rank":2,"fully_diluted_valuation":226428787410,"total_volume":11979198714,"high_24h":1973.9,"low_24h":1866.32,"price_change_24h":-77.42437624723016,"price_change_percentage_24h":-3.7,"market_cap_change_24h":-9349684368.100708,"market_cap_change_percentage_24h":-3.96545,"circulating_supply":120682600.3094177,"total_supply":120682600.3094177,"max_supply":null,"ath":4946.05,"ath_change_percentage":-62.065,"ath_date":"2025-08-24T11:21:03.000Z","atl":0.432979,"atl_change_percentage":433242.96604,"atl_date":"2015-10-19T16:00:00.000Z","roi":{"times":38.671188008885515,"currency":"btc","percentage":3867.118800888551},"last_updated":"2026-07-28T03:36:30.000Z","price_change_percentage_24h_in_currency":-3.7}]"#;

    const YAHOO_SPARK_SAMPLE: &str = r#"{"spark":{"result":[{"symbol":"MSFT","response":[{"meta":{"currency":"USD","symbol":"MSFT","exchangeName":"NMS","fullExchangeName":"NasdaqGS","instrumentType":"EQUITY","regularMarketTime":1785182401,"hasPrePostMarketData":true,"regularMarketPrice":389.1,"fiftyTwoWeekHigh":555.45,"fiftyTwoWeekLow":349.2,"regularMarketDayHigh":394.2,"regularMarketDayLow":387.99,"regularMarketVolume":27753671,"longName":"Microsoft Corporation","shortName":"Microsoft Corporation","chartPreviousClose":381.7,"previousClose":381.7,"dataGranularity":"5m","range":"1d"}}]},{"symbol":"AAPL","response":[{"meta":{"currency":"USD","symbol":"AAPL","exchangeName":"NMS","fullExchangeName":"NasdaqGS","instrumentType":"EQUITY","regularMarketTime":1785182401,"hasPrePostMarketData":true,"regularMarketPrice":336.91,"fiftyTwoWeekHigh":339.57,"fiftyTwoWeekLow":201.5,"regularMarketDayHigh":339.57,"regularMarketDayLow":334.02,"regularMarketVolume":45246885,"longName":"Apple Inc.","shortName":"Apple Inc.","chartPreviousClose":333.02,"previousClose":333.02,"dataGranularity":"5m","range":"1d"}}]}]}}"#;

    const KALSHI_SAMPLE: &str = r#"{"cursor":"abc","events":[{"available_on_brokers":true,"category":"World","event_ticker":"KXELONMARS-99","series_ticker":"KXELONMARS","title":"Will Elon Musk visit Mars in his lifetime?","markets":[{"ticker":"KXELONMARS-99-99","close_time":"2099-08-01T04:59:00Z","status":"active","yes_sub_title":"","last_price_dollars":"0.1300","yes_ask_dollars":"0.1300","yes_bid_dollars":"0.1200","no_ask_dollars":"0.8800","no_bid_dollars":"0.8700","volume_fp":"40010.97"}]},{"available_on_brokers":true,"category":"Elections","event_ticker":"KXNEWPOPE-70","series_ticker":"KXNEWPOPE","title":"Who will the next Pope be?","markets":[{"ticker":"KXNEWPOPE-70-PAROLIN","close_time":"2070-01-01T04:59:00Z","status":"active","yes_sub_title":"Pietro Parolin","last_price_dollars":"0.0700","yes_ask_dollars":"0.0800","yes_bid_dollars":"0.0600","no_ask_dollars":"0.9400","no_bid_dollars":"0.9200","volume_fp":"1200.00"},{"ticker":"KXNEWPOPE-70-TAGLE","close_time":"2070-01-01T04:59:00Z","status":"active","yes_sub_title":"Luis Antonio Tagle","last_price_dollars":"0.0500","yes_ask_dollars":"0.0600","yes_bid_dollars":"0.0400","no_ask_dollars":"0.9600","no_bid_dollars":"0.9400","volume_fp":"800.00"}]}]}"#;

    const POLYMARKET_SAMPLE: &str = r#"[{"id":"1654956","question":"Will the Fed decrease interest rates by 50+ bps after the July 2026 meeting?","conditionId":"0x3d675f","slug":"will-the-fed-decrease-interest-rates-by-50-bps-after-the-july-2026-meeting","endDate":"2026-07-29T00:00:00Z","liquidity":"2517853.46657","outcomes":"[\"Yes\", \"No\"]","outcomePrices":"[\"0.0015\", \"0.9985\"]","volume24hr":2963203.6330970004,"active":true,"closed":false}]"#;

    // ---- Crypto --------------------------------------------------------

    #[test]
    fn coingecko_payload_parses_into_typed_quotes() {
        let raw: Vec<CoinGeckoRaw> = serde_json::from_str(COINGECKO_SAMPLE).unwrap();
        let quotes: Vec<CryptoQuote> = raw.into_iter().map(CryptoQuote::from).collect();
        assert_eq!(quotes.len(), 2);
        assert_eq!(quotes[0].id, "bitcoin");
        assert_eq!(quotes[0].symbol, "BTC", "symbol should be uppercased for display");
        assert_eq!(quotes[0].price, 63236.0);
        assert!(quotes[0].change_percent_24h.unwrap() < 0.0);
        assert_eq!(quotes[1].market_cap_rank, Some(2));
    }

    #[test]
    fn a_ticker_alias_resolves_case_insensitively() {
        assert_eq!(resolve_crypto_id("btc"), "bitcoin");
        assert_eq!(resolve_crypto_id("BTC"), "bitcoin");
        assert_eq!(resolve_crypto_id("Eth"), "ethereum");
    }

    #[test]
    fn an_unknown_ticker_passes_through_lowercased() {
        // Not in the alias table, but might be a real CoinGecko id already.
        assert_eq!(resolve_crypto_id("Pepe-Coin"), "pepe-coin");
    }

    #[test]
    fn crypto_cache_key_ignores_id_order_and_case() {
        let a = CryptoCache::key("usd", &["bitcoin".into(), "ethereum".into()]);
        let b = CryptoCache::key("USD", &["ethereum".into(), "bitcoin".into()]);
        assert_eq!(a, b);
    }

    #[test]
    fn a_fresh_crypto_entry_is_reused_and_marked_cached() {
        let cache = CryptoCache::new();
        let raw: Vec<CoinGeckoRaw> = serde_json::from_str(COINGECKO_SAMPLE).unwrap();
        let quotes: Vec<CryptoQuote> = raw.into_iter().map(CryptoQuote::from).collect();
        let key = CryptoCache::key("usd", &["bitcoin".into(), "ethereum".into()]);
        cache.put(key.clone(), &quotes);
        let hit = cache.get(&key).expect("should hit");
        assert!(hit[0].cached);
    }

    #[test]
    fn a_stale_crypto_entry_is_not_reused() {
        let cache = CryptoCache::new();
        let key = "usd:bitcoin".to_string();
        let old = now_secs() - CRYPTO_TTL.as_secs() - 1;
        cache.inner.write().insert(key.clone(), (old, Vec::new()));
        assert!(cache.get(&key).is_none());
    }

    #[test]
    fn coingecko_error_body_is_readable() {
        let body = r#"{"error":"invalid vs_currency"}"#;
        let err: CoinGeckoError = serde_json::from_str(body).unwrap();
        assert_eq!(err.error, "invalid vs_currency");
    }

    // ---- Stocks ----------------------------------------------------------

    #[test]
    fn yahoo_spark_payload_parses_into_typed_quotes() {
        let parsed: YahooSparkResponse = serde_json::from_str(YAHOO_SPARK_SAMPLE).unwrap();
        let mut entries = parsed.spark.result.unwrap();
        assert_eq!(entries.len(), 2);
        let pos = entries.iter().position(|e| e.symbol == "MSFT").unwrap();
        let msft = entries.remove(pos);
        let quote = meta_to_quote(msft.response.into_iter().next().unwrap().meta).unwrap();
        assert_eq!(quote.symbol, "MSFT");
        assert_eq!(quote.price, 389.1);
        assert_eq!(quote.previous_close, 381.7);
        assert!((quote.change - 7.4).abs() < 0.01);
    }

    #[test]
    fn requested_symbols_absent_from_the_response_are_reported_not_found() {
        let parsed: YahooSparkResponse = serde_json::from_str(YAHOO_SPARK_SAMPLE).unwrap();
        let entries = parsed.spark.result.unwrap();
        let mut found: HashMap<String, StockQuote> = HashMap::new();
        for entry in entries {
            let quote = meta_to_quote(entry.response.into_iter().next().unwrap().meta).unwrap();
            found.insert(entry.symbol.to_ascii_uppercase(), quote);
        }
        let requested = vec!["AAPL".to_string(), "MSFT".to_string(), "NOTASYMBOL".to_string()];
        let mut not_found = Vec::new();
        for s in &requested {
            if !found.contains_key(s) {
                not_found.push(s.clone());
            }
        }
        assert_eq!(not_found, vec!["NOTASYMBOL".to_string()]);
    }

    #[test]
    fn a_meta_block_with_no_price_yields_no_quote() {
        let meta = YahooMeta {
            symbol: "DEAD".into(),
            currency: None,
            full_exchange_name: None,
            long_name: None,
            short_name: None,
            regular_market_price: None,
            previous_close: None,
            chart_previous_close: None,
            regular_market_day_high: None,
            regular_market_day_low: None,
            regular_market_volume: None,
        };
        assert!(meta_to_quote(meta).is_none());
    }

    #[test]
    fn missing_previous_close_falls_back_to_chart_previous_close() {
        let meta = YahooMeta {
            symbol: "AAPL".into(),
            currency: Some("USD".into()),
            full_exchange_name: None,
            long_name: Some("Apple Inc.".into()),
            short_name: None,
            regular_market_price: Some(336.91),
            previous_close: None,
            chart_previous_close: Some(333.02),
            regular_market_day_high: None,
            regular_market_day_low: None,
            regular_market_volume: None,
        };
        let quote = meta_to_quote(meta).unwrap();
        assert_eq!(quote.previous_close, 333.02);
    }

    #[test]
    fn stock_cache_key_normalizes_case_and_order() {
        let a = StockCache::key(&["aapl".into(), "MSFT".into()]);
        let b = StockCache::key(&["msft".into(), "AAPL".into()]);
        assert_eq!(a, b);
    }

    // ---- Kalshi ------------------------------------------------------------

    #[test]
    fn kalshi_payload_flattens_events_into_one_row_per_market() {
        let parsed: KalshiEventsResponse = serde_json::from_str(KALSHI_SAMPLE).unwrap();
        assert_eq!(parsed.events.len(), 2);
        let total_markets: usize = parsed.events.iter().map(|e| e.markets.len()).sum();
        assert_eq!(total_markets, 3, "one Mars market plus two Pope candidates");
    }

    #[test]
    fn a_single_market_event_gets_no_redundant_subtitle() {
        let parsed: KalshiEventsResponse = serde_json::from_str(KALSHI_SAMPLE).unwrap();
        let mars = &parsed.events[0];
        assert_eq!(mars.markets.len(), 1);
        assert_eq!(mars.markets[0].yes_sub_title, "");
    }

    #[test]
    fn a_multi_leg_event_keeps_the_per_leg_subtitle() {
        let parsed: KalshiEventsResponse = serde_json::from_str(KALSHI_SAMPLE).unwrap();
        let pope = &parsed.events[1];
        assert_eq!(pope.markets.len(), 2);
        assert_eq!(pope.markets[0].yes_sub_title, "Pietro Parolin");
    }

    #[test]
    fn kalshi_dollar_strings_parse_as_floats() {
        assert_eq!(parse_kalshi_dollars("0.1300"), 0.13);
        assert_eq!(parse_kalshi_dollars(""), 0.0, "an unparsable string degrades to zero, not a panic");
    }

    // ---- Polymarket ----------------------------------------------------

    #[test]
    fn polymarket_stringified_arrays_are_unpacked() {
        let raw: Vec<PolymarketRaw> = serde_json::from_str(POLYMARKET_SAMPLE).unwrap();
        let markets: Vec<PolymarketMarket> = raw.into_iter().map(PolymarketMarket::from).collect();
        assert_eq!(markets.len(), 1);
        assert_eq!(markets[0].outcomes, vec!["Yes".to_string(), "No".to_string()]);
        assert_eq!(markets[0].outcome_prices, vec![0.0015, 0.9985]);
        assert!((markets[0].liquidity - 2517853.46657).abs() < 0.01);
    }

    // ---- Shared --------------------------------------------------------

    #[test]
    fn urlencoding_escapes_a_plus_sign_in_a_ticker() {
        // Kalshi/CoinGecko ids do not need it, but a hostile or malformed
        // symbol should never end up splicing a raw character into the URL.
        assert_eq!(urlencode("BRK.B"), "BRK.B");
        assert_eq!(urlencode("A B"), "A%20B");
    }
}
