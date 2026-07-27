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
 *
 * # And the failure it makes visible
 *
 * `localStorage.setItem` throws once the origin's quota is full — one very
 * large paste is enough. Swallowing that leaves the page saying "Saved as you
 * type" while nothing is being saved, and the first anybody hears of it is a
 * restart with the note truncated to the last write that fit. So both hooks
 * report the failure back to the caller, whose job is to stop claiming the text
 * is safe.
 */

import { useEffect, useRef, useState } from "react";

/** What the user is told when a write did not fit. */
const FULL = "Could not save — this is too long for the app's storage. Copy it somewhere else.";

/**
 * Persist `value` under `key`, debounced, flushing on unmount.
 *
 * Returns a message to show when the last write failed, or `null` while it is
 * being saved normally.
 */
export function usePersisted(key: string, value: string, delayMs = 300): string | null {
  // Read through a ref so the unmount effect can see the latest value without
  // re-registering — and therefore re-running its cleanup — on every keystroke.
  const latest = useRef(value);
  latest.current = value;
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    const timer = setTimeout(() => setFailed(!write(key, value)), delayMs);
    return () => clearTimeout(timer);
  }, [key, value, delayMs]);

  // Runs once, on unmount, with whatever the last value was. Nothing to report
  // from here — there is no longer a page to report it to.
  useEffect(() => {
    return () => {
      write(key, latest.current);
    };
  }, [key]);

  return failed ? FULL : null;
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

/** As [`usePersisted`], including the failure message it returns. */
export function usePersistedJson(key: string, value: unknown, delayMs = 300): string | null {
  const latest = useRef(value);
  latest.current = value;
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    const timer = setTimeout(() => setFailed(!writeJson(key, value)), delayMs);
    return () => clearTimeout(timer);
  }, [key, value, delayMs]);

  useEffect(() => {
    return () => {
      writeJson(key, latest.current);
    };
  }, [key]);

  return failed ? FULL : null;
}

/** `false` if the write did not happen — quota, or a restricted context. */
function write(key: string, value: string): boolean {
  try {
    localStorage.setItem(key, value);
    return true;
  } catch {
    return false;
  }
}

function writeJson(key: string, value: unknown): boolean {
  try {
    localStorage.setItem(key, JSON.stringify(value));
    return true;
  } catch {
    return false;
  }
}
