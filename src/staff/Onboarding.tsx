/**
 * The first-run setup.
 *
 * Rendered inside the staff window because that is the one surface always on
 * screen — a separate window would need its own always-on-top handling, and
 * this one can sit directly over the thing a permission prompt or a hotkey
 * press is actually about.
 *
 * # Three steps, and only three
 *
 * The previous version of this component was an eight-step tour that taught
 * hovering the staff, clicking it, the hotkey, dictation and the palette's
 * whole syntax before it let go of you, preceded by a three-question survey
 * before that. It was rebuilt from scratch around a narrower brief:
 * permissions, a hotkey, a model. Nothing else about using Caduceus needs
 * teaching up front — the staff and the Command Center are meant to be
 * discovered, not lectured about — and everything discoverable already lives
 * in Settings → Help, replayable on demand.
 *
 * 1. **Permissions.** Accessibility, Screen Recording and Microphone, asked
 *    for once via `PermissionCoach` rather than piecemeal the first time some
 *    feature happens to need one. Declining is a dead end nowhere in this
 *    app: `PermissionCoach`'s own "Skip for now" moves on to the hotkey step,
 *    the same as granting everything does.
 * 2. **Hotkey.** Shows whatever accelerator is actually configured — never a
 *    hardcoded guess, since a taken combination gets silently rebound to a
 *    fallback at startup (see `hotkeys::register_all`) — lets it be changed
 *    right there, and waits for a real press of it before calling itself
 *    confirmed. A hotkey that *looks* bound and does not fire is a worse
 *    first impression than one that plainly asks to be tried.
 * 3. **Model.** Only the `/` and `/c` prefixes need one; everything else —
 *    the launcher, clipboard, dictation, system monitor, search — already
 *    works. Scans for a local runtime already running (Ollama, LM Studio,
 *    llama.cpp, Jan, vLLM — see `agent::discover`) and offers to wire up
 *    whatever it finds in one click, with "connect a cloud model later" as
 *    the honest alternative to picking one now.
 *
 * # What "skippable" means here
 *
 * Every phase can be abandoned two different ways, on purpose: the header's
 * Skip and Escape from anywhere both end onboarding outright (`onFinish`),
 * while a phase-local action — declining permissions, "Connect a cloud model
 * later" — only moves past *that* phase. The header never means "skip to the
 * next question"; it always means "stop asking me things".
 *
 * Nothing here blocks the app. `Staff.tsx` mounts this only while
 * `onboardingDone` is false, and every path through it — finishing normally,
 * a phase's own escape, the header Skip, Escape itself — ends up calling
 * `onFinish` exactly once.
 *
 * # The quiz that used to precede this
 *
 * A three-question preference survey used to run before the tour, in its own
 * component (`OnboardingQuiz.tsx`, now deleted) gated by its own settings
 * flag. It is gone: asking someone what kind of user they are before they
 * have used the product is the exact pattern that made the old flow feel
 * like a form to fill out rather than a tool to use. Its scoring function
 * (`shared/personalization.ts`) is not deleted alongside it, though — that
 * still runs on every palette keystroke, ranking a `favoriteCommandIds` list
 * and a developer/focus bias nobody will ever populate again from scratch,
 * but which an install that already answered the old quiz still has and
 * still benefits from. `onboardingDone` is the only flag left to gate this
 * component on.
 */

import type { ReactNode } from "react";
import { useEffect, useRef, useState } from "react";

import * as api from "@/shared/api";
import { hotkeyLabel } from "@/shared/hotkeyLabel";
import { PermissionCoach } from "@/shared/PermissionCoach";
import type { BackendConfig, DetectedProvider, LocalAiScan, Settings } from "@/shared/types";
import { Button, Callout, Field, HotkeyInput, Select, Spinner, cx } from "@/shared/ui";

export interface OnboardingSignals {
  /**
   * Bumped once for every Command Center open that `commandCenterShown` (see
   * `Staff.tsx`) attributes to the global hotkey. A counter rather than a
   * boolean so the hotkey step can tell "confirmed before I last changed it"
   * apart from "confirmed since" — see `HotkeyStep`.
   */
  hotkeyPressCount: number;
}

// ---------------------------------------------------------------------------
// Keyboard illustration
//
// Text alone ("press Control-Space") makes people hunt across their physical
// keyboard for a symbol they may not recognise (⌃? ⌥?). This draws a small
// keyboard and lights up the exact keys the hotkey step is asking for, held
// keys (modifiers) glowing steadily and the key that gets tapped pulsing on a
// loop, the way you would actually press the combination: hold, then tap.
// ---------------------------------------------------------------------------

type ModifierKey = "control" | "option" | "command" | "shift";

interface KeyCombo {
  modifiers: ModifierKey[];
  /** Keycap id to light up — an uppercased letter/digit, "space", or a named key. */
  main: string | null;
}

/** Keys the hotkey names but this illustration does not lay out physically. */
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
  // grid, so it gets its own labelled pill instead. The Command Center hotkey
  // — the only shortcut this illustrates now — defaults to Space and so never
  // needs that path, but it can be rebound to anything the OS accepts, and a
  // combination this illustration silently failed to depict would be worse
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
// Phases
// ---------------------------------------------------------------------------

type Phase = "permissions" | "hotkey" | "model";

const PHASES: Phase[] = ["permissions", "hotkey", "model"];

const PHASE_TITLE: Record<Phase, string> = {
  permissions: "Permissions",
  hotkey: "Hotkey",
  model: "Model",
};

const PHASE_ARIA_LABEL: Record<Phase, string> = {
  permissions: "Grant permissions",
  hotkey: "Set your Command Center hotkey",
  model: "Pick a model",
};

function ProgressHeader({ phase, onSkip }: { phase: Phase; onSkip: () => void }) {
  const index = PHASES.indexOf(phase);
  return (
    <div className="row items-center justify-between">
      <div className="row items-center gap-2.5">
        <span className="text-2xs font-medium uppercase tracking-[0.1em] text-accent">
          {index + 1} of {PHASES.length} · {PHASE_TITLE[phase]}
        </span>
        <div className="row gap-1">
          {PHASES.map((p, i) => (
            <span
              key={p}
              aria-hidden="true"
              className={cx(
                "h-1 w-1 rounded-full transition-colors",
                i === index ? "bg-accent" : i < index ? "bg-ink-faint" : "bg-overlay",
              )}
            />
          ))}
        </div>
      </div>
      {/* Always ends onboarding outright — see the doc comment at the top of
          this file for why that is a different action from any phase's own
          "skip this one" control. */}
      <button
        type="button"
        onClick={onSkip}
        className="rounded px-1.5 py-0.5 text-2xs text-ink-faint transition-colors hover:bg-raised hover:text-ink"
      >
        Skip
      </button>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Step 2: Hotkey
// ---------------------------------------------------------------------------

function HotkeyStep({
  settings,
  pressCount,
  onBack,
  onContinue,
}: {
  settings: Settings;
  pressCount: number;
  onBack: () => void;
  onContinue: () => void;
}) {
  // Shown immediately on change, ahead of the save round-trip that actually
  // confirms it — see `changeHotkey`. `null` defers to the settings prop.
  const [override, setOverride] = useState<string | null>(null);
  const [warning, setWarning] = useState<string | null>(null);
  // The press count observed the moment the accelerator now on screen took
  // effect. A press only counts as confirmation if it happened at or after
  // that point — otherwise proving key A works and then rebinding to key B
  // would keep showing "confirmed" for a binding nobody has actually pressed.
  // Starts at 0, not the live count, so a binding already proven earlier this
  // session — replaying this from Settings → Help, say — still reads as
  // confirmed without demanding a fresh press before anything has changed.
  const [baseline, setBaseline] = useState(0);

  const accelerator = override ?? settings.general.commandCenterHotkey;
  const label = hotkeyLabel(accelerator);
  const confirmed = pressCount > baseline;

  const changeHotkey = async (next: string) => {
    setOverride(next);
    setWarning(null);
    setBaseline(pressCount);
    const draft = structuredClone(settings);
    draft.general.commandCenterHotkey = next;
    try {
      const report = await api.updateSettings(draft);
      // The accelerator that actually took effect — `next`, unless another
      // app already held it, in which case Rust moved it to a free fallback
      // (see `hotkeys::register_all`). Correcting the display to match beats
      // showing a combination that silently does nothing.
      setOverride(report.settings.general.commandCenterHotkey);
      if (report.hotkeyProblems.length > 0) setWarning(report.hotkeyProblems.join(" "));
    } catch (e) {
      setOverride(null);
      setWarning(api.errorMessage(e));
    }
  };

  return (
    <>
      <p className="text-[20px] font-semibold leading-snug text-ink">Open Caduceus from anywhere</p>
      <p className="mt-2.5 text-[14px] leading-relaxed text-ink-mute">
        This key opens the Command Center no matter what app is in front — Caduceus does not need to
        be focused, or even visible.
      </p>

      <div className="mt-5">
        <Field label="Command Center hotkey" hint="Works globally, even when Caduceus is not the focused app.">
          <HotkeyInput value={accelerator} onChange={(v) => void changeHotkey(v)} />
        </Field>
      </div>
      {warning && <p className="mt-2 text-2xs leading-relaxed text-caution">{warning}</p>}

      {accelerator ? (
        <>
          <MiniKeyboard combo={parseAccelerator(accelerator)} className="mt-5" />
          <div className="mt-4" aria-live="polite">
            {confirmed ? (
              <Callout tone="positive" title="Confirmed">
                {label} reaches Caduceus — that is the same key from anywhere on the Mac.
              </Callout>
            ) : (
              <div className="row gap-2 rounded-lg border border-line bg-base/20 px-3.5 py-3 text-[13px] text-ink-mute">
                <Spinner className="text-accent" />
                <span>Press {label} now to make sure it reaches Caduceus.</span>
              </div>
            )}
          </div>
        </>
      ) : (
        <p className="mt-5 text-[13px] leading-relaxed text-ink-mute">
          Nothing is bound, so only clicking the staff opens the Command Center. Set a key above, or
          continue and bind one later in Settings → General.
        </p>
      )}

      <div className="row mt-6 justify-between">
        <Button tone="ghost" size="md" onClick={onBack}>
          Back
        </Button>
        <Button tone="primary" size="md" onClick={onContinue}>
          Continue
        </Button>
      </div>
    </>
  );
}

// ---------------------------------------------------------------------------
// Step 3: Model
// ---------------------------------------------------------------------------

/**
 * Vision-tagged Ollama models (…-vl, …vision, llava, minicpm-v) reject the
 * plain chat schema Caduceus sends for `/` and `/c`, so they are filtered out
 * of the picker — the same heuristic `useChatModels.ts` applies to the chat
 * composer's own "Connect" rows. Duplicated rather than imported: that module
 * ships in the Command Center's bundle, this one in the staff window's, and
 * neither window's code should have to load the other's.
 */
function looksVisionOnly(tag: string): boolean {
  const t = tag.toLowerCase();
  return (
    /(^|[:/\-_.])vl([:/\-_.]|$)/.test(t) ||
    t.includes("vision") ||
    t.includes("llava") ||
    t.includes("minicpm-v")
  );
}

interface ModelCandidate {
  key: string;
  provider: DetectedProvider;
  model: string;
}

function candidatesFromScan(scan: LocalAiScan | null): ModelCandidate[] {
  if (!scan) return [];
  const rows: ModelCandidate[] = [];
  for (const provider of scan.providers) {
    if (!provider.running) continue;
    for (const model of provider.models) {
      if (looksVisionOnly(model)) continue;
      rows.push({ key: `${provider.id}::${model}`, provider, model });
    }
  }
  return rows;
}

function ModelStep({
  settings,
  onBack,
  onFinish,
}: {
  settings: Settings;
  onBack: () => void;
  onFinish: () => void;
}) {
  const [scan, setScan] = useState<LocalAiScan | null>(null);
  const [scanning, setScanning] = useState(true);
  const [scanError, setScanError] = useState<string | null>(null);
  const [selectedKey, setSelectedKey] = useState("");
  const [connecting, setConnecting] = useState(false);
  const [connectError, setConnectError] = useState<string | null>(null);

  const runScan = () => {
    setScanning(true);
    setScanError(null);
    void api
      .detectLocalAi()
      .then((result) => {
        setScan(result);
        const found = candidatesFromScan(result);
        setSelectedKey((current) => (found.some((c) => c.key === current) ? current : found[0]?.key ?? ""));
      })
      .catch((e) => setScanError(api.errorMessage(e)))
      .finally(() => setScanning(false));
  };

  // Scan once, on mount — this step's only job, so there is nothing else to
  // gate it on. `runScan` is stable enough for this: it closes over nothing
  // that changes between mount and the "Rescan" button being clicked by hand.
  useEffect(runScan, []);

  const candidates = candidatesFromScan(scan);
  const selected = candidates.find((c) => c.key === selectedKey) ?? candidates[0];

  const providerNames = [...new Set(candidates.map((c) => c.provider.displayName))];
  const summary =
    providerNames.length === 1
      ? `${providerNames[0]} is running, with ${candidates.length} model${candidates.length === 1 ? "" : "s"} ready.`
      : `${providerNames.join(" and ")} are running, with ${candidates.length} models between them.`;

  const useLocalModel = async () => {
    if (!selected) return;
    setConnecting(true);
    setConnectError(null);
    try {
      const { provider, model } = selected;
      const id = `local-${provider.id}`;
      const config: BackendConfig = {
        id,
        displayName: `${provider.displayName} — ${model}`,
        kind: "openai_compatible",
        baseUrl: provider.baseUrl,
        model,
        hasApiKey: false,
        maxTokens: 4096,
        temperature: null,
        systemPrompt: "",
        supportsComputerUse: false,
        extraHeaders: [],
        timeoutSecs: 600,
        reasoningEffort: null,
      };
      const next = structuredClone(settings);
      const index = next.agents.backends.findIndex((b) => b.id === id);
      if (index >= 0) next.agents.backends[index] = config;
      else next.agents.backends.push(config);
      next.agents.primaryBackendId = id;
      next.agents.routingOverrideBackendId = id;
      await api.updateSettings(next);
      // `Staff.tsx`'s `onFinish` re-reads settings from Rust before flipping
      // `onboardingDone`, rather than closing over whatever props this
      // component was last rendered with, so the save above is guaranteed to
      // still be there once that runs even though the two calls are
      // independent saves rather than one combined write.
      onFinish();
    } catch (e) {
      setConnectError(api.errorMessage(e));
      setConnecting(false);
    }
  };

  return (
    <>
      <p className="text-[20px] font-semibold leading-snug text-ink">Pick a model</p>
      <p className="mt-2.5 text-[14px] leading-relaxed text-ink-mute">
        Only <span className="font-mono text-ink-soft">/</span> and{" "}
        <span className="font-mono text-ink-soft">/c</span> need one — the launcher, clipboard,
        dictation, system monitor and search already work without it.
      </p>

      <div className="mt-5 rounded-cad border border-line bg-surface/50 p-4" aria-live="polite">
        {scanning ? (
          <div className="row gap-2 text-[13px] text-ink-mute">
            <Spinner className="text-accent" />
            <span>Looking for a model already running on this Mac…</span>
          </div>
        ) : scanError ? (
          <Callout tone="warn" title="Could not scan">
            {scanError}
          </Callout>
        ) : candidates.length > 0 && selected ? (
          <>
            <p className="text-[13px] font-medium text-ink">{summary}</p>
            <div className="mt-3">
              <Field label="Model" hint="Detected automatically — change it if you would rather use a different one.">
                <Select
                  value={selected.key}
                  onChange={setSelectedKey}
                  options={candidates.map((c) => ({
                    value: c.key,
                    label: `${c.provider.displayName} · ${c.model}`,
                  }))}
                />
              </Field>
            </div>
          </>
        ) : (
          <>
            <p className="text-[13px] leading-relaxed text-ink-mute">
              Nothing local answered. Install{" "}
              <button
                type="button"
                className="text-accent underline decoration-dotted underline-offset-2"
                onClick={() => void api.openExternalUrl("https://ollama.com")}
              >
                Ollama
              </button>
              , pull a model, and rescan — or connect a cloud provider any time in Settings.
            </p>
            <Button tone="ghost" size="sm" className="mt-3" onClick={runScan}>
              Rescan
            </Button>
          </>
        )}
        {connectError && <p className="mt-2 text-2xs text-danger">{connectError}</p>}
      </div>

      <div className="row mt-6 justify-between">
        <Button tone="ghost" size="md" onClick={onBack}>
          Back
        </Button>
        <div className="row gap-2">
          <Button tone="ghost" size="md" onClick={onFinish}>
            Connect a cloud model later
          </Button>
          {candidates.length > 0 && (
            <Button tone="primary" size="md" disabled={connecting} onClick={() => void useLocalModel()}>
              {connecting ? "Connecting…" : "Use local model"}
            </Button>
          )}
        </div>
      </div>
    </>
  );
}

// ---------------------------------------------------------------------------
// Shell
// ---------------------------------------------------------------------------

/** Matches `PermissionCoach` buttons, `HotkeyInput`, `Select`, links — everything the trap below can hand focus to. */
const FOCUSABLE_SELECTOR =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function Onboarding({
  signals,
  settings,
  onFinish,
}: {
  signals: OnboardingSignals;
  settings: Settings;
  /** Ends onboarding. The header's Skip, Escape, and every phase's own final
      action all funnel through this one prop; `Staff.tsx`'s implementation is
      what actually flips `onboardingDone`. */
  onFinish: () => void;
}) {
  const [phase, setPhase] = useState<Phase>("permissions");
  const cardRef = useRef<HTMLDivElement>(null);
  const restoreFocusTo = useRef<HTMLElement | null>(null);

  // The staff window is click-through except right at the mark and this
  // card's own bounds, so the card has to tell the Rust side where it is on
  // every phase change — its size and position both move as the content
  // does. Registering the card's own rect rather than forcing the entire
  // window clickable is what leaves the staff draggable and whatever sits
  // behind the window reachable for as long as onboarding is up.
  useEffect(() => {
    const el = cardRef.current;
    if (!el) return;

    const publish = () => {
      const r = el.getBoundingClientRect();
      void api.setStaffCaptureRect({ x: r.left, y: r.top, width: r.width, height: r.height });
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
  }, [phase]);

  // Real focus management, not just a visual overlay: whatever had focus
  // before onboarding opened gets it back once onboarding is gone, rather
  // than leaving focus on whatever DOM node happened to be under it.
  useEffect(() => {
    restoreFocusTo.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    return () => restoreFocusTo.current?.focus();
  }, []);

  // And every phase change moves focus onto the new card, so a screen reader
  // announces the new `aria-label` and Tab starts from a predictable place
  // rather than from a button that just unmounted underneath the cursor.
  useEffect(() => {
    cardRef.current?.focus({ preventScroll: true });
  }, [phase]);

  // Escape always means "stop asking me things", regardless of which control
  // inside the card currently has focus — except while `HotkeyInput` is mid
  // capture, where its own listener (registered in the capture phase, on
  // `window`) already claims Escape to cancel the capture and stops it from
  // reaching this one.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onFinish();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onFinish]);

  // A minimal Tab trap. This overlay sits on top of the staff button rather
  // than replacing it in the DOM, so without this, Tab would eventually walk
  // focus onto a control the scrim is actively hiding.
  const trapTab = (e: React.KeyboardEvent) => {
    if (e.key !== "Tab") return;
    const root = cardRef.current;
    if (!root) return;
    const focusable = Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(
      (n) => n.offsetParent !== null,
    );
    if (focusable.length === 0) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  };

  return (
    <div className="pointer-events-none absolute inset-0 z-40">
      {/* A plain scrim. Every phase here is a centred card with nothing on
          the staff itself worth keeping visible underneath, unlike the old
          tour's first two steps — so there is no punched-hole spotlight to
          animate, just a constant backdrop that mounts once and stays put
          across phase changes. */}
      <div aria-hidden="true" className="pointer-events-none absolute inset-0 bg-black/55 animate-fade-rise" />

      <div
        key={phase}
        ref={cardRef}
        role="dialog"
        aria-modal="true"
        aria-label={PHASE_ARIA_LABEL[phase]}
        tabIndex={-1}
        onKeyDown={trapTab}
        className={cx(
          "pointer-events-auto absolute inset-x-4 top-1/2 mx-auto -translate-y-1/2",
          "w-[min(560px,calc(100%-32px))] overflow-y-auto rounded-cad-lg",
          "glass px-8 py-7 shadow-float animate-fade-rise",
        )}
        style={{ maxHeight: "calc(100% - 32px)" }}
      >
        <ProgressHeader phase={phase} onSkip={onFinish} />

        <div className="mt-5">
          {phase === "permissions" && (
            // `PermissionCoach`'s "onboarding" variant already carries its own
            // eyebrow, heading and per-permission explanation — this phase
            // contributes only the progress chrome every phase shares.
            <PermissionCoach
              ids={["accessibility", "screen-recording", "microphone"]}
              onAllGranted={() => setPhase("hotkey")}
              onSkip={() => setPhase("hotkey")}
              variant="onboarding"
            />
          )}
          {phase === "hotkey" && (
            <HotkeyStep
              settings={settings}
              pressCount={signals.hotkeyPressCount}
              onBack={() => setPhase("permissions")}
              onContinue={() => setPhase("model")}
            />
          )}
          {phase === "model" && (
            <ModelStep settings={settings} onBack={() => setPhase("hotkey")} onFinish={onFinish} />
          )}
        </div>
      </div>
    </div>
  );
}
