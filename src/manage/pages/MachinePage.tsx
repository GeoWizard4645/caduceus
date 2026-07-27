/**
 * Manage → This Mac: the hardware summary and the permission audit, together.
 *
 * The two things you check when something misbehaves, on one page instead of a
 * palette toast that vanishes.
 */

import { useCallback, useEffect, useState } from "react";

import * as api from "@/shared/api";
import type { PermissionReport } from "@/shared/types";
import { Button, Section, cx } from "@/shared/ui";

export function MachinePage() {
  const [summary, setSummary] = useState<string | null>(null);
  const [permissions, setPermissions] = useState<PermissionReport | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [machine, report] = await Promise.all([
        api.machineSummary(),
        api.systemPermissions(),
      ]);
      setSummary(machine.copied ?? machine.message);
      setPermissions(report);
    } catch (e) {
      setMessage(api.errorMessage(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return (
    <div className="mx-auto max-w-[640px] px-6 py-5">
      {message && <p className="mb-4 text-2xs text-danger">{message}</p>}

      <Section
        title="This Mac"
        description="Model, chip, memory, macOS, battery and uptime — ready to paste into a bug report."
      >
        {summary === null ? (
          <p className="text-2xs text-ink-faint">Reading…</p>
        ) : (
          <>
            <pre className="overflow-x-auto rounded-lg border border-line bg-base/30 px-4 py-3 font-mono text-2xs leading-relaxed text-ink-soft">
              {summary}
            </pre>
            <Button
              size="sm"
              className="mt-2"
              onClick={() => {
                void navigator.clipboard
                  .writeText(summary)
                  .then(() => setMessage("Copied."))
                  .catch(() => setMessage("Could not copy."));
              }}
            >
              Copy
            </Button>
          </>
        )}
      </Section>

      <Section
        title="Permissions"
        description="What Caduceus currently holds. Read without prompting — nothing here fires a system dialog."
      >
        {permissions === null ? (
          <p className="text-2xs text-ink-faint">Reading…</p>
        ) : (
          <ul className="space-y-1.5">
            <PermissionRow
              granted={permissions.accessibility}
              label="Accessibility"
              detail="Window management and the brightness keys."
            />
            <PermissionRow
              granted={permissions.screenRecording}
              label="Screen Recording"
              detail="Screenshots and copying text off the screen."
            />
            <PermissionRow
              granted={permissions.nativeHelper}
              label="Native helper"
              detail="The bundled OCR and audio-switching helper is installed."
            />
          </ul>
        )}
        <p className="mt-3 text-2xs text-ink-mute">
          Grant anything missing in System Settings → Privacy &amp; Security.
        </p>
      </Section>
    </div>
  );
}

function PermissionRow({
  granted,
  label,
  detail,
}: {
  granted: boolean;
  label: string;
  detail: string;
}) {
  return (
    <li className="flex items-center gap-3 rounded-lg border border-line bg-base/20 px-3 py-2">
      <span
        className={cx(
          "shrink-0 rounded px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide",
          granted ? "bg-positive/15 text-positive" : "bg-danger/15 text-danger",
        )}
      >
        {granted ? "granted" : "missing"}
      </span>
      <span className="min-w-0 flex-1">
        <span className="block text-[13px] text-ink">{label}</span>
        <span className="block text-2xs text-ink-faint">{detail}</span>
      </span>
    </li>
  );
}
