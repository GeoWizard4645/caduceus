/**
 * Check the conversions and the colour maths.
 *
 * Both modules are pure arithmetic that the type checker cannot see through: a
 * transposed constant, a `+` where a `*` belongs, or a Celsius conversion done
 * as a ratio instead of an offset all compile perfectly and produce a confident
 * wrong number. That is worse than a crash, because nobody double-checks a
 * converter.
 *
 * So the values below are the definitions themselves — an inch *is* 25.4mm,
 * water boils at 212°F — plus the round trips that catch a wrong inverse.
 *
 * Run with `npm run check:units`. Part of `npm run build`.
 */

import { build } from "esbuild";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const root = resolve(import.meta.dirname, "..");
const scratch = await mkdtemp(join(tmpdir(), "caduceus-units-"));

let failures = 0;
function check(label, ok) {
  console.log(`${ok ? "ok  " : "FAIL"} ${label}`);
  if (!ok) failures += 1;
}

/** Floating-point comparison with a tolerance, because 0.1 + 0.2 exists. */
function near(actual, expected, tolerance = 1e-6) {
  return Number.isFinite(actual) && Math.abs(actual - expected) <= tolerance;
}

try {
  const stub = join(scratch, "tauri-stub.js");
  await writeFile(stub, "export const invoke = () => Promise.resolve({});\n");

  const entry = join(scratch, "entry.ts");
  await writeFile(
    entry,
    [
      `export * from ${JSON.stringify(join(root, "src/shared/units.ts"))};`,
      `export * as color from ${JSON.stringify(join(root, "src/shared/color.ts"))};`,
    ].join("\n"),
  );

  const bundle = join(scratch, "units.mjs");
  await build({
    entryPoints: [entry],
    outfile: bundle,
    bundle: true,
    format: "esm",
    platform: "node",
    logLevel: "silent",
    alias: { "@tauri-apps/api/core": stub },
  });

  const { UNITS, convert, findUnit, formatValue, parseConversion, color } = await import(
    pathToFileURL(bundle).href
  );

  const to = (value, from, target) => convert(value, findUnit(from), findUnit(target));

  // --- the definitions ---------------------------------------------------
  check("an inch is exactly 25.4 mm", near(to(1, "in", "mm"), 25.4));
  check("a mile is exactly 1609.344 m", near(to(1, "mi", "m"), 1609.344));
  check("a pound is exactly 0.45359237 kg", near(to(1, "lb", "kg"), 0.45359237));
  check("a foot is twelve inches", near(to(1, "ft", "in"), 12));
  check("a nautical mile is 1852 m", near(to(1, "nmi", "m"), 1852));
  check("a kilobyte is 1000 bytes and a kibibyte is 1024", near(to(1, "kb", "b"), 1000) && near(to(1, "kib", "b"), 1024));
  check("an hour is 3600 seconds", near(to(1, "h", "s"), 3600));

  // --- temperature is an offset scale, not a ratio -----------------------
  // The one conversion in the table that cannot be done by multiplying, and
  // therefore the one most likely to be wrong in a way that looks plausible.
  check("water boils: 100 °C = 212 °F", near(to(100, "c", "f"), 212, 1e-9));
  check("water freezes: 0 °C = 32 °F", near(to(0, "c", "f"), 32, 1e-9));
  check("0 °C = 273.15 K", near(to(0, "c", "k"), 273.15, 1e-9));
  check("absolute zero: 0 K = −273.15 °C", near(to(0, "k", "c"), -273.15, 1e-9));
  check("the scales cross at −40", near(to(-40, "c", "f"), -40, 1e-9));
  check("body heat: 37 °C ≈ 98.6 °F", near(to(37, "c", "f"), 98.6, 1e-9));
  check(
    "temperature is not treated as a ratio (20 °C is not twice 10 °C in °F)",
    !near(to(20, "c", "f"), to(10, "c", "f") * 2, 0.5),
  );

  // --- round trips --------------------------------------------------------
  // A wrong inverse survives every single-direction test.
  for (const [a, b] of [
    ["km", "mi"], ["kg", "lb"], ["l", "gal"], ["c", "f"], ["c", "k"],
    ["mps", "mph"], ["gb", "gib"], ["bar", "psi"], ["deg", "rad"], ["sqm", "acre"],
  ]) {
    const there = to(7.5, a, b);
    const back = convert(there, findUnit(b), findUnit(a));
    check(`${a} → ${b} → ${a} returns 7.5`, near(back, 7.5, 1e-6));
  }

  // --- refusals -----------------------------------------------------------
  check(
    "converting mass to length is refused rather than answered",
    convert(1, findUnit("kg"), findUnit("m")) === null,
  );
  check("an unknown unit is null, not a guess", findUnit("bananas") === null);
  check("every unit resolves by its own id", UNITS.every((u) => findUnit(u.id)?.id === u.id));
  check("every unit resolves by its symbol", UNITS.every((u) => findUnit(u.symbol) !== null));

  // --- parsing what people type ------------------------------------------
  for (const [text, expected] of [
    ["12 km to miles", 7.456454],
    ["3kg in lb", 6.613868],
    ["100 f to c", 37.777778],
    ["1 mi to km", 1.609344],
  ]) {
    const parsed = parseConversion(text);
    const result = parsed ? convert(parsed.value, parsed.from, parsed.to) : null;
    check(`"${text}" parses and converts`, result !== null && near(result, expected, 1e-4));
  }
  check("a sentence is not mistaken for a conversion", parseConversion("what is a mile") === null);
  check("a bare number is not a conversion", parseConversion("42") === null);

  // --- formatting ---------------------------------------------------------
  check("a tiny number keeps its significant digits", formatValue(0.0000254) !== "0");
  check("a large number is not spammed with decimals", !formatValue(1609.344).includes("0000"));
  check("zero is zero", formatValue(0) === "0");

  // --- colour -------------------------------------------------------------
  const { parseColor, rgbToHex, rgbToHsl, hslToRgb, nearestName, wcag, contrastRatio } = color;

  check("hex parses", rgbToHex(parseColor("#3b82f6")) === "#3b82f6");
  check("short hex expands", rgbToHex(parseColor("#abc")) === "#aabbcc");
  check("bare hex works", rgbToHex(parseColor("3b82f6")) === "#3b82f6");
  check("rgb() with commas", rgbToHex(parseColor("rgb(59, 130, 246)")) === "#3b82f6");
  check("rgb() with spaces and alpha", parseColor("rgb(59 130 246 / 50%)")?.a === 0.5);
  check("a CSS name resolves", rgbToHex(parseColor("rebeccapurple")) === "#663399");
  check("nonsense is null, not black", parseColor("not a colour") === null);
  check("an out-of-range hex is refused", parseColor("#gggggg") === null);
  check(
    "a pink-purple swatch is named plum, not an early distant CSS colour",
    nearestName(parseColor("#e394dc")).name === "plum",
  );
  check("an exact CSS colour has zero name distance", nearestName(parseColor("orchid")).distance === 0);

  // Round trip through HSL. Rounding to whole degrees and percents loses a
  // little, so this allows a couple of levels per channel rather than exactness.
  let worst = 0;
  for (const hex of ["#3b82f6", "#ff0000", "#00ff00", "#123456", "#fedcba", "#808080", "#ffffff", "#000000"]) {
    const rgb = parseColor(hex);
    const back = hslToRgb(rgbToHsl(rgb));
    worst = Math.max(worst, Math.abs(rgb.r - back.r), Math.abs(rgb.g - back.g), Math.abs(rgb.b - back.b));
  }
  check(`rgb → hsl → rgb stays within 3 levels (worst ${worst})`, worst <= 3);

  // --- WCAG ---------------------------------------------------------------
  // The published extremes. If these are wrong, every accessibility verdict is.
  check(
    "black on white is 21:1",
    near(contrastRatio(parseColor("#000"), parseColor("#fff")), 21, 0.01),
  );
  check(
    "a colour against itself is 1:1",
    near(contrastRatio(parseColor("#3b82f6"), parseColor("#3b82f6")), 1, 1e-9),
  );
  check(
    "contrast is symmetric",
    near(
      contrastRatio(parseColor("#123"), parseColor("#eee")),
      contrastRatio(parseColor("#eee"), parseColor("#123")),
      1e-9,
    ),
  );

  const black = wcag(parseColor("#000"), parseColor("#fff"));
  check("black on white passes every level", black.aaNormal && black.aaLarge && black.aaaNormal);
  const bad = wcag(parseColor("#777"), parseColor("#888"));
  check("grey on grey passes nothing", !bad.aaNormal && !bad.aaLarge && !bad.aaaNormal);
  check(
    "the AA boundary is 4.5, not 4.4",
    wcag(parseColor("#767676"), parseColor("#fff")).aaNormal,
  );
} finally {
  await rm(scratch, { recursive: true, force: true });
}

if (failures) {
  console.log(`\n${failures} conversion or colour property broken.`);
  process.exit(1);
}
