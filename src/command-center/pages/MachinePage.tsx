/**
 * Manage → This Mac: the hardware summary and the permission audit, together.
 */

import { useCallback, useEffect, useState } from "react";

import * as api from "@/shared/api";
import { Button, Section } from "@/shared/ui";

import { PermissionSetupPanel } from "./PermissionSetupPanel";

export function MachinePage({ active }: { active: boolean }) {
  const [summary, setSummary] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const machine = await api.machineSummary();
      setSummary(machine.copied ?? machine.message);
    } catch (e) {
      setMessage(api.errorMessage(e));
    }
  }, []);

  useEffect(() => {
    if (active) void refresh();
  }, [refresh, active]);

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
        description="Grant or repair what Caduceus needs. Status refreshes while this page is open."
      >
        <PermissionSetupPanel active={active} compact />
      </Section>
    </div>
  );
}
