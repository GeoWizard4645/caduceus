/**
 * Convert anything into anything else of the same kind.
 *
 * # The line this page draws
 *
 * Everything on the left of it — length, weight, temperature, volume, area,
 * speed, time, data, pressure, energy, angle — is arithmetic on definitions. An
 * inch *is* 25.4mm. Those conversions run in this process, cannot be stale, and
 * work with the Wi-Fi off, which matters in an app whose premise is that it
 * does not need a network.
 *
 * Currency is on the other side of that line and the page says so out loud
 * rather than quietly failing when you are on a plane. It is the only tab here
 * that makes a request, it names its source and the day the rates were
 * published, and turning it off is a setting.
 */

import { useCallback, useEffect, useMemo, useState } from "react";

import * as api from "@/shared/api";
import {
  DIMENSION_LABELS,
  convert,
  findUnit,
  formatValue,
  unitsIn,
  type Dimension,
  type Unit,
} from "@/shared/units";
import { Button, Callout, Select, cx } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";

const DIMENSIONS = Object.keys(DIMENSION_LABELS) as Dimension[];

/** Sensible opening pair per dimension, so the page is useful before any input. */
const OPENERS: Record<Dimension, [string, string]> = {
  length: ["km", "mi"],
  mass: ["kg", "lb"],
  temperature: ["c", "f"],
  volume: ["l", "gal"],
  area: ["sqm", "sqft"],
  speed: ["kph", "mph"],
  time: ["h", "min"],
  data: ["gb", "gib"],
  pressure: ["bar", "psi"],
  energy: ["kcal", "kj"],
  angle: ["deg", "rad"],
};

export function ConvertPage({ onSetTitle }: ToolPageProps) {
  const [dimension, setDimension] = useState<Dimension>("length");
  const [amount, setAmount] = useState("1");
  const [fromId, setFromId] = useState(OPENERS.length[0]);
  const [toId, setToId] = useState(OPENERS.length[1]);
  const [currency, setCurrency] = useState(false);

  useEffect(() => onSetTitle("Convert"), [onSetTitle]);

  const units = useMemo(() => unitsIn(dimension), [dimension]);
  const from = findUnit(fromId) ?? units[0];
  const to = findUnit(toId) ?? units[1] ?? units[0];

  const value = Number.parseFloat(amount.replace(/,/g, ""));
  const result =
    Number.isFinite(value) && from && to ? convert(value, from, to) : null;

  const pickDimension = (next: Dimension) => {
    setDimension(next);
    const [a, b] = OPENERS[next];
    setFromId(a);
    setToId(b);
    setCurrency(false);
  };

  const swap = () => {
    setFromId(to.id);
    setToId(from.id);
  };

  return (
    <div className="mx-auto h-full max-w-[760px] overflow-y-auto px-6 py-5">
      <div className="mb-4">
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Convert</h1>
        <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
          Units, temperature and currency. Everything but currency is arithmetic on
          definitions — it runs here, and it works offline.
        </p>
      </div>

      {/* --- what kind of thing ------------------------------------------ */}
      <div className="mb-4 flex flex-wrap gap-1.5">
        {DIMENSIONS.map((d) => (
          <button
            key={d}
            type="button"
            onClick={() => pickDimension(d)}
            className={cx(
              "rounded-full border px-3 py-1 text-2xs transition-colors",
              !currency && dimension === d
                ? "border-accent/40 bg-accent/12 text-accent"
                : "border-line text-ink-mute hover:bg-raised hover:text-ink",
            )}
          >
            {DIMENSION_LABELS[d]}
          </button>
        ))}
        <button
          type="button"
          onClick={() => setCurrency(true)}
          className={cx(
            "rounded-full border px-3 py-1 text-2xs transition-colors",
            currency
              ? "border-accent/40 bg-accent/12 text-accent"
              : "border-line text-ink-mute hover:bg-raised hover:text-ink",
          )}
        >
          Currency
          <span className="ml-1.5 text-[10px] text-ink-faint">needs internet</span>
        </button>
      </div>

      {currency ? (
        <CurrencyPanel />
      ) : (
        <>
          <div className="rounded-cad border border-line bg-surface/50 p-4">
            <div className="flex flex-wrap items-end gap-3">
              <div className="min-w-[140px] flex-1">
                <label htmlFor="convert-amount" className="eyebrow mb-1.5 block">
                  Amount
                </label>
                <input
                  id="convert-amount"
                  value={amount}
                  inputMode="decimal"
                  autoFocus
                  onChange={(e) => setAmount(e.target.value)}
                  className="w-full rounded-lg border border-line bg-base/40 px-3 py-2 font-mono text-[15px] tabular-nums text-ink focus:border-accent/50 focus:outline-none"
                />
              </div>

              <div className="min-w-[150px] flex-1">
                <span className="eyebrow mb-1.5 block">From</span>
                <Select value={from?.id ?? ""} onChange={setFromId} options={options(units)} />
              </div>

              <Button tone="ghost" onClick={swap} title="Swap" className="mb-0.5">
                ⇄
              </Button>

              <div className="min-w-[150px] flex-1">
                <span className="eyebrow mb-1.5 block">To</span>
                <Select value={to?.id ?? ""} onChange={setToId} options={options(units)} />
              </div>
            </div>

            <div className="mt-4 border-t border-line pt-3">
              {result === null ? (
                <p className="text-[13px] text-ink-faint">
                  {Number.isFinite(value) ? "Those measure different things." : "Type a number."}
                </p>
              ) : (
                <button
                  type="button"
                  onClick={() => void navigator.clipboard.writeText(formatValue(result))}
                  title="Copy"
                  className="text-left"
                >
                  <p className="font-mono text-[24px] tabular-nums text-ink">
                    {formatValue(result)}{" "}
                    <span className="text-[15px] text-ink-mute">{to?.symbol}</span>
                  </p>
                  <p className="mt-0.5 text-2xs text-ink-faint">
                    {formatValue(value)} {from?.symbol} · click to copy
                  </p>
                </button>
              )}
            </div>
          </div>

          {/* Everything at once, because the answer you want is often not the
              pair you happened to pick. */}
          {Number.isFinite(value) && from && (
            <section className="mt-5">
              <p className="eyebrow mb-2">In everything else</p>
              <div className="grid gap-1 rounded-cad border border-line bg-surface/50 p-2 sm:grid-cols-2">
                {units
                  .filter((unit) => unit.id !== from.id)
                  .map((unit) => {
                    const each = convert(value, from, unit);
                    return (
                      <button
                        key={unit.id}
                        type="button"
                        onClick={() =>
                          void navigator.clipboard.writeText(formatValue(each ?? 0))
                        }
                        className="flex items-baseline gap-2 rounded-lg px-2.5 py-1.5 text-left transition-colors hover:bg-raised/60"
                      >
                        <span className="w-32 shrink-0 truncate text-2xs text-ink-faint">
                          {unit.name}
                        </span>
                        <span className="min-w-0 flex-1 truncate font-mono text-2xs tabular-nums text-ink">
                          {each === null ? "—" : formatValue(each)} {unit.symbol}
                        </span>
                      </button>
                    );
                  })}
              </div>
            </section>
          )}
        </>
      )}
    </div>
  );
}

const options = (units: Unit[]) =>
  units.map((unit) => ({ value: unit.id, label: `${unit.name} (${unit.symbol})` }));

// ---------------------------------------------------------------------------
// Currency
// ---------------------------------------------------------------------------

/**
 * The one panel that makes a network request.
 *
 * Nothing is fetched until this is opened and Convert is pressed — opening the
 * page must not phone anywhere. When it does fetch, it says where the numbers
 * came from and what day they are for, because a currency answer with no date
 * on it is a guess wearing a decimal point.
 */
function CurrencyPanel() {
  const [amount, setAmount] = useState("100");
  const [from, setFrom] = useState("USD");
  const [to, setTo] = useState("EUR");
  const [table, setTable] = useState<api.RateTable | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async (base: string) => {
    setBusy(true);
    setError(null);
    try {
      setTable(await api.exchangeRates(base));
    } catch (e) {
      setTable(null);
      setError(api.errorMessage(e));
    } finally {
      setBusy(false);
    }
  }, []);

  const value = Number.parseFloat(amount.replace(/,/g, ""));
  const rate = table && table.base === from ? table.rates[to] : undefined;
  const converted = rate !== undefined && Number.isFinite(value) ? value * rate : null;
  const codes = table ? [table.base, ...Object.keys(table.rates)].sort() : [from, to];

  return (
    <div className="rounded-cad border border-line bg-surface/50 p-4">
      <Callout tone="warn" title="This one goes online">
        Rates come from the European Central Bank via frankfurter.dev. The request contains a
        currency code and nothing else — no account, no key, nothing that identifies you.
        Every other conversion on this page is offline arithmetic.
      </Callout>

      <div className="mt-4 flex flex-wrap items-end gap-3">
        <div className="min-w-[120px] flex-1">
          <label htmlFor="currency-amount" className="eyebrow mb-1.5 block">
            Amount
          </label>
          <input
            id="currency-amount"
            value={amount}
            inputMode="decimal"
            onChange={(e) => setAmount(e.target.value)}
            className="w-full rounded-lg border border-line bg-base/40 px-3 py-2 font-mono text-[15px] tabular-nums text-ink focus:border-accent/50 focus:outline-none"
          />
        </div>
        <div className="min-w-[110px]">
          <span className="eyebrow mb-1.5 block">From</span>
          <Select
            value={from}
            onChange={(next) => {
              setFrom(next);
              setTable(null);
            }}
            options={codes.map((c) => ({ value: c, label: c }))}
          />
        </div>
        <div className="min-w-[110px]">
          <span className="eyebrow mb-1.5 block">To</span>
          <Select value={to} onChange={setTo} options={codes.map((c) => ({ value: c, label: c }))} />
        </div>
        <Button tone="primary" onClick={() => void load(from)} disabled={busy} className="mb-0.5">
          {busy ? "Fetching…" : table ? "Refresh" : "Get rates"}
        </Button>
      </div>

      <div className="mt-4 border-t border-line pt-3">
        {error ? (
          <p className="text-[13px] text-danger">{error}</p>
        ) : converted === null ? (
          <p className="text-[13px] text-ink-faint">
            {table ? "No rate for that pair." : "Press Get rates."}
          </p>
        ) : (
          <>
            <button
              type="button"
              onClick={() => void navigator.clipboard.writeText(converted.toFixed(2))}
              className="text-left"
            >
              <p className="font-mono text-[24px] tabular-nums text-ink">
                {converted.toFixed(2)} <span className="text-[15px] text-ink-mute">{to}</span>
              </p>
            </button>
            <p className="mt-0.5 text-2xs text-ink-faint">
              1 {from} = {rate?.toFixed(4)} {to} · {table?.source} · published {table?.date}
              {table?.cached && " · from cache"}
            </p>
          </>
        )}
      </div>
    </div>
  );
}
