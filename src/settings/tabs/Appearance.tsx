import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";

import * as api from "@/shared/api";
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

export function AppearanceTab({ draft }: { draft: Draft }) {
  // Resolved through Rust and served over the asset protocol, so the preview
  // shows the real file rather than a guess at where it went.
  const [backdropPreview, setBackdropPreview] = useState<string | null>(null);
  const [backdropError, setBackdropError] = useState<string | null>(null);
  const [staffMarkError, setStaffMarkError] = useState<string | null>(null);

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
    </>
  );
}
