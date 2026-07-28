/**
 * Shared UI for app-grouped process lists (System Monitor + Processes tool).
 */

import * as api from "@/shared/api";
import type { ProcessGroupRow, ProcessRow } from "@/shared/types";
import { Button } from "@/shared/ui";

export function filterProcessGroups(groups: ProcessGroupRow[], query: string): ProcessGroupRow[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return groups;
  return groups.filter(
    (g) =>
      g.name.toLowerCase().includes(needle) ||
      g.processes.some(
        (p) => p.name.toLowerCase().includes(needle) || String(p.pid).includes(needle),
      ),
  );
}

const collator = new Intl.Collator(undefined, { sensitivity: "base" });

export function compareGroupNames(a: string, b: string): number {
  return collator.compare(a, b);
}

/** Alphabetical order for display; does not reorder on later polls. */
export function stableAlphabeticalGroups(
  groups: ProcessGroupRow[],
  orderRef: { current: string[] | null },
): ProcessGroupRow[] {
  const byName = new Map(groups.map((g) => [g.name, g]));

  if (orderRef.current === null) {
    orderRef.current = [...groups]
      .sort((a, b) => compareGroupNames(a.name, b.name))
      .map((g) => g.name);
  } else {
    const present = new Set(groups.map((g) => g.name));
    orderRef.current = orderRef.current.filter((name) => present.has(name));
    for (const group of groups) {
      if (orderRef.current.includes(group.name)) continue;
      const insertAt = orderRef.current.findIndex((name) => compareGroupNames(name, group.name) > 0);
      if (insertAt === -1) orderRef.current.push(group.name);
      else orderRef.current.splice(insertAt, 0, group.name);
    }
  }

  return orderRef.current
    .map((name) => byName.get(name))
    .filter((g): g is ProcessGroupRow => g != null);
}

export function isCaduceusGroup(group: ProcessGroupRow): boolean {
  return group.name.toLowerCase().includes("caduceus");
}

export async function quitProcessGroup(
  group: ProcessGroupRow,
  force: boolean,
): Promise<{ ok: number; failed: string[] }> {
  const targets = group.processes.filter((p) => p.own);
  let ok = 0;
  const failed: string[] = [];
  for (const row of targets) {
    try {
      await api.systemKill(row.pid, force);
      ok += 1;
    } catch (e) {
      failed.push(api.errorMessage(e));
    }
  }
  return { ok, failed };
}

export function ProcessGroupList({
  groups,
  killingKey,
  onKillGroup,
  onKillProcess,
  variant = "monitor",
}: {
  groups: ProcessGroupRow[];
  killingKey: string | null;
  onKillGroup: (group: ProcessGroupRow, force: boolean) => void;
  onKillProcess: (row: ProcessRow, force: boolean) => void;
  variant?: "monitor" | "tool";
}) {
  if (groups.length === 0) {
    return null;
  }

  return (
    <div className="space-y-px">
      {groups.map((group) => (
        <ProcessGroupBlock
          key={group.name}
          group={group}
          killingKey={killingKey}
          onKillGroup={onKillGroup}
          onKillProcess={onKillProcess}
          variant={variant}
        />
      ))}
    </div>
  );
}

function ProcessGroupBlock({
  group,
  killingKey,
  onKillGroup,
  onKillProcess,
  variant,
}: {
  group: ProcessGroupRow;
  killingKey: string | null;
  onKillGroup: (group: ProcessGroupRow, force: boolean) => void;
  onKillProcess: (row: ProcessRow, force: boolean) => void;
  variant: "monitor" | "tool";
}) {
  const multi = group.processes.length > 1;
  const caduceus = isCaduceusGroup(group);
  const groupKey = `g:${group.name}`;
  const killingGroup = killingKey === groupKey;

  const metrics = (
    <>
      <span className="w-14 shrink-0 text-right font-mono text-2xs tabular-nums text-ink-mute">
        {group.cpu.toFixed(1)}%
      </span>
      <span className="w-16 shrink-0 text-right font-mono text-2xs tabular-nums text-ink-mute">
        {humanBytes(group.memoryBytes)}
      </span>
    </>
  );

  const actions =
    variant === "tool" ? (
      <ToolActions
        caduceus={caduceus}
        own={group.own}
        killing={killingGroup}
        onQuit={() => onKillGroup(group, false)}
        onForce={() => onKillGroup(group, true)}
      />
    ) : (
      <MonitorActions
        caduceus={caduceus}
        own={group.own}
        killing={killingGroup}
        onQuit={() => onKillGroup(group, false)}
        onForce={() => onKillGroup(group, true)}
      />
    );

  if (!multi) {
    const row = group.processes[0]!;
    return (
      <div className="group row justify-between gap-3 rounded-lg px-2.5 py-1.5 transition-colors hover:bg-raised/70">
        <span className="min-w-0 flex-1 truncate text-[13px] text-ink">{group.name}</span>
        {metrics}
        {variant === "monitor" ? (
          <ProcessMonitorActions
            row={row}
            caduceus={caduceus}
            killing={killingKey === `p:${row.pid}`}
            onKill={onKillProcess}
          />
        ) : (
          actions
        )}
      </div>
    );
  }

  return (
    <details className="group/details rounded-lg transition-colors hover:bg-raised/40 open:bg-raised/25">
      <summary className="row cursor-pointer list-none justify-between gap-3 px-2.5 py-1.5 [&::-webkit-details-marker]:hidden">
        <span className="row min-w-0 flex-1 gap-1.5">
          <span
            aria-hidden="true"
            className="shrink-0 text-ink-faint transition-transform group-open/details:rotate-90"
          >
            ›
          </span>
          <span className="min-w-0 truncate text-[13px] font-medium text-ink">{group.name}</span>
          <span className="shrink-0 text-2xs text-ink-faint">{group.processes.length}</span>
        </span>
        {metrics}
        <span className="w-16 shrink-0" onClick={(e) => e.preventDefault()}>
          {actions}
        </span>
      </summary>
      <ul className="space-y-px border-t border-line/60 pb-1 pl-6 pr-2 pt-0.5">
        {group.processes.map((row) => (
          <li
            key={row.pid}
            className="group/row row justify-between gap-3 rounded-md px-2 py-1 hover:bg-raised/60"
          >
            <span className="min-w-0 flex-1 truncate text-2xs text-ink-soft">{row.name}</span>
            <span className="w-14 shrink-0 text-right font-mono text-2xs tabular-nums text-ink-faint">
              {row.cpu.toFixed(1)}%
            </span>
            <span className="w-16 shrink-0 text-right font-mono text-2xs tabular-nums text-ink-faint">
              {humanBytes(row.memoryBytes)}
            </span>
            <span className="w-16 shrink-0 text-right">
              {variant === "monitor" ? (
                <ProcessMonitorActions
                  row={row}
                  caduceus={caduceus}
                  killing={killingKey === `p:${row.pid}`}
                  onKill={onKillProcess}
                />
              ) : (
                <ToolProcessActions
                  row={row}
                  caduceus={caduceus}
                  killing={killingKey === `p:${row.pid}`}
                  onKill={onKillProcess}
                />
              )}
            </span>
          </li>
        ))}
      </ul>
    </details>
  );
}

function MonitorActions({
  caduceus,
  own,
  killing,
  onQuit,
  onForce,
}: {
  caduceus: boolean;
  own: boolean;
  killing: boolean;
  onQuit: () => void;
  onForce: () => void;
}) {
  if (caduceus) {
    return (
      <span className="text-2xs text-ink-faint opacity-0 group-hover/details:opacity-100">
        menu bar
      </span>
    );
  }
  if (!own) {
    return (
      <span className="text-2xs text-ink-faint opacity-0 group-hover/details:opacity-100">
        system
      </span>
    );
  }
  return (
    <span className="row justify-end gap-1 opacity-0 transition-opacity group-hover/details:opacity-100">
      <button
        type="button"
        disabled={killing}
        onClick={(e) => {
          e.stopPropagation();
          onQuit();
        }}
        title="Ask the app to quit (all of its processes)"
        className="rounded px-1.5 py-0.5 text-2xs text-ink-mute hover:bg-overlay hover:text-ink disabled:opacity-40"
      >
        Quit
      </button>
      <button
        type="button"
        disabled={killing}
        onClick={(e) => {
          e.stopPropagation();
          onForce();
        }}
        title="Force quit the app (all of its processes)"
        className="rounded px-1.5 py-0.5 text-2xs text-danger hover:bg-danger/10 disabled:opacity-40"
      >
        Force
      </button>
    </span>
  );
}

function ProcessMonitorActions({
  row,
  caduceus,
  killing,
  onKill,
}: {
  row: ProcessRow;
  caduceus: boolean;
  killing: boolean;
  onKill: (row: ProcessRow, force: boolean) => void;
}) {
  if (caduceus) {
    return (
      <span className="text-2xs text-ink-faint opacity-0 group-hover:opacity-100">menu bar</span>
    );
  }
  if (!row.own) {
    return (
      <span className="text-2xs text-ink-faint opacity-0 group-hover:opacity-100">system</span>
    );
  }
  return (
    <span className="row justify-end gap-1 opacity-0 transition-opacity group-hover/row:opacity-100 group-hover:opacity-100">
      <button
        type="button"
        disabled={killing}
        onClick={() => onKill(row, false)}
        className="rounded px-1.5 py-0.5 text-2xs text-ink-mute hover:bg-overlay hover:text-ink disabled:opacity-40"
      >
        Quit
      </button>
      <button
        type="button"
        disabled={killing}
        onClick={() => onKill(row, true)}
        className="rounded px-1.5 py-0.5 text-2xs text-danger hover:bg-danger/10 disabled:opacity-40"
      >
        Force
      </button>
    </span>
  );
}

function ToolActions({
  caduceus,
  own,
  killing,
  onQuit,
  onForce,
}: {
  caduceus: boolean;
  own: boolean;
  killing: boolean;
  onQuit: () => void;
  onForce: () => void;
}) {
  if (caduceus) {
    return <span className="text-2xs text-ink-faint">Quit from the menu bar</span>;
  }
  if (!own) {
    return <span className="text-2xs text-ink-faint">system</span>;
  }
  return (
    <span className="row justify-end gap-1">
      <Button size="sm" disabled={killing} onClick={onQuit}>
        Stop app
      </Button>
      <Button size="sm" tone="danger" disabled={killing} onClick={onForce}>
        Force app
      </Button>
    </span>
  );
}

function ToolProcessActions({
  row,
  caduceus,
  killing,
  onKill,
}: {
  row: ProcessRow;
  caduceus: boolean;
  killing: boolean;
  onKill: (row: ProcessRow, force: boolean) => void;
}) {
  if (caduceus) return null;
  if (!row.own) return null;
  return (
    <span className="row justify-end gap-1">
      <Button size="sm" disabled={killing} onClick={() => onKill(row, false)}>
        Stop
      </Button>
      <Button size="sm" tone="danger" disabled={killing} onClick={() => onKill(row, true)}>
        Force
      </Button>
    </span>
  );
}

function humanBytes(value: number): string {
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
