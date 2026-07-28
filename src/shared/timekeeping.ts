/**
 * Client-side helpers for the Time page.
 *
 * The state that actually matters — timer deadlines, the stopwatch's
 * accumulated time, the pomodoro's phase and cycle — lives in Rust; see the
 * header comment on `tools::timekeeping` for why. Everything in *this* file
 * is the opposite kind of thing: pure formatting, and the one bit of clock
 * math (`zoneWallClock`) that lets the world clock tick every second without
 * asking Rust for the time every second. `ZoneClock.offsetMinutes` is fetched
 * once (and refreshed occasionally, to catch a DST edge) and then added to
 * the browser's own `Date.now()` locally — the offset changes at most twice a
 * year, so there is nothing to gain from re-deriving it every tick.
 */

/** A zone's current wall-clock time, encoded as a `Date` whose *UTC* fields
 * are the zone's local time — read it back with `getUTCHours` etc., or format
 * it with `timeZone: "UTC"`, never with the browser's own zone. */
export function zoneWallClock(offsetMinutes: number, now: Date = new Date()): Date {
  return new Date(now.getTime() + offsetMinutes * 60_000);
}

const CLOCK_FORMAT = new Intl.DateTimeFormat(undefined, {
  hour: "numeric",
  minute: "2-digit",
  second: "2-digit",
  hour12: true,
  timeZone: "UTC",
});

const DAY_FORMAT = new Intl.DateTimeFormat(undefined, {
  weekday: "short",
  month: "short",
  day: "numeric",
  timeZone: "UTC",
});

/** "3:45:12 PM" for a `zoneWallClock` result. */
export function formatClock(date: Date): string {
  return CLOCK_FORMAT.format(date);
}

/** "Mon, Jan 5" for a `zoneWallClock` result. */
export function formatDay(date: Date): string {
  return DAY_FORMAT.format(date);
}

/** `3725` → `"1:02:05"`; under an hour drops the leading unit → `"2:05"`. */
export function formatHms(totalSeconds: number): string {
  const secs = Math.max(0, Math.round(totalSeconds));
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  const pad = (n: number) => n.toString().padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

/** `65432` (ms) → `"1:05.43"` — the stopwatch's own format, with centiseconds. */
export function formatStopwatch(totalMs: number): string {
  const ms = Math.max(0, Math.round(totalMs));
  const centis = Math.floor((ms % 1000) / 10);
  const totalSeconds = Math.floor(ms / 1000);
  const h = Math.floor(totalSeconds / 3600);
  const m = Math.floor((totalSeconds % 3600) / 60);
  const s = totalSeconds % 60;
  const pad = (n: number) => n.toString().padStart(2, "0");
  const base = h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
  return `${base}.${pad(centis)}`;
}

/** "+1 day" / "-1 day" / `null` for same-day — the converter's own label. */
export function dayOffsetLabel(dayOffset: number): string | null {
  if (dayOffset === 0) return null;
  const n = Math.abs(dayOffset);
  const noun = n === 1 ? "day" : "days";
  return dayOffset > 0 ? `+${n} ${noun}` : `-${n} ${noun}`;
}

/** `"2026-07-27T14:32:10"` → the `Date` it names, read as UTC fields — the
 * inverse of how the Rust side encodes a zone's local time (see the module
 * header), so a `local_iso` round-trips through here without ever touching
 * the browser's own timezone. */
export function parseLocalIso(iso: string): Date {
  const [datePart, timePart = "00:00:00"] = iso.split("T");
  const [year, month, day] = datePart.split("-").map(Number);
  const [hour, minute, second = "0"] = timePart.split(":");
  return new Date(Date.UTC(year, (month ?? 1) - 1, day ?? 1, Number(hour ?? 0), Number(minute ?? 0), Number(second)));
}

/** The value an `<input type="datetime-local">` wants, from a wall-clock `Date`. */
export function toDatetimeLocalValue(date: Date): string {
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${date.getUTCFullYear()}-${pad(date.getUTCMonth() + 1)}-${pad(date.getUTCDate())}T${pad(date.getUTCHours())}:${pad(date.getUTCMinutes())}`;
}
