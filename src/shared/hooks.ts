/**
 * Shared React hooks.
 *
 * Every Orbit window loads settings, listens for the change event, and applies
 * the theme. That is `useSettings`; the rest are small utilities that stop the
 * three windows from each inventing their own version.
 */

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";

import * as api from "./api";
import { applyAppearance, watchSystemTheme } from "./theme";
import type { Settings } from "./types";
import { EVENTS } from "./types";

/**
 * Load settings, keep them in sync, and apply the theme.
 *
 * `settings` is `null` only during the first tick, which every caller handles
 * by rendering nothing — the alternative (a default-shaped placeholder) causes
 * a visible flash of the wrong accent colour.
 */
export function useSettings(): {
  settings: Settings | null;
  reload: () => Promise<void>;
  error: string | null;
} {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const latest = useRef<Settings | null>(null);

  const apply = useCallback((next: Settings) => {
    latest.current = next;
    setSettings(next);
    applyAppearance(next.appearance);
  }, []);

  const reload = useCallback(async () => {
    try {
      apply(await api.getSettings());
      setError(null);
    } catch (e) {
      setError(api.errorMessage(e));
    }
  }, [apply]);

  useEffect(() => {
    void reload();

    let unlisten: UnlistenFn | undefined;
    void listen<Settings>(EVENTS.settingsChanged, (event) => apply(event.payload)).then((fn) => {
      unlisten = fn;
    });

    const stopThemeWatch = watchSystemTheme(
      () => latest.current?.appearance ?? { theme: "dark" } as Settings["appearance"],
    );

    return () => {
      unlisten?.();
      stopThemeWatch();
    };
  }, [apply, reload]);

  return { settings, reload, error };
}

/** Subscribe to a Tauri event for the lifetime of the component. */
export function useTauriEvent<T>(event: string, handler: (payload: T) => void): void {
  // Held in a ref so a caller passing an inline arrow does not re-subscribe on
  // every render.
  const callback = useRef(handler);
  callback.current = handler;

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;

    void listen<T>(event, (e) => callback.current(e.payload)).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [event]);
}

/** Debounce a value. Used to keep the palette from querying on every keystroke. */
export function useDebounced<T>(value: T, delayMs: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const timer = setTimeout(() => setDebounced(value), delayMs);
    return () => clearTimeout(timer);
  }, [value, delayMs]);
  return debounced;
}

/** A transient message shown in a toast. */
export interface Toast {
  id: number;
  message: string;
  tone: "info" | "error";
}

export function useToasts(timeoutMs = 3200) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const nextId = useRef(1);

  const notify = useCallback(
    (message: string, tone: "info" | "error" = "info") => {
      const id = nextId.current++;
      setToasts((current) => [...current, { id, message, tone }]);
      // Errors stay longer: they usually contain something to act on.
      setTimeout(
        () => setToasts((current) => current.filter((t) => t.id !== id)),
        tone === "error" ? timeoutMs * 2 : timeoutMs,
      );
    },
    [timeoutMs],
  );

  const dismiss = useCallback((id: number) => {
    setToasts((current) => current.filter((t) => t.id !== id));
  }, []);

  return { toasts, notify, dismiss };
}

/** Read a value once on mount, with loading and error state. */
export function useAsync<T>(
  load: () => Promise<T>,
  deps: unknown[] = [],
): { data: T | null; loading: boolean; error: string | null; reload: () => void } {
  const [data, setData] = useState<T | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  const loadRef = useRef(load);
  loadRef.current = load;

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    loadRef
      .current()
      .then((value) => {
        if (!cancelled) {
          setData(value);
          setError(null);
        }
      })
      .catch((e) => {
        if (!cancelled) setError(api.errorMessage(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, nonce]);

  return { data, loading, error, reload: () => setNonce((n) => n + 1) };
}
