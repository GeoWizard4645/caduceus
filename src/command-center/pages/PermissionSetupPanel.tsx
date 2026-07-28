/**
 * Shared permission rows with Open Settings, Set up, and Repair actions.
 */

import { useCallback, useEffect, useState } from "react";

import * as api from "@/shared/api";
import { PERMISSIONS, STALE_GRANT_EXPLANATION } from "@/shared/permissions";
import type { PermissionId, Tab } from "@/shared/tabs";
import type { PermissionReport } from "@/shared/types";
import { Button, Callout, cx } from "@/shared/ui";

const POLL_MS = 1200;

const ROWS: {
  key: keyof PermissionReport;
  permissionId?: PermissionId;
  label: string;
  detail: string;
}[] = [
  {
    key: "accessibility",
    permissionId: "accessibility",
    label: "Accessibility",
    detail: "Window management, brightness keys, and reading selected text.",
  },
  {
    key: "screenRecording",
    permissionId: "screen-recording",
    label: "Screen Recording",
    detail: "Screenshots, screen recording, and on-screen OCR.",
  },
  {
    key: "nativeHelper",
    label: "Native helper",
    detail: "Bundled OCR and audio-switching helper inside the app.",
  },
];

export function PermissionSetupPanel({
  active,
  onOpenTab,
  compact,
}: {
  active: boolean;
  onOpenTab?: (request: Omit<Tab, "id">) => void;
  compact?: boolean;
}) {
  const [report, setReport] = useState<PermissionReport | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setReport(await api.systemPermissions());
    } catch (e) {
      setNote(api.errorMessage(e));
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void refresh();
    const timer = setInterval(() => void refresh(), POLL_MS);
    return () => clearInterval(timer);
  }, [active, refresh]);

  const setup = async (id: PermissionId) => {
    setBusy(id);
    setNote(null);
    try {
      await api.requestPermission(id);
      await api.openSystemSettings(PERMISSIONS[id].pane);
      setNote("Follow the steps in System Settings. This list updates when the switch moves.");
      onOpenTab?.({
        kind: "permission",
        permission: id,
        title: `${PERMISSIONS[id].title} permission`,
      });
    } catch (e) {
      setNote(api.errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const openOnly = async (id: PermissionId) => {
    setBusy(`open-${id}`);
    try {
      await api.openSystemSettings(PERMISSIONS[id].pane);
    } catch (e) {
      setNote(api.errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const repair = async (id: PermissionId) => {
    setBusy(`repair-${id}`);
    setNote(null);
    try {
      const outcome = await api.repairPermission(id);
      setNote(
        outcome.willRelaunch
          ? `${outcome.message} If Caduceus closes, it should reopen in a second.`
          : outcome.message,
      );
      await refresh();
    } catch (e) {
      setNote(api.errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  if (report === null) {
    return <p className="text-2xs text-ink-faint">Reading permission state…</p>;
  }

  return (
    <div className={compact ? "space-y-0" : "space-y-4"}>
      <ul className="space-y-2">
        {ROWS.map((row) => {
          const granted = report[row.key];
          const id = row.permissionId;
          const canFix = id !== undefined;

          return (
            <li
              key={row.key}
              className="rounded-lg border border-line bg-base/20 px-3 py-2.5"
            >
              <div className="flex flex-wrap items-start gap-3">
                <span
                  className={cx(
                    "shrink-0 rounded px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide",
                    granted ? "bg-positive/15 text-positive" : "bg-danger/15 text-danger",
                  )}
                >
                  {granted ? "granted" : "not granted"}
                </span>
                <div className="min-w-0 flex-1">
                  <p className="text-[13px] font-medium text-ink">{row.label}</p>
                  <p className="mt-0.5 text-2xs leading-relaxed text-ink-faint">{row.detail}</p>
                </div>
              </div>

              {canFix ? (
                <div className="row mt-2.5 flex-wrap gap-2">
                  <Button
                    size="sm"
                    tone="primary"
                    disabled={busy !== null}
                    onClick={() => void setup(id)}
                  >
                    {busy === id ? "Opening…" : "Set up…"}
                  </Button>
                  <Button
                    size="sm"
                    disabled={busy !== null}
                    onClick={() => void openOnly(id)}
                  >
                    Open Settings
                  </Button>
                  <Button
                    size="sm"
                    tone="ghost"
                    disabled={busy !== null}
                    onClick={() => void repair(id)}
                  >
                    {busy === `repair-${id}` ? "Repairing…" : "Repair stale grant"}
                  </Button>
                  {onOpenTab && (
                    <Button
                      size="sm"
                      tone="ghost"
                      onClick={() =>
                        onOpenTab({
                          kind: "permission",
                          permission: id,
                          title: `${PERMISSIONS[id].title} permission`,
                        })
                      }
                    >
                      Full guide
                    </Button>
                  )}
                </div>
              ) : (
                !granted && (
                  <p className="mt-2 text-2xs text-ink-mute">
                    Reinstall or update Caduceus — the helper ships inside the app bundle.
                  </p>
                )
              )}
            </li>
          );
        })}
      </ul>

      {!compact && (
        <Callout tone="info">
          <p className="text-2xs leading-relaxed text-ink-mute">{STALE_GRANT_EXPLANATION}</p>
        </Callout>
      )}

      {note && <p className="text-2xs text-ink-mute">{note}</p>}
    </div>
  );
}
