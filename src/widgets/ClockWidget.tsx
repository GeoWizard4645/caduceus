import { useEffect, useState } from "react";

import { PixelText } from "./PixelDigits";

function formatTime(date: Date): string {
  const h = date.getHours().toString().padStart(2, "0");
  const m = date.getMinutes().toString().padStart(2, "0");
  return `${h}:${m}`;
}

function formatDate(date: Date): string {
  return date.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
}

/**
 * The trivial widget this whole system exists to prove out: a clock, because
 * it needs no permissions, no backend and nothing but `Date` to demonstrate
 * that a widget window opens, floats, remembers where it was put, and
 * updates on its own without anything driving it from Rust.
 */
export function ClockWidget() {
  const [now, setNow] = useState(() => new Date());

  useEffect(() => {
    // Aligned to the next wall-clock second rather than a flat 1000ms
    // interval, so the displayed minute does not visibly lag real time by up
    // to a second depending on when the widget happened to mount.
    let timer: ReturnType<typeof setTimeout>;
    const tick = () => {
      setNow(new Date());
      timer = setTimeout(tick, 1000 - (Date.now() % 1000));
    };
    timer = setTimeout(tick, 1000 - (Date.now() % 1000));
    return () => clearTimeout(timer);
  }, []);

  return (
    <div className="flex h-full w-full flex-col items-center justify-center gap-1.5">
      <PixelText text={formatTime(now)} cell={6} color="rgb(var(--c-accent))" />
      <span className="text-2xs font-medium uppercase tracking-[0.08em] text-ink-mute">
        {formatDate(now)}
      </span>
    </div>
  );
}
