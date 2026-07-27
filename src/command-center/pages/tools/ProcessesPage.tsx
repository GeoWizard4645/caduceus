/**
 * Force Quit, but useful.
 *
 * macOS's own Force Quit window lists applications. It does not list the
 * background process pinning a core, and it will not tell you which one that
 * is. This lists everything, sorted by whatever is actually costing you, with
 * enough context to decide.
 *
 * # Two things it does deliberately
 *
 * **Terminate before kill.** The first press sends SIGTERM, which lets a
 * process save and close its files. SIGKILL is a second, separate, clearly
 * labelled choice — reaching for it first is how you lose the document that was
 * mid-write.
 *
 * **It refuses to kill Caduceus.** Not because you should not be able to quit
 * it, but because doing it from this list looks like the app crashing, and
 * there is a Quit item in the menu bar that says what it means.
 */

import { useCallback, useEffect, useMemo, useState } from "react";

import * as api from "@/shared/api";
import type { ProcessRow, SystemSnapshot } from "@/shared/types";
import { Button, cx } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";

type SortKey = "cpu" | "memory" | "name";

export function ProcessesPage({ active, onSetTitle }: ToolPageProps) {
  const [snapshot, setSnapshot] = useState<SystemSnapshot | null>(null);
  const [filter, setFilter] = useState("");
  const [sort, setSort] = useState<SortKey>("cpu");
  const [armed, setArmed] = useState<{ pid: number; force: boolean } | null>(null);
  const [note, setNote] = useState<string | null>(null);

  useEffect(() => onSetTitle("Processes"), [onSetTitle]);

  const refresh = useCallback(async () => {
    try {
      setSnapshot(await api.systemSnapshot(200, sort === "memory"));
    } catch (e) {
      setNote(api.errorMessage(e));
    }
  }, [sort]);

  // Two seconds: fast enough that a spike is visible, slow enough that the
  // list is readable and the poll itself is not the busiest thing running.
  useEffect(() => {
    if (!active) return;
    void refresh();
    const timer = setInterval(() => void refresh(), 2000);
    return () => clearInterval(timer);
  }, [active, refresh]);

  const rows = useMemo(() => {
    const all = snapshot?.processes ?? [];
    const needle = filter.trim().toLowerCase();
    const matched = needle
      ? all.filter(
          (p) => p.name.toLowerCase().includes(needle) || String(p.pid).includes(needle),
        )
      : all;
    return [...matched].sort((a, b) =>
      sort === "name" ? a.name.localeCompare(b.name)
      : sort === "memory" ? b.memoryBytes - a.memoryBytes
      : b.cpu - a.cpu,
    );
  }, [snapshot, filter, sort]);

  const end = async (row: ProcessRow, force: boolean) => {
    if (armed?.pid !== row.pid || armed.force !== force) {
      setArmed({ pid: row.pid, force });
      return;
    }
    setArmed(null);
    try {
      await api.systemKill(row.pid, force);
      setNote(`${force ? "Killed" : "Asked"} ${row.name} (${row.pid}) to stop.`);
      await refresh();
    } catch (e) {
      setNote(api.errorMessage(e));
    }
  };

  return (
    <div className="flex h-full flex-col">
      <div className="shrink-0 border-b border-line px-5 py-3">
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Processes</h1>
        <p className="mt-0.5 text-[13px] text-ink-mute">
          Everything running, refreshed every two seconds. Stop asks nicely; Force does not.
        </p>

        <div className="row mt-3 flex-wrap gap-2">
          <input
            type="search"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Filter by name or pid…"
            className="min-w-[180px] flex-1 rounded-lg border border-line bg-base/40 px-3 py-1.5 text-2xs text-ink placeholder:text-ink-faint focus:border-accent/50 focus:outline-none"
          />
          {(["cpu", "memory", "name"] as SortKey[]).map((key) => (
            <button
              key={key}
              type="button"
              onClick={() => setSort(key)}
              className={cx(
                "rounded-full border px-3 py-1 text-2xs capitalize transition-colors",
                sort === key
                  ? "border-accent/40 bg-accent/12 text-accent"
                  : "border-line text-ink-mute hover:bg-raised hover:text-ink",
              )}
            >
              {key}
            </button>
          ))}
        </div>

        {snapshot && (
          <p className="mt-2 text-2xs text-ink-faint">
            {snapshot.processTotal} processes · CPU {snapshot.cpuPercent.toFixed(0)}% ·
            memory {humanBytes(snapshot.memoryUsedBytes)} of {humanBytes(snapshot.memoryTotalBytes)}
          </p>
        )}
        {note && <p className="mt-1 text-2xs text-ink-mute">{note}</p>}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-2 py-2">
        {rows.map((row) => {
          const armedHere = armed?.pid === row.pid;
          const isSelf = row.name.toLowerCase().includes("caduceus");
          return (
            <div
              key={row.pid}
              className="flex items-center gap-3 rounded-lg px-3 py-1.5 transition-colors hover:bg-raised/60"
            >
              <span className="w-16 shrink-0 text-right font-mono text-2xs tabular-nums text-ink-faint">
                {row.pid}
              </span>
              <span className="min-w-0 flex-1 truncate text-[13px] text-ink">{row.name}</span>
              <span
                className={cx(
                  "w-14 shrink-0 text-right font-mono text-2xs tabular-nums",
                  row.cpu > 50 ? "text-danger" : "text-ink-mute",
                )}
              >
                {row.cpu.toFixed(1)}%
              </span>
              <span className="w-20 shrink-0 text-right font-mono text-2xs tabular-nums text-ink-mute">
                {humanBytes(row.memoryBytes)}
              </span>

              {isSelf ? (
                <span className="w-[132px] shrink-0 text-right text-2xs text-ink-faint">
                  Quit from the menu bar
                </span>
              ) : (
                <span className="row w-[132px] shrink-0 justify-end gap-1">
                  <Button size="sm" onClick={() => void end(row, false)}>
                    {armedHere && !armed.force ? "Sure?" : "Stop"}
                  </Button>
                  <Button size="sm" tone="danger" onClick={() => void end(row, true)}>
                    {armedHere && armed.force ? "Sure?" : "Force"}
                  </Button>
                </span>
              )}
            </div>
          );
        })}

        {rows.length === 0 && (
          <p className="px-3 py-10 text-center text-2xs text-ink-faint">
            {snapshot ? "Nothing matches that." : "Reading the process list…"}
          </p>
        )}
      </div>
    </div>
  );
}

/** `1.2 GB`. Bytes are unreadable and this list is meant to be scanned. */
function humanBytes(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value < 10 && unit > 0 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}
