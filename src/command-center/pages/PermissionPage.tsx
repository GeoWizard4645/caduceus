/**
 * "Caduceus needs a permission" — as a page you can act on, not a toast.
 *
 * The old behaviour was a red line of text saying the grant was missing and
 * naming a pane. That is the least useful moment in the app: the reader wanted
 * to snap a window, and instead got a sentence about a Settings pane they now
 * have to go and find, with the thing they were doing forgotten by the time
 * they get back.
 *
 * So this opens instead — in a tab, beside whatever else is open:
 *
 * * the button that opens the exact pane, so nobody hunts for it;
 * * the clicks, numbered, in the words the Settings app uses;
 * * a live status line, because the switch is flipped in another app and
 *   polling is the only way to notice;
 * * and the command that hit the wall, offered again the moment it would work.
 */

import { useCallback, useEffect, useState } from "react";

import * as api from "@/shared/api";
import { COMMANDS, type CommandActions } from "@/shared/commands";
import { PERMISSIONS, STALE_GRANT_EXPLANATION } from "@/shared/permissions";
import type { PermissionId, Tab } from "@/shared/tabs";
import { Button, Callout, Spinner, cx } from "@/shared/ui";

/** How often to re-check while the page is open and the grant is still missing. */
const POLL_MS = 1200;

export function PermissionPage({
  active,
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

  const [granted, setGranted] = useState<boolean | null>(null);
  const [opening, setOpening] = useState(false);
  const [repairing, setRepairing] = useState(false);
  const [result, setResult] = useState<{ text: string; ok: boolean } | null>(null);

  const check = useCallback(async () => {
    if (!info.detectable) return;
    try {
      const report = await api.systemPermissions();
      setGranted(
        info.id === "accessibility" ? report.accessibility : report.screenRecording,
      );
    } catch {
      // A permission we cannot read is one we do not claim to know about.
      setGranted(null);
    }
  }, [info]);

  // Polled rather than pushed: the switch lives in another application, and
  // macOS tells nobody when it moves.
  useEffect(() => {
    if (!active || !info.detectable) return;
    void check();
    const timer = setInterval(() => void check(), POLL_MS);
    return () => clearInterval(timer);
  }, [active, check, info.detectable]);

  const open = async () => {
    setOpening(true);
    try {
      await api.openSystemSettings(info.pane);
    } catch (error) {
      setResult({ text: api.errorMessage(error), ok: false });
    } finally {
      setOpening(false);
    }
  };

  const repair = async () => {
    setRepairing(true);
    try {
      const outcome = await api.repairPermission(info.id);
      setResult({ text: outcome.message, ok: outcome.ok });
      if (outcome.granted) setGranted(true);
      else void check();
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
        <p className="mt-2 max-w-prose text-[13px] leading-relaxed text-ink-mute">{info.why}</p>
      </div>

      <div className="rounded-cad border border-line bg-surface/50 p-5">
        <ol className="space-y-3">
          {info.steps.map((step, index) => (
            <li key={index} className="flex gap-3">
              <span
                aria-hidden="true"
                className="mt-px flex h-[20px] w-[20px] shrink-0 items-center justify-center rounded-full bg-accent/15 text-2xs font-semibold text-accent"
              >
                {index + 1}
              </span>
              <span className="text-[13px] leading-relaxed text-ink-soft">{step}</span>
            </li>
          ))}
        </ol>

        <div className="mt-5 flex flex-wrap items-center gap-2">
          <Button tone="primary" onClick={() => void open()} disabled={opening}>
            {opening ? "Opening…" : `Open ${info.path.split(" → ").slice(-1)[0]}`}
          </Button>
          <span className="text-2xs text-ink-faint">System Settings → {info.path}</span>
        </div>
      </div>

      {/* --- the one that catches everybody ------------------------------ */}
      <div className="mt-4 rounded-cad border border-caution/30 bg-caution/[0.06] p-4">
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

      {/* --- status ---------------------------------------------------- */}
      <div className="mt-4">
        {info.detectable ? (
          granted ? (
            <Callout tone="positive" title="Granted">
              Caduceus can use {info.title} now.
              {retry && " The command you were running is ready to go again."}
            </Callout>
          ) : (
            <div className="row gap-2 rounded-lg border border-line bg-base/20 px-3.5 py-3 text-[13px] text-ink-mute">
              <Spinner className="text-accent" />
              <span>
                Waiting for the switch — this page updates by itself, so leave it open and
                come back.
              </span>
            </div>
          )
        ) : (
          <Callout tone="info">
            macOS does not let an app read this one back, so Caduceus cannot show you a tick.
            Grant it and try again — if it works, it worked.
          </Callout>
        )}
      </div>

      {retry && (
        <div className="mt-4 flex flex-wrap items-center gap-2">
          <Button
            tone={granted || !info.detectable ? "primary" : "default"}
            onClick={() => void runAgain()}
            disabled={info.detectable && granted === false}
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
    </div>
  );
}
