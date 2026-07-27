/**
 * Writing something down as the user types, without losing the last keystroke.
 *
 * # The bug this exists to make impossible
 *
 * The obvious debounce is wrong in a way that only shows up at the worst moment:
 *
 * ```ts
 * useEffect(() => {
 *   const timer = setTimeout(() => save(value), 300);
 *   return () => clearTimeout(timer);   // ← cancels the pending write
 * }, [value]);
 * ```
 *
 * On every keystroke the cleanup cancels the previous timer and a new one is
 * set, which is the point. But the cleanup **also runs when the component
 * unmounts**, and there it cancels a write that will never be rescheduled. Type
 * a sentence and close the tab within the debounce window and the sentence is
 * gone — silently, with no error and nothing to undo.
 *
 * For a scratch surface people trust with notes taken during a meeting, that is
 * not an acceptable failure. This flushes instead of cancelling.
 */

import { useEffect, useRef } from "react";

/**
 * Persist `value` under `key`, debounced, flushing on unmount.
 *
 * Storage failures — quota, a restricted context — are swallowed on purpose.
 * What is on screen is still there and still editable; throwing an error box
 * over the top of somebody's notes does not save any of them.
 */
export function usePersisted(key: string, value: string, delayMs = 300): void {
  // Read through a ref so the unmount effect can see the latest value without
  // re-registering — and therefore re-running its cleanup — on every keystroke.
  const latest = useRef(value);
  latest.current = value;

  useEffect(() => {
    const timer = setTimeout(() => write(key, value), delayMs);
    return () => clearTimeout(timer);
  }, [key, value, delayMs]);

  // Runs once, on unmount, with whatever the last value was.
  useEffect(() => {
    return () => write(key, latest.current);
  }, [key]);
}

/** Read a string back, or the fallback if storage is unavailable or empty. */
export function readPersisted(key: string, fallback = ""): string {
  try {
    return localStorage.getItem(key) ?? fallback;
  } catch {
    return fallback;
  }
}

/**
 * The same, for anything JSON-shaped.
 *
 * `validate` decides whether what came back is still usable — a shape written
 * by an older version, or a file somebody edited by hand, must not reach the
 * UI as `undefined` and render as a crash.
 */
export function readPersistedJson<T>(key: string, validate: (value: unknown) => T | null): T | null {
  try {
    const raw = localStorage.getItem(key);
    if (!raw) return null;
    return validate(JSON.parse(raw));
  } catch {
    return null;
  }
}

export function usePersistedJson(key: string, value: unknown, delayMs = 300): void {
  const latest = useRef(value);
  latest.current = value;

  useEffect(() => {
    const timer = setTimeout(() => writeJson(key, value), delayMs);
    return () => clearTimeout(timer);
  }, [key, value, delayMs]);

  useEffect(() => {
    return () => writeJson(key, latest.current);
  }, [key]);
}

function write(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // See the note above: nothing useful to do, and a lot of harm available.
  }
}

function writeJson(key: string, value: unknown): void {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // As above.
  }
}
