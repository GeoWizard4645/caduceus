/**
 * Light / dark toggle for any Caduceus window — same behaviour as the website
 * control, but persisted through Settings → Appearance.
 */

import { useState } from "react";

import * as api from "@/shared/api";
import { useSettings } from "@/shared/hooks";
import { resolvedAppearanceMode, toggleAppearanceTheme } from "@/shared/theme";
import type { Theme } from "@/shared/types";
import { cx } from "@/shared/ui";

export function ThemeToggle({ className }: { className?: string }) {
  const { settings } = useSettings();
  const [busy, setBusy] = useState(false);

  if (!settings) return null;

  const mode = resolvedAppearanceMode(settings.appearance.theme);
  const label = mode === "light" ? "Switch to dark mode" : "Switch to light mode";

  const onToggle = () => {
    if (busy) return;
    const nextTheme: Theme = toggleAppearanceTheme(settings.appearance.theme);
    const next = structuredClone(settings);
    next.appearance.theme = nextTheme;
    setBusy(true);
    void api
      .updateSettings(next)
      .catch(() => {
        /* useSettings reloads on success via settings-changed; ignore toast noise */
      })
      .finally(() => setBusy(false));
  };

  return (
    <button
      type="button"
      className={cx(
        "no-drag inline-flex shrink-0 overflow-hidden rounded-full border border-line bg-raised p-0.5",
        "text-[13px] leading-none transition-colors hover:border-line-strong disabled:opacity-60",
        className,
      )}
      aria-label={label}
      title={label}
      disabled={busy}
      onClick={onToggle}
    >
      <span
        className={cx(
          "rounded-full px-2 py-1 transition-colors",
          mode === "light" ? "bg-accent/15 text-accent" : "text-ink-faint",
        )}
        aria-hidden="true"
      >
        ☀
      </span>
      <span
        className={cx(
          "rounded-full px-2 py-1 transition-colors",
          mode === "dark" ? "bg-accent/15 text-accent" : "text-ink-faint",
        )}
        aria-hidden="true"
      >
        ☾
      </span>
    </button>
  );
}
