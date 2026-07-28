/**
 * Settings → Workflows: what one-click workflow links are, and the review
 * inbox for anything staged from one.
 *
 * A `caduceus://import/<slug>?data=…` link — shared as a QR code, a chat
 * message, or a plain URL — can hand you a small bundle of shortcuts in one
 * click instead of typing each one in by hand. Opening the link only ever
 * *stages* it (see `src-tauri/src/workflows.rs`'s module doc for the full
 * threat model); nothing is written until it is reviewed here or on the
 * dedicated import tab that opens when a link arrives.
 *
 * This tab exists so that review path is findable on its own, before a link
 * has ever shown up — someone hearing about the feature, or trying the paste
 * box below to test one, should not need a link to have just arrived to find
 * where the review happens.
 *
 * The review UI intentionally mirrors `WorkflowImportPage` (same verbatim
 * command blocks, same off-by-default high-risk toggle, same equally-sized
 * Import/Dismiss pair) rather than importing it — Settings and the Command
 * Center are separate trees in this codebase with no cross-imports between
 * them, so the two surfaces are kept in step by hand instead. If you change
 * the consent rules in one, change them in the other.
 */

import { useCallback, useEffect, useState } from "react";

import * as api from "@/shared/api";
import type { ImportRisk, PendingAction, PendingImport } from "@/shared/api";
import { useTauriEvent } from "@/shared/hooks";
import { relativeTime } from "@/shared/providers";
import { ShortcutIcon } from "@/shared/ShortcutIcon";
import type { ShortcutKind } from "@/shared/types";
import { Button, Callout, EmptyState, Section, Spinner, TextInput, Toggle, cx } from "@/shared/ui";

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

function isShellLike(kind: ShortcutKind): boolean {
  return kind === "run_command" || kind === "run_applescript";
}

function WorkflowActionRow({ action }: { action: PendingAction }) {
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
        <span className={cx("shrink-0 rounded px-1.5 py-0.5 text-2xs font-semibold", RISK_STYLES[action.risk])}>
          {RISK_LABELS[action.risk]}
        </span>
      </div>

      {action.description && (
        <p className="mt-1.5 text-2xs leading-relaxed text-ink-mute">{action.description}</p>
      )}

      <p className="mt-2 text-2xs text-ink-faint">{KIND_LABELS[action.kind]}</p>

      {/* Verbatim, in full — see the module doc above for why this is never
          shortened or prettified, especially for a shell command. */}
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

interface CardState {
  accepted: boolean;
  busy: "import" | "dismiss" | null;
  error: string | null;
}

function PendingImportSection({
  pending,
  state,
  onToggleAccept,
  onImport,
  onDismiss,
}: {
  pending: PendingImport;
  state: CardState;
  onToggleAccept: (accepted: boolean) => void;
  onImport: () => void;
  onDismiss: () => void;
}) {
  const isHighRisk = pending.maxRisk === "high";
  // Same rule as the dedicated import tab: high risk needs the toggle *and*
  // the click, so it can never be one accidental motion.
  const importDisabled = state.busy !== null || (isHighRisk && !state.accepted);

  return (
    <Section title={pending.label} description={pending.description || undefined}>
      <p className="text-2xs leading-relaxed text-ink-faint">
        Arrived via <code className="text-ink-soft">caduceus://import/{pending.slug}</code>{" "}
        {relativeTime(new Date(pending.receivedAt).getTime())}. macOS does not tell Caduceus which
        app, page, or person sent it — treat the sender as unverified.{" "}
        <strong className="text-ink-soft">Caduceus has not run or added anything yet.</strong>
      </p>

      <div className="mt-4 flex flex-col gap-2.5">
        {pending.actions.map((action, i) => (
          <WorkflowActionRow key={`${action.previewId}-${i}`} action={action} />
        ))}
      </div>

      {isHighRisk && (
        <div className="mt-4 rounded-cad border border-danger/35 bg-danger/[0.06] p-3.5">
          <p className="mb-2 text-[13px] font-semibold text-danger">
            This workflow runs a shell command or AppleScript directly on your Mac
          </p>
          <p className="mb-3 text-2xs leading-relaxed text-ink-soft">
            That is full access to do anything your account can do. Read every command above —
            Caduceus cannot judge it for you.
          </p>
          <Toggle
            checked={state.accepted}
            onChange={onToggleAccept}
            label="I have read the command(s) above and choose to allow them to run"
            hint="Off by default, and separate from the Import button below on purpose."
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
          <span className="text-2xs text-ink-faint">Turn on the toggle above to enable Import.</span>
        )}
      </div>
    </Section>
  );
}

export function WorkflowsTab() {
  const [pending, setPending] = useState<PendingImport[] | null>(null);
  const [cardState, setCardState] = useState<Record<string, CardState>>({});
  const [notice, setNotice] = useState<string | null>(null);

  const [pasteUrl, setPasteUrl] = useState("");
  const [pasteError, setPasteError] = useState<string | null>(null);
  const [pasteBusy, setPasteBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const list = await api.workflowsListPending();
      setPending(list);
      setCardState((prev) => {
        const next: Record<string, CardState> = {};
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

  useTauriEvent<void>(api.WORKFLOW_PENDING_EVENT, () => {
    void refresh();
  });

  const patch = (t: string, change: Partial<CardState>) => {
    setCardState((prev) => ({ ...prev, [t]: { ...prev[t], ...change } as CardState }));
  };

  const doImport = async (p: PendingImport) => {
    const state = cardState[p.token];
    if (!state || state.busy) return;
    if (p.maxRisk === "high" && !state.accepted) return;
    patch(p.token, { busy: "import", error: null });
    try {
      const outcome = await api.workflowsCommitImport(p.token, state.accepted);
      setNotice(
        outcome.addedShortcutIds.length === 1
          ? `Imported "${p.label}" — added 1 shortcut.`
          : `Imported "${p.label}" — added ${outcome.addedShortcutIds.length} shortcuts.`,
      );
      await refresh();
    } catch (e) {
      patch(p.token, { busy: null, error: api.errorMessage(e) });
    }
  };

  const doDismiss = async (p: PendingImport) => {
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
  };

  const stage = async () => {
    if (!pasteUrl.trim()) return;
    setPasteBusy(true);
    setPasteError(null);
    try {
      await api.workflowsStageFromUrl(pasteUrl.trim());
      setPasteUrl("");
      await refresh();
    } catch (e) {
      setPasteError(api.errorMessage(e));
    } finally {
      setPasteBusy(false);
    }
  };

  return (
    <>
      <Section
        title="What this is"
        description="A workflow link bundles a few shortcuts — actions the Command Center can run — behind one click, so someone can hand you a starting point instead of you typing each one in by hand."
      >
        <p className="text-[13px] leading-relaxed text-ink-mute">
          Opening a <code>caduceus://import/…</code> link never adds anything by itself. It only
          stages the bundle for review — the same review shown below, or on the dedicated tab that
          opens when a link arrives while Caduceus is running. Every action it would add is shown in
          full before you decide, and a workflow that runs a shell command or AppleScript needs a
          separate, explicit opt-in beyond the Import button.
        </p>
        <p className="mt-2 text-[13px] leading-relaxed text-ink-mute">
          A staged import that is never approved has no lasting effect — it is only held in memory,
          never written to disk, and disappears on its own if you close Caduceus without reviewing
          it.
        </p>
      </Section>

      <Section
        title="Import a link"
        description="Paste a caduceus://import/… link to stage it, the same as clicking one would."
      >
        <div className="row items-start gap-2">
          <div className="min-w-0 flex-1">
            <TextInput
              value={pasteUrl}
              onChange={setPasteUrl}
              placeholder="caduceus://import/example?data=…"
              mono
            />
          </div>
          <Button tone="primary" disabled={pasteBusy || !pasteUrl.trim()} onClick={() => void stage()}>
            {pasteBusy ? "Staging…" : "Stage"}
          </Button>
        </div>
        {pasteError && <p className="mt-2 text-2xs leading-relaxed text-danger">{pasteError}</p>}
      </Section>

      {/* Not a `Section` here on purpose: each pending import below is its own
          full `Section` (title, description, the whole review card), and
          nesting one bordered box inside another reads as a UI bug, not
          hierarchy. This is a plain heading instead. */}
      <div className="mb-3">
        <h2 className="text-[15px] font-semibold tracking-[-0.01em] text-ink">Pending review</h2>
        <p className="mt-1 text-[13px] leading-relaxed text-ink-mute">
          {pending && pending.length > 0
            ? `${pending.length} import${pending.length === 1 ? "" : "s"} waiting — nothing here has been added yet.`
            : "Nothing waiting right now."}
        </p>
      </div>

      {notice && <p className="mb-3 text-2xs text-ink-faint">{notice}</p>}

      {pending === null ? (
        <div className="row justify-center py-10">
          <Spinner />
        </div>
      ) : pending.length === 0 ? (
        <div className="rounded-cad border border-line bg-surface/50">
          <EmptyState
            icon="⇩"
            title="Nothing waiting for review"
            hint="Open a workflow link, or paste one above, and it will show up here before Caduceus adds anything."
          />
        </div>
      ) : (
        <div className="flex flex-col gap-1">
          {pending.map((p) => {
            const state = cardState[p.token];
            if (!state) return null;
            return (
              <PendingImportSection
                key={p.token}
                pending={p}
                state={state}
                onToggleAccept={(accepted) => patch(p.token, { accepted })}
                onImport={() => void doImport(p)}
                onDismiss={() => void doDismiss(p)}
              />
            );
          })}
        </div>
      )}
    </>
  );
}
