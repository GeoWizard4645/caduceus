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
 *    `pointer-events: none` except for its own buttons, so the staff underneath
 *    stays usable while the card is up.
 *
 * The staff window is a fixed 340px square, so the card is deliberately small
 * and sits beside the mark rather than over it.
 */

import { useEffect, useState } from "react";

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
  onFinish,
}: {
  signals: OnboardingSignals;
  settings: Settings;
  onFinish: () => void;
}) {
  const [index, setIndex] = useState(0);
  // Read at render, not baked in: startup may have rebound this to a fallback
  // because another app held the configured key.
  const STEPS = buildSteps(hotkeyLabel(commandCenterKey(settings)));

  // The staff window is click-through except right at the staff, so this card's
  // own buttons would otherwise land on whatever is behind it.
  useEffect(() => {
    void api.setStaffInteractive(true);
    return () => {
      void api.setStaffInteractive(false);
    };
  }, []);
  const step = STEPS[index];
  const isLast = index === STEPS.length - 1;
  const satisfied = step.done ? step.done(signals) : true;

  // Auto-advance the moment the real action happens, so completing a step feels
  // like the product responding rather than a form being submitted.
  useEffect(() => {
    if (!step.done || !step.done(signals)) return;
    const timer = setTimeout(() => setIndex((i) => Math.min(i + 1, STEPS.length - 1)), 550);
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [signals.hovered, signals.expanded, signals.commandCenterOpened, signals.hotkeyUsed, index]);

  return (
    <div className="pointer-events-none absolute inset-0 z-50 flex items-center justify-center">
      <div
        className={cx(
          "pointer-events-auto w-[290px] animate-fade-rise rounded-cad px-4 py-3.5",
          "glass shadow-float",
        )}
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
