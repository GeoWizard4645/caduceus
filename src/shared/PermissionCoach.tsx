/**
 * A permission ask, coached rather than announced.
 *
 * Every place in Caduceus that hits a macOS privacy wall used to say so and
 * stop — "Caduceus needs Accessibility" — and leave the reader to go and find
 * the right pane themselves, work out which switch among a dozen, and then
 * remember what they were doing by the time they get back. `PermissionCoach`
 * is the one component built to close that gap wherever it opens: at first
 * run, where several grants are asked for in a row before the rest of the app
 * makes sense, and inline on a tool page that has just discovered it cannot do
 * what was asked of it.
 *
 * It is deliberately opinionated about *how* to ask:
 *
 * * the copy, the pane, and the numbered clicks all come from `PERMISSIONS`
 *   in `./permissions` — this file renders that data, it does not repeat it;
 * * a button opens the exact System Settings pane, never "go to Settings";
 * * Accessibility and Screen Recording report themselves back, so their
 *   status is polled and shown live; Microphone, Speech Recognition and
 *   Automation do not, so those ask for a plain "I've turned it on" instead of
 *   showing a tick that would really be a guess — see `detectable` on
 *   `PermissionInfo`;
 * * an animated arrow sits on the one action that matters right now, because
 *   "click the button below" is a sentence people skim past, and a pointer
 *   drawing the eye to the actual button is not;
 * * Screen Recording only takes effect once Caduceus has restarted — macOS
 *   only reads that grant at process launch — so the moment it flips on this
 *   coaches through the restart rather than leaving capture silently broken.
 *   Accessibility occasionally needs the same nudge, so a manual restart
 *   button is always within reach once it is granted;
 * * and none of it is mandatory. `onSkip`, when given, is always rendered —
 *   nobody granting permissions is held hostage by this component.
 *
 * Every animation here is a Tailwind core utility (`animate-bounce`,
 * `animate-pulse`, the project's own `animate-fade-rise`) composed on plain
 * elements. Nothing is fetched, and nothing depends on a library: the app is
 * offline and stays that way.
 */

import { useEffect, useRef, useState } from "react";

import * as api from "./api";
import { PERMISSIONS } from "./permissions";
import type { PermissionId } from "./tabs";
import { Button, Callout, Spinner, cx } from "./ui";

export interface PermissionCoachProps {
  ids: PermissionId[];
  onAllGranted?: () => void;
  onSkip?: () => void;
  /** "onboarding" — large, generous, full-width. "inline" (default) — a compact banner. */
  variant?: "onboarding" | "inline";
}

/** How often to re-check the grants macOS will actually tell Caduceus about. */
const POLL_MS = 1200;

/**
 * The small "look here" callout that sits on the one button worth pressing
 * right now. A fade-in on the outer wrapper, a bounce on the inner group —
 * two elements rather than one, because stacking two `animate-*` utilities on
 * the same node is not reliable: whichever rule Tailwind happens to emit last
 * wins, and nothing here should depend on that ordering.
 */
function Nudge({ label }: { label: string }) {
  return (
    <div
      aria-hidden="true"
      className="pointer-events-none absolute -right-2 -top-12 z-20 animate-fade-rise"
    >
      <div className="flex flex-col items-end animate-bounce">
        <span className="mb-1 whitespace-nowrap rounded-full bg-accent px-2.5 py-1 text-2xs font-semibold text-accent-ink shadow-float">
          {label}
        </span>
        <svg width="28" height="28" viewBox="0 0 28 28" fill="none" className="text-accent drop-shadow-sm">
          <path d="M6 6 L22 22" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" />
          <path
            d="M22 11 L22 22 L11 22"
            stroke="currentColor"
            strokeWidth="2.5"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      </div>
    </div>
  );
}

// The `JSX.Element` return type in this component's brief is the *global*
// JSX namespace React's own types stopped exporting once this repo moved to
// React 19 — only `React.JSX` exists now, and nothing else in the codebase
// annotates a component's return type at all. Leaving it inferred keeps this
// file honest with `npx tsc --noEmit` while matching the same contract: every
// path below returns a real element.
export function PermissionCoach({
  ids,
  onAllGranted,
  onSkip,
  variant = "inline",
}: PermissionCoachProps) {
  const isOnboarding = variant === "onboarding";

  // Detectable ids (accessibility, screen-recording) get a real reading from
  // macOS. The rest live here as a plain yes/no the reader gave us themselves.
  const [statuses, setStatuses] = useState<Partial<Record<PermissionId, boolean>>>({});
  const [confirmed, setConfirmed] = useState<Partial<Record<PermissionId, boolean>>>({});
  const [openedOnce, setOpenedOnce] = useState<Partial<Record<PermissionId, boolean>>>({});
  const [openingId, setOpeningId] = useState<PermissionId | null>(null);
  const [relaunching, setRelaunching] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  // Lets a reader browse a permission other than "whatever's next" — a click
  // on one of the progress chips below — without losing the walkthrough.
  const [pinnedIndex, setPinnedIndex] = useState<number | null>(null);

  const grantedFiredRef = useRef(false);
  const wasScreenRecordingGranted = useRef<boolean | null>(null);

  const isSatisfied = (id: PermissionId) =>
    PERMISSIONS[id].detectable ? statuses[id] === true : confirmed[id] === true;

  const idsKey = ids.join(",");

  // Polled rather than pushed: the switch lives in another application, and
  // macOS tells nobody when it moves. Only the ids that can actually report
  // themselves are worth asking about.
  useEffect(() => {
    const detectableIds = ids.filter((id) => PERMISSIONS[id].detectable);
    if (detectableIds.length === 0) return;

    let cancelled = false;
    const check = async () => {
      try {
        const report = await api.systemPermissions();
        if (cancelled) return;
        setStatuses((prev) => {
          const next = { ...prev };
          for (const id of detectableIds) {
            next[id] = id === "accessibility" ? report.accessibility : report.screenRecording;
          }
          return next;
        });
      } catch {
        // A grant Caduceus cannot read is left unknown, not guessed at.
      }
    };

    void check();
    const timer = setInterval(() => void check(), POLL_MS);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [idsKey]);

  const firstUnsatisfiedIndex = ids.findIndex((id) => !isSatisfied(id));
  const allGranted = firstUnsatisfiedIndex === -1;

  // Fires once, ever, for this mount — not once per satisfied streak. A grant
  // that goes stale later (see STALE_GRANT_EXPLANATION in ./permissions) is a
  // repair story, not a reason to re-run whatever onAllGranted kicked off.
  useEffect(() => {
    if (allGranted && ids.length > 0 && !grantedFiredRef.current) {
      grantedFiredRef.current = true;
      onAllGranted?.();
    }
  }, [allGranted, ids.length, onAllGranted]);

  // Screen Recording only takes effect after a relaunch — macOS re-reads it
  // once, at process start. The moment it flips from off to on, get that
  // relaunch scheduled rather than leaving capture quietly broken.
  const hasScreenRecording = ids.includes("screen-recording");
  useEffect(() => {
    if (!hasScreenRecording) return;
    const granted = statuses["screen-recording"];
    if (granted === undefined) return;
    if (wasScreenRecordingGranted.current === false && granted === true) {
      setNote("Screen Recording is on. Restarting Caduceus so macOS applies it…");
      void api.relaunchApp();
    }
    wasScreenRecordingGranted.current = granted;
  }, [hasScreenRecording, statuses]);

  const activeIndex =
    pinnedIndex !== null && pinnedIndex < ids.length
      ? pinnedIndex
      : firstUnsatisfiedIndex === -1
        ? ids.length - 1
        : firstUnsatisfiedIndex;
  const activeId = ids.length > 0 ? ids[activeIndex] : undefined;

  if (!activeId) {
    // Nothing was asked for. Rendering nothing is the honest answer — this is
    // not a wall, and a wall with nothing behind it is still a wall.
    return <></>;
  }

  const info = PERMISSIONS[activeId];
  const activeSatisfied = isSatisfied(activeId);
  // Before the pane has been opened, the step worth pointing at is "click the
  // button". After, it is whichever step actually flips the switch — every
  // page's steps put that second, right after "open the pane".
  const stepIndex = openedOnce[activeId] ? Math.min(1, info.steps.length - 1) : 0;
  const showPointer = !activeSatisfied && activeIndex === firstUnsatisfiedIndex;

  const openPane = async (id: PermissionId) => {
    const target = PERMISSIONS[id];
    setOpeningId(id);
    setOpenedOnce((prev) => ({ ...prev, [id]: true }));
    try {
      // Only Accessibility and Screen Recording have a programmatic prompt;
      // the rest are asked for by whichever framework needs them, the moment
      // it needs them — see window::grants::request on the Rust side.
      if (id === "accessibility" || id === "screen-recording") {
        await api.requestPermission(id);
      }
      await api.openSystemSettings(target.pane);
    } catch (error) {
      setNote(api.errorMessage(error));
    } finally {
      setOpeningId(null);
    }
  };

  const restartNow = async () => {
    setRelaunching(true);
    try {
      await api.relaunchApp();
    } catch (error) {
      setNote(api.errorMessage(error));
      setRelaunching(false);
    }
  };

  return (
    <div
      className={cx(
        "animate-fade-rise",
        isOnboarding
          ? "mx-auto w-full max-w-[640px] rounded-cad-lg border border-line bg-surface/60 p-8 shadow-panel"
          : "w-full rounded-cad border border-accent/25 bg-accent/[0.05] p-4",
      )}
    >
      {ids.length > 1 && (
        <div className="mb-4 flex flex-wrap items-center gap-1.5">
          {ids.map((id, index) => {
            const done = isSatisfied(id);
            const isActive = index === activeIndex;
            return (
              <button
                key={id}
                type="button"
                onClick={() => setPinnedIndex(index)}
                className={cx(
                  "flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-2xs font-medium transition-colors duration-150",
                  done
                    ? "border-positive/30 bg-positive/10 text-positive"
                    : isActive
                      ? "border-accent/40 bg-accent/10 text-accent"
                      : "border-line text-ink-faint hover:text-ink-soft",
                )}
              >
                <span aria-hidden="true">{done ? "✓" : index + 1}</span>
                {PERMISSIONS[id].title}
              </button>
            );
          })}
        </div>
      )}

      <div className={isOnboarding ? "mb-6" : "mb-3"}>
        <p className="eyebrow">{isOnboarding ? "Let's get you set up" : "Permission needed"}</p>
        <h2
          className={cx(
            "mt-1 font-semibold tracking-[-0.015em] text-ink",
            isOnboarding ? "text-[22px]" : "text-[15px]",
          )}
        >
          Let Caduceus use {info.title}
        </h2>
        <p
          className={cx(
            "mt-1.5 max-w-prose leading-relaxed text-ink-mute",
            isOnboarding ? "text-[13.5px]" : "text-2xs",
          )}
        >
          {info.why}
        </p>
      </div>

      <div className={cx("rounded-cad border border-line bg-surface/50", isOnboarding ? "p-5" : "p-4")}>
        <ol className="space-y-3">
          {info.steps.map((step, index) => {
            const highlighted = !activeSatisfied && activeIndex === firstUnsatisfiedIndex && index === stepIndex;
            return (
              <li key={index} className="flex gap-3">
                <span
                  aria-hidden="true"
                  className={cx(
                    "mt-px flex h-[20px] w-[20px] shrink-0 items-center justify-center rounded-full text-2xs font-semibold transition-colors duration-200",
                    highlighted ? "bg-accent text-accent-ink" : "bg-accent/15 text-accent",
                  )}
                >
                  {index + 1}
                </span>
                <span
                  className={cx(
                    "text-[13px] leading-relaxed",
                    highlighted ? "font-medium text-ink" : "text-ink-soft",
                  )}
                >
                  {step}
                </span>
              </li>
            );
          })}
        </ol>

        <div className="mt-5 flex flex-wrap items-center gap-2">
          <div className="relative inline-block">
            {showPointer && (
              <span
                aria-hidden="true"
                className="pointer-events-none absolute -inset-1.5 rounded-[10px] ring-2 ring-accent/60 animate-pulse"
              />
            )}
            {showPointer && <Nudge label="Click this" />}
            <Button tone="primary" onClick={() => void openPane(activeId)} disabled={openingId === activeId}>
              {openingId === activeId ? "Opening…" : `Open ${info.path.split(" → ").slice(-1)[0]}`}
            </Button>
          </div>
          <span className="text-2xs text-ink-faint">System Settings → {info.path}</span>
        </div>

        {info.id === "screen-recording" && (
          <p className="mt-3 text-2xs leading-relaxed text-ink-faint">
            This one only takes effect after Caduceus restarts — macOS checks it once, at launch.
            Caduceus restarts itself the moment the switch flips on.
          </p>
        )}
        {info.id === "accessibility" && activeSatisfied && (
          <div className="mt-3 flex flex-wrap items-center gap-2">
            <p className="text-2xs leading-relaxed text-ink-faint">
              Occasionally this needs a restart to fully take hold.
            </p>
            <Button tone="ghost" size="sm" onClick={() => void restartNow()} disabled={relaunching}>
              {relaunching ? "Restarting…" : "Restart Caduceus"}
            </Button>
          </div>
        )}
      </div>

      <div className="mt-4">
        {info.detectable ? (
          activeSatisfied ? (
            <Callout tone="positive" title="Granted">
              Caduceus can use {info.title} now.
            </Callout>
          ) : (
            <div className="row gap-2 rounded-lg border border-line bg-base/20 px-3.5 py-3 text-[13px] text-ink-mute">
              <Spinner className="text-accent" />
              <span>Waiting for the switch — this updates on its own, so leave it open.</span>
            </div>
          )
        ) : activeSatisfied ? (
          <Callout tone="positive" title="Marked as on">
            Taking your word for it — macOS does not let Caduceus check this one.{" "}
            <button
              type="button"
              className="underline decoration-dotted underline-offset-2"
              onClick={() => setConfirmed((prev) => ({ ...prev, [activeId]: false }))}
            >
              Actually, not yet
            </button>
          </Callout>
        ) : (
          <div className="rounded-lg border border-line bg-base/20 px-3.5 py-3">
            <p className="text-[13px] leading-relaxed text-ink-mute">
              macOS does not let an app read this one back, so Caduceus cannot show you a tick here.
            </p>
            <Button
              className="mt-2.5"
              size="sm"
              onClick={() => setConfirmed((prev) => ({ ...prev, [activeId]: true }))}
            >
              I've turned it on
            </Button>
          </div>
        )}
      </div>

      {allGranted && (
        <div className="mt-5">
          <Callout tone="positive" title="All set">
            {ids.length > 1 ? "Every permission on this list is" : "This permission is"} granted.
          </Callout>
        </div>
      )}

      {!allGranted && pinnedIndex !== null && pinnedIndex !== firstUnsatisfiedIndex && firstUnsatisfiedIndex !== -1 && (
        <button
          type="button"
          className="mt-3 text-2xs text-accent underline decoration-dotted underline-offset-2"
          onClick={() => setPinnedIndex(null)}
        >
          Back to what's next →
        </button>
      )}

      {onSkip && !allGranted && (
        <div className={cx("mt-5 flex", isOnboarding ? "justify-center" : "justify-end")}>
          <Button tone="ghost" size="sm" onClick={onSkip}>
            {isOnboarding ? "Skip for now" : "Not now"}
          </Button>
        </div>
      )}

      {note && <p className="mt-3 text-2xs text-ink-mute">{note}</p>}
    </div>
  );
}
