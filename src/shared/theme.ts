/**
 * Applies the user's theme and accent colour to the document.
 *
 * Colours live in CSS custom properties (see `src/styles.css`), so changing the
 * accent is a matter of writing three variables rather than re-rendering
 * anything. Tailwind reads the same variables through `rgb(var(--c-x) / alpha)`.
 */

import type { AppearanceSettings, Theme } from "./types";

/** `#7c7cff` → `"124 124 255"`, the format the CSS variables expect. */
export function hexToRgbChannels(hex: string): string | null {
  const cleaned = hex.trim().replace(/^#/, "");
  const expanded =
    cleaned.length === 3
      ? cleaned
          .split("")
          .map((c) => c + c)
          .join("")
      : cleaned;

  if (!/^[0-9a-fA-F]{6}$/.test(expanded)) return null;

  const value = parseInt(expanded, 16);
  return `${(value >> 16) & 255} ${(value >> 8) & 255} ${value & 255}`;
}

/** Relative luminance, per WCAG. Used to pick readable text on the accent. */
function luminance(r: number, g: number, b: number): number {
  const channel = (v: number) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

function resolveTheme(theme: Theme): "dark" | "light" {
  if (theme !== "system") return theme;
  return window.matchMedia?.("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

/** Exported for UI that shows or toggles the active mode. */
export function resolvedAppearanceMode(theme: Theme): "dark" | "light" {
  return resolveTheme(theme);
}

/** Flip between light and dark; leaves an explicit choice (not system). */
export function toggleAppearanceTheme(theme: Theme): "dark" | "light" {
  return resolveTheme(theme) === "light" ? "dark" : "light";
}

/**
 * Write the appearance settings into the document.
 *
 * Safe to call on every settings change: it only touches `data-theme` and a
 * handful of custom properties.
 */
export function applyAppearance(appearance: AppearanceSettings): void {
  const root = document.documentElement;
  const mode = resolveTheme(appearance.theme);
  root.setAttribute("data-theme", mode);

  const channels = hexToRgbChannels(appearance.accent);
  if (channels) {
    root.style.setProperty("--c-accent", channels);

    const [r, g, b] = channels.split(" ").map(Number) as [number, number, number];

    // A muted companion for tinted backgrounds. Dark themes want a darker
    // partner; light themes want a paler one.
    const soft =
      mode === "dark"
        ? `${Math.round(r * 0.42)} ${Math.round(g * 0.42)} ${Math.round(b * 0.42)}`
        : `${Math.round(r + (255 - r) * 0.82)} ${Math.round(g + (255 - g) * 0.82)} ${Math.round(
            b + (255 - b) * 0.82,
          )}`;
    root.style.setProperty("--c-accent-soft", soft);

    // Text drawn *on* the accent. A bright accent needs dark text, or the
    // primary button becomes unreadable — which is exactly what happens if a
    // user picks yellow and we always assume white.
    root.style.setProperty("--c-accent-ink", luminance(r, g, b) > 0.55 ? "12 14 22" : "255 255 255");
  }

  // Solid surfaces instead of translucent ones.
  root.style.setProperty("--c-glass-alpha", appearance.reduceTransparency ? "1" : mode === "dark" ? "0.72" : "0.78");
}

/**
 * Re-apply when the OS theme changes, for users on "System".
 * Returns an unsubscribe function.
 */
export function watchSystemTheme(getAppearance: () => AppearanceSettings): () => void {
  const media = window.matchMedia?.("(prefers-color-scheme: light)");
  if (!media) return () => {};

  const handler = () => {
    const appearance = getAppearance();
    if (appearance.theme === "system") applyAppearance(appearance);
  };
  media.addEventListener("change", handler);
  return () => media.removeEventListener("change", handler);
}
