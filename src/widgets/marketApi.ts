/**
 * Everything `MarketWidget.tsx` and `SportsWidget.tsx` need that is not
 * itself UI: typed mirrors of the Rust payloads in
 * `src-tauri/src/tools/markets.rs` / `sports.rs`, thin `invoke()` wrappers
 * around the commands in `src-tauri/src/tools/markets_widget.rs`, the
 * encode/parse pair for a widget's `kind` string, and small formatting/
 * online-status helpers both widgets need. One file rather than two (a
 * `sportsApi.ts` twin) because this change owns exactly one new frontend
 * module for backend calls — see the file-ownership notes on the task this
 * shipped under.
 *
 * # Configuration lives inside `kind`, not a new field
 *
 * `WidgetLayout` (`src/widgets/types.ts`, mirroring `widgets.rs`) has no
 * "widget config" field, and this change is not allowed to add one — that
 * would mean editing `widgets.rs`. But `kind` is already persisted with the
 * rest of a widget's layout and, per `widgets.rs`'s own module docs, is
 * "handed to the webview verbatim... Rust never branches on this string" —
 * it is opaque to the Rust side by design. That makes it exactly the right
 * place to carry "which tickers" or "which team": encoding config into
 * `kind` (e.g. `market:crypto:BTC,ETH,SOL`) gets it saved and restored for
 * free through the persistence `widgets.rs` already has, with zero changes
 * to that file. [`encodeMarketKind`]/[`parseMarketKind`] and their sports
 * counterparts are the two ends of that encoding.
 */

import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

// ---------------------------------------------------------------------------
// Payload types — mirror the `#[serde(rename_all = "camelCase")]` Rust
// structs in markets.rs / sports.rs by hand, the same way types.ts mirrors
// WidgetLayout. No shared codegen between the two sides anywhere in this
// crate; this is not an exception to that.
// ---------------------------------------------------------------------------

export interface CryptoQuote {
  id: string;
  symbol: string;
  name: string;
  price: number;
  marketCap: number | null;
  marketCapRank: number | null;
  volume24h: number | null;
  high24h: number | null;
  low24h: number | null;
  change24h: number | null;
  changePercent24h: number | null;
  image: string | null;
  lastUpdated: string | null;
  cached: boolean;
}

export interface StockQuote {
  symbol: string;
  name: string;
  price: number;
  previousClose: number;
  change: number;
  changePercent: number;
  dayHigh: number | null;
  dayLow: number | null;
  volume: number | null;
  currency: string;
  exchange: string | null;
  cached: boolean;
}

export interface StockQuotesResult {
  quotes: StockQuote[];
  notFound: string[];
}

export interface KalshiMarket {
  eventTicker: string;
  marketTicker: string;
  category: string;
  title: string;
  subtitle: string | null;
  yesBid: number;
  yesAsk: number;
  noBid: number;
  noAsk: number;
  lastPrice: number;
  volume: number;
  status: string;
  closeTime: string;
  cached: boolean;
}

export interface PolymarketMarket {
  id: string;
  question: string;
  slug: string;
  outcomes: string[];
  outcomePrices: number[];
  volume24h: number;
  liquidity: number;
  endDate: string | null;
  cached: boolean;
}

export interface GameStatus {
  /** "pre" | "in" | "post" — ESPN's own vocabulary, passed through as-is. */
  state: string;
  detail: string;
  completed: boolean;
}

export interface TeamScore {
  team: string;
  abbreviation: string;
  homeAway: "home" | "away" | string;
  score: string;
  winner: boolean | null;
}

export interface GameEvent {
  id: string;
  name: string;
  shortName: string;
  date: string;
  status: GameStatus;
  competitors: TeamScore[];
}

export interface Scoreboard {
  league: string;
  events: GameEvent[];
  cached: boolean;
}

export interface DriverResult {
  position: number;
  driver: string;
  winner: boolean;
}

export interface RaceSession {
  session: string;
  completed: boolean;
  top: DriverResult[];
}

export interface RaceWeekend {
  id: string;
  name: string;
  date: string;
  status: GameStatus;
  sessions: RaceSession[];
}

export interface RaceScoreboard {
  weekends: RaceWeekend[];
  cached: boolean;
}

// ---------------------------------------------------------------------------
// Commands — src-tauri/src/tools/markets_widget.rs
// ---------------------------------------------------------------------------

export function fetchCrypto(ids: string[], vsCurrency = "usd"): Promise<CryptoQuote[]> {
  return invoke("widgets_fetch_crypto", { ids, vsCurrency });
}

export function fetchStocks(symbols: string[]): Promise<StockQuotesResult> {
  return invoke("widgets_fetch_stocks", { symbols });
}

export function fetchKalshiMarkets(limit?: number): Promise<KalshiMarket[]> {
  return invoke("widgets_fetch_kalshi", { limit });
}

export function fetchPolymarketMarkets(limit?: number): Promise<PolymarketMarket[]> {
  return invoke("widgets_fetch_polymarket", { limit });
}

export function fetchScoreboard(league: string): Promise<Scoreboard> {
  return invoke("widgets_fetch_scoreboard", { league });
}

export function fetchF1(): Promise<RaceScoreboard> {
  return invoke("widgets_fetch_f1");
}

// ---------------------------------------------------------------------------
// Poll intervals — must match, not outrun, the TTLs markets.rs/sports.rs
// already chose (crypto/stocks 30s, sports 15s, prediction markets 60s).
// Polling faster than a cache's TTL cannot get fresher data, it can only
// spend more IPC round-trips finding out the answer has not changed yet —
// see the module docs on markets.rs for exactly why each number is what it
// is. Kept here, not re-derived from a constant shared with Rust, because
// there is no codegen bridge in this crate for that either; a comment
// pointing at the source of truth is the existing convention.
// ---------------------------------------------------------------------------

export const CRYPTO_POLL_MS = 30_000;
export const STOCK_POLL_MS = 30_000;
export const PREDICTION_POLL_MS = 60_000;
export const SPORTS_POLL_MS = 15_000;

// ---------------------------------------------------------------------------
// Market widget kind encoding — "market:<source>:<config>"
// ---------------------------------------------------------------------------

export type MarketSource = "crypto" | "stocks" | "kalshi" | "polymarket";

export type MarketWidgetConfig =
  | { source: "crypto"; ids: string[] }
  | { source: "stocks"; symbols: string[] }
  | { source: "kalshi"; limit: number }
  | { source: "polymarket"; limit: number };

/** What a brand-new market widget shows before anyone has configured it. */
export const DEFAULT_MARKET_KIND = "market:crypto:BTC,ETH,SOL";

/**
 * Parse a widget's `kind` string into market config, or `null` if it is not
 * a market kind at all (e.g. `"clock"`, `"sports:nfl"`, or something a
 * future build invented). Deliberately forgiving of malformed tails — a
 * hand-edited store file or a future format change should degrade to "use
 * the default", never to a crash inside a floating window with no console.
 */
export function parseMarketKind(kind: string): MarketWidgetConfig | null {
  const parts = kind.split(":");
  if (parts[0] !== "market") return null;
  const source = parts[1] as MarketSource | undefined;
  const rest = parts.slice(2).join(":");

  switch (source) {
    case "crypto": {
      const ids = splitList(rest);
      return { source: "crypto", ids: ids.length > 0 ? ids : ["BTC", "ETH", "SOL"] };
    }
    case "stocks": {
      const symbols = splitList(rest);
      return { source: "stocks", symbols: symbols.length > 0 ? symbols : ["AAPL", "MSFT"] };
    }
    case "kalshi":
      return { source: "kalshi", limit: clampLimit(rest) };
    case "polymarket":
      return { source: "polymarket", limit: clampLimit(rest) };
    default:
      return null;
  }
}

export function encodeMarketKind(config: MarketWidgetConfig): string {
  switch (config.source) {
    case "crypto":
      return `market:crypto:${config.ids.join(",")}`;
    case "stocks":
      return `market:stocks:${config.symbols.join(",")}`;
    case "kalshi":
      return `market:kalshi:${config.limit}`;
    case "polymarket":
      return `market:polymarket:${config.limit}`;
  }
}

// ---------------------------------------------------------------------------
// Sports widget kind encoding — "sports:<league>[:<team>]"
// ---------------------------------------------------------------------------

export type SportsLeague = "nfl" | "nba" | "mlb" | "worldcup" | "f1";

export interface SportsWidgetConfig {
  league: SportsLeague;
  /** Team abbreviation to follow (e.g. "KC"), or `undefined` for "whichever
   * game is live or next" — meaningless for F1, which has no teams. */
  team?: string;
}

export const DEFAULT_SPORTS_KIND = "sports:nfl";

const KNOWN_LEAGUES: SportsLeague[] = ["nfl", "nba", "mlb", "worldcup", "f1"];

export function parseSportsKind(kind: string): SportsWidgetConfig | null {
  const parts = kind.split(":");
  if (parts[0] !== "sports") return null;
  const league = (parts[1] ?? "").toLowerCase() as SportsLeague;
  if (!KNOWN_LEAGUES.includes(league)) return null;
  const team = parts[2]?.trim();
  return team ? { league, team: team.toUpperCase() } : { league };
}

export function encodeSportsKind(config: SportsWidgetConfig): string {
  return config.team ? `sports:${config.league}:${config.team}` : `sports:${config.league}`;
}

function splitList(raw: string): string[] {
  return raw
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

function clampLimit(raw: string): number {
  const n = Number.parseInt(raw, 10);
  if (!Number.isFinite(n) || n <= 0) return 5;
  return Math.min(n, 10);
}

// ---------------------------------------------------------------------------
// Shared formatting
// ---------------------------------------------------------------------------

export function formatPrice(value: number, currency = "USD"): string {
  const abs = Math.abs(value);
  // A widget is ~200px wide; a full `Intl.NumberFormat` currency string for
  // a market-cap-sized number would overflow it before the ticker symbol
  // even fits. Compact notation only kicks in once plain digits would not.
  if (abs >= 1000) {
    return new Intl.NumberFormat(undefined, {
      style: "currency",
      currency,
      notation: "compact",
      maximumFractionDigits: 2,
    }).format(value);
  }
  return new Intl.NumberFormat(undefined, {
    style: "currency",
    currency,
    minimumFractionDigits: 2,
    maximumFractionDigits: abs < 1 ? 4 : 2,
  }).format(value);
}

export function formatPercent(value: number): string {
  const sign = value > 0 ? "+" : "";
  return `${sign}${value.toFixed(1)}%`;
}

/** "3s" / "2m" / "1h" — short enough to sit next to a ✕ button. */
export function formatAge(ms: number): string {
  const seconds = Math.max(0, Math.round(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.round(minutes / 60);
  return `${hours}h`;
}

// ---------------------------------------------------------------------------
// Online status
// ---------------------------------------------------------------------------

/**
 * `navigator.onLine`, reactively. A widget checks this before every poll so
 * "you are offline" is instant and does not wait on a `reqwest` timeout to
 * find out what the OS already knows — and reacts immediately to the
 * `online`/`offline` events rather than waiting for the next poll tick.
 */
export function useOnlineStatus(): boolean {
  const [online, setOnline] = useState(() => (typeof navigator === "undefined" ? true : navigator.onLine));

  useEffect(() => {
    const goOnline = () => setOnline(true);
    const goOffline = () => setOnline(false);
    window.addEventListener("online", goOnline);
    window.addEventListener("offline", goOffline);
    return () => {
      window.removeEventListener("online", goOnline);
      window.removeEventListener("offline", goOffline);
    };
  }, []);

  return online;
}

/**
 * Best-effort "is the US equity market in its regular session right now"
 * check, used only to label a stocks widget "MARKET CLOSED" instead of
 * quietly showing a 15m-old candle as if it were live. Deliberately a
 * heuristic, not a fact `markets.rs` reports: Yahoo's spark endpoint (see
 * that module's docs) does not send a market-state flag, and adding one
 * would mean editing a file this change is not allowed to touch. Weekday +
 * 9:30–16:00 America/New_York covers the overwhelming common case; it does
 * not know about market holidays, which is an honest gap, not a silent one —
 * worst case on a holiday this says "open" a few times a year while the
 * actual price data itself is still real, just unchanging.
 */
export function isUsMarketHoursNow(now: Date = new Date()): boolean {
  const parts = new Intl.DateTimeFormat("en-US", {
    timeZone: "America/New_York",
    hour: "numeric",
    minute: "numeric",
    hour12: false,
    weekday: "short",
  }).formatToParts(now);

  const get = (type: string) => parts.find((p) => p.type === type)?.value ?? "";
  const weekday = get("weekday");
  const hour = Number.parseInt(get("hour"), 10);
  const minute = Number.parseInt(get("minute"), 10);
  if (weekday === "Sat" || weekday === "Sun") return false;

  const minutesSinceMidnight = hour * 60 + minute;
  return minutesSinceMidnight >= 9 * 60 + 30 && minutesSinceMidnight < 16 * 60;
}
