/**
 * Add a floating widget: clock, crypto/stock tickers, sports scoreboard, or a
 * handful of prediction markets — pin whichever one to the desktop.
 *
 * # Why this exists
 *
 * `createWidget()` (`src/widgets/widgetApi.ts`) and every widget kind it can
 * build (`src/widgets/WidgetApp.tsx`, `marketApi.ts`) have been wired end to
 * end since the widget system shipped — window creation, persistence, live
 * data polling, drag/resize, all of it. The one thing missing was a way to
 * *ask* for one: nothing in the palette or the Command Center ever called
 * `createWidget()`. This page is that missing call site, not new backend
 * surface.
 *
 * # Why a bespoke page instead of `formFor`
 *
 * A generic form renders every field at once; this one needs the fields to
 * change with the kind you picked — a crypto widget wants tickers, a sports
 * widget wants a league and (optionally) a team, a clock wants nothing at
 * all. `CommandForm`'s `Field[]` has no notion of "only when X is selected",
 * so getting that right needs a real component rather than a declared shape.
 *
 * # Where configuration goes
 *
 * There is no separate "widget config" argument to `widgets_create` — a
 * widget's entire configuration is encoded into its opaque `kind` string
 * (`marketApi.ts`'s `encodeMarketKind`/`encodeSportsKind`), which is what
 * already gets saved and restored by the Rust side. This page only has to
 * build that string correctly; it never touches `widgets.rs`.
 */

import { useCallback, useEffect, useMemo, useState } from "react";

import { Button, EmptyState, Field, NumberInput, Section, Select, Spinner, TextInput, cx } from "@/shared/ui";
import { createWidget, destroyWidget, listWidgets } from "@/widgets/widgetApi";
import {
  encodeMarketKind,
  encodeSportsKind,
  parseMarketKind,
  parseSportsKind,
  type SportsLeague,
} from "@/widgets/marketApi";
import type { WidgetLayout } from "@/widgets/types";

import type { ToolPageProps } from "../ToolPage";

type WidgetKind = "clock" | "crypto" | "stocks" | "sports" | "kalshi" | "polymarket";

const KIND_OPTIONS: { value: WidgetKind; label: string }[] = [
  { value: "clock", label: "Clock" },
  { value: "crypto", label: "Crypto prices" },
  { value: "stocks", label: "Stock prices" },
  { value: "sports", label: "Sports scoreboard" },
  { value: "kalshi", label: "Kalshi markets" },
  { value: "polymarket", label: "Polymarket markets" },
];

const LEAGUE_OPTIONS: { value: SportsLeague; label: string }[] = [
  { value: "nfl", label: "NFL" },
  { value: "nba", label: "NBA" },
  { value: "mlb", label: "MLB" },
  { value: "worldcup", label: "World Cup" },
  { value: "f1", label: "Formula 1" },
];

/** A short, human line for a widget already on screen — the inverse of the
 * encode functions in `marketApi.ts`, for the "currently open" list below. */
function describeKind(kind: string): string {
  if (kind === "clock") return "Clock";

  const market = parseMarketKind(kind);
  if (market) {
    switch (market.source) {
      case "crypto":
        return `Crypto · ${market.ids.join(", ")}`;
      case "stocks":
        return `Stocks · ${market.symbols.join(", ")}`;
      case "kalshi":
        return `Kalshi · top ${market.limit}`;
      case "polymarket":
        return `Polymarket · top ${market.limit}`;
    }
  }

  const sports = parseSportsKind(kind);
  if (sports) {
    const league = LEAGUE_OPTIONS.find((l) => l.value === sports.league)?.label ?? sports.league;
    return sports.team ? `${league} · ${sports.team}` : league;
  }

  return kind;
}

export function WidgetsPage({ onSetTitle }: ToolPageProps) {
  useEffect(() => onSetTitle("Add a widget"), [onSetTitle]);

  const [kind, setKind] = useState<WidgetKind>("crypto");
  const [cryptoIds, setCryptoIds] = useState("BTC,ETH,SOL");
  const [stockSymbols, setStockSymbols] = useState("AAPL,MSFT");
  const [league, setLeague] = useState<SportsLeague>("nfl");
  const [team, setTeam] = useState("");
  const [predictionLimit, setPredictionLimit] = useState(5);

  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<{ text: string; ok: boolean } | null>(null);

  const [widgets, setWidgets] = useState<WidgetLayout[] | null>(null);
  const [removing, setRemoving] = useState<string | null>(null);

  const refresh = useCallback(() => {
    void listWidgets()
      .then(setWidgets)
      .catch(() => setWidgets([]));
  }, []);

  useEffect(() => refresh(), [refresh]);

  /** The `kind` string `createWidget()` would be called with right now, given
   * whatever is currently in the form. */
  const pendingKind = useMemo(() => {
    switch (kind) {
      case "clock":
        return "clock";
      case "crypto":
        return encodeMarketKind({ source: "crypto", ids: splitList(cryptoIds, ["BTC", "ETH", "SOL"]) });
      case "stocks":
        return encodeMarketKind({ source: "stocks", symbols: splitList(stockSymbols, ["AAPL", "MSFT"]) });
      case "kalshi":
        return encodeMarketKind({ source: "kalshi", limit: predictionLimit });
      case "polymarket":
        return encodeMarketKind({ source: "polymarket", limit: predictionLimit });
      case "sports":
        return encodeSportsKind({ league, team: team.trim() ? team.trim().toUpperCase() : undefined });
    }
  }, [kind, cryptoIds, stockSymbols, predictionLimit, league, team]);

  const add = async () => {
    setBusy(true);
    try {
      await createWidget(pendingKind);
      setNote({ text: "Widget added — look for it floating on your desktop.", ok: true });
      refresh();
    } catch (error) {
      setNote({ text: error instanceof Error ? error.message : String(error), ok: false });
    } finally {
      setBusy(false);
    }
  };

  const remove = async (id: string) => {
    setRemoving(id);
    try {
      await destroyWidget(id);
      refresh();
    } catch (error) {
      setNote({ text: error instanceof Error ? error.message : String(error), ok: false });
    } finally {
      setRemoving(null);
    }
  };

  return (
    <div className="mx-auto h-full max-w-[760px] overflow-y-auto px-6 py-5">
      <div className="mb-4">
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Add a widget</h1>
        <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
          A small always-on-top panel that sits above every app and every Space. Pick what it should
          show, then drag it wherever you want it — it remembers its spot across restarts.
        </p>
      </div>

      <Section title="What to show">
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Widget type">
            <Select value={kind} onChange={setKind} options={KIND_OPTIONS} />
          </Field>

          {kind === "crypto" && (
            <Field label="Tickers" hint="Comma-separated, e.g. BTC,ETH,SOL.">
              <TextInput value={cryptoIds} onChange={setCryptoIds} placeholder="BTC,ETH,SOL" mono />
            </Field>
          )}

          {kind === "stocks" && (
            <Field label="Tickers" hint="Comma-separated, e.g. AAPL,MSFT.">
              <TextInput value={stockSymbols} onChange={setStockSymbols} placeholder="AAPL,MSFT" mono />
            </Field>
          )}

          {kind === "sports" && (
            <>
              <Field label="League">
                <Select value={league} onChange={setLeague} options={LEAGUE_OPTIONS} />
              </Field>
              <Field
                label="Team (optional)"
                hint={
                  league === "f1"
                    ? "Formula 1 has no teams to follow — this widget always shows the race weekend."
                    : "Abbreviation, e.g. KC. Leave blank for whichever game is live or next."
                }
              >
                <TextInput
                  value={team}
                  onChange={setTeam}
                  placeholder="KC"
                  disabled={league === "f1"}
                  mono
                />
              </Field>
            </>
          )}

          {(kind === "kalshi" || kind === "polymarket") && (
            <Field label="How many markets" hint="1 to 10, ranked by volume.">
              <NumberInput value={predictionLimit} onChange={setPredictionLimit} min={1} max={10} />
            </Field>
          )}

          {kind === "clock" && (
            <div className="flex items-end">
              <p className="text-2xs text-ink-faint">Nothing to configure — just add it.</p>
            </div>
          )}
        </div>

        <div className="mt-4 row gap-2">
          <Button tone="primary" onClick={() => void add()} disabled={busy}>
            {busy ? "Adding…" : "Add widget"}
          </Button>
          {busy && <Spinner className="text-accent" />}
          {note && (
            <span className={cx("text-2xs", note.ok ? "text-ink-mute" : "text-danger")}>{note.text}</span>
          )}
        </div>
      </Section>

      <Section
        title="On your desktop"
        description="Every widget you have added, open or not. Removing one here closes its window for good."
        actions={
          <Button size="sm" tone="ghost" onClick={refresh}>
            Refresh
          </Button>
        }
      >
        {widgets === null ? (
          <div className="row justify-center gap-2 py-4">
            <Spinner className="text-ink-faint" />
          </div>
        ) : widgets.length === 0 ? (
          <EmptyState
            title="No widgets yet"
            hint="Add one above and it will show up here."
            icon="▢"
          />
        ) : (
          <ul className="divide-y divide-line/60">
            {widgets.map((widget) => (
              <li key={widget.id} className="flex items-center justify-between gap-3 py-2">
                <span className="min-w-0 truncate text-[13px] text-ink">{describeKind(widget.kind)}</span>
                <Button
                  size="sm"
                  tone="danger"
                  onClick={() => void remove(widget.id)}
                  disabled={removing === widget.id}
                >
                  {removing === widget.id ? "Removing…" : "Remove"}
                </Button>
              </li>
            ))}
          </ul>
        )}
      </Section>
    </div>
  );
}

function splitList(raw: string, fallback: string[]): string[] {
  const list = raw
    .split(",")
    .map((s) => s.trim().toUpperCase())
    .filter((s) => s.length > 0);
  return list.length > 0 ? list : fallback;
}
