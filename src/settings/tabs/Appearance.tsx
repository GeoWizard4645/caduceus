import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";

import * as api from "@/shared/api";
import { readPersisted } from "@/shared/persist";
import { StaffMark } from "@/shared/StaffMark";
import { hexToRgbChannels } from "@/shared/theme";
import type { Theme } from "@/shared/types";
import { Button, Field, NumberInput, Section, Select, TextInput, Toggle, cx } from "@/shared/ui";

import type { Draft } from "../useDraft";

/**
 * Curated accents. Any hex works — these exist so the common case is one click
 * rather than opening a colour picker and guessing at something that reads well
 * against a dark background.
 */
const ACCENTS = [
  { name: "Periwinkle", hex: "#7c7cff" },
  { name: "Cyan", hex: "#3fd0d8" },
  { name: "Mint", hex: "#4acd9e" },
  { name: "Amber", hex: "#e8a54a" },
  { name: "Coral", hex: "#f0706a" },
  { name: "Magenta", hex: "#e263c4" },
  { name: "Violet", hex: "#a06cff" },
  { name: "Slate", hex: "#8a92a8" },
];

// ---------------------------------------------------------------------------
// Theme presets
//
// Each preset is just a coordinated set of values for fields the appearance
// system already has (`accent`, `reduceTransparency`, `windowRadius`,
// `staffIdleAnimation`) — see `src/shared/theme.ts`'s `applyAppearance`. This
// is deliberate: a preset is a shortcut for the swatches and knobs already
// below it on this page, not a second theming mechanism. Picking one is
// exactly as reversible as changing any one of those fields by hand, and
// nobody who never opens this section is affected — the fields keep whatever
// they already had.
// ---------------------------------------------------------------------------

interface ThemePreset {
  name: string;
  description: string;
  accent: string;
  reduceTransparency: boolean;
  windowRadius: number;
  staffIdleAnimation: boolean;
}

const THEME_PRESETS: ThemePreset[] = [
  {
    name: "Cyberpunk",
    description: "Hot magenta, square corners, restless idle animation.",
    accent: "#ff2fd6",
    reduceTransparency: false,
    windowRadius: 4,
    staffIdleAnimation: true,
  },
  {
    name: "Nord",
    description: "Cool frost blue, flat surfaces, a still staff.",
    accent: "#88c0d0",
    reduceTransparency: true,
    windowRadius: 10,
    staffIdleAnimation: false,
  },
  {
    name: "Dracula",
    description: "Dracula's signature purple, soft glass, gentle motion.",
    accent: "#bd93f9",
    reduceTransparency: false,
    windowRadius: 12,
    staffIdleAnimation: true,
  },
  {
    name: "Apple Minimalist",
    description: "System blue, flat surfaces, generous rounding, no motion.",
    accent: "#0a84ff",
    reduceTransparency: true,
    windowRadius: 20,
    staffIdleAnimation: false,
  },
];

function presetIsActive(preset: ThemePreset, appearance: { accent: string; reduceTransparency: boolean; windowRadius?: number; staffIdleAnimation: boolean }): boolean {
  return (
    appearance.accent.toLowerCase() === preset.accent &&
    appearance.reduceTransparency === preset.reduceTransparency &&
    (appearance.windowRadius ?? 14) === preset.windowRadius &&
    appearance.staffIdleAnimation === preset.staffIdleAnimation
  );
}

// ---------------------------------------------------------------------------
// Sound effects
//
// Deliberately *not* part of `AppearanceSettings` (the persisted, Rust-backed
// settings tree defined in `src/shared/types.ts` and
// `src-tauri/src/settings/model.rs`) — this file does not own either of those
// schemas. The preference lives in `localStorage` instead, read/written only
// through the two helpers below, so wiring an actual palette-action call site
// up to it later is a one-line `if (isSoundEffectsEnabled()) playActionSound(...)`
// wherever that action fires, without this file needing to change.
//
// Tones are synthesised with the Web Audio API rather than shipping .wav/.mp3
// assets: Caduceus's whole pitch includes being a ~10MB app, and a handful of
// short audio files would eat into that for something this disposable. Four
// numbers (frequency, gain envelope, duration) reproduce a click/confirm tone
// close enough for a UI accent, and it costs zero bytes on disk.
// ---------------------------------------------------------------------------

const SOUND_EFFECTS_KEY = "caduceus:sound-effects-enabled";

/** Off unless the user has explicitly turned it on — see the section header. */
export function isSoundEffectsEnabled(): boolean {
  return readPersisted(SOUND_EFFECTS_KEY, "0") === "1";
}

function setSoundEffectsEnabled(enabled: boolean): void {
  try {
    localStorage.setItem(SOUND_EFFECTS_KEY, enabled ? "1" : "0");
  } catch {
    // Best-effort: worst case the toggle does not survive a restart, which is
    // a much smaller failure than losing a setting that changes what the app
    // *does*. Nothing audible plays either way if this silently fails.
  }
}

// One shared context rather than one-per-sound: browsers cap how many can
// exist, and reusing it means the very first click after enabling the
// toggle is not the one paying for construction latency.
let sharedAudioContext: AudioContext | null = null;

function getAudioContext(): AudioContext | null {
  if (typeof window === "undefined") return null;
  const Ctor = window.AudioContext ?? (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
  if (!Ctor) return null;
  if (!sharedAudioContext || sharedAudioContext.state === "closed") {
    sharedAudioContext = new Ctor();
  }
  return sharedAudioContext;
}

/**
 * Play a short, synthesised UI tone if sound effects are enabled.
 *
 * `"click"` is a low, quick tick for a palette action firing; `"confirm"` is a
 * slightly longer tone that rises in pitch, for something completing. Neither
 * throws — a browser that blocks audio until a user gesture, or has no Web
 * Audio support at all, just means silence rather than a broken settings page.
 */
export function playActionSound(kind: "click" | "confirm" = "click"): void {
  if (!isSoundEffectsEnabled()) return;
  const ctx = getAudioContext();
  if (!ctx) return;

  try {
    if (ctx.state === "suspended") void ctx.resume();

    const now = ctx.currentTime;
    const osc = ctx.createOscillator();
    const gain = ctx.createGain();

    osc.type = "sine";
    const baseFreq = kind === "confirm" ? 720 : 540;
    osc.frequency.setValueAtTime(baseFreq, now);
    if (kind === "confirm") {
      // A small upward glide is what reads as "confirm" rather than "click" —
      // the same envelope with a flat pitch just sounds like a second click.
      osc.frequency.exponentialRampToValueAtTime(baseFreq * 1.5, now + 0.09);
    }

    const duration = kind === "confirm" ? 0.18 : 0.09;
    // Fast attack, exponential decay: a percussive envelope reads as a UI
    // tick. `exponentialRampToValueAtTime` cannot ramp to exactly 0, hence the
    // 0.0001 floor rather than a true silence target.
    gain.gain.setValueAtTime(0.0001, now);
    gain.gain.exponentialRampToValueAtTime(0.15, now + 0.008);
    gain.gain.exponentialRampToValueAtTime(0.0001, now + duration);

    osc.connect(gain);
    gain.connect(ctx.destination);
    osc.start(now);
    osc.stop(now + duration + 0.02);
  } catch {
    // Sound is decoration, never a dependency — any Web Audio failure here
    // must be invisible to the rest of the app.
  }
}

export function AppearanceTab({ draft }: { draft: Draft }) {
  // Resolved through Rust and served over the asset protocol, so the preview
  // shows the real file rather than a guess at where it went.
  const [backdropPreview, setBackdropPreview] = useState<string | null>(null);
  const [backdropError, setBackdropError] = useState<string | null>(null);
  const [staffMarkError, setStaffMarkError] = useState<string | null>(null);

  // Not `draft.settings` — see the "Sound effects" section below for why this
  // lives in localStorage instead. Read once at mount; `onChange` below keeps
  // this state and the persisted value in lockstep from then on.
  const [soundEffects, setSoundEffects] = useState<boolean>(() => isSoundEffectsEnabled());

  const backdropToken = draft.settings?.appearance.commandCenterBackground ?? "";
  useEffect(() => {
    let cancelled = false;
    if (!backdropToken) {
      setBackdropPreview(null);
      return;
    }
    void (async () => {
      try {
        const path = await api.resolveBackdrop(backdropToken);
        const { convertFileSrc } = await import("@tauri-apps/api/core");
        // Cache-busted: the file keeps its name when replaced, so without this
        // the preview shows the previous image.
        if (!cancelled) {
          setBackdropPreview(path ? `url("${convertFileSrc(path)}?v=${Date.now()}")` : null);
        }
      } catch {
        if (!cancelled) setBackdropPreview(null);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [backdropToken]);

  const settings = draft.settings;
  if (!settings) return null;
  const appearance = settings.appearance;
  const validAccent = hexToRgbChannels(appearance.accent) !== null;

  return (
    <>
      <Section title="Theme">
        <div className="grid grid-cols-2 gap-5">
          <Field label="Appearance">
            <Select
              value={appearance.theme}
              onChange={(v) => draft.update((d) => (d.appearance.theme = v as Theme))}
              options={[
                { value: "dark", label: "Dark" },
                { value: "light", label: "Light" },
                { value: "system", label: "Match the system" },
              ]}
            />
          </Field>
        </div>

        <div className="mt-4 border-t border-line pt-4">
          <Toggle
            label="Reduce transparency"
            hint="Replaces the blurred glass surfaces with solid ones. Easier to read, and lighter on older GPUs."
            checked={appearance.reduceTransparency}
            onChange={(checked) => draft.update((d) => (d.appearance.reduceTransparency = checked))}
          />
        </div>
      </Section>

      <Section
        title="Presets"
        description="One click sets the accent and a few matching knobs below — nothing you can't already do by hand, in one step. Pick your own accent afterward and it's just an accent again."
      >
        <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
          {THEME_PRESETS.map((preset) => {
            const active = presetIsActive(preset, appearance);
            return (
              <button
                key={preset.name}
                type="button"
                onClick={() => {
                  draft.update((d) => {
                    d.appearance.accent = preset.accent;
                    d.appearance.reduceTransparency = preset.reduceTransparency;
                    d.appearance.windowRadius = preset.windowRadius;
                    d.appearance.staffIdleAnimation = preset.staffIdleAnimation;
                  });
                  playActionSound("confirm");
                }}
                className={cx(
                  "flex items-center gap-3 rounded-lg border px-3 py-2.5 text-left transition-colors duration-150",
                  active
                    ? "border-ink bg-base/80 shadow-glow"
                    : "border-line hover:border-line-strong/60 hover:bg-base/40",
                )}
              >
                <span
                  className="h-6 w-6 shrink-0 rounded-full border border-line-strong/40"
                  style={{ backgroundColor: preset.accent }}
                  aria-hidden
                />
                <span className="min-w-0">
                  <span className="block text-sm font-medium text-ink">{preset.name}</span>
                  <span className="block truncate text-2xs text-ink-faint">{preset.description}</span>
                </span>
              </button>
            );
          })}
        </div>
      </Section>

      <Section
        title="Accent"
        description="One colour, used for focus rings, the staff, and highlighted rows."
      >
        <div className="flex flex-wrap gap-2">
          {ACCENTS.map((accent) => (
            <button
              key={accent.hex}
              type="button"
              title={accent.name}
              aria-label={accent.name}
              onClick={() => draft.update((d) => (d.appearance.accent = accent.hex))}
              style={{ backgroundColor: accent.hex }}
              className={cx(
                "h-8 w-8 rounded-full border-2 transition-transform duration-150 hover:scale-110",
                appearance.accent.toLowerCase() === accent.hex
                  ? "border-ink shadow-glow"
                  : "border-transparent",
              )}
            />
          ))}
        </div>

        <div className="mt-4 grid grid-cols-2 gap-5">
          <Field
            label="Custom colour"
            hint="Any six-digit hex value."
            error={validAccent ? null : "Not a valid hex colour"}
          >
            <div className="row">
              <TextInput
                mono
                value={appearance.accent}
                onChange={(v) => draft.update((d) => (d.appearance.accent = v))}
                placeholder="#7c7cff"
              />
              <input
                type="color"
                value={validAccent ? appearance.accent : "#7c7cff"}
                onChange={(e) => draft.update((d) => (d.appearance.accent = e.target.value))}
                className="h-[38px] w-12 shrink-0 cursor-pointer rounded-lg border border-line-strong/60 bg-transparent p-1"
              />
            </div>
          </Field>
        </div>
      </Section>

      <Section
        title="The staff"
        description="Changes apply live — drag a value and watch the staff on screen."
      >
        <div className="grid grid-cols-2 gap-5">
          <Field
            label="Staff size"
            hint="Height of the mark (28–160 px). Or hover the staff and drag a corner knob."
          >
            <NumberInput
              value={appearance.staffSize}
              min={28}
              max={160}
              suffix="px"
              onChange={(value) => draft.update((d) => (d.appearance.staffSize = value))}
            />
          </Field>

          <Field label="Pop-out distance" hint="How far the icons sit from the staff's centre.">
            <NumberInput
              value={appearance.popoutRadius}
              min={56}
              max={132}
              suffix="px"
              onChange={(value) => draft.update((d) => (d.appearance.popoutRadius = value))}
            />
          </Field>

          <Field label="Pop-out icon size">
            <NumberInput
              value={appearance.popoutIconSize}
              min={24}
              max={52}
              suffix="px"
              onChange={(value) => draft.update((d) => (d.appearance.popoutIconSize = value))}
            />
          </Field>

          <Field
            label="Idle opacity"
            hint="How visible the staff is when you are not near it. 1 is fully opaque."
          >
            <NumberInput
              value={appearance.staffIdleOpacity}
              min={0.15}
              max={1}
              step={0.05}
              onChange={(value) => draft.update((d) => (d.appearance.staffIdleOpacity = value))}
            />
          </Field>
        </div>

        <div className="mt-4 border-t border-line pt-4">
          <Field
            label="Staff image"
            hint="Replace the default Caduceus mark with your own pixel art (PNG recommended). Shown with crisp pixels, not smoothing."
          >
            <div className="flex flex-wrap items-center gap-4">
              <div className="flex h-20 w-20 items-center justify-center rounded-lg border border-line bg-base/60">
                <StaffMark
                  height={56}
                  icon={appearance.staffMarkIcon}
                  className="drop-shadow-sm"
                />
              </div>
              <div className="flex flex-wrap gap-2">
                <Button
                  size="sm"
                  onClick={async () => {
                    setStaffMarkError(null);
                    const path = await open({
                      multiple: false,
                      filters: [
                        {
                          name: "Images",
                          extensions: ["png", "jpg", "jpeg", "webp", "gif", "heic"],
                        },
                      ],
                    });
                    if (!path || typeof path !== "string") return;
                    try {
                      const token = await api.importStaffMark(path);
                      draft.update((d) => (d.appearance.staffMarkIcon = token));
                    } catch (e) {
                      setStaffMarkError(api.errorMessage(e));
                    }
                  }}
                >
                  Upload image…
                </Button>
                {appearance.staffMarkIcon ? (
                  <Button
                    size="sm"
                    onClick={async () => {
                      await api.clearStaffMark();
                      draft.update((d) => (d.appearance.staffMarkIcon = ""));
                    }}
                  >
                    Use default mark
                  </Button>
                ) : null}
              </div>
            </div>
            {staffMarkError && (
              <p className="mt-2 text-2xs text-danger">{staffMarkError}</p>
            )}
          </Field>
        </div>

        <div className="mt-4 border-t border-line pt-4">
          <Toggle
            label="Animate the idle staff"
            hint="A slow breathing pulse and a rotating ring. Turn off for a completely still staff."
            checked={appearance.staffIdleAnimation}
            onChange={(checked) => draft.update((d) => (d.appearance.staffIdleAnimation = checked))}
          />
        </div>

        <p className="mt-4 text-2xs leading-relaxed text-ink-faint">
          The staff stays above full-screen apps. Caduceus also respects your system's “reduce
          motion” setting, which disables every animation regardless of what is set here.
        </p>
      </Section>

      <Section
        title="The Command Center"
        description="How the one window looks. None of this changes what anything does."
      >
        <Field
          label="Background image"
          hint="Yours, behind the results. Kept faint and blurred by default — a wallpaper that makes the list hard to read has failed at being either."
        >
          <div className="flex flex-wrap items-center gap-4">
            <div
              className="h-20 w-32 shrink-0 rounded-lg border border-line bg-base/60 bg-cover bg-center"
              style={{
                backgroundImage: backdropPreview ?? undefined,
                opacity: appearance.commandCenterBackground ? 1 : 0.5,
              }}
            />
            <div className="flex flex-wrap gap-2">
              <Button
                size="sm"
                onClick={async () => {
                  const path = await open({
                    multiple: false,
                    filters: [
                      { name: "Images", extensions: ["png", "jpg", "jpeg", "webp", "heic"] },
                    ],
                  });
                  if (!path || typeof path !== "string") return;
                  try {
                    const token = await api.importBackdrop(path);
                    draft.update((d) => (d.appearance.commandCenterBackground = token));
                  } catch (e) {
                    setBackdropError(api.errorMessage(e));
                  }
                }}
              >
                Choose an image…
              </Button>
              {appearance.commandCenterBackground ? (
                <Button
                  size="sm"
                  onClick={async () => {
                    await api.clearBackdrop();
                    draft.update((d) => (d.appearance.commandCenterBackground = ""));
                  }}
                >
                  Remove it
                </Button>
              ) : null}
            </div>
          </div>
          {backdropError && <p className="mt-2 text-2xs text-danger">{backdropError}</p>}
        </Field>

        {appearance.commandCenterBackground ? (
          <div className="mt-4 grid gap-4 sm:grid-cols-2">
            <Field label="How strongly it shows" hint="0 is invisible, 1 is full strength.">
              <NumberInput
                value={Math.round((appearance.backgroundOpacity ?? 0.35) * 100)}
                min={0}
                max={100}
                step={5}
                suffix="%"
                onChange={(value) =>
                  draft.update((d) => (d.appearance.backgroundOpacity = value / 100))
                }
              />
            </Field>
            <Field
              label="Blur"
              hint="What makes an arbitrary photograph usable behind text."
            >
              <NumberInput
                value={appearance.backgroundBlur ?? 8}
                min={0}
                max={40}
                step={2}
                suffix="px"
                onChange={(value) => draft.update((d) => (d.appearance.backgroundBlur = value))}
              />
            </Field>
          </div>
        ) : null}

        <div className="mt-4 grid gap-4 sm:grid-cols-2">
          <Field label="Corner radius" hint="0 for square corners.">
            <NumberInput
              value={appearance.windowRadius ?? 14}
              min={0}
              max={28}
              step={2}
              suffix="px"
              onChange={(value) => draft.update((d) => (d.appearance.windowRadius = value))}
            />
          </Field>
          <Field label="Text size" hint="Scales everything in the window together.">
            <NumberInput
              value={Math.round((appearance.uiScale ?? 1) * 100)}
              min={85}
              max={140}
              step={5}
              suffix="%"
              onChange={(value) => draft.update((d) => (d.appearance.uiScale = value / 100))}
            />
          </Field>
        </div>
      </Section>

      <Section
        title="Sound effects"
        description="A short tone on palette actions. Off by default — nothing changes until you turn this on."
      >
        <Toggle
          label="Play a sound on palette actions"
          hint="A quick click when an action runs, and a rising confirm tone when something completes. Synthesised on the fly — no audio files are shipped with Caduceus."
          checked={soundEffects}
          onChange={(checked) => {
            setSoundEffects(checked);
            setSoundEffectsEnabled(checked);
            if (checked) playActionSound("confirm");
          }}
        />

        {soundEffects ? (
          <div className="mt-4 flex flex-wrap gap-2 border-t border-line pt-4">
            <Button size="sm" onClick={() => playActionSound("click")}>
              Preview click
            </Button>
            <Button size="sm" onClick={() => playActionSound("confirm")}>
              Preview confirm
            </Button>
          </div>
        ) : null}

        <p className="mt-4 text-2xs leading-relaxed text-ink-faint">
          This is a settings-only preview today — hooking it up to the actual palette and staff
          action handlers is a follow-up outside this page.
        </p>
      </Section>
    </>
  );
}
