/**
 * Live view of an agent session — either driving the screen (computer use,
 * `agent_start_session`) or calling MCP tools (`agent_start_tool_session`).
 * Both stream the identical `AgentStep` union on `caduceus://agent-step` and
 * share `AgentRuntime` for stop/approve, so one feed renders either; only the
 * approval copy needs to know which it is — see `sessionKind` below.
 *
 * Two things here are not decoration:
 *
 * 1. **The approval gate.** Nothing runs — no click, keystroke, or tool call —
 *    until the user presses Allow. The prompt names the specific first action
 *    and, since "an agent" can now mean two very different kinds of access,
 *    says which one this session has (screen control vs. connected tools)
 *    rather than a single generic warning that would be misleading for half
 *    of what can trigger it.
 * 2. **The stop control.** Always visible while a session runs, and it takes
 *    effect at the next step boundary (mid-action stops could leave a mouse
 *    button held down).
 */

import { useEffect, useMemo, useRef, useState } from "react";

import * as api from "@/shared/api";
import { useTauriEvent } from "@/shared/hooks";
import type { AgentOutcome, AgentStep } from "@/shared/types";
import { EVENTS } from "@/shared/types";
import { Button, Spinner, cx } from "@/shared/ui";

interface FeedEntry {
  key: string;
  kind: "thinking" | "action" | "result" | "error" | "screenshot";
  text: string;
  ok?: boolean;
  image?: string;
}

/** What kind of access this session's approval gate is actually asking for. */
type SessionKind = "computer" | "tool";

const SESSION_LABEL: Record<SessionKind, string> = {
  computer: "Screen control",
  tool: "Tool-calling agent",
};

export function AgentPanel({
  sessionId,
  task,
  sessionKind,
  onClose,
}: {
  sessionId: string;
  task: string;
  /** "computer" drives the mouse/keyboard; "tool" calls MCP tools. Only
   *  changes the approval copy and the header label — the feed itself
   *  renders whatever steps arrive either way. */
  sessionKind: SessionKind;
  onClose: () => void;
}) {
  const [feed, setFeed] = useState<FeedEntry[]>([]);
  const [latestShot, setLatestShot] = useState<string | null>(null);
  const [pendingApproval, setPendingApproval] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<AgentOutcome | null>(null);
  const [stopping, setStopping] = useState(false);
  const feedRef = useRef<HTMLDivElement>(null);
  const allowRef = useRef<HTMLButtonElement>(null);
  const counter = useRef(0);

  useTauriEvent<AgentStep>(EVENTS.agentStep, (step) => {
    const key = `s${counter.current++}`;

    switch (step.type) {
      case "started":
        setFeed([{ key, kind: "thinking", text: `Running on ${step.backend} (${step.model})` }]);
        break;
      case "thinking":
        if (step.text.trim()) setFeed((f) => [...f, { key, kind: "thinking", text: step.text }]);
        break;
      case "screenshot":
        // Screenshots go to the preview pane, not the feed: a scrolling column
        // of near-identical desktop captures is noise.
        setLatestShot(step.image);
        break;
      case "action":
        setFeed((f) => [...f, { key, kind: "action", text: step.summary }]);
        break;
      case "actionResult":
        setFeed((f) => [...f, { key, kind: "result", text: step.detail, ok: step.ok }]);
        break;
      case "awaitingApproval":
        if (step.sessionId === sessionId) setPendingApproval(step.summary);
        break;
      case "error":
        setFeed((f) => [...f, { key, kind: "error", text: step.message }]);
        break;
      case "finished":
        setOutcome(step.outcome);
        setPendingApproval(null);
        break;
    }
  });

  // Keep the newest entry in view without yanking the scroll position when the
  // user has deliberately scrolled up to read something.
  useEffect(() => {
    const el = feedRef.current;
    if (!el) return;
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
    if (nearBottom) el.scrollTop = el.scrollHeight;
  }, [feed]);

  // The approval gate is the one moment this panel truly needs someone's
  // attention — everything stops until it is answered — so focus moves to
  // its primary action instead of waiting for a click or a Tab press to find
  // it. Allow, not Cancel: declining is one Tab plus Enter away, which is a
  // safer default lean than a stray Enter approving something unread.
  useEffect(() => {
    if (pendingApproval) allowRef.current?.focus();
  }, [pendingApproval]);

  const running = outcome === null;

  const stop = async () => {
    setStopping(true);
    await api.agentStopSession(sessionId);
  };

  const approve = async (allowed: boolean) => {
    setPendingApproval(null);
    await api.agentApprove(sessionId, allowed);
  };

  const status = useMemo(() => {
    if (pendingApproval) return { label: "Waiting for you", tone: "caution" as const };
    if (running) return { label: stopping ? "Stopping…" : "Running", tone: "accent" as const };
    switch (outcome?.stopReason) {
      case "completed":
        return { label: "Done", tone: "positive" as const };
      case "user_stopped":
        return { label: "Stopped", tone: "mute" as const };
      case "declined":
        return { label: "Declined", tone: "mute" as const };
      case "max_steps":
        return { label: "Step limit reached", tone: "caution" as const };
      default:
        return { label: "Failed", tone: "danger" as const };
    }
  }, [running, stopping, outcome, pendingApproval]);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {/* --- header ---------------------------------------------------- */}
      <div className="flex items-center gap-3 border-b border-line px-5 py-3">
        <StatusDot tone={status.tone} pulse={running} />
        <div className="min-w-0 flex-1">
          <p className="truncate text-[13px] font-medium text-ink">{task}</p>
          <p className="text-2xs text-ink-faint">
            {SESSION_LABEL[sessionKind]} · {status.label}
            {outcome ? ` · ${outcome.steps} step${outcome.steps === 1 ? "" : "s"}` : ""}
            {outcome?.usage?.outputTokens ? ` · ${outcome.usage.outputTokens} out tokens` : ""}
          </p>
        </div>
        {running ? (
          <Button tone="danger" size="sm" onClick={() => void stop()} disabled={stopping}>
            {stopping ? <Spinner /> : "■"} Stop
          </Button>
        ) : (
          <Button size="sm" onClick={onClose}>
            Close
          </Button>
        )}
      </div>

      {/* --- approval gate --------------------------------------------- */}
      {pendingApproval && (
        <div
          role="alertdialog"
          aria-label={
            sessionKind === "tool"
              ? "Let Caduceus use tools on your behalf?"
              : "Let Caduceus control this computer?"
          }
          className="border-b border-caution/30 bg-caution/[0.08] px-5 py-4"
        >
          <p className="text-[13px] font-semibold text-ink">
            {sessionKind === "tool"
              ? "Let Caduceus use tools on your behalf?"
              : "Let Caduceus control this computer?"}
          </p>
          <p className="mt-1 text-2xs leading-relaxed text-ink-soft">
            The agent wants to start by: <span className="text-ink">{pendingApproval}</span>.{" "}
            {sessionKind === "tool"
              ? "It can keep calling any tool connected in Settings → MCP until the task is done or you press Stop."
              : "It will keep taking screenshots and acting until the task is done or you press Stop."}
          </p>
          <div className="row mt-3">
            <Button ref={allowRef} tone="primary" size="sm" onClick={() => void approve(true)}>
              Allow
            </Button>
            <Button size="sm" onClick={() => void approve(false)}>
              Cancel
            </Button>
          </div>
        </div>
      )}

      {/* --- body ------------------------------------------------------ */}
      <div className="flex min-h-0 flex-1">
        <div ref={feedRef} className="min-w-0 flex-1 overflow-y-auto px-5 py-3">
          {feed.length === 0 ? (
            <p className="py-6 text-center text-2xs text-ink-faint">Starting…</p>
          ) : (
            <ol className="space-y-2">
              {feed.map((entry) => (
                <li key={entry.key} className="flex gap-2.5 text-[13px] leading-relaxed">
                  <span
                    aria-hidden="true"
                    className={cx(
                      "mt-[7px] h-1.5 w-1.5 shrink-0 rounded-full",
                      entry.kind === "action" && "bg-accent",
                      entry.kind === "result" && (entry.ok ? "bg-positive" : "bg-danger"),
                      entry.kind === "error" && "bg-danger",
                      entry.kind === "thinking" && "bg-ink-faint",
                    )}
                  />
                  <span
                    className={cx(
                      "min-w-0 whitespace-pre-wrap break-words selectable",
                      entry.kind === "thinking" && "text-ink-soft",
                      entry.kind === "action" && "font-medium text-ink",
                      entry.kind === "result" && (entry.ok ? "text-ink-mute" : "text-danger"),
                      entry.kind === "error" && "text-danger",
                    )}
                  >
                    {entry.text}
                  </span>
                </li>
              ))}
            </ol>
          )}

          {outcome && outcome.finalMessage && (
            <div className="mt-4 rounded-lg border border-line bg-raised/60 p-3 text-[13px] leading-relaxed text-ink-soft selectable">
              {outcome.finalMessage}
            </div>
          )}
        </div>

        {/* Live screen preview: the fastest way to tell whether the agent is
            looking at what you think it is. */}
        {latestShot && (
          <div className="w-[42%] shrink-0 border-l border-line p-3">
            <p className="eyebrow mb-2">What the agent sees</p>
            <img
              src={latestShot}
              alt="Current screen as seen by the agent"
              className="w-full rounded-lg border border-line object-contain"
            />
          </div>
        )}
      </div>
    </div>
  );
}

function StatusDot({
  tone,
  pulse,
}: {
  tone: "accent" | "positive" | "danger" | "caution" | "mute";
  pulse: boolean;
}) {
  const colours = {
    accent: "bg-accent",
    positive: "bg-positive",
    danger: "bg-danger",
    caution: "bg-caution",
    mute: "bg-ink-faint",
  };
  return (
    <span className="relative flex h-2.5 w-2.5 shrink-0">
      {pulse && (
        <span className={cx("absolute inline-flex h-full w-full animate-ping rounded-full opacity-60", colours[tone])} />
      )}
      <span className={cx("relative inline-flex h-2.5 w-2.5 rounded-full", colours[tone])} />
    </span>
  );
}
