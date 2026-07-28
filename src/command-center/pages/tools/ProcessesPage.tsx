/**
 * Force Quit, but useful.
 *
 * macOS's own Force Quit window lists applications. It does not list the
 * background process pinning a core, and it will not tell you which one that
 * is. This lists everything, with live CPU and memory, in a stable
 * alphabetical list so Stop / Force stays under your cursor while stats refresh.
 *
 * Processes are grouped by application (bundle), like Activity Monitor's App view.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import * as api from "@/shared/api";
import { useEscape } from "@/shared/hooks";
import type { ProcessGroupRow, ProcessRow, SystemSnapshot } from "@/shared/types";
import type { ToolPageProps } from "../ToolPage";

import {
  ProcessGroupList,
  compareGroupNames,
  filterProcessGroups,
  quitProcessGroup,
  stableAlphabeticalGroups,
} from "../../processGroups";

export function ProcessesPage({ active, onSetTitle }: ToolPageProps) {
  const [snapshot, setSnapshot] = useState<SystemSnapshot | null>(null);
  const [filter, setFilter] = useState("");
  const [armed, setArmed] = useState<{ key: string; force: boolean } | null>(null);
  const [killing, setKilling] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const groupOrderRef = useRef<string[] | null>(null);

  useEffect(() => onSetTitle("Processes"), [onSetTitle]);

  const refresh = useCallback(async () => {
    try {
      setSnapshot(await api.systemSnapshot(200, false, true));
    } catch (e) {
      setNote(api.errorMessage(e));
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void refresh();
    const timer = setInterval(() => void refresh(), 2000);
    return () => clearInterval(timer);
  }, [active, refresh]);

  useEffect(() => {
    if (!active) groupOrderRef.current = null;
  }, [active]);

  const groups = useMemo(() => {
    const raw = snapshot?.processGroups ?? [];
    const filtered = filterProcessGroups(raw, filter);
    if (filter.trim()) {
      return [...filtered].sort((a, b) => compareGroupNames(a.name, b.name));
    }
    return stableAlphabeticalGroups(filtered, groupOrderRef);
  }, [snapshot, filter]);

  useEscape(active, () => {
    if (armed) {
      setArmed(null);
      return true;
    }
    if (filter) {
      setFilter("");
      return true;
    }
    return false;
  });

  const runKill = async (key: string, force: boolean, run: () => Promise<void>) => {
    if (armed?.key !== key || armed.force !== force) {
      setArmed({ key, force });
      return;
    }
    setArmed(null);
    setKilling(key);
    try {
      await run();
      await refresh();
    } catch (e) {
      setNote(api.errorMessage(e));
    } finally {
      setKilling(null);
    }
  };

  const killGroup = (group: ProcessGroupRow, force: boolean) =>
    runKill(`g:${group.name}:${force}`, force, async () => {
      const { ok, failed } = await quitProcessGroup(group, force);
      if (ok > 0) {
        setNote(`${force ? "Killed" : "Asked"} ${group.name} (${ok} process${ok === 1 ? "" : "es"}).`);
      }
      if (failed.length) throw new Error(failed[0]);
    });

  const killProcess = (row: ProcessRow, force: boolean) =>
    runKill(`p:${row.pid}:${force}`, force, async () => {
      await api.systemKill(row.pid, force);
      setNote(`${force ? "Killed" : "Asked"} ${row.name} (${row.pid}) to stop.`);
    });

  return (
    <div className="flex h-full flex-col">
      <div className="shrink-0 border-b border-line px-5 py-3">
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Processes</h1>
        <p className="mt-0.5 text-[13px] text-ink-mute">
          Grouped by app, A–Z. Stats refresh every two seconds; row order stays put. Stop asks
          nicely; Force does not.
        </p>

        <div className="row mt-3 flex-wrap gap-2">
          <input
            type="search"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Filter by app, process, or pid…"
            className="min-w-[180px] flex-1 rounded-lg border border-line bg-base/40 px-3 py-1.5 text-2xs text-ink placeholder:text-ink-faint focus:border-accent/50 focus:outline-none"
          />
        </div>

        {snapshot && (
          <p className="mt-2 text-2xs text-ink-faint">
            {snapshot.processGroups.length} apps · {snapshot.processTotal} processes · CPU{" "}
            {snapshot.cpuPercent.toFixed(0)}% · memory {humanBytes(snapshot.memoryUsedBytes)} of{" "}
            {humanBytes(snapshot.memoryTotalBytes)}
          </p>
        )}
        {note && <p className="mt-1 text-2xs text-ink-mute">{note}</p>}
        {armed && (
          <p className="mt-1 text-2xs text-caution">Press again to confirm, or Escape to cancel.</p>
        )}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-2 py-2">
        <ProcessGroupList
          groups={groups}
          killingKey={killing}
          onKillGroup={killGroup}
          onKillProcess={killProcess}
          variant="tool"
        />

        {groups.length === 0 && (
          <p className="px-3 py-10 text-center text-2xs text-ink-faint">
            {snapshot ? "Nothing matches that." : "Reading the process list…"}
          </p>
        )}
      </div>
    </div>
  );
}

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
