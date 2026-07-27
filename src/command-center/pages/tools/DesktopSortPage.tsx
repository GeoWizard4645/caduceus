/**
 * Tidy a folder — usually the Desktop.
 *
 * # Plan, look, then apply
 *
 * The whole page is built around not surprising you. Choosing a folder and a
 * grouping produces a *plan*: here is every file, here is the folder it would
 * go into, nothing has moved. Only Apply moves anything, and Undo puts it all
 * back exactly where it was.
 *
 * A one-click "tidy my Desktop" that rearranges ninety files before you can
 * read what it decided is not a feature.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import * as api from "@/shared/api";
import { useDebounced } from "@/shared/hooks";
import { Button, Select, cx } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";

const GROUPINGS: { value: api.SortBy; label: string }[] = [
  { value: "kind", label: "What it is (Images, Documents, Code…)" },
  { value: "extension", label: "File extension" },
  { value: "month", label: "Month it was last changed" },
  { value: "year", label: "Year it was last changed" },
  { value: "alphabetical", label: "First letter" },
  { value: "size", label: "Size" },
];

const PLACES = [
  { label: "Desktop", path: "~/Desktop" },
  { label: "Downloads", path: "~/Downloads" },
  { label: "Documents", path: "~/Documents" },
];

export function DesktopSortPage({ onOpenTab, onSetTitle }: ToolPageProps) {
  const [directory, setDirectory] = useState("~/Desktop");
  const [sortBy, setSortBy] = useState<api.SortBy>("kind");
  const [plan, setPlan] = useState<api.SortPlan | null>(null);
  const [undo, setUndo] = useState<api.SortMove[] | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  /**
   * Which question the plan on screen is allowed to be the answer to.
   *
   * A slow scan for the folder you were typing a moment ago can land after the
   * fast one for the folder you are looking at now. Each move carries the
   * absolute path it was planned from, so Apply would then move files out of a
   * folder the page is not showing — the one thing this page promises cannot
   * happen.
   */
  const asked = useRef("");

  // Planning is a real directory scan, so it waits for the typing to stop
  // rather than starting one per character of "~/Projects/some-nested-folder".
  // The quick-pick buttons still feel instant at this delay.
  const settled = useDebounced(directory, 150);

  useEffect(() => onSetTitle("Tidy a folder"), [onSetTitle]);

  const preview = useCallback(async () => {
    const question = `${settled}\n${sortBy}`;
    asked.current = question;
    setBusy(true);
    setNote(null);
    setUndo(null);
    try {
      const next = await api.sortPlan(settled, sortBy);
      if (asked.current !== question) return;
      setPlan(next);
    } catch (e) {
      if (asked.current !== question) return;
      setPlan(null);
      setNote(api.errorMessage(e));
    } finally {
      if (asked.current === question) setBusy(false);
    }
  }, [settled, sortBy]);

  // Re-plan whenever the question changes. Planning moves nothing, so there is
  // no reason to make people press a button to see the answer.
  useEffect(() => {
    void preview();
  }, [preview]);

  const apply = async () => {
    if (!plan) return;
    setBusy(true);
    try {
      const result = await api.sortApply(plan.moves);
      setNote(result.message);
      setUndo(result.moved);
      setPlan(null);
    } catch (e) {
      setNote(api.errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const revert = async () => {
    if (!undo) return;
    setBusy(true);
    try {
      const result = await api.sortRevert(undo);
      setNote(result.message);
      setUndo(null);
      await preview();
    } catch (e) {
      setNote(api.errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const grouped = useMemo(() => {
    const map = new Map<string, api.SortMove[]>();
    for (const move of plan?.moves ?? []) {
      const bucket = map.get(move.folder);
      if (bucket) bucket.push(move);
      else map.set(move.folder, [move]);
    }
    return [...map.entries()].sort((a, b) => b[1].length - a[1].length);
  }, [plan]);

  return (
    <div className="flex h-full flex-col">
      <div className="shrink-0 border-b border-line px-5 py-3">
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Tidy a folder</h1>
        <p className="mt-0.5 max-w-prose text-[13px] text-ink-mute">
          Nothing moves until you press Apply, and Undo puts every file back where it was.
        </p>

        <div className="row mt-3 flex-wrap gap-2">
          {PLACES.map((place) => (
            <button
              key={place.path}
              type="button"
              onClick={() => setDirectory(place.path)}
              className={cx(
                "rounded-full border px-3 py-1 text-2xs transition-colors",
                directory === place.path
                  ? "border-accent/40 bg-accent/12 text-accent"
                  : "border-line text-ink-mute hover:bg-raised hover:text-ink",
              )}
            >
              {place.label}
            </button>
          ))}
          <input
            value={directory}
            spellCheck={false}
            onChange={(e) => setDirectory(e.target.value)}
            onKeyDown={(e) => {
              if (e.key !== "Escape" || directory === "~/Desktop") return;
              // Escape puts the field back to where it started rather than
              // going unclaimed and closing the tab.
              e.preventDefault();
              setDirectory("~/Desktop");
            }}
            placeholder="~/somewhere/else"
            className="min-w-[200px] flex-1 rounded-lg border border-line bg-base/40 px-3 py-1.5 font-mono text-2xs text-ink placeholder:text-ink-faint focus:border-accent/50 focus:outline-none"
          />
        </div>

        <div className="row mt-2 gap-2">
          <span className="shrink-0 text-2xs text-ink-faint">Group by</span>
          <div className="min-w-[260px] flex-1">
            <Select
              value={sortBy}
              onChange={(next) => setSortBy(next as api.SortBy)}
              options={GROUPINGS}
            />
          </div>
        </div>

        {directory === "~/Desktop" && (
          <p className="mt-2 text-2xs text-ink-faint">
            Would rather keep the files where they are?{" "}
            <button
              type="button"
              onClick={() =>
                onOpenTab({
                  kind: "tool",
                  commandId: "page.desktop-shapes",
                  title: "Desktop icon shapes",
                })
              }
              className="text-accent underline underline-offset-2"
            >
              Arrange the icons into a shape
            </button>{" "}
            instead.
          </p>
        )}

        {note && <p className="mt-2 text-2xs text-ink-mute">{note}</p>}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">
        {undo ? (
          <div className="rounded-cad border border-positive/30 bg-positive/[0.06] p-4">
            <p className="text-[13px] text-ink">{note}</p>
            <Button className="mt-3" onClick={() => void revert()} disabled={busy}>
              Undo — put them all back
            </Button>
          </div>
        ) : !plan ? (
          <p className="py-10 text-center text-2xs text-ink-faint">
            {busy ? "Looking…" : "Nothing to show."}
          </p>
        ) : plan.moves.length === 0 ? (
          <p className="py-10 text-center text-2xs text-ink-faint">
            Nothing loose in that folder — it is already tidy.
          </p>
        ) : (
          <div className="space-y-3">
            {grouped.map(([folder, moves]) => (
              <div key={folder} className="rounded-cad border border-line bg-surface/40">
                <p className="border-b border-line px-3 py-1.5 text-2xs font-medium text-ink">
                  {folder}
                  <span className="ml-2 text-ink-faint">
                    {moves.length} file{moves.length === 1 ? "" : "s"}
                  </span>
                </p>
                <ul className="max-h-[180px] overflow-y-auto px-3 py-1.5">
                  {moves.map((move) => (
                    <li key={move.from} className="truncate text-2xs text-ink-mute">
                      {move.name}
                    </li>
                  ))}
                </ul>
              </div>
            ))}

            {plan.skipped.length > 0 && (
              <p className="text-2xs text-ink-faint">
                Left alone: {plan.skipped.slice(0, 6).join(", ")}
                {plan.skipped.length > 6 && ` and ${plan.skipped.length - 6} more`}
              </p>
            )}
          </div>
        )}
      </div>

      {plan && plan.moves.length > 0 && !undo && (
        <div className="row shrink-0 justify-between gap-3 border-t border-line px-5 py-3">
          <span className="text-[13px] text-ink">
            {plan.moves.length} file{plan.moves.length === 1 ? "" : "s"} into{" "}
            {Object.keys(plan.folders).length} folder
            {Object.keys(plan.folders).length === 1 ? "" : "s"}
          </span>
          <Button tone="primary" onClick={() => void apply()} disabled={busy}>
            {busy ? "Moving…" : "Apply"}
          </Button>
        </div>
      )}
    </div>
  );
}
