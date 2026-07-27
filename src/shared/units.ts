/**
 * Unit conversion.
 *
 * # Everything here is offline and exact
 *
 * Every factor below is a definition, not a measurement: an inch *is* 25.4mm, a
 * pound *is* 0.45359237kg. So conversion is arithmetic on constants, computed
 * in this process, with no network and no staleness. That is the whole reason
 * this is a table in a file rather than an API call.
 *
 * Currency is the exception and is deliberately **not** in here — see
 * `shared/rates.ts`. A conversion whose answer depends on what day it is has
 * different rules, and mixing it in with metres and grams would quietly make
 * "convert 10 m to ft" a thing that could fail because the Wi-Fi was off.
 */

export type Dimension =
  | "length"
  | "mass"
  | "temperature"
  | "volume"
  | "area"
  | "speed"
  | "time"
  | "data"
  | "pressure"
  | "energy"
  | "angle";

export interface Unit {
  id: string;
  /** What it is called in full, singular. */
  name: string;
  /** The short form people type: `km`, `lb`, `°C`. */
  symbol: string;
  dimension: Dimension;
  /**
   * How many base units one of these is.
   *
   * Base per dimension: metre, kilogram, litre, square metre, metre/second,
   * second, byte, pascal, joule, degree. Temperature does not work this way and
   * is special-cased.
   */
  factor: number;
  /** Other spellings that should resolve to this unit. */
  aliases?: string[];
}

export const UNITS: Unit[] = [
  // --- length --------------------------------------------------------------
  u("nm", "nanometre", "nm", "length", 1e-9),
  u("um", "micrometre", "µm", "length", 1e-6, ["micron", "µm"]),
  u("mm", "millimetre", "mm", "length", 0.001, ["millimeter"]),
  u("cm", "centimetre", "cm", "length", 0.01, ["centimeter"]),
  u("m", "metre", "m", "length", 1, ["meter"]),
  u("km", "kilometre", "km", "length", 1000, ["kilometer"]),
  u("in", "inch", "in", "length", 0.0254, ["inches", '"']),
  u("ft", "foot", "ft", "length", 0.3048, ["feet", "'"]),
  u("yd", "yard", "yd", "length", 0.9144, ["yards"]),
  u("mi", "mile", "mi", "length", 1609.344, ["miles"]),
  u("nmi", "nautical mile", "nmi", "length", 1852),

  // --- mass ----------------------------------------------------------------
  u("mg", "milligram", "mg", "mass", 1e-6),
  u("g", "gram", "g", "mass", 0.001, ["grams", "gramme"]),
  u("kg", "kilogram", "kg", "mass", 1, ["kilo", "kilos", "kilogramme"]),
  u("t", "tonne", "t", "mass", 1000, ["ton", "metricton"]),
  u("oz", "ounce", "oz", "mass", 0.028349523125, ["ounces"]),
  u("lb", "pound", "lb", "mass", 0.45359237, ["lbs", "pounds"]),
  u("st", "stone", "st", "mass", 6.35029318, ["stones"]),

  // --- temperature ---------------------------------------------------------
  // Factor is unused; `convert` handles these by name.
  u("c", "Celsius", "°C", "temperature", 1, ["celsius", "centigrade", "degc", "°c"]),
  u("f", "Fahrenheit", "°F", "temperature", 1, ["fahrenheit", "degf", "°f"]),
  u("k", "Kelvin", "K", "temperature", 1, ["kelvin"]),

  // --- volume --------------------------------------------------------------
  u("ml", "millilitre", "ml", "volume", 0.001, ["milliliter", "cc"]),
  u("l", "litre", "L", "volume", 1, ["liter", "litres", "liters"]),
  u("tsp", "teaspoon", "tsp", "volume", 0.00492892159375),
  u("tbsp", "tablespoon", "tbsp", "volume", 0.01478676478125),
  u("floz", "fluid ounce (US)", "fl oz", "volume", 0.0295735295625, ["fluidounce"]),
  u("cup", "cup (US)", "cup", "volume", 0.2365882365, ["cups"]),
  u("pt", "pint (US)", "pt", "volume", 0.473176473, ["pints"]),
  u("qt", "quart (US)", "qt", "volume", 0.946352946),
  u("gal", "gallon (US)", "gal", "volume", 3.785411784, ["gallons"]),
  u("ukpt", "pint (imperial)", "UK pt", "volume", 0.56826125),
  u("ukgal", "gallon (imperial)", "UK gal", "volume", 4.54609),

  // --- area ----------------------------------------------------------------
  u("sqm", "square metre", "m²", "area", 1, ["m2", "sqmeter"]),
  u("sqkm", "square kilometre", "km²", "area", 1e6, ["km2"]),
  u("sqft", "square foot", "ft²", "area", 0.09290304, ["ft2"]),
  u("sqmi", "square mile", "mi²", "area", 2589988.110336, ["mi2"]),
  u("ha", "hectare", "ha", "area", 10000),
  u("acre", "acre", "acre", "area", 4046.8564224, ["acres"]),

  // --- speed ---------------------------------------------------------------
  u("mps", "metre per second", "m/s", "speed", 1, ["m/s"]),
  u("kph", "kilometre per hour", "km/h", "speed", 1 / 3.6, ["kmh", "km/h"]),
  u("mph", "mile per hour", "mph", "speed", 0.44704),
  u("knot", "knot", "kn", "speed", 0.514444, ["knots", "kn"]),

  // --- time ----------------------------------------------------------------
  u("ms", "millisecond", "ms", "time", 0.001),
  u("s", "second", "s", "time", 1, ["sec", "secs", "seconds"]),
  u("min", "minute", "min", "time", 60, ["mins", "minutes"]),
  u("h", "hour", "h", "time", 3600, ["hr", "hrs", "hours"]),
  u("d", "day", "d", "time", 86400, ["days"]),
  u("wk", "week", "wk", "time", 604800, ["weeks"]),
  u("yr", "year", "yr", "time", 31557600, ["years"]),

  // --- data ----------------------------------------------------------------
  // Decimal and binary both listed, because the ambiguity is the whole problem.
  u("b", "byte", "B", "data", 1, ["bytes"]),
  u("kb", "kilobyte", "kB", "data", 1000),
  u("mb", "megabyte", "MB", "data", 1e6),
  u("gb", "gigabyte", "GB", "data", 1e9),
  u("tb", "terabyte", "TB", "data", 1e12),
  u("kib", "kibibyte", "KiB", "data", 1024),
  u("mib", "mebibyte", "MiB", "data", 1024 ** 2),
  u("gib", "gibibyte", "GiB", "data", 1024 ** 3),
  u("tib", "tebibyte", "TiB", "data", 1024 ** 4),

  // --- pressure ------------------------------------------------------------
  u("pa", "pascal", "Pa", "pressure", 1),
  u("kpa", "kilopascal", "kPa", "pressure", 1000),
  u("bar", "bar", "bar", "pressure", 100000),
  u("psi", "pound per square inch", "psi", "pressure", 6894.757293168),
  u("atm", "atmosphere", "atm", "pressure", 101325),

  // --- energy --------------------------------------------------------------
  u("j", "joule", "J", "energy", 1),
  u("kj", "kilojoule", "kJ", "energy", 1000),
  u("cal", "calorie", "cal", "energy", 4.184),
  u("kcal", "kilocalorie", "kcal", "energy", 4184, ["calories"]),
  u("wh", "watt hour", "Wh", "energy", 3600),
  u("kwh", "kilowatt hour", "kWh", "energy", 3.6e6),

  // --- angle ---------------------------------------------------------------
  u("deg", "degree", "°", "angle", 1, ["degrees"]),
  u("rad", "radian", "rad", "angle", 180 / Math.PI, ["radians"]),
  u("turn", "turn", "turn", "angle", 360),
];

function u(
  id: string,
  name: string,
  symbol: string,
  dimension: Dimension,
  factor: number,
  aliases?: string[],
): Unit {
  return { id, name, symbol, dimension, factor, aliases };
}

export const DIMENSION_LABELS: Record<Dimension, string> = {
  length: "Length",
  mass: "Weight",
  temperature: "Temperature",
  volume: "Volume",
  area: "Area",
  speed: "Speed",
  time: "Time",
  data: "Data",
  pressure: "Pressure",
  energy: "Energy",
  angle: "Angle",
};

/** Find a unit by id, symbol, name or alias. Case- and plural-insensitive. */
export function findUnit(text: string): Unit | null {
  const needle = text.trim().toLowerCase().replace(/\s+/g, "");
  if (!needle) return null;

  for (const unit of UNITS) {
    const names = [
      unit.id,
      unit.symbol.toLowerCase(),
      unit.name.toLowerCase(),
      unit.name.toLowerCase().replace(/\s+/g, ""),
      ...(unit.aliases ?? []),
    ];
    if (names.some((n) => n.toLowerCase().replace(/\s+/g, "") === needle)) return unit;
  }

  // A trailing "s" is almost always a plural, and "gas" is not a unit.
  if (needle.endsWith("s") && needle.length > 2) return findUnit(needle.slice(0, -1));
  return null;
}

export function unitsIn(dimension: Dimension): Unit[] {
  return UNITS.filter((unit) => unit.dimension === dimension);
}

/**
 * Convert a value between two units of the same dimension.
 *
 * `null` when the units measure different things — converting kilograms to
 * metres is not a rounding question, it is a mistake, and answering it with a
 * number would be worse than answering it with nothing.
 */
export function convert(value: number, from: Unit, to: Unit): number | null {
  if (from.dimension !== to.dimension) return null;
  if (from.dimension === "temperature") return convertTemperature(value, from.id, to.id);
  return (value * from.factor) / to.factor;
}

/**
 * Temperature is an offset scale, not a ratio one.
 *
 * 20°C is not "twice" 10°C, and multiplying by a factor — which is what every
 * other dimension here does — gets it wrong in a way that looks plausible.
 */
function convertTemperature(value: number, from: string, to: string): number {
  const kelvin =
    from === "c" ? value + 273.15
    : from === "f" ? (value - 32) * (5 / 9) + 273.15
    : value;

  return to === "c" ? kelvin - 273.15
    : to === "f" ? (kelvin - 273.15) * (9 / 5) + 32
    : kelvin;
}

/**
 * Read "12 km to miles", "3kg in lb", "100 f to c".
 *
 * Used by `conversionProvider` in `providers.ts`, so it has to be certain:
 * anything it is not sure about returns `null` and falls through to a web
 * search rather than turning a sentence into a number.
 */
export interface ParsedConversion {
  value: number;
  from: Unit;
  to: Unit;
}

export function parseConversion(input: string): ParsedConversion | null {
  // The unit tokens allow digits (`m2`, `ft2`) but the *value* is anchored to
  // the start, so "12 km" cannot be read as the unit. `to|in|as` are the words
  // people actually type; `in` is also a unit, which is why it only counts as
  // a separator when something follows it.
  const match =
    /^\s*(-?[\d.,]+)\s*([a-zA-Z0-9°"'µ/]+)\s*(?:to|in|as|→|>)\s+([a-zA-Z0-9°"'µ/]+)\s*$/.exec(
      input,
    );
  if (!match) return null;

  const value = Number.parseFloat(match[1].replace(/,/g, ""));
  if (!Number.isFinite(value)) return null;

  const from = findUnit(match[2]);
  const to = findUnit(match[3]);
  if (!from || !to || from.dimension !== to.dimension) return null;

  return { value, from, to };
}

/**
 * Format a converted number so it reads like an answer.
 *
 * Significant figures rather than a fixed number of decimals: 0.00003048 and
 * 1609.344 are both correct answers, and rounding both to two places makes one
 * of them zero.
 */
export function formatValue(value: number): string {
  if (!Number.isFinite(value)) return "—";
  if (value === 0) return "0";

  const magnitude = Math.abs(value);
  if (magnitude >= 1e12 || magnitude < 1e-6) return value.toExponential(4);

  const decimals =
    magnitude >= 100 ? 2
    : magnitude >= 1 ? 4
    : magnitude >= 0.01 ? 6
    : 8;

  // `parseFloat` drops the trailing zeros `toFixed` insists on.
  return String(Number.parseFloat(value.toFixed(decimals)));
}
