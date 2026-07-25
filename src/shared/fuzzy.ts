/**
 * A small subsequence fuzzy matcher.
 *
 * Written by hand rather than pulled from npm: the whole thing is ~60 lines, it
 * has no dependencies to audit, and the scoring is tuned for short command
 * labels rather than for file paths (which is what most fuzzy libraries
 * optimise for).
 *
 * Scoring, highest first:
 *   exact match  ▸  prefix  ▸  word-boundary run  ▸  contiguous run  ▸  scattered
 */

export interface FuzzyMatch {
  score: number;
  /** Indices in the haystack that matched, for highlighting. */
  positions: number[];
}

const SCORE_EXACT = 1000;
const SCORE_PREFIX = 400;
const SCORE_WORD_START = 80;
const SCORE_CONSECUTIVE = 40;
const SCORE_MATCH = 12;
const PENALTY_GAP = -2;
const PENALTY_LEADING = -1;

/**
 * Match `needle` against `haystack`, case-insensitively.
 * Returns `null` when the needle is not a subsequence of the haystack.
 */
export function fuzzyMatch(needle: string, haystack: string): FuzzyMatch | null {
  if (!needle) return { score: 1, positions: [] };
  if (!haystack) return null;

  const n = needle.toLowerCase();
  const h = haystack.toLowerCase();

  if (h === n) return { score: SCORE_EXACT, positions: range(h.length) };
  if (h.startsWith(n)) return { score: SCORE_PREFIX + n.length, positions: range(n.length) };

  const positions: number[] = [];
  let score = 0;
  let hi = 0;
  let previousMatch = -2;

  for (let ni = 0; ni < n.length; ni++) {
    const target = n[ni];
    let found = -1;

    while (hi < h.length) {
      if (h[hi] === target) {
        found = hi;
        break;
      }
      hi++;
    }
    if (found === -1) return null;

    score += SCORE_MATCH;

    // A match right after a separator is what a human means by "the start of a
    // word", and is worth far more than one in the middle of a token.
    const before = found > 0 ? h[found - 1] : " ";
    if (found === 0 || before === " " || before === "-" || before === "_" || before === "/" || before === ".") {
      score += SCORE_WORD_START;
    }
    if (found === previousMatch + 1) score += SCORE_CONSECUTIVE;
    else if (previousMatch >= 0) score += PENALTY_GAP * Math.min(found - previousMatch - 1, 10);

    positions.push(found);
    previousMatch = found;
    hi = found + 1;
  }

  // Prefer matches that start early.
  score += PENALTY_LEADING * Math.min(positions[0] ?? 0, 20);
  return { score, positions };
}

/**
 * Score a needle against several fields, returning the best.
 * Later fields are worth slightly less, so a label match beats a keyword match.
 */
export function fuzzyScore(needle: string, fields: (string | undefined | null)[]): number | null {
  let best: number | null = null;
  fields.forEach((field, index) => {
    if (!field) return;
    const match = fuzzyMatch(needle, field);
    if (!match) return;
    const weighted = match.score * (1 - index * 0.12);
    if (best === null || weighted > best) best = weighted;
  });
  return best;
}

function range(n: number): number[] {
  return Array.from({ length: n }, (_, i) => i);
}

/** Split a string into matched/unmatched runs, for rendering highlights. */
export function highlightSegments(
  text: string,
  positions: number[],
): { text: string; match: boolean }[] {
  if (positions.length === 0) return [{ text, match: false }];

  const set = new Set(positions);
  const segments: { text: string; match: boolean }[] = [];
  let current = "";
  let currentMatch = set.has(0);

  for (let i = 0; i < text.length; i++) {
    const isMatch = set.has(i);
    if (isMatch !== currentMatch) {
      if (current) segments.push({ text: current, match: currentMatch });
      current = "";
      currentMatch = isMatch;
    }
    current += text[i];
  }
  if (current) segments.push({ text: current, match: currentMatch });
  return segments;
}
