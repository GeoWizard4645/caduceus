import { hexToRgbChannels } from "@/shared/theme";
import type { Theme } from "@/shared/types";
import { Field, NumberInput, Section, Select, TextInput, Toggle, cx } from "@/shared/ui";

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

export function AppearanceTab({ draft }: { draft: Draft }) {
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
        title="Accent"
        description="One colour, used for focus rings, the orb, and highlighted rows."
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
        title="The orb"
        description="Changes apply live — drag a value and watch the orb on screen."
      >
        <div className="grid grid-cols-2 gap-5">
          <Field label="Orb size">
            <NumberInput
              value={appearance.orbSize}
              min={28}
              max={88}
              suffix="px"
              onChange={(value) => draft.update((d) => (d.appearance.orbSize = value))}
            />
          </Field>

          <Field label="Pop-out distance" hint="How far the icons sit from the orb's centre.">
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
            hint="How visible the orb is when you are not near it. 1 is fully opaque."
          >
            <NumberInput
              value={appearance.orbIdleOpacity}
              min={0.15}
              max={1}
              step={0.05}
              onChange={(value) => draft.update((d) => (d.appearance.orbIdleOpacity = value))}
            />
          </Field>
        </div>

        <div className="mt-4 border-t border-line pt-4">
          <Toggle
            label="Animate the idle orb"
            hint="A slow breathing pulse and a rotating ring. Turn off for a completely still orb."
            checked={appearance.orbIdleAnimation}
            onChange={(checked) => draft.update((d) => (d.appearance.orbIdleAnimation = checked))}
          />
        </div>

        <p className="mt-4 text-2xs leading-relaxed text-ink-faint">
          Orbit also respects your system's “reduce motion” setting, which disables every animation
          regardless of what is set here.
        </p>
      </Section>
    </>
  );
}
