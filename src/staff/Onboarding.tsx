/**
 * The first-run walkthrough.
 *
 * Rendered inside the staff window because that is the one surface always on
 * screen — a separate window would need its own always-on-top handling and
 * would cover the very thing it is pointing at.
 *
 * # Three phases, one flow
 *
 * `onFinish` (which the caller uses to flip `onboardingDone`) only fires once
 * this component's own two phases are both behind the user:
 *
 * 1. **Permissions.** Microphone, Speech Recognition and Accessibility used to
 *    get asked for piecemeal, the moment some feature first needed them —
 *    which meant a brand-new user could see a macOS permission prompt before
 *    they had even worked out what the staff was. Now they are asked for once,
 *    here, before the tour starts, via `PermissionCoach`. Declining is a
 *    dead end nowhere in this app: `onSkip` moves straight on to the tour, the
 *    same as `onAllGranted` does.
 * 2. **Tour.** The walkthrough proper — `buildSteps` below.
 *
 * The quiz that precedes this component lives in `OnboardingQuiz.tsx` and is
 * gated by a separate settings flag in `Staff.tsx`; by the time `Onboarding`
 * mounts at all, the quiz is already done. Keeping the permission step as an
 * internal phase of this component, rather than a third flag Staff.tsx has to
 * know about, is what makes survey → permissions → tour read as one
 * continuous thing instead of three screens that happen to run back to back.
 *
 * # Two rules make the tour a walkthrough rather than a slideshow
 *
 * 1. **Steps advance on the real action.** "Hover the staff" completes when the
 *    staff is actually hovered, not when you press Next. A tutorial you can
 *    click through without touching the product teaches nothing.
 * 2. **It never blocks the thing it is teaching.** Only the first two steps —
 *    hover, then click — need the real mark reachable under the card, so only
 *    those two are drawn in a strip along the top with a spotlight punched
 *    through the scrim at the mark. Every other step is prose, or a global
 *    keyboard shortcut that works "from anywhere", so nothing about the staff
 *    itself needs to stay visible, and the card is free to grow to a proper,
 *    centred modal for the rest of the tour.
 *
 * # Why a step used to disappear before anyone read it
 *
 * The very first release of this walkthrough auto-advanced the moment
 * `step.done(signals)` turned true, with nothing else gating it. If the
 * pointer already happened to be resting over the staff when the tour opened
 * — entirely possible, since the staff is what you clicked or hovered a
 * moment earlier to get here — `signals.hovered` was already `true` on the
 * very first render, and the step vanished in half a second flat. Every step
 * now also has to survive `MIN_STEP_DWELL_MS` of wall-clock time before it can
 * be marked satisfied, whether that satisfaction comes from a real signal or
 * from the user clicking Next. Pre-existing signal state can no longer skip a
 * step the user has not actually had a chance to read.
 */

import type { ReactNode } from "react";
import { useEffect, useRef, useState } from "react";

import * as api from "@/shared/api";
import { commandCenterKey, hotkeyLabel } from "@/shared/hotkeyLabel";
import { PermissionCoach } from "@/shared/PermissionCoach";
import type { Settings } from "@/shared/types";
import { Button, cx } from "@/shared/ui";

export interface OnboardingSignals {
  hovered: boolean;
  expanded: boolean;
  commandCenterOpened: boolean;
  hotkeyUsed: boolean;
}

// ---------------------------------------------------------------------------
// Keyboard illustration
//
// Text alone ("press Control-Space") makes people hunt across their physical
// keyboard for a symbol they may not recognise (⌃? ⌥?). This draws a small
// keyboard and lights up the exact keys a step is asking for, held keys
// (modifiers) glowing steadily and the key that gets tapped pulsing on a
// loop, the way you would actually press the combination: hold, then tap.
// ---------------------------------------------------------------------------

type ModifierKey = "control" | "option" | "command" | "shift";

interface KeyCombo {
  modifiers: ModifierKey[];
  /** Keycap id to light up — an uppercased letter/digit, "space", or a named key. */
  main: string | null;
}

/** Keys a step names but this illustration does not lay out physically. */
const NAMED_KEY_LABELS: Record<string, string> = {
  space: "Space",
  escape: "Esc",
  tab: "Tab",
  return: "Return",
  enter: "Return",
  backspace: "Delete",
  delete: "Delete",
  capslock: "Caps",
  arrowup: "↑",
  arrowdown: "↓",
  arrowleft: "←",
  arrowright: "→",
};

/**
 * Reads the same accelerator strings `hotkeyLabel` turns into prose ("⌃Space")
 * and turns them into keycap ids instead, so the two never risk disagreeing
 * about which keys a shortcut actually presses.
 */
function parseAccelerator(accelerator: string): KeyCombo {
  const parts = accelerator
    .trim()
    .split("+")
    .map((p) => p.trim().toLowerCase())
    .filter(Boolean);

  const modifiers: ModifierKey[] = [];
  let main: string | null = null;

  for (const part of parts) {
    switch (part) {
      case "commandorcontrol":
      case "command":
      case "cmd":
      case "super":
      case "meta":
        if (!modifiers.includes("command")) modifiers.push("command");
        break;
      case "control":
      case "ctrl":
        if (!modifiers.includes("control")) modifiers.push("control");
        break;
      case "alt":
      case "option":
        if (!modifiers.includes("option")) modifiers.push("option");
        break;
      case "shift":
        if (!modifiers.includes("shift")) modifiers.push("shift");
        break;
      default:
        // The last non-modifier token wins — accelerators only ever carry one.
        main = part;
    }
  }

  return { modifiers, main };
}

const KEY_ROW_2 = "QWERTYUIOP".split("");
const KEY_ROW_3 = "ASDFGHJKL".split("");
const KEY_ROW_4 = "ZXCVBNM".split("");
const KEY_ROW_1 = "1234567890".split("");

function Keycap({
  active,
  pulse,
  className,
  children,
}: {
  active: boolean;
  /** Only the tapped key pulses; held modifiers glow steadily instead. */
  pulse?: boolean;
  className?: string;
  children: ReactNode;
}) {
  return (
    <span
      aria-hidden="true"
      className={cx(
        "flex items-center justify-center rounded-md border text-[10px] font-medium leading-none",
        "transition-colors duration-200",
        active
          ? "border-accent/60 bg-accent/20 text-ink shadow-glow"
          : "border-line-strong/50 bg-raised/70 text-ink-faint",
        pulse && active && "animate-[cad-key-press_1.7s_ease-in-out_infinite]",
        className,
      )}
    >
      {children}
    </span>
  );
}

/** Small on-screen keyboard with the keys of `combo` lit up. */
function MiniKeyboard({ combo, className }: { combo: KeyCombo; className?: string }) {
  const isModifier = (id: ModifierKey) => combo.modifiers.includes(id);

  // A single alphanumeric character sits inside the letter/number grid; any
  // other named key (Escape, Return, an arrow…) has nowhere to live in that
  // grid, so it gets its own labelled pill instead. Nothing in this app's two
  // taught shortcuts — the Command Center hotkey and push-to-talk — currently
  // needs that path, but a user can rebind either to anything the OS accepts,
  // and a shortcut this illustration silently failed to depict would be worse
  // than one drawn slightly off-layout.
  const mainIsGridChar = !!combo.main && /^[a-z0-9]$/.test(combo.main);
  const mainGridId = mainIsGridChar ? combo.main!.toUpperCase() : null;
  const mainIsEscape = combo.main === "escape";
  const mainIsSpace = combo.main === "space";
  const mainNamedLabel =
    combo.main && !mainIsGridChar && !mainIsSpace && !mainIsEscape
      ? NAMED_KEY_LABELS[combo.main] ?? combo.main.charAt(0).toUpperCase() + combo.main.slice(1)
      : null;

  return (
    <div className={cx("select-none", className)}>
      {/* The keyframes live here rather than in tailwind.config.js: this is
          the only place in the app that presses a key on a loop, and an
          arbitrary-value Tailwind class (`animate-[cad-key-press_…]`) only
          needs a matching `@keyframes` to exist somewhere in the document —
          it does not have to be registered with Tailwind's build. */}
      <style>{`
        @keyframes cad-key-press {
          0%, 55%, 100% { transform: translateY(0) scale(1); }
          72% { transform: translateY(2px) scale(0.92); }
        }
      `}</style>

      <div className="flex flex-col gap-[3px] rounded-lg border border-line bg-base/40 p-2.5">
        <div className="flex gap-[3px]">
          <Keycap active={mainIsEscape} pulse className="h-6 w-9 text-[9px]">
            esc
          </Keycap>
          {KEY_ROW_1.map((k) => (
            <Keycap key={k} active={mainGridId === k} pulse className="h-6 w-6">
              {k}
            </Keycap>
          ))}
        </div>
        <div className="ml-2 flex gap-[3px]">
          {KEY_ROW_2.map((k) => (
            <Keycap key={k} active={mainGridId === k} pulse className="h-6 w-6">
              {k}
            </Keycap>
          ))}
        </div>
        <div className="ml-4 flex gap-[3px]">
          {KEY_ROW_3.map((k) => (
            <Keycap key={k} active={mainGridId === k} pulse className="h-6 w-6">
              {k}
            </Keycap>
          ))}
        </div>
        <div className="ml-6 flex gap-[3px]">
          {KEY_ROW_4.map((k) => (
            <Keycap key={k} active={mainGridId === k} pulse className="h-6 w-6">
              {k}
            </Keycap>
          ))}
        </div>
        <div className="mt-0.5 flex gap-[3px]">
          <Keycap active={isModifier("control")} className="h-6 w-9 text-[9px]">
            &#x2303;
          </Keycap>
          <Keycap active={isModifier("option")} className="h-6 w-9 text-[9px]">
            &#x2325;
          </Keycap>
          <Keycap active={isModifier("command")} className="h-6 w-9 text-[9px]">
            &#x2318;
          </Keycap>
          <Keycap active={mainIsSpace} pulse className="h-6 flex-1 text-[9px]">
            space
          </Keycap>
          <Keycap active={isModifier("command")} className="h-6 w-9 text-[9px]">
            &#x2318;
          </Keycap>
          <Keycap active={isModifier("option")} className="h-6 w-9 text-[9px]">
            &#x2325;
          </Keycap>
          {isModifier("shift") && (
            <Keycap active className="h-6 w-9 text-[9px]">
              &#x21e7;
            </Keycap>
          )}
        </div>
      </div>

      {mainNamedLabel && (
        <p className="mt-1.5 text-2xs text-ink-faint">
          Plus <span className="font-medium text-ink-soft">{mainNamedLabel}</span> — not shown above.
        </p>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Tour steps
// ---------------------------------------------------------------------------

interface Step {
  title: string;
  body: string;
  /** Completed when this returns true; undefined means "advance on Next". */
  done?: (s: OnboardingSignals) => boolean;
  /** Shown instead of Next while incomplete. */
  waiting?: string;
  /** Drawn under the body when this step is teaching a keyboard shortcut. */
  keys?: KeyCombo;
}

function buildSteps(settings: Settings): Step[] {
  const rawHotkey = commandCenterKey(settings);
  const hotkey = hotkeyLabel(rawHotkey);
  const rawPushToTalk = settings.voice.pushToTalkHotkey;
  const pushToTalk = hotkeyLabel(rawPushToTalk);

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
      waiting: hotkey ? `Press ${hotkey}…` : undefined,
      keys: rawHotkey ? parseAccelerator(rawHotkey) : undefined,
    },
    {
      title: "Type anything",
      body: "Search apps by name, or type a question. Enter runs whatever is highlighted.",
    },
    {
      title: "Hold to dictate",
      body: pushToTalk
        ? `Anywhere text can go, hold ${pushToTalk} and talk. Release the key and what you said is typed in for you.`
        : "Push-to-talk dictation needs a key of its own — bind one in Settings → Voice, then this step will name it.",
      keys: rawPushToTalk ? parseAccelerator(rawPushToTalk) : undefined,
    },
    {
      title: "What the bar understands",
      body: "chrome → launches the app · 2+2 → answers inline · your text → searches the web · / → asks your AI · /v → your clipboard history · /c → an agent that drives your Mac",
    },
    {
      title: "154 features, each with its own page",
      body: "Notes, meeting recording, colours, conversions, disk cleanup, window snapping — pick one and it opens as a tab with an interface built for it. Nothing needs you to know a syntax. ⌘T for another tab, ⌘1–⌘9 to switch. Searching by the app you came from works too — amphetamine, cleanshot, cleanmymac, stickies all find their counterpart here.",
    },
    {
      title: "One last thing",
      body: "The / and /c prefixes need a model. Nothing else does — the launcher, clipboard, dictation, system monitor and search all work as they are. Want to configure AI features? Settings → Learn walks you through it and can find what is already on your Mac.",
    },
  ];
}

/**
 * How long a step must be on screen before it can be marked satisfied, either
 * by its own `done` signal or by the user pressing Next. Long enough to rule
 * out "the pointer already happened to be there"; short enough that reading a
 * two-line step and moving on never feels throttled.
 */
const MIN_STEP_DWELL_MS = 1100;

/** Only these two steps need the real staff mark reachable under the card. */
const MARK_DEPENDENT_STEPS = new Set([0, 1]);

export function Onboarding({
  signals,
  settings,
  staffSize,
  onFinish,
}: {
  signals: OnboardingSignals;
  settings: Settings;
  /** Used to park the compact card clear of the mark, and to size the spotlight. */
  staffSize: number;
  onFinish: () => void;
}) {
  const [phase, setPhase] = useState<"permissions" | "tour">("permissions");
  const [index, setIndex] = useState(0);
  // Read at render, not baked in: startup may have rebound either hotkey to a
  // fallback because another app held the configured key.
  const STEPS = buildSteps(settings);

  const cardRef = useRef<HTMLDivElement>(null);

  // The staff window is click-through except right at the staff and this
  // card's own bounds, so the card has to tell the Rust side where it is on
  // every phase and step change — its size and position both move as the
  // content does. Registering the card's own rect rather than forcing the
  // entire window clickable is what leaves the staff draggable and whatever
  // sits behind the window reachable for the length of onboarding.
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
    const observer = new ResizeObserver(publish);
    observer.observe(el);
    window.addEventListener("resize", publish);

    return () => {
      observer.disconnect();
      window.removeEventListener("resize", publish);
      void api.setStaffCaptureRect(null);
    };
  }, [phase, index]);

  const step = STEPS[index];
  const isLast = index === STEPS.length - 1;
  const markDependent = phase === "tour" && MARK_DEPENDENT_STEPS.has(index);

  // See the doc comment at the top of the file for why this exists: without
  // it, a step whose `done` signal is already true the moment it mounts —
  // because the user was already hovering or had just clicked the staff to
  // get here — vanishes before it can be read.
  const [dwellPassed, setDwellPassed] = useState(false);
  useEffect(() => {
    setDwellPassed(false);
    const timer = setTimeout(() => setDwellPassed(true), MIN_STEP_DWELL_MS);
    return () => clearTimeout(timer);
  }, [phase, index]);

  const satisfied = dwellPassed && (step.done ? step.done(signals) : true);

  const go = (delta: number) =>
    setIndex((i) => Math.min(Math.max(i + delta, 0), STEPS.length - 1));

  // Free movement in both directions, unlike Next, which waits for the step's
  // action (and now for its dwell time too). Re-reading a step you have
  // already done should not require redoing it, and being unable to go back
  // at all was the complaint that added this. Scoped to the tour: the
  // permission phase's own controls live inside PermissionCoach.
  useEffect(() => {
    if (phase !== "tour") return;
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phase, STEPS.length]);

  // Auto-advance the moment the real action happens (and the step has been up
  // long enough), so completing a step feels like the product responding
  // rather than a form being submitted.
  useEffect(() => {
    if (phase !== "tour") return;
    if (!dwellPassed) return;
    if (!step.done || !step.done(signals)) return;
    const timer = setTimeout(() => setIndex((i) => Math.min(i + 1, STEPS.length - 1)), 550);
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phase, dwellPassed, signals.hovered, signals.expanded, signals.commandCenterOpened, signals.hotkeyUsed, index]);

  // Combined numbering across both phases — permissions is "1 of N", the tour
  // steps pick up from "2 of N" — so the progress dots read as one sequence
  // rather than resetting partway through, which is the whole point of
  // folding permissions into this component instead of bolting it on before.
  const TOTAL = 1 + STEPS.length;
  const position = phase === "permissions" ? 1 : 2 + index;

  // Half the window must fit: top gap, the card, a gap, and the mark's
  // radius. Mirrors the budget `staff_window_side` reserves in
  // `src-tauri/src/window/mod.rs` for the compact top-strip card.
  const staffClearance = Math.round(staffSize / 2) + 16;

  // A rough stand-in for the pop-out reach: this component is only handed
  // `staffSize`, not the popout radius/icon size `staff_window_side` uses, so
  // this cannot be exact. It only has to be generous enough that the fanned-
  // out shortcuts from "hover the staff" land inside the clear circle.
  const spotlightRadius = markDependent ? Math.min(Math.max(staffSize * 1.9, 90), 190) : 0;

  const header = (
    <div className="row items-center justify-between">
      <div className="row items-center gap-2.5">
        <span className="text-2xs font-medium uppercase tracking-[0.1em] text-accent">
          {position} of {TOTAL}
        </span>
        <div className="row gap-1">
          {Array.from({ length: TOTAL }).map((_, i) => (
            <span
              key={i}
              aria-hidden="true"
              className={cx(
                "h-1 w-1 rounded-full transition-colors",
                i === position - 1 ? "bg-accent" : i < position - 1 ? "bg-ink-faint" : "bg-overlay",
              )}
            />
          ))}
        </div>
      </div>
      <button
        type="button"
        onClick={phase === "permissions" ? () => setPhase("tour") : onFinish}
        className="rounded px-1.5 py-0.5 text-2xs text-ink-faint transition-colors hover:bg-raised hover:text-ink"
      >
        Skip
      </button>
    </div>
  );

  return (
    <div className="pointer-events-none absolute inset-0 z-40">
      {/* The scrim. A 0×0 circle with a 9999px spread still darkens the whole
          window evenly, so this one element covers both "plain backdrop" and
          "backdrop with a hole punched in it" — the radius just animates
          between the two, which is what makes the step-2-to-3 handoff (mark
          exposed → mark irrelevant) read as a deliberate widening rather than
          a jump cut. */}
      <div
        aria-hidden="true"
        className="pointer-events-none absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 rounded-full transition-[width,height] duration-300 ease-cad"
        style={{
          width: spotlightRadius * 2,
          height: spotlightRadius * 2,
          boxShadow: "0 0 0 9999px rgb(0 0 0 / 0.55)",
        }}
      />

      {phase === "permissions" && (
        <div
          key="permissions-card"
          ref={cardRef}
          role="dialog"
          aria-modal="true"
          aria-label="Grant permissions"
          className={cx(
            "pointer-events-auto absolute inset-x-4 top-1/2 mx-auto -translate-y-1/2",
            "w-[min(560px,calc(100%-32px))] overflow-y-auto rounded-cad-lg",
            "glass px-8 py-7 shadow-float animate-fade-rise",
          )}
          style={{ maxHeight: "calc(100% - 32px)" }}
        >
          {/* `PermissionCoach` in its "onboarding" variant already carries its
              own eyebrow, heading and explanation per permission — adding
              another title above it here would just repeat "let's get you set
              up" in different words, so this phase's card contributes only
              the progress chrome every phase shares and then gets out of the
              way. */}
          {header}
          <div className="mt-5">
            <PermissionCoach
              ids={["microphone", "speech-recognition", "accessibility"]}
              onAllGranted={() => setPhase("tour")}
              onSkip={() => setPhase("tour")}
              variant="onboarding"
            />
          </div>
        </div>
      )}

      {phase === "tour" && markDependent && (
        <div
          key="compact-card"
          ref={cardRef}
          role="dialog"
          aria-modal="true"
          aria-label="Caduceus walkthrough"
          className={cx(
            "pointer-events-auto absolute inset-x-3 top-3 mx-auto",
            "w-[min(440px,calc(100%-24px))] overflow-y-auto rounded-cad-lg",
            "glass px-6 py-5 shadow-float animate-fade-rise",
          )}
          style={{ maxHeight: `calc(50% - ${staffClearance}px)` }}
        >
          {header}
          <p className="mt-3 text-[17px] font-semibold leading-snug text-ink">{step.title}</p>
          <p className="mt-2 text-[13px] leading-relaxed text-ink-mute">{step.body}</p>

          <div className="row mt-4 justify-between">
            <div className="row gap-2">
              <Button
                tone="ghost"
                size="sm"
                disabled={index === 0}
                onClick={() => go(-1)}
                title="Previous step (←)"
              >
                Back
              </Button>
              <Button
                tone="ghost"
                size="sm"
                disabled={isLast}
                onClick={() => go(1)}
                title="Next step (→)"
              >
                Forward
              </Button>
            </div>

            {satisfied ? (
              <Button tone="primary" size="sm" onClick={() => setIndex((i) => i + 1)}>
                Next
              </Button>
            ) : (
              <span className="self-center text-2xs text-ink-faint">{step.waiting}</span>
            )}
          </div>
        </div>
      )}

      {phase === "tour" && !markDependent && (
        <div
          key="big-card"
          ref={cardRef}
          role="dialog"
          aria-modal="true"
          aria-label="Caduceus walkthrough"
          className={cx(
            "pointer-events-auto absolute inset-x-4 top-1/2 mx-auto -translate-y-1/2",
            "flex w-[min(560px,calc(100%-32px))] flex-col overflow-y-auto rounded-cad-lg",
            "glass px-8 py-7 shadow-float animate-fade-rise",
          )}
          style={{ maxHeight: "calc(100% - 32px)" }}
        >
          {header}
          <p className="mt-4 text-[20px] font-semibold leading-snug text-ink">{step.title}</p>
          <p className="mt-2.5 text-[14px] leading-relaxed text-ink-mute">{step.body}</p>

          {step.keys && <MiniKeyboard combo={step.keys} className="mt-5" />}

          <div className="row mt-6 justify-between">
            <div className="row gap-2">
              <Button
                tone="ghost"
                size="md"
                disabled={index === 0}
                onClick={() => go(-1)}
                title="Previous step (←)"
              >
                Back
              </Button>
              {!isLast && (
                <Button
                  tone="ghost"
                  size="md"
                  disabled={isLast}
                  onClick={() => go(1)}
                  title="Next step (→)"
                >
                  Forward
                </Button>
              )}
            </div>

            {isLast ? (
              <div className="row gap-2">
                <Button
                  tone="primary"
                  size="md"
                  onClick={() => {
                    onFinish();
                    void api.openSettingsWindow("help");
                  }}
                >
                  Set up AI
                </Button>
                <Button tone="ghost" size="md" onClick={onFinish}>
                  Done
                </Button>
              </div>
            ) : satisfied ? (
              <Button tone="primary" size="md" onClick={() => setIndex((i) => i + 1)}>
                Next
              </Button>
            ) : (
              <span className="self-center text-2xs text-ink-faint">{step.waiting}</span>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
