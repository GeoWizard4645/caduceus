/**
 * The shell every widget window shares: pixel-panel chrome (drag strip,
 * remove button, resize grip) around whichever content its `kind` selects.
 *
 * Content is deliberately dumb about layout — a widget's Rust-side window is
 * already sized and positioned to match its own content by the time this
 * mounts (see `widgets.rs::spawn_widget_window`), so nothing here measures or
 * centres itself against the screen the way the staff does.
 */

import { useMemo } from "react";

import { cx } from "@/shared/ui";

import { ClockWidget } from "./ClockWidget";
import { MarketWidget } from "./MarketWidget";
import { SportsWidget } from "./SportsWidget";
import { WidgetChrome, WidgetResizeGrip } from "./WidgetChrome";
import type { WidgetLayout } from "./types";

function WidgetContent({ kind }: { kind: string }) {
  // "market:..." and "sports:..." carry their own config in the rest of the
  // string (which tickers, which league/team) — see the encode/parse pair in
  // marketApi.ts for why that lives here instead of a new persisted field.
  // Prefix match, not exact match, is what makes that possible while keeping
  // this switch the single place `kind` is dispatched from.
  if (kind.startsWith("market:")) {
    return <MarketWidget kind={kind} />;
  }
  if (kind.startsWith("sports:")) {
    return <SportsWidget kind={kind} />;
  }

  switch (kind) {
    case "clock":
      return <ClockWidget />;
    default:
      // A kind this build does not recognise — e.g. a layout saved by a
      // newer version. Saying so beats rendering nothing and leaving the
      // user staring at an empty floating panel with no explanation.
      return (
        <span className="px-2 text-center text-2xs text-ink-faint">
          Unknown widget “{kind}”
        </span>
      );
  }
}

export function WidgetApp() {
  // Set once, before this mounts, by the init script `spawn_widget_window`
  // attaches when the window is built — read once rather than watched, since
  // a widget's `kind` never changes for the lifetime of its window.
  const init = useMemo<WidgetLayout | undefined>(() => window.__CADUCEUS_WIDGET__, []);

  if (!init) {
    // Only reachable if `widget.html` were loaded some other way than
    // through `spawn_widget_window`, which is the sole place that sets
    // `__CADUCEUS_WIDGET__`. Nothing to render without it.
    return null;
  }

  return (
    <div className="relative h-full w-full overflow-hidden">
      <div
        className={cx(
          "glass shadow-panel flex h-full w-full flex-col overflow-hidden",
          "rounded-cad border border-line",
        )}
      >
        <WidgetChrome id={init.id} />
        <div className="no-drag flex flex-1 items-center justify-center overflow-hidden px-2 pb-2">
          <WidgetContent kind={init.kind} />
        </div>
      </div>
      <WidgetResizeGrip />
    </div>
  );
}
