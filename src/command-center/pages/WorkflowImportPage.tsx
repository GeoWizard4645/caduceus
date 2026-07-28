/**
 * Review a staged workflow import — the one and only place a `caduceus://`
 * link's contents get shown to a human before anything from it is written.
 *
 * # Why this page exists at all
 *
 * `src-tauri/src/workflows.rs` will happily parse and *stage* a link the
 * instant macOS hands it to Caduceus — no click, no confirmation, nothing
 * asked of the sending process. That is deliberate: staging is inert, it
 * touches no disk and runs nothing. But it also means that until a page like
 * this one exists, a staged import is invisible and un-actionable — the whole
 * mechanism is a write-only mailbox. This page is the read side.
 *
 * # The one rule everything here follows
 *
 * The backend already did the safety-critical work (closed schema, size caps,
 * risk classification — see the module doc in `workflows.rs`). What is left
 * for the UI is *informed consent*: show every action in full, make the
 * dangerous ones impossible to miss, and never let "yes" happen by accident.
 * Concretely:
 *   - `target` for `run_command` / `run_applescript` is rendered verbatim in a
 *     monospace block. Never truncated, never summarised — the user is being
 *     asked to judge whether a shell command is safe, and hiding any of it
 *     defeats the point of asking.
 *   - A workflow whose `maxRisk` is `"high"` cannot be imported with the
 *     Import button alone. It requires a second, separate opt-in (a toggle
 *     that is off by default) before Import will even accept a click — see
 *     `accepted` below. That two-step is intentional friction, not a bug to
 *     streamline away.
 *   - Dismiss is exactly as easy as Import: same button size, same row, no
 *     "small grey link" that reads as the unimportant choice.
 *   - Nothing here claims to know who sent the link. macOS does not tell
 *     Caduceus, so the honest thing to say is that it does not know — not to
 *     invent a source.
 */

import { useCallback, useEffect, useState } from "react";

import * as api from "@/shared/api";
import type { CommitOutcome, ImportRisk, PendingAction, PendingImport } from "@/shared/api";
import { useTauriEvent } from "@/shared/hooks";
import { relativeTime } from "@/shared/providers";
import { ShortcutIcon } from "@/shared/ShortcutIcon";
import type { ShortcutKind } from "@/shared/types";
import { Button, Callout, EmptyState, Section, Spinner, Toggle, cx } from "@/shared/ui";

/** Human labels for `ShortcutKind`, mirroring `KIND_LABELS` in Settings → Shortcuts. */
const KIND_LABELS: Record<ShortcutKind, string> = {
  open_url: "Opens a URL",
  open_app: "Launches an app",
  run_command: "Runs a shell command",
  run_applescript: "Runs AppleScript",
  open_feature: "Opens a Caduceus feature",
  clipboard_view: "Opens clipboard history",
  system_monitor: "Opens system status",
};

const RISK_STYLES: Record<ImportRisk, string> = {
  low: "bg-positive/15 text-positive",
  medium: "bg-caution/15 text-caution",
  high: "bg-danger/15 text-danger",
};

const RISK_LABELS: Record<ImportRisk, string> = {
  low: "Low risk",
  medium: "Medium risk",
  high: "High risk",
};

function RiskBadge({ risk }: { risk: ImportRisk }) {
  return (
    <span
      className={cx(
        "shrink-0 rounded px-1.5 py-0.5 text-2xs font-semibold",
        RISK_STYLES[risk],
      )}
    >
      {RISK_LABELS[risk]}
    </span>
  );
}

/** Is this action's `target` the kind of thing that needs a literal, unmissable block? */
function isShellLike(kind: ShortcutKind): boolean {
  return kind === "run_command" || kind === "run_applescript";
}

function ActionCard({ action }: { action: PendingAction }) {
  const shellLike = isShellLike(action.kind);
  return (
    <div
      className={cx(
        "rounded-cad border p-3",
        shellLike ? "border-danger/40 bg-danger/[0.04]" : "border-line bg-raised/40",
      )}
    >
      <div className="flex items-start justify-between gap-3">
        <div className="row min-w-0 items-center gap-2">
          <ShortcutIcon icon={action.icon} label={action.label} className="h-5 w-5 text-ink-mute" />
          <p className="truncate text-[13px] font-medium text-ink">{action.label}</p>
        </div>
        <RiskBadge risk={action.risk} />
      </div>

      {action.description && (
        <p className="mt-1.5 text-2xs leading-relaxed text-ink-mute">{action.description}</p>
      )}

      <p className="mt-2 text-2xs text-ink-faint">{KIND_LABELS[action.kind]}</p>

      {/* This is the load-bearing part of the whole page: the exact text an
          action would run, shown in full. No truncation, no "…", no pretty
          rendering that could hide a trailing command a summary would drop. */}
      {action.target && (
        <div className="mt-2">
          <p className="mb-1 text-2xs font-medium text-ink-soft">
            {shellLike ? "Exactly what would run — shown in full, unedited" : "Target"}
          </p>
          <pre
            className={cx(
              "max-h-64 overflow-auto whitespace-pre-wrap break-all rounded-md border p-2.5 font-mono text-2xs leading-relaxed",
              shellLike
                ? "border-danger/30 bg-danger/[0.06] text-ink"
                : "border-line bg-base/60 text-ink-soft",
            )}
          >
            {action.target}
          </pre>
        </div>
      )}

      {action.args.length > 0 && (
        <div className="mt-2">
          <p className="mb-1 text-2xs font-medium text-ink-soft">Arguments</p>
          <pre className="whitespace-pre-wrap break-all rounded-md border border-line bg-base/60 p-2.5 font-mono text-2xs leading-relaxed text-ink-soft">
            {action.args.join("\n")}
          </pre>
        </div>
      )}

      {action.keywords.length > 0 && (
        <div className="row mt-2 flex-wrap gap-1">
          {action.keywords.map((k) => (
            <span key={k} className="rounded bg-overlay px-1.5 py-0.5 text-2xs text-ink-faint">
              {k}
            </span>
          ))}
        </div>
      )}

      <p className="mt-2 text-2xs text-ink-faint">
        Would be added as <code className="text-ink-soft">{action.previewId}</code>
      </p>
    </div>
  );
}

export interface ImportCardState {
  /** The separate, off-by-default opt-in a high-risk import requires. */
  accepted: boolean;
  busy: "import" | "dismiss" | null;
  error: string | null;
}

/**
 * The review card for one staged import — every action in full, the high-risk
 * gate, and the Import/Dismiss pair. Exported so Settings → Workflows can show
 * the exact same consent flow without a link having just arrived; duplicating
 * this by hand in two places is how the two surfaces would eventually say
 * different things about the same risk.
 */
export function ImportCard({
  pending,
  state,
  onToggleAccept,
  onImport,
  onDismiss,
}: {
  pending: PendingImport;
  state: ImportCardState;
  onToggleAccept: (accepted: boolean) => void;
  onImport: () => void;
  onDismiss: () => void;
}) {
  const isHighRisk = pending.maxRisk === "high";
  // High risk needs the toggle *and* the button click — two deliberate,
  // separate actions. Anything else only needs the one Import click, because
  // its worst case is "opened a URL" or "launched an app already installed".
  const importDisabled = state.busy !== null || (isHighRisk && !state.accepted);

  return (
    <Section
      title={pending.label}
      description={pending.description || undefined}
    >
      <p className="text-2xs leading-relaxed text-ink-faint">
        Arrived via <code className="text-ink-soft">caduceus://import/{pending.slug}</code>{" "}
        {relativeTime(new Date(pending.receivedAt).getTime())}. macOS does not tell Caduceus which
        app, page, or person sent it — treat the sender as unverified, the way you would an email
        attachment. <strong className="text-ink-soft">Caduceus has not run or added anything yet</strong>
        {" "}— everything below is a preview.
      </p>

      <div className="mt-4 flex flex-col gap-2.5">
        {pending.actions.map((action, i) => (
          <ActionCard key={`${action.previewId}-${i}`} action={action} />
        ))}
      </div>

      {isHighRisk && (
        <div className="mt-4 rounded-cad border border-danger/35 bg-danger/[0.06] p-3.5">
          <p className="mb-2 text-[13px] font-semibold text-danger">
            This workflow runs a shell command or AppleScript directly on your Mac
          </p>
          <p className="mb-3 text-2xs leading-relaxed text-ink-soft">
            That is full access to do anything your account can do — read or delete files, install
            software, send network requests. Read every command above. Caduceus cannot tell you
            whether it is safe; only reading it can.
          </p>
          <Toggle
            checked={state.accepted}
            onChange={onToggleAccept}
            label="I have read the command(s) above and choose to allow them to run"
            hint="Off by default. This is separate from the Import button below on purpose."
          />
        </div>
      )}

      {state.error && (
        <div className="mt-3">
          <Callout tone="danger">{state.error}</Callout>
        </div>
      )}

      <div className="mt-4 flex items-center gap-2 border-t border-line pt-4">
        <Button tone="primary" disabled={importDisabled} onClick={onImport}>
          {state.busy === "import" ? "Importing…" : "Import"}
        </Button>
        <Button tone="danger" disabled={state.busy !== null} onClick={onDismiss}>
          {state.busy === "dismiss" ? "Dismissing…" : "Dismiss"}
        </Button>
        {isHighRisk && !state.accepted && (
          <span className="text-2xs text-ink-faint">
            Turn on the toggle above to enable Import.
          </span>
        )}
      </div>
    </Section>
  );
}

/**
 * Fetches the pending-import inbox, keeps per-card consent/busy/error state in
 * step with it, and exposes the Import/Dismiss actions. Shared by this page
 * and Settings → Workflows so the two surfaces can never drift into offering
 * different levels of scrutiny for the same staged import.
 */
export function usePendingWorkflowImports() {
  const [pending, setPending] = useState<PendingImport[] | null>(null);
  const [cardState, setCardState] = useState<Record<string, ImportCardState>>({});
  const [notice, setNotice] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const list = await api.workflowsListPending();
      setPending(list);
      // Seed state for anything new; drop state for anything no longer pending
      // so a re-arrival of the same slug under a new token starts clean.
      setCardState((prev) => {
        const next: Record<string, ImportCardState> = {};
        for (const p of list) {
          next[p.token] = prev[p.token] ?? { accepted: false, busy: null, error: null };
        }
        return next;
      });
    } catch {
      setPending([]);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // A link opened while this view is already open should show up without the
  // user having to leave and come back.
  useTauriEvent<void>(api.WORKFLOW_PENDING_EVENT, () => {
    void refresh();
  });

  const patch = useCallback((t: string, change: Partial<ImportCardState>) => {
    setCardState((prev) => ({ ...prev, [t]: { ...prev[t], ...change } as ImportCardState }));
  }, []);

  const doImport = useCallback(
    async (p: PendingImport) => {
      const state = cardState[p.token];
      if (!state || state.busy) return;
      if (p.maxRisk === "high" && !state.accepted) return; // belt-and-suspenders; button is disabled
      patch(p.token, { busy: "import", error: null });
      try {
        const outcome: CommitOutcome = await api.workflowsCommitImport(p.token, state.accepted);
        setNotice(
          outcome.addedShortcutIds.length === 1
            ? `Imported "${p.label}" — added 1 shortcut.`
            : `Imported "${p.label}" — added ${outcome.addedShortcutIds.length} shortcuts.`,
        );
        await refresh();
      } catch (e) {
        patch(p.token, { busy: null, error: api.errorMessage(e) });
      }
    },
    [cardState, patch, refresh],
  );

  const doDismiss = useCallback(
    async (p: PendingImport) => {
      const state = cardState[p.token];
      if (!state || state.busy) return;
      patch(p.token, { busy: "dismiss", error: null });
      try {
        await api.workflowsDismissPending(p.token);
        setNotice(`Dismissed "${p.label}" — nothing was added.`);
        await refresh();
      } catch (e) {
        patch(p.token, { busy: null, error: api.errorMessage(e) });
      }
    },
    [cardState, patch, refresh],
  );

  return { pending, cardState, notice, refresh, patch, doImport, doDismiss };
}

export function WorkflowImportPage({
  active: _active,
  token,
  onSetTitle,
}: {
  active: boolean;
  /** Which import brought this tab open, if any — informational only; every pending import is shown. */
  token?: string;
  onSetTitle?: (title: string | undefined) => void;
}) {
  const { pending, cardState, notice, patch, doImport, doDismiss } = usePendingWorkflowImports();

  useEffect(() => {
    const count = pending?.length ?? 0;
    onSetTitle?.(count > 0 ? `Workflow import (${count})` : "Workflow import");
  }, [pending, onSetTitle]);

  return (
    <div className="mx-auto h-full max-w-[760px] overflow-y-auto px-6 py-5">
      <div className="mb-4">
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Workflow import</h1>
        <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
          Review what a shared workflow link would add before any of it is written or run. Dismissing
          costs nothing — a staged import that is never approved has no lasting effect.
        </p>
      </div>

      {notice && <p className="mb-4 text-2xs text-ink-faint">{notice}</p>}

      {pending === null ? (
        <div className="row justify-center py-16">
          <Spinner />
        </div>
      ) : pending.length === 0 ? (
        <EmptyState
          icon="⇩"
          title="Nothing waiting for review"
          hint="Open a caduceus://import/… link — from a QR code, a chat message, or the paste box in Settings → Workflows — and it will show up here before Caduceus adds anything."
        />
      ) : (
        <div className="flex flex-col gap-1">
          {pending.map((p) => {
            const state = cardState[p.token];
            if (!state) return null;
            return (
              <div
                key={p.token}
                className={cx(token === p.token && "rounded-cad ring-2 ring-accent/40")}
              >
                <ImportCard
                  pending={p}
                  state={state}
                  onToggleAccept={(accepted) => patch(p.token, { accepted })}
                  onImport={() => void doImport(p)}
                  onDismiss={() => void doDismiss(p)}
                />
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
