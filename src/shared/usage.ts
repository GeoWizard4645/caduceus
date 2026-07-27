/**
 * How often you have run each thing, and how that feeds the palette's order.
 *
 * A launcher whose list looks the same on day 200 as on day 1 makes every user
 * pay for the average user's habits. Caduceus ships an opinion about what is
 * most likely to be wanted — see `commandWeight` in `commands.ts` — and then
 * lets your own use overrule it.
 *
 * Counts live in a JSON file next to the clipboard database and never leave the
 * machine. Settings → Command Center clears them.
 *
 * # Why there is a synchronous cache
 *
 * Ranking happens inside `search()`, which the palette calls on every keystroke
 * and cannot await. The counts are loaded once when the palette opens and kept
 * in this module; a use is applied to the cache immediately and written to disk
 * in the background, so the row you just ran is already ranked higher by the
 * time the list re-renders.
 */

import * as api from "./api";

export interface UsageEntry {
  count: number;
  lastUsedMs: number;
}

let cache: Record<string, UsageEntry> = {};
let loaded: Promise<void> | null = null;

/** Load counts once. Concurrent callers share the one request. */
export function loadUsage(): Promise<void> {
  loaded ??= api
    .usageCounts()
    .then((counts) => {
      // Merge rather than replace: a use recorded while this was in flight
      // must not be thrown away by the response landing after it.
      for (const [id, entry] of Object.entries(counts)) {
        const local = cache[id];
        if (!local || entry.count > local.count) cache[id] = entry;
      }
    })
    .catch(() => {
      // Ranking falls back to the shipped order. Not worth a message.
    });
  return loaded;
}

/** Drop the cache so the next `loadUsage` re-reads from disk. */
export function invalidateUsage(): void {
  cache = {};
  loaded = null;
}

export function usageOf(id: string): UsageEntry | undefined {
  return cache[id];
}

/**
 * Count one use.
 *
 * Applied to the cache first and persisted after, so ordering reacts on the
 * next render rather than on the next launch. A failed write costs a count.
 */
export function recordUsage(id: string): void {
  const current = cache[id];
  cache[id] = {
    count: (current?.count ?? 0) + 1,
    lastUsedMs: Date.now(),
  };
  void api.recordUsage(id).catch(() => {});
}

export async function clearUsage(): Promise<void> {
  cache = {};
  await api.clearUsage();
}

/** A day, in milliseconds. */
const DAY = 86_400_000;

/**
 * The step between "you have run this" and "you have not".
 *
 * Larger than the 0–100 the shipped weights span, so the browse list is
 * genuinely ordered most-used first: one use of anything puts it above
 * everything untouched, whatever Caduceus guessed.
 *
 * Smaller than a strong fuzzy match (~1000), which is the other thing this has
 * to be true of. Typing `shut` must still find "Shut down" ahead of whatever you
 * happen to run most — history should break ties, not override what you typed.
 */
const USED_AT_ALL = 500;

/**
 * How much a row's own history adds to its ranking.
 *
 * Flat after fifty uses on purpose: the gap between "never" and "a few times"
 * is the one carrying information, while the gap between 40 and 80 is noise
 * that would freeze the top of the list against anything new.
 */
export function usageBoost(id: string): number {
  const entry = cache[id];
  if (!entry || entry.count === 0) return 0;

  const uses = Math.min(entry.count, 50) * 10;

  // A nudge for the last few days, so equally-used rows sort in favour of
  // whatever you are working on this week.
  const age = Date.now() - entry.lastUsedMs;
  const recency = age < DAY ? 6 : age < 7 * DAY ? 3 : 0;

  return USED_AT_ALL + uses + recency;
}

/** Sort helper: most used first, then by the shipped weight. */
export function byUsageThen<T>(
  items: T[],
  id: (item: T) => string,
  weight: (item: T) => number,
): T[] {
  return [...items].sort((a, b) => {
    const scored = usageBoost(id(b)) + weight(b) - (usageBoost(id(a)) + weight(a));
    return scored;
  });
}
