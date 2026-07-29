/**
 * Time management: a world clock, a timezone converter, countdown timers, a
 * stopwatch, and a pomodoro cycle.
 *
 * # Why the countdowns do not live in this component
 *
 * Every piece of state that actually *counts down* — a timer's deadline, the
 * stopwatch's accumulated time, the pomodoro's current phase — lives in Rust,
 * behind `tools::timekeeping::TimekeepingRuntime`, not in `useState` here.
 * The reason is the Command Center window itself: closing it hides the
 * window rather than destroying it (see `handle_window_event` in `lib.rs`),
 * which is the whole point — you summon it, glance at a timer, and it goes
 * away again. A `setInterval` counting down in this component would still be
 * "running" while hidden in the sense that its JavaScript technically exists,
 * but WebKit throttles a non-visible page's timers by an amount that is not
 * documented to stop anywhere, and this component itself gets unmounted
 * whenever its tab is not the active one. A pomodoro that silently loses
 * track of what phase it is in the moment you switch away from its tab is not
 * a pomodoro worth shipping.
 *
 * So this file *displays* state it does not own. It polls Rust on a plain
 * interval while the tab is visible (see `useInterval` below) and — for the
 * two displays where a once-a-second poll would look choppy, the stopwatch's
 * face and the world clock's ticking seconds — interpolates smoothly between
 * polls using the browser's own clock. The one exception is the world clock's
 * per-zone *offset*, which is genuinely just arithmetic (UTC + N minutes) and
 * is computed entirely in `@/shared/timekeeping` without touching Rust more
 * than once every few minutes — there is no "state" to lose there at all.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import * as api from "@/shared/api";
import { useDebounced } from "@/shared/hooks";
import {
  dayOffsetLabel,
  formatClock,
  formatDay,
  formatHms,
  formatStopwatch,
  parseLocalIso,
  toDatetimeLocalValue,
  zoneWallClock,
} from "@/shared/timekeeping";
import { Button, EmptyState, Field, NumberInput, Section, Select, TextInput, cx } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";

// ---------------------------------------------------------------------------
// A plain polling interval, gated on the tab actually being visible.
// ---------------------------------------------------------------------------

function useInterval(callback: () => void, delayMs: number | null): void {
  const latest = useRef(callback);
  latest.current = callback;

  useEffect(() => {
    if (delayMs === null) return;
    const id = setInterval(() => latest.current(), delayMs);
    return () => clearInterval(id);
  }, [delayMs]);
}

const dtInputClass =
  "w-full rounded-lg border border-line-strong/60 bg-base/60 px-3 py-2 text-[13px] text-ink " +
  "transition-[border-color,box-shadow] duration-150 focus:border-accent/70 " +
  "focus:shadow-[0_0_0_3px_rgb(var(--c-accent)/0.18)] focus:outline-none";

type SubTab = "clock" | "convert" | "timers" | "stopwatch" | "pomodoro";

const TABS: { key: SubTab; label: string }[] = [
  { key: "clock", label: "World clock" },
  { key: "convert", label: "Converter" },
  { key: "timers", label: "Timers" },
  { key: "stopwatch", label: "Stopwatch" },
  { key: "pomodoro", label: "Pomodoro" },
];

export function TimePage({ active, onSetTitle }: ToolPageProps) {
  useEffect(() => onSetTitle("Time"), [onSetTitle]);

  const [tab, setTab] = useState<SubTab>("clock");
  const [pomodoro, setPomodoro] = useState<api.PomodoroStatus | null>(null);
  const [stoppingPomodoro, setStoppingPomodoro] = useState(false);
  // If a session is already running when Time opens, land on Pomodoro so Stop
  // is obvious — otherwise notifications feel like they came from nowhere.
  const autoOpenedPomodoro = useRef(false);

  const refreshPomodoro = useCallback(() => {
    void api.timePomodoroStatus().then((status) => {
      setPomodoro(status);
      if (status.running && !autoOpenedPomodoro.current) {
        autoOpenedPomodoro.current = true;
        setTab("pomodoro");
      }
    });
  }, []);
  useEffect(() => refreshPomodoro(), [refreshPomodoro]);
  useInterval(refreshPomodoro, active ? 1000 : null);

  const stopPomodoro = async () => {
    setStoppingPomodoro(true);
    try {
      setPomodoro(await api.timePomodoroStop());
    } finally {
      setStoppingPomodoro(false);
    }
  };

  // The zone catalogue is shared between the World clock and Converter tabs —
  // fetched once here rather than by each tab, so switching between them
  // never shows a blank picker while a second copy loads.
  const [zones, setZones] = useState<api.ZoneClock[] | null>(null);
  const refreshZones = useCallback(() => {
    void api.timeListZones().then(setZones);
  }, []);
  useEffect(() => refreshZones(), [refreshZones]);
  // DST edges are the only thing that can change an offset, and none of them
  // move more than twice a year — this just keeps that rare case correct
  // without polling Rust on every tick.
  useInterval(refreshZones, active ? 5 * 60 * 1000 : null);

  const pomodoroRunning = pomodoro?.running ?? false;

  return (
    <div className="flex h-full flex-col">
      <div className="shrink-0 border-b border-line px-5 py-3">
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Time</h1>
        <p className="mt-0.5 max-w-prose text-[13px] text-ink-mute">
          Timers, the stopwatch and the pomodoro keep running even after you close this window —
          they live outside the page, not inside it.
        </p>
        {pomodoroRunning && pomodoro && (
          <div className="mt-3 flex flex-wrap items-center justify-between gap-3 rounded-lg border border-accent/35 bg-accent/10 px-3 py-2">
            <div className="min-w-0">
              <p className="text-[13px] font-medium text-ink">
                Pomodoro running —{" "}
                {pomodoro.phase ? PHASE_LABEL[pomodoro.phase] : "session"} ·{" "}
                {formatHms(pomodoro.remainingSecs)}
              </p>
              <p className="text-2xs text-ink-mute">
                Work session {pomodoro.cycle}
                {pomodoro.totalCycles > 0 ? ` of ${pomodoro.totalCycles}` : ""}
                . Notifications will keep firing until you stop it.
              </p>
            </div>
            <Button tone="danger" size="sm" onClick={() => void stopPomodoro()} disabled={stoppingPomodoro}>
              Stop pomodoro
            </Button>
          </div>
        )}
        <div className="row mt-3 flex-wrap gap-2">
          {TABS.map(({ key, label }) => (
            <button
              key={key}
              type="button"
              onClick={() => setTab(key)}
              className={cx(
                "rounded-full border px-3 py-1 text-2xs transition-colors",
                tab === key
                  ? "border-accent/40 bg-accent/12 text-accent"
                  : "border-line text-ink-mute hover:bg-raised hover:text-ink",
              )}
            >
              {label}
              {key === "pomodoro" && pomodoroRunning ? " · on" : ""}
            </button>
          ))}
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">
        {tab === "clock" && <WorldClockTab active={active} zones={zones} />}
        {tab === "convert" && <ConverterTab zones={zones} />}
        {tab === "timers" && <TimersTab active={active} />}
        {tab === "stopwatch" && <StopwatchTab active={active} />}
        {tab === "pomodoro" && (
          <PomodoroTab active={active} status={pomodoro} onStatus={setPomodoro} />
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// World clock
// ---------------------------------------------------------------------------

const ZONES_STORAGE_KEY = "caduceus:time:zones";
const DEFAULT_ZONE_IDS = ["America/New_York", "Europe/London", "Asia/Tokyo", "Australia/Sydney"];

function loadSelectedZones(): string[] {
  try {
    const raw = localStorage.getItem(ZONES_STORAGE_KEY);
    if (!raw) return DEFAULT_ZONE_IDS;
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed) && parsed.every((v) => typeof v === "string") && parsed.length
      ? (parsed as string[])
      : DEFAULT_ZONE_IDS;
  } catch {
    return DEFAULT_ZONE_IDS;
  }
}

function WorldClockTab({ active, zones }: { active: boolean; zones: api.ZoneClock[] | null }) {
  const [selected, setSelected] = useState<string[]>(loadSelectedZones);
  const [query, setQuery] = useState("");
  const [tick, setTick] = useState(() => new Date());

  useEffect(() => {
    try {
      localStorage.setItem(ZONES_STORAGE_KEY, JSON.stringify(selected));
    } catch {
      // Private browsing, or storage is full — the picker still works for
      // the rest of this session, it just will not be remembered.
    }
  }, [selected]);

  useInterval(() => setTick(new Date()), active ? 1000 : null);

  const byId = useMemo(() => new Map((zones ?? []).map((z) => [z.id, z])), [zones]);
  const shown = selected.map((id) => byId.get(id)).filter((z): z is api.ZoneClock => Boolean(z));

  const candidates = useMemo(() => {
    if (!zones) return [];
    const q = query.trim().toLowerCase();
    return zones
      .filter((z) => !selected.includes(z.id))
      .filter((z) => !q || z.label.toLowerCase().includes(q) || z.id.toLowerCase().includes(q))
      .slice(0, 8);
  }, [zones, selected, query]);

  return (
    <>
      <Section title="Your zones">
        {zones === null ? (
          <p className="py-6 text-center text-2xs text-ink-faint">Loading…</p>
        ) : shown.length === 0 ? (
          <EmptyState
            title="No zones added"
            hint="Search for a city below to add its clock."
            icon="◷"
          />
        ) : (
          <div className="grid grid-cols-2 gap-3">
            {shown.map((zone) => {
              const wall = zoneWallClock(zone.offsetMinutes, tick);
              return (
                <div key={zone.id} className="rounded-lg border border-line bg-base/40 p-3">
                  <div className="row items-start justify-between gap-2">
                    <span className="text-[13px] font-medium text-ink">{zone.label}</span>
                    <button
                      type="button"
                      title={`Remove ${zone.label}`}
                      onClick={() => setSelected((s) => s.filter((id) => id !== zone.id))}
                      className="no-drag shrink-0 text-ink-faint transition-colors hover:text-ink"
                    >
                      ×
                    </button>
                  </div>
                  <p className="mt-1 font-mono text-xl tabular-nums text-ink">{formatClock(wall)}</p>
                  <p className="mt-0.5 text-2xs text-ink-faint">
                    {formatDay(wall)} · {zone.utcOffsetLabel}
                    {zone.isDst ? " · DST" : ""}
                  </p>
                </div>
              );
            })}
          </div>
        )}
      </Section>

      <Section title="Add a zone">
        <TextInput value={query} onChange={setQuery} placeholder="Search a city or zone…" />
        {query.trim() && (
          <div className="mt-2 flex flex-col gap-1">
            {candidates.length === 0 ? (
              <p className="px-1 py-2 text-2xs text-ink-faint">No matching zones.</p>
            ) : (
              candidates.map((z) => (
                <button
                  key={z.id}
                  type="button"
                  onClick={() => {
                    setSelected((s) => [...s, z.id]);
                    setQuery("");
                  }}
                  className="flex items-center justify-between rounded-lg px-2.5 py-1.5 text-left text-[13px] text-ink transition-colors hover:bg-raised"
                >
                  <span>{z.label}</span>
                  <span className="text-2xs text-ink-faint">{z.utcOffsetLabel}</span>
                </button>
              ))
            )}
          </div>
        )}
      </Section>
    </>
  );
}

// ---------------------------------------------------------------------------
// Converter
// ---------------------------------------------------------------------------

function ConverterTab({ zones }: { zones: api.ZoneClock[] | null }) {
  const [sourceZoneId, setSourceZoneId] = useState("America/New_York");
  const [datetime, setDatetime] = useState(() => toDatetimeLocalValue(new Date()));
  const [targetIds, setTargetIds] = useState<string[]>(["Europe/London", "Asia/Tokyo"]);
  const [results, setResults] = useState<api.ConvertedTime[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const debouncedDatetime = useDebounced(datetime, 150);

  useEffect(() => {
    if (!debouncedDatetime || targetIds.length === 0) {
      setResults(null);
      return;
    }
    let cancelled = false;
    api
      .timeConvert({ zoneId: sourceZoneId, localDatetime: debouncedDatetime }, targetIds)
      .then((r) => {
        if (cancelled) return;
        setResults(r);
        setError(null);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setResults(null);
        setError(api.errorMessage(e));
      });
    return () => {
      cancelled = true;
    };
  }, [sourceZoneId, debouncedDatetime, targetIds]);

  const options = (zones ?? []).map((z) => ({ value: z.id, label: `${z.label} (${z.utcOffsetLabel})` }));

  return (
    <>
      <Section title="Convert a time" description="Pick a moment in one zone and see it land in the others.">
        <div className="grid grid-cols-2 gap-3">
          <Field label="In this zone">
            <Select value={sourceZoneId} onChange={setSourceZoneId} options={options} />
          </Field>
          <Field label="At this time">
            <input
              type="datetime-local"
              value={datetime}
              onChange={(e) => setDatetime(e.target.value)}
              className={dtInputClass}
            />
          </Field>
        </div>
      </Section>

      <Section title="Show it in">
        <div className="row flex-wrap gap-2">
          {(zones ?? [])
            .filter((z) => z.id !== sourceZoneId)
            .map((z) => {
              const on = targetIds.includes(z.id);
              return (
                <button
                  key={z.id}
                  type="button"
                  onClick={() =>
                    setTargetIds((t) => (on ? t.filter((id) => id !== z.id) : [...t, z.id]))
                  }
                  className={cx(
                    "rounded-full border px-3 py-1 text-2xs transition-colors",
                    on
                      ? "border-accent/40 bg-accent/12 text-accent"
                      : "border-line text-ink-mute hover:bg-raised hover:text-ink",
                  )}
                >
                  {z.label}
                </button>
              );
            })}
        </div>
      </Section>

      {error && <p className="mb-4 text-2xs text-danger">{error}</p>}

      {results && results.length > 0 && (
        <Section title="Results">
          <div className="flex flex-col divide-y divide-line">
            {results.map((r) => {
              const label = dayOffsetLabel(r.dayOffset);
              return (
                <div key={r.id} className="row items-center justify-between gap-3 py-2 first:pt-0 last:pb-0">
                  <span className="text-[13px] text-ink">{r.label}</span>
                  <div className="row items-center gap-2">
                    {label && (
                      <span className="rounded-full bg-raised px-2 py-0.5 text-2xs text-ink-mute">{label}</span>
                    )}
                    <span className="font-mono text-[13px] text-ink">{formatClock(parseLocalIso(r.localIso))}</span>
                    <span className="text-2xs text-ink-faint">{r.utcOffsetLabel}</span>
                  </div>
                </div>
              );
            })}
          </div>
        </Section>
      )}
    </>
  );
}

// ---------------------------------------------------------------------------
// Countdown timers
// ---------------------------------------------------------------------------

function TimersTab({ active }: { active: boolean }) {
  const [timers, setTimers] = useState<api.TimerSnapshot[]>([]);
  const [name, setName] = useState("");
  const [hours, setHours] = useState(0);
  const [minutes, setMinutes] = useState(5);
  const [seconds, setSeconds] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(() => {
    void api.timeListTimers().then(setTimers);
  }, []);

  useEffect(() => refresh(), [refresh]);
  useInterval(refresh, active ? 1000 : null);

  const totalSeconds = hours * 3600 + minutes * 60 + seconds;

  const start = async () => {
    setBusy(true);
    try {
      await api.timeStartTimer(name.trim() || "Timer", totalSeconds);
      setName("");
      setError(null);
      refresh();
    } catch (e) {
      setError(api.errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const dismiss = async (id: number) => {
    await api.timeDismissTimer(id);
    refresh();
  };

  return (
    <>
      <Section title="New timer">
        <Field label="Name">
          <TextInput value={name} onChange={setName} placeholder="Pasta" />
        </Field>
        <div className="mt-3 grid grid-cols-3 gap-3">
          <Field label="Hours">
            <NumberInput value={hours} onChange={setHours} min={0} max={23} />
          </Field>
          <Field label="Minutes">
            <NumberInput value={minutes} onChange={setMinutes} min={0} max={59} />
          </Field>
          <Field label="Seconds">
            <NumberInput value={seconds} onChange={setSeconds} min={0} max={59} />
          </Field>
        </div>
        <div className="row mt-3 gap-2">
          <Button tone="primary" onClick={() => void start()} disabled={busy || totalSeconds === 0}>
            Start timer
          </Button>
        </div>
        {error && <p className="mt-2 text-2xs text-danger">{error}</p>}
      </Section>

      <Section title="Running">
        {timers.length === 0 ? (
          <EmptyState
            title="No timers yet"
            hint="Start one above — it keeps counting down even if you close this window, and notifies you when it ends."
            icon="◷"
          />
        ) : (
          <div className="flex flex-col gap-2">
            {timers.map((t) => {
              const fraction = t.totalSecs > 0 ? 1 - t.remainingSecs / t.totalSecs : 1;
              return (
                <div
                  key={t.id}
                  className={cx(
                    "rounded-lg border p-3",
                    t.completed ? "border-positive/40 bg-positive/[0.06]" : "border-line",
                  )}
                >
                  <div className="row items-start justify-between gap-2">
                    <span className="text-[13px] font-medium text-ink">{t.name}</span>
                    <Button size="sm" tone="ghost" onClick={() => void dismiss(t.id)}>
                      {t.completed ? "Dismiss" : "Cancel"}
                    </Button>
                  </div>
                  <p className="mt-1 font-mono text-xl tabular-nums text-ink">
                    {t.completed ? "Done" : formatHms(t.remainingSecs)}
                  </p>
                  {!t.completed && (
                    <div className="mt-2 h-1 overflow-hidden rounded-full bg-raised">
                      <div
                        className="h-full bg-accent transition-[width] duration-500"
                        style={{ width: `${Math.min(100, Math.max(0, fraction * 100))}%` }}
                      />
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </Section>
    </>
  );
}

// ---------------------------------------------------------------------------
// Stopwatch
// ---------------------------------------------------------------------------

const IDLE_STOPWATCH: api.StopwatchStatus = { running: false, elapsedMs: 0, lapsMs: [] };

function StopwatchTab({ active }: { active: boolean }) {
  const [status, setStatus] = useState<api.StopwatchStatus>(IDLE_STOPWATCH);
  const [baseAt, setBaseAt] = useState(() => performance.now());
  const [displayMs, setDisplayMs] = useState(0);
  const [busy, setBusy] = useState(false);

  const applyStatus = useCallback((next: api.StopwatchStatus) => {
    setStatus(next);
    setBaseAt(performance.now());
    setDisplayMs(next.elapsedMs);
  }, []);

  const refresh = useCallback(() => {
    void api.timeStopwatchStatus().then(applyStatus);
  }, [applyStatus]);

  useEffect(() => refresh(), [refresh]);
  // Whole-second truth from Rust; the display between polls is interpolated
  // with the browser's own clock (below) rather than fetched, so a stopwatch
  // face does not stutter to the poll interval.
  useInterval(refresh, active ? 1000 : null);

  useEffect(() => {
    if (!active || !status.running) return;
    let frame: number;
    const tick = () => {
      setDisplayMs(status.elapsedMs + (performance.now() - baseAt));
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [active, status, baseAt]);

  const act = async (fn: () => Promise<api.StopwatchStatus>) => {
    setBusy(true);
    try {
      applyStatus(await fn());
    } finally {
      setBusy(false);
    }
  };

  const laps = status.lapsMs;

  return (
    <Section title="Stopwatch">
      <p className="py-4 text-center font-mono text-5xl tabular-nums text-ink">
        {formatStopwatch(displayMs)}
      </p>
      <div className="row justify-center gap-2">
        {status.running ? (
          <Button tone="primary" onClick={() => void act(api.timeStopwatchStop)} disabled={busy}>
            Stop
          </Button>
        ) : (
          <Button tone="primary" onClick={() => void act(api.timeStopwatchStart)} disabled={busy}>
            {status.elapsedMs > 0 ? "Resume" : "Start"}
          </Button>
        )}
        <Button onClick={() => void act(api.timeStopwatchLap)} disabled={busy || !status.running}>
          Lap
        </Button>
        <Button tone="ghost" onClick={() => void act(api.timeStopwatchReset)} disabled={busy || status.elapsedMs === 0}>
          Reset
        </Button>
      </div>

      {laps.length > 0 && (
        <div className="mt-5 flex flex-col divide-y divide-line border-t border-line">
          {laps
            .map((cumulative, i) => ({ i, cumulative, split: cumulative - (laps[i - 1] ?? 0) }))
            .reverse()
            .map(({ i, cumulative, split }) => (
              <div key={i} className="row items-center justify-between gap-3 py-2 text-2xs">
                <span className="text-ink-faint">Lap {i + 1}</span>
                <span className="font-mono text-ink">{formatStopwatch(split)}</span>
                <span className="font-mono text-ink-faint">{formatStopwatch(cumulative)}</span>
              </div>
            ))}
        </div>
      )}
    </Section>
  );
}

// ---------------------------------------------------------------------------
// Pomodoro
// ---------------------------------------------------------------------------

const PHASE_LABEL: Record<api.PomodoroPhase, string> = {
  work: "Work",
  shortBreak: "Short break",
  longBreak: "Long break",
};

function PomodoroTab({
  active,
  status,
  onStatus,
}: {
  active: boolean;
  status: api.PomodoroStatus | null;
  onStatus: (status: api.PomodoroStatus) => void;
}) {
  const [workMinutes, setWorkMinutes] = useState(25);
  const [shortBreakMinutes, setShortBreakMinutes] = useState(5);
  const [longBreakMinutes, setLongBreakMinutes] = useState(15);
  const [cyclesBeforeLongBreak, setCyclesBeforeLongBreak] = useState(4);
  // Classic four-session day by default — the old default of 8 kept notifying
  // for hours if Start was pressed once and forgotten.
  const [totalCycles, setTotalCycles] = useState(4);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // Parent already polls while Time is active; refresh once more when this
  // sub-tab becomes visible so the face is current even if we just switched in.
  useEffect(() => {
    if (!active) return;
    void api.timePomodoroStatus().then(onStatus);
  }, [active, onStatus]);

  const start = async () => {
    setBusy(true);
    try {
      const next = await api.timePomodoroStart({
        workMinutes,
        shortBreakMinutes,
        longBreakMinutes,
        cyclesBeforeLongBreak,
        totalCycles,
      });
      onStatus(next);
      setError(null);
    } catch (e) {
      setError(api.errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const stop = async () => {
    setBusy(true);
    try {
      onStatus(await api.timePomodoroStop());
    } finally {
      setBusy(false);
    }
  };

  const running = status?.running ?? false;

  return (
    <Section
      title={running && status?.phase ? PHASE_LABEL[status.phase] : "Pomodoro"}
      description={
        running
          ? undefined
          : "Press Start only when you want a focus cycle. Caduceus notifies at every work/break change — and keeps doing so after you close this window — until the session finishes or you stop it."
      }
    >
      {running && status ? (
        <>
          <p className="py-4 text-center font-mono text-5xl tabular-nums text-ink">
            {formatHms(status.remainingSecs)}
          </p>
          <p className="text-center text-2xs text-ink-faint">
            Work session {status.cycle}
            {status.totalCycles > 0 ? ` of ${status.totalCycles}` : ""}
          </p>
          <div className="row mt-4 justify-center">
            <Button tone="danger" onClick={() => void stop()} disabled={busy}>
              Stop
            </Button>
          </div>
        </>
      ) : (
        <>
          <div className="grid grid-cols-2 gap-3">
            <Field label="Work">
              <NumberInput value={workMinutes} onChange={setWorkMinutes} min={1} max={180} suffix="min" />
            </Field>
            <Field label="Short break">
              <NumberInput value={shortBreakMinutes} onChange={setShortBreakMinutes} min={1} max={180} suffix="min" />
            </Field>
            <Field label="Long break">
              <NumberInput value={longBreakMinutes} onChange={setLongBreakMinutes} min={1} max={180} suffix="min" />
            </Field>
            <Field label="Cycles before a long break" hint="0 = always a short break">
              <NumberInput value={cyclesBeforeLongBreak} onChange={setCyclesBeforeLongBreak} min={0} max={20} />
            </Field>
            <Field label="Total work sessions" wide>
              {/* No "0 = run until stopped by hand" option any more — that
                  was the actual source of "random" pomodoro notifications:
                  an unbounded run notifies at every phase boundary until
                  someone remembers it exists and stops it by hand, which for
                  a background utility can be never. 16 sessions, even at the
                  shortest sensible cadence, is already a full working day —
                  see `MAX_TOTAL_CYCLES` in timekeeping.rs, which enforces the
                  same ceiling server-side regardless of what this input
                  allows. */}
              <NumberInput value={totalCycles} onChange={setTotalCycles} min={1} max={16} />
            </Field>
          </div>
          <div className="row mt-3 gap-2">
            <Button tone="primary" onClick={() => void start()} disabled={busy}>
              Start
            </Button>
          </div>
          {error && <p className="mt-2 text-2xs text-danger">{error}</p>}
        </>
      )}
    </Section>
  );
}
