/**
 * A floating widget that tracks crypto, stocks, or a handful of prediction
 * markets — whichever `parseMarketKind(kind)` says. See `marketApi.ts` for
 * why configuration lives inside the widget's `kind` string instead of a new
 * persisted field.
 *
 * # The one rule this file exists to enforce: no data before this mounts
 *
 * Every network call a market widget ever makes happens inside the
 * `useEffect` below, which starts on mount and is torn down on unmount. There
 * is no module-level timer, no store, no subscription that outlives one
 * widget's window — closing the widget (which destroys its whole webview,
 * per `widgets_destroy` in `widgets.rs`) is by itself enough to stop polling;
 * nothing extra has to remember to do it. A widget kind that is never
 * created never runs this component at all, so it fetches nothing, exactly
 * as the app's "no tracking until you add it" claim requires.
 */

import { useEffect, useMemo, useState } from "react";

import { cx } from "@/shared/ui";

import {
  CRYPTO_POLL_MS,
  DEFAULT_MARKET_KIND,
  PREDICTION_POLL_MS,
  STOCK_POLL_MS,
  fetchCrypto,
  fetchKalshiMarkets,
  fetchPolymarketMarkets,
  fetchStocks,
  formatAge,
  formatPercent,
  formatPrice,
  isUsMarketHoursNow,
  parseMarketKind,
  useOnlineStatus,
  type MarketWidgetConfig,
} from "./marketApi";

interface MarketRow {
  key: string;
  /** Ticker, team-style short name, or (for a prediction market) its
   * question — the thing shown big. */
  label: string;
  sublabel?: string;
  valueText: string;
  /** `null` means "no direction to show yet" — the honest state for a
   * prediction market's first poll, which has no prior price to compare
   * against. */
  deltaPct: number | null;
  closedTag?: string;
}

function pollIntervalFor(config: MarketWidgetConfig): number {
  switch (config.source) {
    case "crypto":
      return CRYPTO_POLL_MS;
    case "stocks":
      return STOCK_POLL_MS;
    case "kalshi":
    case "polymarket":
      return PREDICTION_POLL_MS;
  }
}

/** One fetch, for whichever source this widget is configured for. `previous`
 * is this widget's own last-poll snapshot (ticker/market id → price) — used
 * only by the prediction-market sources, which unlike crypto/stocks carry no
 * built-in "change since when" field from the API itself, so a direction
 * arrow has to be computed client-side from one poll to the next. */
async function loadRows(
  config: MarketWidgetConfig,
  previous: Map<string, number>,
): Promise<{ rows: MarketRow[]; snapshot: Map<string, number> }> {
  const snapshot = new Map<string, number>();

  switch (config.source) {
    case "crypto": {
      const quotes = await fetchCrypto(config.ids);
      const rows = quotes.map((q) => {
        snapshot.set(q.id, q.price);
        return {
          key: q.id,
          label: q.symbol,
          sublabel: q.name,
          valueText: formatPrice(q.price),
          deltaPct: q.changePercent24h ?? null,
        };
      });
      return { rows, snapshot };
    }

    case "stocks": {
      const result = await fetchStocks(config.symbols);
      const marketClosed = !isUsMarketHoursNow();
      const rows: MarketRow[] = result.quotes.map((q) => {
        snapshot.set(q.symbol, q.price);
        return {
          key: q.symbol,
          label: q.symbol,
          sublabel: q.name,
          valueText: formatPrice(q.price, q.currency),
          deltaPct: q.changePercent,
          closedTag: marketClosed ? "CLOSED" : undefined,
        };
      });
      for (const symbol of result.notFound) {
        rows.push({ key: symbol, label: symbol, valueText: "—", deltaPct: null, closedTag: "NOT FOUND" });
      }
      return { rows, snapshot };
    }

    case "kalshi": {
      const markets = await fetchKalshiMarkets(config.limit);
      const rows = markets.map((m) => {
        const prev = previous.get(m.marketTicker);
        snapshot.set(m.marketTicker, m.lastPrice);
        const deltaPct = prev != null && prev > 0 ? ((m.lastPrice - prev) / prev) * 100 : null;
        return {
          key: m.marketTicker,
          label: m.subtitle ?? m.title,
          sublabel: m.subtitle ? m.title : undefined,
          valueText: `${Math.round(m.lastPrice * 100)}¢`,
          deltaPct,
          closedTag: m.status !== "active" ? m.status.toUpperCase() : undefined,
        };
      });
      return { rows, snapshot };
    }

    case "polymarket": {
      const markets = await fetchPolymarketMarkets(config.limit);
      const rows = markets.map((m) => {
        const yesIdx = m.outcomes.findIndex((o) => o.toLowerCase() === "yes");
        const price = yesIdx >= 0 ? (m.outcomePrices[yesIdx] ?? 0) : (m.outcomePrices[0] ?? 0);
        const prev = previous.get(m.id);
        snapshot.set(m.id, price);
        const deltaPct = prev != null && prev > 0 ? ((price - prev) / prev) * 100 : null;
        return {
          key: m.id,
          label: m.question,
          valueText: `${Math.round(price * 100)}¢`,
          deltaPct,
        };
      });
      return { rows, snapshot };
    }
  }
}

function DeltaArrow({ deltaPct }: { deltaPct: number | null }) {
  if (deltaPct == null || Number.isNaN(deltaPct)) {
    return <span className="text-2xs text-ink-faint">·</span>;
  }
  // Direction is carried by the arrow glyph itself, not only by colour — a
  // red/green-blind viewer still gets "up" or "down" from the character.
  const up = deltaPct > 0;
  const flat = Math.abs(deltaPct) < 0.005;
  return (
    <span
      className={cx(
        "flex items-center gap-0.5 text-2xs font-medium tabular-nums",
        flat ? "text-ink-faint" : up ? "text-positive" : "text-danger",
      )}
    >
      <span aria-hidden="true">{flat ? "→" : up ? "▲" : "▼"}</span>
      {formatPercent(deltaPct)}
    </span>
  );
}

function MarketRowView({ row }: { row: MarketRow }) {
  return (
    <div className="flex w-full items-center justify-between gap-1.5 py-0.5">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1">
          <span className="truncate text-2xs font-semibold text-ink">{row.label}</span>
          {row.closedTag && (
            <span className="shrink-0 rounded-sm bg-overlay px-1 text-[8px] font-semibold uppercase tracking-wide text-ink-faint">
              {row.closedTag}
            </span>
          )}
        </div>
        {row.sublabel && <div className="truncate text-[9px] text-ink-faint">{row.sublabel}</div>}
      </div>
      <div className="flex shrink-0 flex-col items-end">
        <span className="text-2xs font-semibold tabular-nums text-ink">{row.valueText}</span>
        <DeltaArrow deltaPct={row.deltaPct} />
      </div>
    </div>
  );
}

/** The one place "is this data trustworthy right now" gets decided — see the
 * module docs on why a stale number must never render as if it were live. */
function FreshnessBadge({
  hasRows,
  lastError,
  online,
  fetchedAt,
  now,
}: {
  hasRows: boolean;
  lastError: string | null;
  online: boolean;
  fetchedAt: number | null;
  now: number;
}) {
  if (!online) {
    return <StatusPill tone="danger" label="OFFLINE" />;
  }
  if (lastError) {
    const rateLimited = /rate.?limit/i.test(lastError);
    return <StatusPill tone={rateLimited ? "caution" : "danger"} label={rateLimited ? "RATE LIMITED" : "OFFLINE"} />;
  }
  if (!hasRows || fetchedAt == null) {
    return <StatusPill tone="muted" label="LOADING" />;
  }
  return <span className="text-[9px] text-ink-faint">{formatAge(now - fetchedAt)} ago</span>;
}

function StatusPill({ tone, label }: { tone: "danger" | "caution" | "muted"; label: string }) {
  const tones = {
    danger: "text-danger",
    caution: "text-caution",
    muted: "text-ink-faint",
  } as const;
  return (
    <span className={cx("flex items-center gap-1 text-[9px] font-semibold uppercase tracking-wide", tones[tone])}>
      <span
        aria-hidden="true"
        className={cx(
          "h-1.5 w-1.5 rounded-full",
          tone === "danger" ? "bg-danger" : tone === "caution" ? "bg-caution" : "bg-ink-faint",
        )}
      />
      {label}
    </span>
  );
}

export function MarketWidget({ kind }: { kind: string }) {
  const config = useMemo(() => parseMarketKind(kind) ?? parseMarketKind(DEFAULT_MARKET_KIND)!, [kind]);
  const online = useOnlineStatus();

  const [rows, setRows] = useState<MarketRow[]>([]);
  const [fetchedAt, setFetchedAt] = useState<number | null>(null);
  const [lastError, setLastError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());

  // Re-renders the "Xs ago" text once a second without ever re-fetching —
  // purely cosmetic, and cheap enough at this scale not to worry about.
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let previousSnapshot = new Map<string, number>();

    async function tick() {
      if (cancelled) return;
      if (!navigator.onLine) {
        // Skip the IPC round-trip entirely — the OS already knows the
        // answer, no reason to wait out a reqwest timeout to confirm it.
        setLastError("Offline.");
        schedule();
        return;
      }
      try {
        const { rows: nextRows, snapshot } = await loadRows(config, previousSnapshot);
        if (cancelled) return;
        previousSnapshot = snapshot;
        setRows(nextRows);
        setFetchedAt(Date.now());
        setLastError(null);
      } catch (e) {
        if (cancelled) return;
        setLastError(e instanceof Error ? e.message : String(e));
        // Deliberately not clearing `rows`/`fetchedAt` here — the last known
        // good values stay, so a viewer sees what they last had, dimmed and
        // timestamped by FreshnessBadge, rather than an empty panel for a
        // transient blip. What must never happen is presenting them as
        // fresh, which is exactly what the dimming + badge below prevents.
      }
      schedule();
    }
    function schedule() {
      if (cancelled) return;
      timer = setTimeout(() => void tick(), pollIntervalFor(config));
    }

    void tick();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [config]);

  const degraded = !online || lastError != null;
  const hasRows = rows.length > 0;

  return (
    <div className="flex h-full w-full flex-col gap-1">
      <div className="flex items-center justify-between">
        <span className="text-[9px] font-semibold uppercase tracking-[0.08em] text-ink-faint">
          {sourceLabel(config)}
        </span>
        <FreshnessBadge hasRows={hasRows} lastError={lastError} online={online} fetchedAt={fetchedAt} now={now} />
      </div>

      {!hasRows && !degraded && (
        // PixelText's glyph table only knows digits, ":" and " " (see
        // PixelDigits.tsx) — a dash placeholder would silently render as
        // blank cells, which is worse than plain text for "nothing to show
        // yet, this is not an error".
        <div className="flex flex-1 items-center justify-center text-2xs text-ink-faint">Loading…</div>
      )}

      {!hasRows && degraded && (
        <div className="flex flex-1 items-center justify-center px-1 text-center text-2xs text-ink-faint">
          {online ? (lastError ?? "Could not load.") : "You're offline."}
        </div>
      )}

      {hasRows && (
        <div className={cx("flex-1 divide-y divide-line/60 overflow-y-auto", degraded && "opacity-50 grayscale")}>
          {rows.map((row) => (
            <MarketRowView key={row.key} row={row} />
          ))}
        </div>
      )}
    </div>
  );
}

function sourceLabel(config: MarketWidgetConfig): string {
  switch (config.source) {
    case "crypto":
      return "Crypto";
    case "stocks":
      return "Stocks";
    case "kalshi":
      return "Kalshi";
    case "polymarket":
      return "Polymarket";
  }
}
