/**
 * The first-run walkthrough.
 *
 * Rendered inside the staff window because that is the one surface always on
 * screen — a separate window would need its own always-on-top handling and
 * would cover the very thing it is pointing at.
 *
 * Two rules make this a walkthrough rather than a slideshow:
 *
 * 1. **Steps advance on the real action.** "Hover the staff" completes when the
 *    staff is actually hovered, not when you press Next. A tutorial you can
 *    click through without touching the product teaches nothing.
 * 2. **It never blocks the thing it is teaching.** The card is
 *    `pointer-events: none` except for its own buttons, and it is parked
 *    above the mark (not over it) so hover and click still reach the staff.
 */

import { useEffect, useRef, useState } from "react";

import * as api from "@/shared/api";
import { commandCenterKey, hotkeyLabel } from "@/shared/hotkeyLabel";
import type { Settings } from "@/shared/types";
import { cx } from "@/shared/ui";

export interface OnboardingSignals {
  hovered: boolean;
  expanded: boolean;
  commandCenterOpened: boolean;
  hotkeyUsed: boolean;
}

interface Step {
  title: string;
  body: string;
  /** Completed when this returns true; undefined means "advance on Next". */
  done?: (s: OnboardingSignals) => boolean;
  /** Shown instead of Next while incomplete. */
  waiting?: string;
}

function buildSteps(hotkey: string): Step[] {
  return [
  {
    title: "This is the staff",
    body: "It floats above everything, on every space. Hover it — your shortcuts fan out around it.",
    done: (s) => s.expanded || s.hovered,
    waiting: "Hover the staff…",
  },
  {
    title: "Click it to search",
    body: "A single click opens the Command Center. Give it a click now.",
    done: (s) => s.commandCenterOpened,
    waiting: "Click the staff…",
  },
  {
    title: "Close it, then use the key",
    body: hotkey
      ? `Escape closes the Command Center. Now press ${hotkey} — it opens from anywhere, whatever app you are in.`
      : "Escape closes the Command Center. No global shortcut is bound right now — set one in Settings → General, then this step will name it.",
    // Without a bound shortcut this step is unsatisfiable, which would trap
    // the walkthrough on a screen with no way forward.
    done: (s) => s.hotkeyUsed || !hotkey,
    waiting: `Press ${hotkey}…`,
  },
  {
    title: "Type anything",
    body: "Search apps by name, or type a question. Enter runs whatever is highlighted.",
  },
  {
    title: "What the bar understands",
    body: "chrome → launches the app · 2+2 → answers inline · your text → searches the web · / → asks your AI · /v → your clipboard history · /c → an agent that drives your Mac",
  },
  {
    title: "One last thing",
    body: "The / and /c prefixes need a model. Nothing else does — the launcher, clipboard, dictation, system monitor and search all work as they are. Want to configure AI features? Settings → Learn walks you through it and can find what is already on your Mac.",
  },
  ];
}

export function Onboarding({
  signals,
  settings,
  staffSize,
  onFinish,
}: {
  signals: OnboardingSignals;
  settings: Settings;
  /** Used to park the card clear of the mark so the walkthrough cannot cover it. */
  staffSize: number;
  onFinish: () => void;
}) {
  const [index, setIndex] = useState(0);
  // Read at render, not baked in: startup may have rebound this to a fallback
  // because another app held the configured key.
  const STEPS = buildSteps(hotkeyLabel(commandCenterKey(settings)));

  const cardRef = useRef<HTMLDivElement>(null);

  // The staff window is click-through except right at the staff, so this card's
  // buttons would otherwise land on whatever is behind it. Register the card's
  // own bounds rather than forcing the entire window clickable: doing the latter
  // made the whole 340px square swallow clicks for the length of the
  // walkthrough, so the staff could not be dragged and nothing behind the window
  // could be reached.
  useEffect(() => {
    const el = cardRef.current;
    if (!el) return;

    const publish = () => {
      const r = el.getBoundingClientRect();
      void api.setStaffCaptureRect({
        x: r.left,
        y: r.top,
        width: r.width,
        height: r.height,
      });
    };

    publish();
    // The card changes height with each step's body text, and the window
    // resizes with the staff, so a rect measured once goes stale.
    const observer = new ResizeObserver(publish);
    observer.observe(el);
    window.addEventListener("resize", publish);

    return () => {
      observer.disconnect();
      window.removeEventListener("resize", publish);
      void api.setStaffCaptureRect(null);
    };
  }, [index]);

  const step = STEPS[index];
  const isLast = index === STEPS.length - 1;
  const satisfied = step.done ? step.done(signals) : true;

  const go = (delta: number) =>
    setIndex((i) => Math.min(Math.max(i + delta, 0), STEPS.length - 1));

  // Free movement in both directions, unlike Next, which waits for the step's
  // action. Re-reading a step you have already done should not require redoing
  // it, and being unable to go back at all was the complaint that added this.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "ArrowLeft") {
        e.preventDefault();
        go(-1);
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        go(1);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [STEPS.length]);

  // Auto-advance the moment the real action happens, so completing a step feels
  // like the product responding rather than a form being submitted.
  useEffect(() => {
    if (!step.done || !step.done(signals)) return;
    const timer = setTimeout(() => setIndex((i) => Math.min(i + 1, STEPS.length - 1)), 550);
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [signals.hovered, signals.expanded, signals.commandCenterOpened, signals.hotkeyUsed, index]);

  // Park the card in the top half of the staff window so the mark at the centre
  // stays visible and clickable. (1.0.1 centred the card on the mark and hid it.)
  // Rust grows the window while the walkthrough is unfinished so this half is
  // actually big enough for the card — see `staff_window_side`.
  const staffClearance = Math.round(staffSize / 2) + 16;

  return (
    <div className="pointer-events-none absolute inset-0 z-40">
      {/* Centred with auto margins, NOT `left-1/2 -translate-x-1/2`: the
          `animate-fade-rise` keyframes end on a `transform` and the animation is
          declared with fill-mode `both`, so that final transform sticks and
          silently wins over any translate utility on the same element. The card
          lost its centring offset, sat with its left edge on the window's
          midline, and ran off the right-hand side. */}
      <div
        ref={cardRef}
        className={cx(
          "pointer-events-auto absolute inset-x-2 top-2 mx-auto w-[min(290px,calc(100%-16px))]",
          "overflow-y-auto animate-fade-rise rounded-cad px-4 py-3.5",
          "glass shadow-float",
        )}
        style={{
          maxHeight: `calc(50% - ${staffClearance}px)`,
        }}
      >
        <div className="row justify-between">
          <span className="text-2xs font-medium uppercase tracking-[0.1em] text-accent">
            {index + 1} of {STEPS.length}
          </span>
          <button
            type="button"
            onClick={onFinish}
            className="rounded px-1.5 py-0.5 text-2xs text-ink-faint transition-colors hover:bg-raised hover:text-ink"
          >
            Skip
          </button>
        </div>

        <p className="mt-2 text-[14px] font-semibold leading-tight text-ink">{step.title}</p>
        <p className="mt-1.5 text-2xs leading-relaxed text-ink-mute">{step.body}</p>

        <div className="row mt-3.5 justify-between">
          {/* Arrows as buttons, not just key handlers: the staff window is
              created unfocused and usually stays that way, so ArrowLeft and
              ArrowRight only reach this card once it has been clicked. The
              buttons always work. */}
          <div className="row gap-2">
            <button
              type="button"
              onClick={() => go(-1)}
              disabled={index === 0}
              aria-label="Previous step"
              title="Previous step (←)"
              className={cx(
                "rounded-lg border px-2 py-1 text-2xs font-medium transition-colors",
                index === 0
                  ? "cursor-default border-transparent text-overlay"
                  : "border-line text-ink-mute hover:bg-raised hover:text-ink",
              )}
            >
              Back
            </button>

            <div className="row gap-1">
              {STEPS.map((_, i) => (
                <span
                  key={i}
                  aria-hidden="true"
                  className={cx(
                    "h-1 w-1 rounded-full transition-colors",
                    i === index ? "bg-accent" : i < index ? "bg-ink-faint" : "bg-overlay",
                  )}
                />
              ))}
            </div>

            <button
              type="button"
              onClick={() => go(1)}
              disabled={isLast}
              aria-label="Next step"
              title="Next step (→)"
              className={cx(
                "rounded-lg border px-2 py-1 text-2xs font-medium transition-colors",
                isLast
                  ? "cursor-default border-transparent text-overlay"
                  : "border-line text-ink-mute hover:bg-raised hover:text-ink",
              )}
            >
              Forward
            </button>
          </div>

          {isLast ? (
            <div className="row">
              <button
                type="button"
                onClick={() => {
                  onFinish();
                  void api.openSettingsWindow("help");
                }}
                className="rounded-lg bg-accent px-2.5 py-1 text-2xs font-medium text-accent-ink transition-[filter] hover:brightness-110"
              >
                Set up AI
              </button>
              <button
                type="button"
                onClick={onFinish}
                className="rounded-lg px-2 py-1 text-2xs text-ink-mute transition-colors hover:bg-raised hover:text-ink"
              >
                Done
              </button>
            </div>
          ) : satisfied ? (
            <button
              type="button"
              onClick={() => setIndex((i) => i + 1)}
              className="rounded-lg bg-accent px-2.5 py-1 text-2xs font-medium text-accent-ink transition-[filter] hover:brightness-110"
            >
              Next
            </button>
          ) : (
            <span className="text-2xs text-ink-faint">{step.waiting}</span>
          )}
        </div>
      </div>
    </div>
  );
}
