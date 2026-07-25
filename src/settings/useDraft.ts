/**
 * Settings editing state.
 *
 * Caduceus persists **immediately** rather than behind a Save button: edits apply
 * to a local draft on the keystroke (so the UI never lags), and the draft is
 * written to disk 500ms after you stop. A single status line reports what
 * happened. This is one pattern applied consistently across all seven tabs —
 * the alternative (a Save button per tab) means every tab needs dirty-tracking
 * and a discard-changes confirmation, for no benefit on a local config file.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import * as api from "@/shared/api";
import { applyAppearance } from "@/shared/theme";
import type { Settings } from "@/shared/types";

const SAVE_DEBOUNCE_MS = 500;

export type SaveState =
  | { status: "idle" }
  | { status: "saving" }
  | { status: "saved"; at: number }
  | { status: "error"; message: string };

export interface Draft {
  settings: Settings | null;
  /** Apply a change. The updater receives a structured clone. */
  update: (mutate: (draft: Settings) => void) => void;
  /** Replace everything (used by "Reset to defaults"). */
  replace: (next: Settings) => void;
  save: SaveState;
  /** Warnings from the last save: hotkey clashes, autostart failures. */
  warnings: string[];
  reload: () => Promise<void>;
}

export function useDraft(): Draft {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [save, setSave] = useState<SaveState>({ status: "idle" });
  const [warnings, setWarnings] = useState<string[]>([]);

  const pending = useRef<Settings | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Bumped on every edit so a slow save cannot overwrite a newer draft.
  const generation = useRef(0);

  const reload = useCallback(async () => {
    try {
      const loaded = await api.getSettings();
      setSettings(loaded);
      applyAppearance(loaded.appearance);
    } catch (e) {
      setSave({ status: "error", message: api.errorMessage(e) });
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  const flush = useCallback(async () => {
    const next = pending.current;
    if (!next) return;
    pending.current = null;

    const mine = ++generation.current;
    setSave({ status: "saving" });

    try {
      const report = await api.updateSettings(next);
      // A newer edit landed while this save was in flight; its own save will
      // report the final state.
      if (mine !== generation.current) return;

      const problems = [
        ...report.hotkeyProblems,
        ...(report.autostartError ? [report.autostartError] : []),
        ...(report.encryptionReport && report.encryptionReport.dropped > 0
          ? [
              `${report.encryptionReport.dropped} clipboard ${
                report.encryptionReport.dropped === 1 ? "entry" : "entries"
              } could not be decrypted and ${
                report.encryptionReport.dropped === 1 ? "was" : "were"
              } removed.`,
            ]
          : []),
      ];
      setWarnings(problems);
      setSave({ status: "saved", at: Date.now() });
    } catch (e) {
      if (mine !== generation.current) return;
      setSave({ status: "error", message: api.errorMessage(e) });
    }
  }, []);

  const schedule = useCallback(
    (next: Settings) => {
      pending.current = next;
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => void flush(), SAVE_DEBOUNCE_MS);
    },
    [flush],
  );

  const update = useCallback(
    (mutate: (draft: Settings) => void) => {
      setSettings((current) => {
        if (!current) return current;
        const next = structuredClone(current);
        mutate(next);
        // Appearance is applied optimistically so colour and theme changes are
        // visible while you drag a slider, not 500ms later.
        applyAppearance(next.appearance);
        schedule(next);
        return next;
      });
    },
    [schedule],
  );

  const replace = useCallback(
    (next: Settings) => {
      setSettings(next);
      applyAppearance(next.appearance);
      schedule(next);
    },
    [schedule],
  );

  // Never lose an edit because the window was closed mid-debounce.
  useEffect(() => {
    const onHide = () => {
      if (pending.current) void flush();
    };
    window.addEventListener("blur", onHide);
    window.addEventListener("beforeunload", onHide);
    return () => {
      window.removeEventListener("blur", onHide);
      window.removeEventListener("beforeunload", onHide);
    };
  }, [flush]);

  return { settings, update, replace, save, warnings, reload };
}
