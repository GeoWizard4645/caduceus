/**
 * "Caduceus needs a permission" — as a page you can act on, not a toast.
 *
 * The very old behaviour was a red line of text saying the grant was missing
 * and naming a pane. This page replaced that with a full walkthrough of its
 * own — numbered steps, a status line, a poll loop — which then got
 * duplicated a second time in `PermissionGate` and a third time in
 * `PermissionSetupPanel`, each with its own idea of what the wording and the
 * polling interval should be. `PermissionCoach` is where that now lives,
 * once, so this page's job shrinks to what is actually specific to arriving
 * here from a wall: naming which command hit it, and offering to run that
 * command again the moment the grant is in place.
 */

import { useCallback, useRef, useState } from "react";

import * as api from "@/shared/api";
import { COMMANDS, type CommandActions } from "@/shared/commands";
import { PERMISSIONS, STALE_GRANT_EXPLANATION } from "@/shared/permissions";
import { PermissionCoach } from "@/shared/PermissionCoach";
import { rememberResume, type PermissionId, type Tab } from "@/shared/tabs";
import { Button, cx } from "@/shared/ui";

export function PermissionPage({
  // No longer read here — `PermissionCoach` owns its own polling regardless
  // of whether this tab happens to be the frontmost one, the same as it
  // would need to if this page were the only place it was ever mounted.
  // Kept in the props because `CommandCenter` passes it to every tab kind
  // uniformly.
  active: _active,
  permission,
  retryCommandId,
  onOpenTab,
}: {
  active: boolean;
  permission: PermissionId;
  retryCommandId?: string;
  onOpenTab: (request: Omit<Tab, "id">) => void;
}) {
  // Falls back rather than indexing blindly. `permission` can come from a
  // restored tab written by an older version, and `PERMISSIONS[undefined]`
  // would throw during render — which, with no error boundary above this,
  // blanked the entire window.
  const info = PERMISSIONS[permission] ?? PERMISSIONS.accessibility;
  const retry = retryCommandId ? COMMANDS.find((c) => c.id === retryCommandId) : undefined;

  const [granted, setGranted] = useState(false);
  const [repairing, setRepairing] = useState(false);
  const [result, setResult] = useState<{ text: string; ok: boolean } | null>(null);
  // Guards against writing the resume point twice if `onAllGranted` fires
  // more than once before the relaunch it triggers actually tears the
  // window down.
  const resumeRemembered = useRef(false);

  const handleGranted = useCallback(() => {
    setGranted(true);
    // Screen Recording is the one grant that can read as on before capture
    // actually works — macOS applies it fully only after Caduceus restarts.
    // Write down what to come back to before asking the process to end: the
    // tab autosave is debounced and the relaunch does not wait for it, so
    // the command someone was about to retry is exactly what would be lost.
    if (info.id === "screen-recording") {
      if (retryCommandId && !resumeRemembered.current) {
        resumeRemembered.current = true;
        rememberResume({ kind: "tool", commandId: retryCommandId });
      }
      setResult({
        text: "Screen Recording is on. Restarting Caduceus so macOS applies it…",
        ok: true,
      });
      void api.relaunchApp();
    }
  }, [info.id, retryCommandId]);

  const repair = async () => {
    setRepairing(true);
    try {
      const outcome = await api.repairPermission(info.id);
      const text = outcome.willRelaunch
        ? `${outcome.message} If Caduceus closes, it should reopen in a second.`
        : outcome.message;
      setResult({ text, ok: outcome.ok });
      if (outcome.granted) setGranted(true);
    } catch (error) {
      setResult({ text: api.errorMessage(error), ok: false });
    } finally {
      setRepairing(false);
    }
  };

  const runAgain = async () => {
    if (!retry) return;
    const actions: CommandActions = {
      notify: (message, tone) => setResult({ text: message, ok: tone !== "error" }),
      showOutput: (output) =>
        setResult({ text: `${output.title} — ${output.message || "done"}`, ok: true }),
      setInput: () => {},
      openTab: onOpenTab,
      close: () => {},
    };
    try {
      await retry.run({ input: "", actions, values: {} });
      // A command that changes something visible says nothing on success, so
      // say it here rather than leaving the page looking like nothing happened.
      setResult((current) => current ?? { text: `Ran ${retry.title}.`, ok: true });
    } catch (error) {
      setResult({ text: api.errorMessage(error), ok: false });
    }
  };

  return (
    <div className="mx-auto max-w-[620px] px-6 py-6">
      <div className="mb-5">
        <p className="eyebrow">Permission needed</p>
        <h1 className="mt-1 text-[19px] font-semibold tracking-[-0.015em] text-ink">
          Let Caduceus use {info.title}
        </h1>
        {retry && (
          <p className="mt-2 max-w-prose text-[13px] leading-relaxed text-ink-mute">
            {retry.title} needs this to work.
          </p>
        )}
      </div>

      <PermissionCoach
        ids={[info.id]}
        variant="inline"
        onAllGranted={handleGranted}
        onSkip={() => {}}
      />

      {retry && (
        <div className="mt-4 flex flex-wrap items-center gap-2">
          <Button
            tone={granted || !info.detectable ? "primary" : "default"}
            onClick={() => void runAgain()}
            disabled={info.detectable && !granted}
          >
            Run “{retry.title}” again
          </Button>
          <Button tone="ghost" onClick={() => onOpenTab({ kind: "tool", commandId: retry.id })}>
            Open its page
          </Button>
        </div>
      )}

      {result && (
        <p className={cx("mt-3 text-2xs", result.ok ? "text-ink-mute" : "text-danger")}>
          {result.text}
        </p>
      )}

      {/* --- the one that catches everybody, kept below the walkthrough so it
          is not the first thing a page like this says ---------------------- */}
      <div className="mt-6 rounded-cad border border-caution/30 bg-caution/[0.06] p-4">
        <p className="text-[13px] font-semibold text-ink">It is already on and still not working</p>
        <p className="mt-1.5 max-w-prose text-[13px] leading-relaxed text-ink-mute">
          {STALE_GRANT_EXPLANATION}
        </p>
        <div className="row mt-3 gap-2">
          <Button onClick={() => void repair()} disabled={repairing}>
            {repairing ? "Repairing…" : "Repair it for me"}
          </Button>
          <span className="text-2xs text-ink-faint">
            Or switch Caduceus off and back on in the list — same effect, more clicks.
          </span>
        </div>
      </div>
    </div>
  );
}
