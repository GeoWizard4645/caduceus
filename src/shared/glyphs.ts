/**
 * The built-in shortcut glyphs.
 *
 * These replaced a set of hand-redrawn brand marks (Chrome, Gmail, Gemini,
 * Claude). Approximating someone else's logo in five paths looks wrong at every
 * size, goes stale the moment they rebrand, and ships their trademark inside our
 * bundle. A neutral family sidesteps all three, and — because these are path
 * data rendered inline rather than `<img>` files — they inherit `currentColor`,
 * so they tint with the accent and theme instead of sitting on top of it.
 *
 * One 24×24 grid, stroke-only, no fills. Add to this list freely; anything here
 * shows up in the picker automatically.
 */

export const GLYPHS = {
  sparkle: "M12 3.2 L13.7 9.4 L20 12 L13.7 14.6 L12 20.8 L10.3 14.6 L4 12 L10.3 9.4 Z",
  chat: "M4 6.5A2.5 2.5 0 0 1 6.5 4h11A2.5 2.5 0 0 1 20 6.5v7a2.5 2.5 0 0 1-2.5 2.5H10l-4.5 3.5V16H6.5A2.5 2.5 0 0 1 4 13.5Z",
  mail: "M3.5 7.5A2.5 2.5 0 0 1 6 5h12a2.5 2.5 0 0 1 2.5 2.5v9A2.5 2.5 0 0 1 18 19H6a2.5 2.5 0 0 1-2.5-2.5Z M4 7.8 12 13.2 20 7.8",
  globe:
    "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18Z M3.2 12h17.6 M12 3c2.4 2.5 3.7 5.7 3.7 9S14.4 18.5 12 21c-2.4-2.5-3.7-5.7-3.7-9S9.6 5.5 12 3Z",
  clipboard:
    "M9.5 4.5H7.5A2.5 2.5 0 0 0 5 7v12a2.5 2.5 0 0 0 2.5 2.5h9A2.5 2.5 0 0 0 19 19V7a2.5 2.5 0 0 0-2.5-2.5h-2 M9.5 3.2h5a1 1 0 0 1 1 1v1.6a1 1 0 0 1-1 1h-5a1 1 0 0 1-1-1V4.2a1 1 0 0 1 1-1Z",
  search: "M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14Z M16.2 16.2 21 21",
  terminal:
    "M3.5 6A2.5 2.5 0 0 1 6 3.5h12A2.5 2.5 0 0 1 20.5 6v12a2.5 2.5 0 0 1-2.5 2.5H6A2.5 2.5 0 0 1 3.5 18Z M7.5 9.5 10.5 12 7.5 14.5 M13 15h4",
  folder:
    "M3.5 7.5A2 2 0 0 1 5.5 5.5h3.2a2 2 0 0 1 1.5.7l1.1 1.3h7.2a2 2 0 0 1 2 2v7.5a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2Z",
  calendar:
    "M4.5 8A2.5 2.5 0 0 1 7 5.5h10A2.5 2.5 0 0 1 19.5 8v9.5A2.5 2.5 0 0 1 17 20H7a2.5 2.5 0 0 1-2.5-2.5Z M8 3.5v4 M16 3.5v4 M4.5 10.5h15",
  note: "M6 3.5h7.5L19 9v11.5a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-17a1 1 0 0 1 1-1Z M13.5 3.5V9H19 M8 13h8 M8 16.5h5",
  music:
    "M9 18V6.5l10-2v11.5 M9 18a2.5 2.5 0 1 1-5 0 2.5 2.5 0 0 1 5 0Z M19 16a2.5 2.5 0 1 1-5 0 2.5 2.5 0 0 1 5 0Z",
  image:
    "M3.5 6.5A2 2 0 0 1 5.5 4.5h13a2 2 0 0 1 2 2v11a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2Z M3.8 16.5 9 11.5l3.5 3.2 3-2.6 4.7 4.4 M9 9.2a1.3 1.3 0 1 1-2.6 0 1.3 1.3 0 0 1 2.6 0Z",
  code: "M9 7.5 4.5 12 9 16.5 M15 7.5 19.5 12 15 16.5",
  bolt: "M13.2 3 5.5 13.4h5.6L10.8 21l7.7-10.4h-5.6Z",
  window:
    "M3.5 6.5A2 2 0 0 1 5.5 4.5h13a2 2 0 0 1 2 2v11a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2Z M3.5 9h17",
  star: "M12 3.8 14.5 9l5.7.8-4.1 4 1 5.7-5.1-2.7-5.1 2.7 1-5.7-4.1-4L9.5 9Z",
} as const;

export type GlyphName = keyof typeof GLYPHS;

export const GLYPH_NAMES = Object.keys(GLYPHS) as GlyphName[];

export const GLYPH_PREFIX = "glyph:";

/**
 * Icons saved before the brand marks were removed.
 *
 * Settings on disk are migrated on load, but a token can still reach the UI
 * from an older config that has not been rewritten yet, so the renderer
 * resolves these too rather than falling back to a bare letter.
 */
const LEGACY_BRAND: Record<string, GlyphName> = {
  chrome: "globe",
  gmail: "mail",
  gemini: "sparkle",
  claude: "chat",
  clipboard: "clipboard",
};

/** Resolve an icon token to a glyph, or null if it is not one. */
export function glyphFor(icon: string): GlyphName | null {
  if (icon.startsWith(GLYPH_PREFIX)) {
    const name = icon.slice(GLYPH_PREFIX.length);
    return name in GLYPHS ? (name as GlyphName) : null;
  }
  if (icon.startsWith("brand:")) return LEGACY_BRAND[icon.slice(6)] ?? null;
  return null;
}
