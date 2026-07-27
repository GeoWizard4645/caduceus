/**
 * Where the disk went, and getting it back.
 *
 * # The three rules, restated where you can see them
 *
 * 1. **Everything goes to the Trash.** Nothing here unlinks a file. If a
 *    category turns out to have contained something you wanted, it is one
 *    Put Back away.
 * 2. **Nothing is ticked for you.** A cleaner with a big green button decides
 *    on your behalf what you were not using. This one lists what it found, what
 *    each thing is, and what removing it costs — and waits.
 * 3. **Only regenerable things are offered.** Caches, logs, build
 *    intermediates. The three categories that could hold something you meant to
 *    keep are marked, and can never be selected by "select all".
 */

import { useCallback, useEffect, useMemo, useState } from "react";

import * as api from "@/shared/api";
import type { Leftover } from "@/shared/types";
import { Button, Spinner, cx } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";

type Tab = "reclaim" | "apps";

export function StoragePage({ active, onSetTitle }: ToolPageProps) {
  const [tab, setTab] = useState<Tab>("reclaim");
  const [groups, setGroups] = useState<api.JunkGroup[] | null>(null);
  const [apps, setApps] = useState<api.InstalledAppSize[] | null>(null);
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  const [scanning, setScanning] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [armed, setArmed] = useState(false);

  useEffect(() => onSetTitle("Storage"), [onSetTitle]);

  const scan = useCallback(async () => {
    setScanning(true);
    setNote(null);
    try {
      const [junk, installed] = await Promise.all([api.scanJunk(), api.listInstalledAppSizes()]);
      setGroups(junk.filter((g) => g.bytes > 0));
      setApps(installed);
    } catch (e) {
      setNote(api.errorMessage(e));
    } finally {
      setScanning(false);
    }
  }, []);

  useEffect(() => {
    if (active && groups === null && !scanning) void scan();
  }, [active, groups, scan, scanning]);

  const selectedBytes = useMemo(
    () => (groups ?? []).filter((g) => chosen.has(g.kind)).reduce((sum, g) => sum + g.bytes, 0),
    [groups, chosen],
  );

  const toggle = (kind: string) =>
    setChosen((current) => {
      const next = new Set(current);
      if (next.has(kind)) next.delete(kind);
      else next.add(kind);
      setArmed(false);
      return next;
    });

  const clean = async () => {
    if (!armed) {
      setArmed(true);
      return;
    }
    setArmed(false);
    const kinds = (groups ?? []).filter((g) => chosen.has(g.kind)).map((g) => g.kind);
    if (kinds.length === 0) return;

    try {
      // By category, not by path — see `cleanJunk`. The Trash cannot be moved
      // to the Trash, and Rust re-scans so nothing acts on a stale list.
      const outcome = await api.cleanJunk(kinds);
      setNote(outcome.message);
      setChosen(new Set());
      await scan();
    } catch (e) {
      setNote(api.errorMessage(e));
    }
  };

  return (
    <div className="flex h-full flex-col">
      <div className="shrink-0 border-b border-line px-5 py-3">
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Storage</h1>
        <p className="mt-0.5 max-w-prose text-[13px] text-ink-mute">
          Everything here goes to the Trash, never straight to nothing. Sizes are measured,
          which is why the scan takes a moment.
        </p>

        <div className="row mt-3 gap-2">
          {(["reclaim", "apps"] as Tab[]).map((key) => (
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
              {key === "reclaim" ? "Reclaim space" : "Applications"}
            </button>
          ))}
          <Button size="sm" tone="ghost" onClick={() => void scan()} disabled={scanning}>
            {scanning ? "Scanning…" : "Rescan"}
          </Button>
          {scanning && <Spinner className="text-accent" />}
        </div>

        {note && <p className="mt-2 text-2xs text-ink-mute">{note}</p>}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">
        {tab === "reclaim" ? (
          groups === null ? (
            <p className="py-10 text-center text-2xs text-ink-faint">Measuring…</p>
          ) : groups.length === 0 ? (
            <p className="py-10 text-center text-2xs text-ink-faint">
              Nothing worth reclaiming. Genuinely — this machine is tidy.
            </p>
          ) : (
            <div className="space-y-2">
              {groups.map((group) => (
                <label
                  key={group.kind}
                  className={cx(
                    "flex cursor-pointer gap-3 rounded-cad border p-3 transition-colors",
                    chosen.has(group.kind)
                      ? "border-accent/40 bg-accent/[0.07]"
                      : "border-line bg-surface/40 hover:bg-raised/40",
                  )}
                >
                  <input
                    type="checkbox"
                    checked={chosen.has(group.kind)}
                    onChange={() => toggle(group.kind)}
                    className="mt-0.5 h-4 w-4 shrink-0 accent-current"
                  />
                  <span className="min-w-0 flex-1">
                    <span className="row justify-between gap-3">
                      <span className="text-[13px] font-medium text-ink">
                        {group.label}
                        {group.risky && (
                          <span className="ml-2 rounded border border-caution/40 bg-caution/10 px-1.5 py-px text-[10px] text-caution">
                            look first
                          </span>
                        )}
                      </span>
                      <span className="shrink-0 font-mono text-[13px] tabular-nums text-ink">
                        {group.human}
                      </span>
                    </span>
                    <span className="mt-1 block text-2xs leading-relaxed text-ink-mute">
                      {group.detail}
                    </span>
                    <span className="mt-1 block text-[10px] text-ink-faint">
                      {group.items} item{group.items === 1 ? "" : "s"}
                      {group.paths.length < group.items && " · showing the first few"}
                    </span>
                  </span>
                </label>
              ))}
            </div>
          )
        ) : apps === null ? (
          <p className="py-10 text-center text-2xs text-ink-faint">Measuring…</p>
        ) : (
          <AppList apps={apps} onChanged={() => void scan()} />
        )}
      </div>

      {tab === "reclaim" && chosen.size > 0 && (
        <div className="row shrink-0 justify-between gap-3 border-t border-line px-5 py-3">
          <span className="text-[13px] text-ink">
            {chosen.size} categor{chosen.size === 1 ? "y" : "ies"} ·{" "}
            <span className="font-mono tabular-nums">{human(selectedBytes)}</span>
          </span>
          <div className="row gap-2">
            <Button tone="ghost" onClick={() => setChosen(new Set())}>
              Clear
            </Button>
            <Button tone={armed ? "danger" : "primary"} onClick={() => void clean()}>
              {armed ? "Yes — move it all to the Trash" : "Move to Trash"}
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}

/**
 * Installed apps by size, with what removing one actually leaves behind.
 *
 * Dragging an app to the Trash leaves its preferences, caches and support files
 * scattered through ~/Library. This finds them, shows them, and lets you take
 * them too — which is the entire reason people install a "cleaner" in the first
 * place.
 */
function AppList({
  apps,
  onChanged,
}: {
  apps: api.InstalledAppSize[];
  onChanged: () => void;
}) {
  const [filter, setFilter] = useState("");
  const [open, setOpen] = useState<string | null>(null);
  const [leftovers, setLeftovers] = useState<Leftover[]>([]);
  const [note, setNote] = useState<string | null>(null);
  const [armed, setArmed] = useState(false);

  const shown = useMemo(() => {
    const needle = filter.trim().toLowerCase();
    return needle ? apps.filter((a) => a.name.toLowerCase().includes(needle)) : apps;
  }, [apps, filter]);

  const inspect = async (app: api.InstalledAppSize) => {
    if (open === app.path) {
      setOpen(null);
      return;
    }
    setOpen(app.path);
    setArmed(false);
    setNote(null);
    try {
      setLeftovers(await api.appLeftovers(app.path));
    } catch (e) {
      setLeftovers([]);
      setNote(api.errorMessage(e));
    }
  };

  const uninstall = async (app: api.InstalledAppSize) => {
    if (!armed) {
      setArmed(true);
      return;
    }
    setArmed(false);
    try {
      const outcome = await api.trashPaths([app.path, ...leftovers.map((l) => l.path)]);
      setNote(outcome.message);
      setOpen(null);
      onChanged();
    } catch (e) {
      setNote(api.errorMessage(e));
    }
  };

  return (
    <>
      <input
        type="search"
        value={filter}
        onChange={(e) => setFilter(e.target.value)}
        placeholder="Filter applications…"
        className="mb-3 w-full rounded-lg border border-line bg-base/40 px-3 py-1.5 text-2xs text-ink placeholder:text-ink-faint focus:border-accent/50 focus:outline-none"
      />
      {note && <p className="mb-2 text-2xs text-ink-mute">{note}</p>}

      <div className="space-y-1">
        {shown.map((app) => (
          <div key={app.path} className="rounded-lg border border-line bg-surface/40">
            <button
              type="button"
              onClick={() => void inspect(app)}
              className="flex w-full items-center gap-3 px-3 py-2 text-left"
            >
              <span className="min-w-0 flex-1 truncate text-[13px] text-ink">{app.name}</span>
              <span className="shrink-0 text-2xs text-ink-faint">{lastUsed(app.lastOpened)}</span>
              <span className="w-20 shrink-0 text-right font-mono text-2xs tabular-nums text-ink-mute">
                {app.human}
              </span>
            </button>

            {open === app.path && (
              <div className="border-t border-line px-3 py-2">
                {leftovers.length === 0 ? (
                  <p className="text-2xs text-ink-faint">
                    Nothing else of this app's found outside the bundle.
                  </p>
                ) : (
                  <>
                    <p className="mb-1.5 text-2xs text-ink-mute">
                      Also leaves behind {leftovers.length} item
                      {leftovers.length === 1 ? "" : "s"}:
                    </p>
                    <ul className="mb-2 max-h-[160px] overflow-y-auto">
                      {leftovers.map((l) => (
                        <li
                          key={l.path}
                          className="truncate font-mono text-[10px] text-ink-faint"
                          title={l.path}
                        >
                          {l.path}
                        </li>
                      ))}
                    </ul>
                  </>
                )}
                <Button
                  size="sm"
                  tone={armed ? "danger" : "default"}
                  onClick={() => void uninstall(app)}
                >
                  {armed
                    ? `Yes — Trash ${app.name} and ${leftovers.length} leftover${leftovers.length === 1 ? "" : "s"}`
                    : "Uninstall"}
                </Button>
              </div>
            )}
          </div>
        ))}
      </div>
    </>
  );
}

function lastUsed(epoch: number | null): string {
  if (!epoch) return "never opened";
  const days = Math.round((Date.now() / 1000 - epoch) / 86_400);
  if (days <= 0) return "today";
  if (days === 1) return "yesterday";
  if (days < 30) return `${days}d ago`;
  if (days < 365) return `${Math.round(days / 30)}mo ago`;
  return `${Math.round(days / 365)}y ago`;
}

function human(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value < 10 && unit > 0 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}
