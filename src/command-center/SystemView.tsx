/**
 * The system monitor: what is running, what it costs, and what to quit.
 *
 * Polls rather than streams, because CPU usage is only meaningful as a delta
 * between two samples — see `src-tauri/src/sysmon.rs`. The interval is
 * deliberately slow (1s): this is a panel you glance at while something is
 * misbehaving, and sampling every process on the machine faster than that costs
 * more than the information is worth.
 *
 * Polling stops the moment the view unmounts. A background timer walking the
 * whole process table while the Command Center is hidden is exactly the kind of
 * thing that makes a launcher feel heavy.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import * as api from "@/shared/api";
import type { ProcessGroupRow, ProcessRow, SystemSnapshot } from "@/shared/types";
import { Spinner, cx } from "@/shared/ui";

import {
  ProcessGroupList,
  filterProcessGroups,
  quitProcessGroup,
} from "./processGroups";

const POLL_MS = 1000;
const GROUP_LIMIT = 50;

export function SystemView({
  query,
  onNotify,
}: {
  query: string;
  onNotify: (message: string, tone: "info" | "error") => void;
}) {
  const [snap, setSnap] = useState<SystemSnapshot | null>(null);
  const [sortByMemory, setSortByMemory] = useState(false);
  const [killing, setKilling] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Ref, not state: the poll loop reads it without needing to be torn down and
  // rebuilt every time the sort flips.
  const sortRef = useRef(sortByMemory);
  sortRef.current = sortByMemory;

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;

    const tick = async () => {
      try {
        const next = await api.systemSnapshot(GROUP_LIMIT, sortRef.current);
        if (cancelled) return;
        setSnap(next);
        setError(null);
      } catch (e) {
        if (!cancelled) setError(api.errorMessage(e));
      }
      // Chained timeout rather than setInterval: a slow sample must not stack
      // up behind the next one.
      if (!cancelled) timer = setTimeout(tick, POLL_MS);
    };

    void tick();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, []);

  const removePids = useCallback((pids: Set<number>) => {
    setSnap((current) => {
      if (!current) return current;
      const processGroups = current.processGroups
        .map((g) => ({
          ...g,
          processes: g.processes.filter((p) => !pids.has(p.pid)),
        }))
        .filter((g) => g.processes.length > 0)
        .map((g) => ({
          ...g,
          cpu: g.processes.reduce((s, p) => s + p.cpu, 0),
          memoryBytes: g.processes.reduce((s, p) => s + p.memoryBytes, 0),
        }));
      return {
        ...current,
        processGroups,
        processes: current.processes.filter((p) => !pids.has(p.pid)),
      };
    });
  }, []);

  const killProcess = useCallback(
    async (row: ProcessRow, force: boolean) => {
      setKilling(`p:${row.pid}`);
      try {
        await api.systemKill(row.pid, force);
        onNotify(`${force ? "Force quit" : "Asked"} ${row.name} to quit`, "info");
        removePids(new Set([row.pid]));
      } catch (e) {
        onNotify(api.errorMessage(e), "error");
      } finally {
        setKilling(null);
      }
    },
    [onNotify, removePids],
  );

  const killGroup = useCallback(
    async (group: ProcessGroupRow, force: boolean) => {
      setKilling(`g:${group.name}`);
      try {
        const { ok, failed } = await quitProcessGroup(group, force);
        if (ok > 0) {
          onNotify(
            `${force ? "Force quit" : "Asked"} ${group.name} to quit (${ok} process${ok === 1 ? "" : "es"})`,
            "info",
          );
          const gone = new Set(group.processes.map((p) => p.pid));
          removePids(gone);
        }
        if (failed.length) onNotify(failed[0]!, "error");
      } finally {
        setKilling(null);
      }
    },
    [onNotify, removePids],
  );

  const kill = killProcess;

  if (error && !snap) {
    return <p className="px-5 py-8 text-center text-[13px] text-danger">{error}</p>;
  }

  if (!snap) {
    return (
      <div className="row justify-center px-5 py-10 text-2xs text-ink-faint">
        <Spinner /> Reading system status…
      </div>
    );
  }

  const groups = filterProcessGroups(snap.processGroups, query);

  const memPercent = snap.memoryTotalBytes
    ? (snap.memoryUsedBytes / snap.memoryTotalBytes) * 100
    : 0;

  return (
    <div className="min-h-0 flex-1 overflow-y-auto px-3 pb-3">
      {/* --- headline meters ------------------------------------------- */}
      <div className="grid grid-cols-2 gap-2 px-2 pb-3 sm:grid-cols-4">
        <Meter
          label="CPU"
          value={`${snap.cpuPercent.toFixed(0)}%`}
          detail={`${snap.coreCount} cores`}
          percent={snap.cpuPercent}
        />
        <Meter
          label="Memory"
          value={`${bytes(snap.memoryUsedBytes)}`}
          detail={`of ${bytes(snap.memoryTotalBytes)}`}
          percent={memPercent}
        />
        <Meter
          label="Network"
          value={`↓ ${bytes(snap.netDownBytes)}`}
          detail={`↑ ${bytes(snap.netUpBytes)}`}
        />
        <Meter label="Uptime" value={uptime(snap.uptimeSecs)} detail={loadAverage(snap)} />
      </div>

      {/* --- disks ------------------------------------------------------ */}
      {snap.disks.length > 0 && (
        <div className="mb-3 space-y-1.5 px-2">
          {snap.disks.slice(0, 3).map((disk) => {
            const used = disk.totalBytes - disk.availableBytes;
            const pct = disk.totalBytes ? (used / disk.totalBytes) * 100 : 0;
            return (
              <div key={disk.mountPoint} className="text-2xs">
                <div className="row justify-between text-ink-mute">
                  <span className="truncate">{disk.name || disk.mountPoint}</span>
                  <span className="shrink-0 text-ink-faint">
                    {bytes(disk.availableBytes)} free of {bytes(disk.totalBytes)}
                  </span>
                </div>
                <Bar percent={pct} />
              </div>
            );
          })}
        </div>
      )}

      {/* --- process table ---------------------------------------------- */}
      <div className="row justify-between px-2 pb-1.5">
        <span className="text-2xs text-ink-faint">
          {query.trim()
            ? `${groups.length} matching app${groups.length === 1 ? "" : "s"}`
            : `Top ${groups.length} apps · ${snap.processTotal} processes`}
        </span>
        <button
          type="button"
          onClick={() => setSortByMemory((v) => !v)}
          className="rounded-md px-2 py-0.5 text-2xs text-ink-mute transition-colors hover:bg-raised hover:text-ink"
        >
          Sort: {sortByMemory ? "Memory" : "CPU"}
        </button>
      </div>

      <ProcessGroupList
        groups={groups}
        killingKey={killing}
        onKillGroup={(g, force) => void killGroup(g, force)}
        onKillProcess={(row, force) => void kill(row, force)}
        variant="monitor"
      />

      {groups.length === 0 && (
        <p className="py-8 text-center text-2xs text-ink-faint">
          No app matches “{query.trim()}”.
        </p>
      )}

      <p className="px-2 pt-3 text-2xs text-ink-faint">
        {[snap.hostName, snap.osVersion, snap.kernelVersion].filter(Boolean).join(" · ")}
      </p>
    </div>
  );
}

function Meter({
  label,
  value,
  detail,
  percent,
}: {
  label: string;
  value: string;
  detail: string;
  percent?: number;
}) {
  return (
    <div className="rounded-lg border border-line bg-base/30 px-2.5 py-2">
      <p className="text-2xs text-ink-faint">{label}</p>
      <p className="mt-0.5 font-mono text-[15px] tabular-nums leading-none text-ink">{value}</p>
      <p className="mt-1 truncate text-2xs text-ink-faint">{detail}</p>
      {percent !== undefined && <Bar percent={percent} />}
    </div>
  );
}

function Bar({ percent }: { percent: number }) {
  const clamped = Math.max(0, Math.min(100, percent));
  return (
    <div className="mt-1.5 h-1 overflow-hidden rounded-full bg-overlay">
      <div
        className={cx(
          "h-full rounded-full transition-[width] duration-500 ease-cad",
          clamped > 90 ? "bg-danger" : clamped > 70 ? "bg-caution" : "bg-accent",
        )}
        style={{ width: `${clamped}%` }}
      />
    </div>
  );
}

/** Binary units, matching what Activity Monitor and Finder report. */
function bytes(value: number): string {
  if (value < 1024) return `${value} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let n = value / 1024;
  let unit = 0;
  while (n >= 1024 && unit < units.length - 1) {
    n /= 1024;
    unit += 1;
  }
  return `${n < 10 ? n.toFixed(1) : Math.round(n)} ${units[unit]}`;
}

function uptime(secs: number): string {
  const days = Math.floor(secs / 86_400);
  const hours = Math.floor((secs % 86_400) / 3600);
  const mins = Math.floor((secs % 3600) / 60);
  if (days) return `${days}d ${hours}h`;
  if (hours) return `${hours}h ${mins}m`;
  return `${mins}m`;
}

function loadAverage(snap: SystemSnapshot): string {
  const [one, five, fifteen] = snap.loadAverage;
  if (!one && !five && !fifteen) return "";
  return `load ${one.toFixed(2)} ${five.toFixed(2)} ${fifteen.toFixed(2)}`;
}
