/**
 * Colour parsing, conversion and contrast.
 *
 * # Why this is in TypeScript and not Rust
 *
 * The colour tool is interactive — a swatch you drag, a value you type, a
 * contrast figure that updates as you go. Round-tripping every keystroke
 * through IPC to compute six numbers would make it feel like a form submission
 * instead of a colour picker. None of this touches the disk or the network, so
 * there is nothing here that wants to be on the other side.
 *
 * `convert a colour` used to call into Rust and hand back a formatted blob. It
 * also did not work, because it only accepted two of the notations people
 * actually type. This parses everything CSS does, and the tool built on it can
 * show a swatch.
 */

export interface Rgb {
  r: number;
  g: number;
  b: number;
  /** 0–1. */
  a: number;
}

export interface Hsl {
  h: number;
  s: number;
  l: number;
  a: number;
}

/**
 * Parse anything somebody might reasonably paste.
 *
 * `#abc`, `#aabbcc`, `#aabbccdd`, `rgb(1,2,3)`, `rgb(1 2 3 / 50%)`,
 * `rgba(1,2,3,.5)`, `hsl(210, 50%, 40%)`, `hsl(210 50% 40% / .5)`, and the 148
 * CSS colour names. Returns `null` rather than guessing.
 */
export function parseColor(input: string): Rgb | null {
  const text = input.trim().toLowerCase();
  if (!text) return null;

  const named = CSS_COLORS[text];
  if (named) return parseHex(named);

  if (text.startsWith("#")) return parseHex(text);
  // A bare `aabbcc` is what you get from copying out of a design tool.
  if (/^[0-9a-f]{3,8}$/.test(text) && [3, 4, 6, 8].includes(text.length)) {
    return parseHex(`#${text}`);
  }

  const fn = /^(rgba?|hsla?)\s*\(([^)]*)\)$/.exec(text);
  if (!fn) return null;

  // CSS accepts commas or spaces, with an optional `/ alpha`. Normalising both
  // to one list is simpler than two parsers that disagree at the edges.
  const [head, alphaPart] = fn[2].split("/");
  const parts = head
    .split(/[\s,]+/)
    .map((p) => p.trim())
    .filter(Boolean);
  if (parts.length < 3) return null;

  const alpha = alphaPart !== undefined ? parseAlpha(alphaPart) : parseAlpha(parts[3] ?? "1");
  if (alpha === null) return null;

  if (fn[1].startsWith("rgb")) {
    const [r, g, b] = parts.slice(0, 3).map(parseChannel);
    if ([r, g, b].some((v) => v === null)) return null;
    return { r: r!, g: g!, b: b!, a: alpha };
  }

  const h = parseAngle(parts[0]);
  const s = parsePercent(parts[1]);
  const l = parsePercent(parts[2]);
  if (h === null || s === null || l === null) return null;
  return hslToRgb({ h, s, l, a: alpha });
}

function parseHex(text: string): Rgb | null {
  const hex = text.replace("#", "");
  const expand = (c: string) => parseInt(c + c, 16);
  if (hex.length === 3 || hex.length === 4) {
    const [r, g, b, a] = hex.split("");
    if (!/^[0-9a-f]+$/i.test(hex)) return null;
    return { r: expand(r), g: expand(g), b: expand(b), a: a ? expand(a) / 255 : 1 };
  }
  if (hex.length === 6 || hex.length === 8) {
    if (!/^[0-9a-f]+$/i.test(hex)) return null;
    return {
      r: parseInt(hex.slice(0, 2), 16),
      g: parseInt(hex.slice(2, 4), 16),
      b: parseInt(hex.slice(4, 6), 16),
      a: hex.length === 8 ? parseInt(hex.slice(6, 8), 16) / 255 : 1,
    };
  }
  return null;
}

/** `128`, `50%` → 0–255. */
function parseChannel(part: string): number | null {
  const value = part.endsWith("%")
    ? (Number.parseFloat(part) / 100) * 255
    : Number.parseFloat(part);
  return Number.isFinite(value) ? clamp(Math.round(value), 0, 255) : null;
}

function parseAlpha(part: string): number | null {
  const text = part.trim();
  if (!text) return 1;
  const value = text.endsWith("%") ? Number.parseFloat(text) / 100 : Number.parseFloat(text);
  return Number.isFinite(value) ? clamp(value, 0, 1) : null;
}

/** Degrees, with the `deg`/`turn`/`rad` suffixes CSS allows. */
function parseAngle(part: string): number | null {
  const value = Number.parseFloat(part);
  if (!Number.isFinite(value)) return null;
  if (part.endsWith("turn")) return ((value * 360) % 360 + 360) % 360;
  if (part.endsWith("rad")) return (((value * 180) / Math.PI) % 360 + 360) % 360;
  return ((value % 360) + 360) % 360;
}

function parsePercent(part: string): number | null {
  const value = Number.parseFloat(part);
  return Number.isFinite(value) ? clamp(value, 0, 100) : null;
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

export function rgbToHex({ r, g, b, a }: Rgb, includeAlpha = false): string {
  const pair = (v: number) => v.toString(16).padStart(2, "0");
  const base = `#${pair(r)}${pair(g)}${pair(b)}`;
  if (!includeAlpha || a >= 1) return base;
  return `${base}${pair(Math.round(a * 255))}`;
}

export function rgbToHsl({ r, g, b, a }: Rgb): Hsl {
  const rn = r / 255;
  const gn = g / 255;
  const bn = b / 255;
  const max = Math.max(rn, gn, bn);
  const min = Math.min(rn, gn, bn);
  const delta = max - min;

  let h = 0;
  if (delta !== 0) {
    if (max === rn) h = ((gn - bn) / delta) % 6;
    else if (max === gn) h = (bn - rn) / delta + 2;
    else h = (rn - gn) / delta + 4;
  }
  h = Math.round(((h * 60) % 360 + 360) % 360);

  const l = (max + min) / 2;
  const s = delta === 0 ? 0 : delta / (1 - Math.abs(2 * l - 1));
  return { h, s: Math.round(s * 100), l: Math.round(l * 100), a };
}

export function hslToRgb({ h, s, l, a }: Hsl): Rgb {
  const sn = s / 100;
  const ln = l / 100;
  const c = (1 - Math.abs(2 * ln - 1)) * sn;
  const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
  const m = ln - c / 2;

  const [r1, g1, b1] =
    h < 60 ? [c, x, 0]
    : h < 120 ? [x, c, 0]
    : h < 180 ? [0, c, x]
    : h < 240 ? [0, x, c]
    : h < 300 ? [x, 0, c]
    : [c, 0, x];

  return {
    r: Math.round((r1 + m) * 255),
    g: Math.round((g1 + m) * 255),
    b: Math.round((b1 + m) * 255),
    a,
  };
}

/** CMYK, for anyone whose colour ends up on paper. */
export function rgbToCmyk({ r, g, b }: Rgb): [number, number, number, number] {
  const rn = r / 255;
  const gn = g / 255;
  const bn = b / 255;
  const k = 1 - Math.max(rn, gn, bn);
  if (k === 1) return [0, 0, 0, 100];
  return [
    Math.round(((1 - rn - k) / (1 - k)) * 100),
    Math.round(((1 - gn - k) / (1 - k)) * 100),
    Math.round(((1 - bn - k) / (1 - k)) * 100),
    Math.round(k * 100),
  ];
}

// ---------------------------------------------------------------------------
// Contrast
// ---------------------------------------------------------------------------

/** Relative luminance, per WCAG 2.1. */
export function luminance({ r, g, b }: Rgb): number {
  const channel = (v: number) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

/** WCAG contrast ratio, 1–21. */
export function contrastRatio(a: Rgb, b: Rgb): number {
  const la = luminance(a);
  const lb = luminance(b);
  const [light, dark] = la > lb ? [la, lb] : [lb, la];
  return (light + 0.05) / (dark + 0.05);
}

export interface WcagVerdict {
  ratio: number;
  /** 4.5:1 — body text. */
  aaNormal: boolean;
  /** 3:1 — 18pt, or 14pt bold. */
  aaLarge: boolean;
  /** 7:1. */
  aaaNormal: boolean;
  aaaLarge: boolean;
}

export function wcag(foreground: Rgb, background: Rgb): WcagVerdict {
  const ratio = contrastRatio(foreground, background);
  return {
    ratio,
    aaNormal: ratio >= 4.5,
    aaLarge: ratio >= 3,
    aaaNormal: ratio >= 7,
    aaaLarge: ratio >= 4.5,
  };
}

/** Black or white, whichever is readable on this. */
export function readableOn(background: Rgb): "#000000" | "#ffffff" {
  return contrastRatio({ r: 0, g: 0, b: 0, a: 1 }, background) >= 4.5 ? "#000000" : "#ffffff";
}

// ---------------------------------------------------------------------------
// Naming and palettes
// ---------------------------------------------------------------------------

/** The nearest CSS colour name, and how far off it is (0 = exact). */
export function nearestName(rgb: Rgb): { name: string; distance: number } {
  let bestName = "black";
  let bestDistanceSquared = Number.POSITIVE_INFINITY;
  for (const [name, hex] of Object.entries(CSS_COLORS)) {
    const other = parseHex(hex);
    if (!other) continue;
    // Squared distance in RGB. Not perceptually uniform, but for "what would
    // you call this" it agrees with people often enough, and it is honest about
    // how far off it is rather than insisting.
    const distance =
      (rgb.r - other.r) ** 2 + (rgb.g - other.g) ** 2 + (rgb.b - other.b) ** 2;
    if (distance < bestDistanceSquared) {
      bestName = name;
      bestDistanceSquared = distance;
    }
  }
  return { name: bestName, distance: Math.sqrt(bestDistanceSquared) };
}

/** Tints and shades: the same hue at ten lightnesses. */
export function scale(rgb: Rgb): { label: string; hex: string }[] {
  const hsl = rgbToHsl(rgb);
  return [95, 90, 80, 70, 60, 50, 40, 30, 20, 10].map((l) => ({
    label: String((100 - l) * 10 || 50),
    hex: rgbToHex(hslToRgb({ ...hsl, l })),
  }));
}

/** The usual harmonies, for when one colour needs company. */
export function harmonies(rgb: Rgb): { label: string; hex: string }[] {
  const hsl = rgbToHsl(rgb);
  const at = (offset: number) => rgbToHex(hslToRgb({ ...hsl, h: (hsl.h + offset + 360) % 360 }));
  return [
    { label: "Complement", hex: at(180) },
    { label: "Triad +120°", hex: at(120) },
    { label: "Triad −120°", hex: at(-120) },
    { label: "Analogous +30°", hex: at(30) },
    { label: "Analogous −30°", hex: at(-30) },
    { label: "Split +150°", hex: at(150) },
    { label: "Split −150°", hex: at(-150) },
  ];
}

/**
 * Every distinct colour in an image, most common first.
 *
 * Quantised to a 16-level cube per channel before counting: a photograph has
 * tens of thousands of literally-distinct pixels and none of that is a palette.
 * The reported hex is the average of the bucket, not the bucket's corner, so
 * the swatch matches what is actually in the image.
 */
export function extractPalette(
  pixels: Uint8ClampedArray,
  max = 12,
): { hex: string; share: number }[] {
  const buckets = new Map<number, { r: number; g: number; b: number; n: number }>();

  for (let i = 0; i < pixels.length; i += 4) {
    // Skip anything mostly transparent: it is not a colour anyone chose.
    if (pixels[i + 3] < 128) continue;
    const r = pixels[i];
    const g = pixels[i + 1];
    const b = pixels[i + 2];
    const key = ((r >> 4) << 8) | ((g >> 4) << 4) | (b >> 4);
    const bucket = buckets.get(key);
    if (bucket) {
      bucket.r += r;
      bucket.g += g;
      bucket.b += b;
      bucket.n += 1;
    } else {
      buckets.set(key, { r, g, b, n: 1 });
    }
  }

  const total = [...buckets.values()].reduce((sum, b) => sum + b.n, 0);
  if (total === 0) return [];

  return [...buckets.values()]
    .sort((a, b) => b.n - a.n)
    .slice(0, max)
    .map((b) => ({
      hex: rgbToHex({
        r: Math.round(b.r / b.n),
        g: Math.round(b.g / b.n),
        b: Math.round(b.b / b.n),
        a: 1,
      }),
      share: b.n / total,
    }));
}

/** Everything about one colour, as copyable lines. */
export function describe(rgb: Rgb): { label: string; value: string }[] {
  const hsl = rgbToHsl(rgb);
  const [c, m, y, k] = rgbToCmyk(rgb);
  const name = nearestName(rgb);
  return [
    { label: "Hex", value: rgbToHex(rgb, true) },
    { label: "RGB", value: `rgb(${rgb.r}, ${rgb.g}, ${rgb.b})` },
    {
      label: "RGBA",
      value: `rgba(${rgb.r}, ${rgb.g}, ${rgb.b}, ${round(rgb.a, 2)})`,
    },
    { label: "HSL", value: `hsl(${hsl.h}, ${hsl.s}%, ${hsl.l}%)` },
    { label: "CMYK", value: `cmyk(${c}%, ${m}%, ${y}%, ${k}%)` },
    {
      label: "Nearest name",
      value: name.distance < 1 ? name.name : `${name.name} (approx.)`,
    },
    { label: "Luminance", value: round(luminance(rgb), 4).toString() },
  ];
}

const round = (v: number, places: number) => Number(v.toFixed(places));

/**
 * The CSS named colours.
 *
 * Here rather than fetched or generated: it is a fixed list that has not
 * changed since CSS Color 4, and "what would you call this colour" should not
 * depend on a network.
 */
const CSS_COLORS: Record<string, string> = {
  aliceblue: "#f0f8ff", antiquewhite: "#faebd7", aqua: "#00ffff", aquamarine: "#7fffd4",
  azure: "#f0ffff", beige: "#f5f5dc", bisque: "#ffe4c4", black: "#000000",
  blanchedalmond: "#ffebcd", blue: "#0000ff", blueviolet: "#8a2be2", brown: "#a52a2a",
  burlywood: "#deb887", cadetblue: "#5f9ea0", chartreuse: "#7fff00", chocolate: "#d2691e",
  coral: "#ff7f50", cornflowerblue: "#6495ed", cornsilk: "#fff8dc", crimson: "#dc143c",
  cyan: "#00ffff", darkblue: "#00008b", darkcyan: "#008b8b", darkgoldenrod: "#b8860b",
  darkgray: "#a9a9a9", darkgreen: "#006400", darkgrey: "#a9a9a9", darkkhaki: "#bdb76b",
  darkmagenta: "#8b008b", darkolivegreen: "#556b2f", darkorange: "#ff8c00",
  darkorchid: "#9932cc", darkred: "#8b0000", darksalmon: "#e9967a", darkseagreen: "#8fbc8f",
  darkslateblue: "#483d8b", darkslategray: "#2f4f4f", darkturquoise: "#00ced1",
  darkviolet: "#9400d3", deeppink: "#ff1493", deepskyblue: "#00bfff", dimgray: "#696969",
  dodgerblue: "#1e90ff", firebrick: "#b22222", floralwhite: "#fffaf0", forestgreen: "#228b22",
  fuchsia: "#ff00ff", gainsboro: "#dcdcdc", ghostwhite: "#f8f8ff", gold: "#ffd700",
  goldenrod: "#daa520", gray: "#808080", green: "#008000", greenyellow: "#adff2f",
  grey: "#808080", honeydew: "#f0fff0", hotpink: "#ff69b4", indianred: "#cd5c5c",
  indigo: "#4b0082", ivory: "#fffff0", khaki: "#f0e68c", lavender: "#e6e6fa",
  lavenderblush: "#fff0f5", lawngreen: "#7cfc00", lemonchiffon: "#fffacd",
  lightblue: "#add8e6", lightcoral: "#f08080", lightcyan: "#e0ffff",
  lightgoldenrodyellow: "#fafad2", lightgray: "#d3d3d3", lightgreen: "#90ee90",
  lightpink: "#ffb6c1", lightsalmon: "#ffa07a", lightseagreen: "#20b2aa",
  lightskyblue: "#87cefa", lightslategray: "#778899", lightsteelblue: "#b0c4de",
  lightyellow: "#ffffe0", lime: "#00ff00", limegreen: "#32cd32", linen: "#faf0e6",
  magenta: "#ff00ff", maroon: "#800000", mediumaquamarine: "#66cdaa", mediumblue: "#0000cd",
  mediumorchid: "#ba55d3", mediumpurple: "#9370db", mediumseagreen: "#3cb371",
  mediumslateblue: "#7b68ee", mediumspringgreen: "#00fa9a", mediumturquoise: "#48d1cc",
  mediumvioletred: "#c71585", midnightblue: "#191970", mintcream: "#f5fffa",
  mistyrose: "#ffe4e1", moccasin: "#ffe4b5", navajowhite: "#ffdead", navy: "#000080",
  oldlace: "#fdf5e6", olive: "#808000", olivedrab: "#6b8e23", orange: "#ffa500",
  orangered: "#ff4500", orchid: "#da70d6", palegoldenrod: "#eee8aa", palegreen: "#98fb98",
  paleturquoise: "#afeeee", palevioletred: "#db7093", papayawhip: "#ffefd5",
  peachpuff: "#ffdab9", peru: "#cd853f", pink: "#ffc0cb", plum: "#dda0dd",
  powderblue: "#b0e0e6", purple: "#800080", rebeccapurple: "#663399", red: "#ff0000",
  rosybrown: "#bc8f8f", royalblue: "#4169e1", saddlebrown: "#8b4513", salmon: "#fa8072",
  sandybrown: "#f4a460", seagreen: "#2e8b57", seashell: "#fff5ee", sienna: "#a0522d",
  silver: "#c0c0c0", skyblue: "#87ceeb", slateblue: "#6a5acd", slategray: "#708090",
  snow: "#fffafa", springgreen: "#00ff7f", steelblue: "#4682b4", tan: "#d2b48c",
  teal: "#008080", thistle: "#d8bfd8", tomato: "#ff6347", turquoise: "#40e0d0",
  violet: "#ee82ee", wheat: "#f5deb3", white: "#ffffff", whitesmoke: "#f5f5f5",
  yellow: "#ffff00", yellowgreen: "#9acd32",
};

export const COLOR_NAMES = Object.keys(CSS_COLORS);
export const colorByName = (name: string): string | undefined => CSS_COLORS[name.toLowerCase()];
