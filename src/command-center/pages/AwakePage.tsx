/**
 * Manage → Keep Awake: sessions with a duration, in the Amphetamine mould.
 *
 * What Amphetamine calls a session maps directly: indefinite or timed, an
 * "allow display sleep" mode for overnight work, a live countdown, and one
 * click to end it. What is deliberately absent: app-triggered and
 * download-triggered sessions (they need a process watcher this page does not
 * justify yet) and a separate menu-bar icon (the staff already shows state).
 *
 * The engine is `tools::awake` on the Rust side; the palette's quick
 * `awake` commands drive the same runtime, so this page and the palette can
 * never disagree about whether the machine is being held awake.
 */

import { useCallback, useEffect, useState } from "react";

import * as api from "@/shared/api";
import type { AwakeStatus } from "@/shared/types";
import { Button, Section, cx } from "@/shared/ui";

const PRESETS: { label: string; minutes: number }[] = [
  { label: "15 minutes", minutes: 15 },
  { label: "30 minutes", minutes: 30 },
  { label: "1 hour", minutes: 60 },
  { label: "2 hours", minutes: 120 },
  { label: "5 hours", minutes: 300 },
  { label: "12 hours", minutes: 720 },
];

export function AwakePage({ active }: { active: boolean }) {
  const [status, setStatus] = useState<AwakeStatus | null>(null);
  const [displayMaySleep, setDisplayMaySleep] = useState(false);
  const [customMinutes, setCustomMinutes] = useState("");
  const [untilTime, setUntilTime] = useState("");
  const [message, setMessage] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setStatus(await api.awakeStatus());
    } catch {
      // The page can render without the number; the next tick retries.
    }
  }, []);

  // One-second cadence while a session runs, lazier when idle. The interval is
  // in a ref-driven effect so switching tabs (which keeps this mounted but
  // hidden) does not stack timers.
  const running = status?.active ?? false;
  useEffect(() => {
    if (!active) return;
    void refresh();
    const timer = setInterval(() => void refresh(), running ? 1000 : 5000);
    return () => clearInterval(timer);
  }, [refresh, active, running]);

  const start = async (minutes: number | null) => {
    try {
      const outcome = await api.awakeStart(minutes, displayMaySleep);
      setMessage(outcome.message);
      await refresh();
    } catch (error) {
      setMessage(api.errorMessage(error));
    }
  };

  const startCustom = () => {
    const minutes = Number.parseInt(customMinutes, 10);
    if (!Number.isFinite(minutes) || minutes < 1) {
      setMessage("Type a number of minutes first.");
      return;
    }
    void start(Math.min(minutes, 7 * 24 * 60));
  };

  const startUntil = () => {
    // <input type="time"> gives "HH:MM". A time earlier than now means
    // tomorrow — "until 07:00" said at 23:00 is an overnight session.
    const [h, m] = untilTime.split(":").map((part) => Number.parseInt(part, 10));
    if (!Number.isFinite(h) || !Number.isFinite(m)) {
      setMessage("Pick a time first.");
      return;
    }
    const now = new Date();
    const target = new Date(now);
    target.setHours(h, m, 0, 0);
    if (target <= now) target.setDate(target.getDate() + 1);
    const minutes = Math.max(1, Math.round((target.getTime() - now.getTime()) / 60_000));
    void start(minutes);
  };

  return (
    <div className="mx-auto max-w-[640px] px-6 py-5">
      <StatusCard status={status} onEnd={() => void api.awakeStop().then(refresh)} />

      <Section
        title="Start a session"
        description="While a session runs, this Mac will not sleep — not from idleness, not from the lid closing on AC power. Starting a new session replaces the running one."
      >
        <div className="flex flex-wrap gap-2">
          <Button tone="primary" onClick={() => void start(null)}>
            Indefinitely
          </Button>
          {PRESETS.map((preset) => (
            <Button key={preset.minutes} onClick={() => void start(preset.minutes)}>
              {preset.label}
            </Button>
          ))}
        </div>

        <div className="mt-4 flex flex-wrap items-center gap-4">
          <label className="row gap-2 text-2xs text-ink-soft">
            <input
              type="number"
              min={1}
              max={10080}
              value={customMinutes}
              onChange={(event) => setCustomMinutes(event.target.value)}
              onKeyDown={(event) => event.key === "Enter" && startCustom()}
              placeholder="minutes"
              className="w-24 rounded-lg border border-line bg-base/40 px-3 py-1.5 text-[13px] text-ink placeholder:text-ink-faint focus:border-accent/50 focus:outline-none"
            />
            <Button size="sm" onClick={startCustom}>
              Start
            </Button>
          </label>

          <label className="row gap-2 text-2xs text-ink-soft">
            until
            <input
              type="time"
              value={untilTime}
              onChange={(event) => setUntilTime(event.target.value)}
              className="rounded-lg border border-line bg-base/40 px-3 py-1.5 text-[13px] text-ink focus:border-accent/50 focus:outline-none"
            />
            <Button size="sm" onClick={startUntil}>
              Start
            </Button>
          </label>
        </div>

        <label className="mt-4 flex items-start gap-2.5 text-2xs text-ink-soft">
          <input
            type="checkbox"
            checked={displayMaySleep}
            onChange={(event) => setDisplayMaySleep(event.target.checked)}
            className="mt-0.5"
          />
          <span>
            <span className="font-medium text-ink">Allow the display to sleep</span>
            <span className="mt-0.5 block text-ink-mute">
              The machine keeps running — downloads, builds, servers — while the screen dims
              and locks as usual. The right mode for overnight work; applies to sessions
              started after changing it.
            </span>
          </span>
        </label>

        {message && <p className="mt-3 text-2xs text-ink-mute">{message}</p>}
      </Section>

      <Section
        title="How this behaves"
        description="Sessions are tied to Caduceus's own process, so quitting the app always re-enables sleep — a session can never outlive the thing you would use to end it. Battery is respected: like caffeinate itself, closing the lid on battery still sleeps the machine."
      >
        <p className="text-2xs text-ink-mute">
          From the Command Center: type <kbd className="rounded border border-line bg-raised px-1">awake 45</kbd> for
          45 minutes, <kbd className="rounded border border-line bg-raised px-1">awake 2h</kbd> for two hours, or
          just <kbd className="rounded border border-line bg-raised px-1">awake</kbd> to open this page.
        </p>
      </Section>
    </div>
  );
}

function StatusCard({ status, onEnd }: { status: AwakeStatus | null; onEnd: () => void }) {
  const active = status?.active ?? false;

  return (
    <div
      className={cx(
        "mb-5 flex items-center gap-4 rounded-xl border px-4 py-3.5",
        active ? "border-accent/40 bg-accent/8" : "border-line bg-base/20",
      )}
    >
      <span
        aria-hidden="true"
        className={cx(
          "flex h-10 w-10 items-center justify-center rounded-full border text-[18px]",
          active
            ? "border-accent/40 bg-accent/15 text-accent"
            : "border-line bg-raised text-ink-faint",
        )}
      >
        {active ? "☀" : "☾"}
      </span>

      <div className="min-w-0 flex-1">
        <p className="text-[14px] font-semibold text-ink">
          {status === null
            ? "Checking…"
            : !active
              ? "Sleep is normal"
              : status.remainingSecs === null
                ? "Staying awake until you end it"
                : `Staying awake — ${countdown(status.remainingSecs)} left`}
        </p>
        <p className="text-2xs text-ink-mute">
          {active && status
            ? status.displayMaySleep
              ? "System awake; the display may sleep."
              : "System and display both held awake."
            : "Start a session below to keep this Mac up."}
        </p>
        {status?.remainingSecs != null && status.totalSecs != null && (
          <ProgressBar remaining={status.remainingSecs} total={status.totalSecs} />
        )}
      </div>

      {active && (
        <Button size="sm" onClick={onEnd}>
          End session
        </Button>
      )}
    </div>
  );
}

function ProgressBar({ remaining, total }: { remaining: number; total: number }) {
  const fraction = total === 0 ? 0 : remaining / total;
  return (
    <div className="mt-2 h-1 overflow-hidden rounded-full bg-line/60">
      <div
        className="h-full rounded-full bg-accent transition-[width] duration-1000 ease-linear"
        style={{ width: `${Math.max(1, fraction * 100)}%` }}
      />
    </div>
  );
}

/** "2:07:33" or "43:12" — a clock, not a sentence, since it ticks every second. */
function countdown(totalSecs: number): string {
  const hours = Math.floor(totalSecs / 3600);
  const minutes = Math.floor((totalSecs % 3600) / 60);
  const seconds = totalSecs % 60;
  const pad = (value: number) => String(value).padStart(2, "0");
  return hours > 0
    ? `${hours}:${pad(minutes)}:${pad(seconds)}`
    : `${minutes}:${pad(seconds)}`;
}
