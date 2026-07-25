/**
 * The floating orb and its radial pop-out.
 *
 * The window is a fixed 340×340 transparent square; everything visible is drawn
 * here, centred. Hover and collapse state arrive from Rust (see
 * `src-tauri/src/window/mod.rs`) rather than from DOM events, because the
 * webview stops receiving pointer events the moment the cursor leaves the
 * window — which is precisely when "collapse after N seconds" needs to be timed.
 */

import { currentMonitor, getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import * as api from "@/shared/api";
import { useSettings, useTauriEvent } from "@/shared/hooks";
import { cx } from "@/shared/ui";
import type { OrbHoverState, Shortcut } from "@/shared/types";
import { EVENTS, ORB_POPOUT_LIMIT } from "@/shared/types";

/** How far the arc spreads either side of straight-out-from-the-edge. */
const ARC_SPREAD_DEG = 76;

/** Pointer travel, in px, before a press becomes a drag rather than a click. */
const DRAG_THRESHOLD = 4;

export function Orb() {
  const { settings } = useSettings();
  const [hover, setHover] = useState<OrbHoverState>({ hovering: false, expanded: false });
  const [side, setSide] = useState<"left" | "right">("right");
  const [busyId, setBusyId] = useState<string | null>(null);
  const [flash, setFlash] = useState<{ text: string; ok: boolean } | null>(null);

  useTauriEvent<OrbHoverState>(EVENTS.orbHover, setHover);

  // Which way the arc opens depends on where the orb currently sits, not on the
  // saved edge preference — the user may have dragged it across the screen.
  const recomputeSide = useCallback(async () => {
    try {
      const window = getCurrentWindow();
      const [position, size, monitor] = await Promise.all([
        window.outerPosition(),
        window.outerSize(),
        currentMonitor(),
      ]);
      if (!monitor) return;
      const centre = position.x + size.width / 2;
      const monitorCentre = monitor.position.x + monitor.size.width / 2;
      setSide(centre > monitorCentre ? "right" : "left");
    } catch {
      // Monitor information is unavailable in some environments; the default
      // (right) is still perfectly usable.
    }
  }, []);

  useEffect(() => {
    void recomputeSide();
  }, [recomputeSide]);

  const orbShortcuts = useMemo<Shortcut[]>(() => {
    if (!settings) return [];
    return settings.shortcuts
      .filter((s) => s.showInOrb)
      .sort((a, b) => a.orderIndex - b.orderIndex)
      .slice(0, ORB_POPOUT_LIMIT);
  }, [settings]);

  // --- drag vs click -------------------------------------------------------
  const pressOrigin = useRef<{ x: number; y: number } | null>(null);
  const dragging = useRef(false);

  const onPointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    pressOrigin.current = { x: e.screenX, y: e.screenY };
    dragging.current = false;
  };

  const onPointerMove = (e: React.PointerEvent) => {
    const origin = pressOrigin.current;
    if (!origin || dragging.current) return;
    const travelled = Math.hypot(e.screenX - origin.x, e.screenY - origin.y);
    if (travelled > DRAG_THRESHOLD) {
      dragging.current = true;
      // Hands the gesture to the window manager; the webview stops receiving
      // pointer events until the drag ends.
      void getCurrentWindow()
        .startDragging()
        .then(() => {
          // Persist where it was dropped, and re-evaluate the arc direction.
          setTimeout(() => {
            void api.saveOrbPosition();
            void recomputeSide();
          }, 120);
        });
    }
  };

  const onPointerUp = () => {
    const wasDrag = dragging.current;
    pressOrigin.current = null;
    dragging.current = false;
    if (!wasDrag) void api.openCommandCenter();
  };

  const runShortcut = async (shortcut: Shortcut) => {
    setBusyId(shortcut.id);
    try {
      const outcome = await api.runShortcut(shortcut.id);
      if (outcome.frontendAction === "clipboard_view") {
        await api.openCommandCenter("clipboard");
      } else if (!outcome.ok) {
        setFlash({ text: outcome.message, ok: false });
      }
    } catch (error) {
      setFlash({ text: api.errorMessage(error), ok: false });
    } finally {
      setBusyId(null);
    }
  };

  // Errors are shown briefly on the orb itself: there is no other surface here,
  // and silently doing nothing is the worst possible outcome.
  useEffect(() => {
    if (!flash) return;
    const timer = setTimeout(() => setFlash(null), 4200);
    return () => clearTimeout(timer);
  }, [flash]);

  if (!settings) return null;

  const { orbSize, popoutRadius, popoutIconSize, orbIdleOpacity, orbIdleAnimation } =
    settings.appearance;
  const expanded = hover.expanded;

  return (
    <div className="relative h-full w-full overflow-hidden">
      {/* Everything is positioned from the exact centre of the window. */}
      <div className="absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2">
        {/* --- pop-out icons ------------------------------------------- */}
        {orbShortcuts.map((shortcut, index) => {
          const { x, y } = arcPosition(index, orbShortcuts.length, side, popoutRadius);
          const isBusy = busyId === shortcut.id;

          return (
            <button
              key={shortcut.id}
              type="button"
              title={`${shortcut.label}${shortcut.description ? ` — ${shortcut.description}` : ""}`}
              aria-label={shortcut.label}
              onClick={() => void runShortcut(shortcut)}
              style={{
                width: popoutIconSize,
                height: popoutIconSize,
                // Icons animate out from the orb's centre, staggered so the
                // ring unfurls rather than snapping into place.
                transform: expanded
                  ? `translate(calc(-50% + ${x}px), calc(-50% + ${y}px)) scale(1)`
                  : "translate(-50%, -50%) scale(0.4)",
                opacity: expanded ? 1 : 0,
                transitionDelay: expanded ? `${index * 26}ms` : `${(orbShortcuts.length - index) * 12}ms`,
                pointerEvents: expanded ? "auto" : "none",
              }}
              className={cx(
                "absolute left-0 top-0 flex items-center justify-center rounded-full",
                "glass-raised shadow-float backdrop-blur-glass",
                "text-[15px] leading-none text-ink",
                "transition-[transform,opacity,box-shadow,background-color] duration-[260ms] ease-orbit",
                "hover:!scale-[1.14] hover:border-accent/40 hover:text-accent",
                "focus-visible:ring-2 focus-visible:ring-accent",
                isBusy && "animate-pulse",
              )}
            >
              <span className="pointer-events-none select-none">
                {shortcut.icon || shortcut.label.charAt(0)}
              </span>
            </button>
          );
        })}

        {/* --- the orb -------------------------------------------------- */}
        <button
          type="button"
          aria-label="Open Orbit Command Center"
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onContextMenu={(e) => {
            e.preventDefault();
            void api.openSettingsWindow();
          }}
          style={{
            width: orbSize,
            height: orbSize,
            opacity: expanded || hover.hovering ? 1 : orbIdleOpacity,
          }}
          className={cx(
            "group relative -translate-x-1/2 -translate-y-1/2 rounded-full",
            "transition-[opacity,transform] duration-300 ease-orbit",
            "hover:scale-[1.06] active:scale-[0.97]",
            "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2",
          )}
        >
          {/* Outer bloom. Sits behind the sphere and grows on hover. */}
          <span
            aria-hidden="true"
            className={cx(
              "absolute inset-[-38%] rounded-full transition-opacity duration-500 ease-orbit",
              expanded ? "opacity-100" : "opacity-55",
            )}
            style={{
              background:
                "radial-gradient(circle, rgb(var(--o-accent) / 0.34) 0%, rgb(var(--o-accent) / 0) 68%)",
            }}
          />

          {/* A slowly rotating orbital ring, echoing the app icon. */}
          <span
            aria-hidden="true"
            className={cx(
              "absolute inset-[-16%] rounded-full border transition-opacity duration-500",
              orbIdleAnimation && "animate-spin-slow",
              expanded ? "border-accent/45 opacity-100" : "border-accent/22 opacity-70",
            )}
            style={{ borderStyle: "solid", borderWidth: 1, transform: "rotateX(64deg)" }}
          />

          {/* The sphere itself. */}
          <span
            aria-hidden="true"
            className={cx(
              "absolute inset-0 rounded-full shadow-orb",
              orbIdleAnimation && !expanded && "animate-orb-breathe",
            )}
            style={{
              background:
                "radial-gradient(circle at 34% 30%, rgb(var(--o-accent) / 0.98) 0%, " +
                "rgb(var(--o-accent) / 0.82) 38%, rgb(var(--o-accent-soft)) 100%)",
              boxShadow:
                "inset 0 1px 1px rgb(255 255 255 / 0.42), inset 0 -6px 12px rgb(0 0 0 / 0.30), " +
                "0 6px 18px -4px rgb(var(--o-accent) / 0.45)",
            }}
          />

          {/* Specular highlight. */}
          <span
            aria-hidden="true"
            className="absolute rounded-full bg-white/45 blur-[2px]"
            style={{
              width: "26%",
              height: "18%",
              left: "22%",
              top: "20%",
              transform: "rotate(-22deg)",
            }}
          />
        </button>
      </div>

      {/* --- transient error ------------------------------------------- */}
      {flash && (
        <div
          className={cx(
            "absolute left-1/2 top-[calc(50%+62px)] w-[240px] -translate-x-1/2 animate-fade-rise",
            "glass rounded-lg px-3 py-2 text-2xs leading-relaxed shadow-float",
            flash.ok ? "text-ink-soft" : "text-danger",
          )}
        >
          {flash.text}
        </div>
      )}
    </div>
  );
}

/**
 * Position of the `index`-th icon on an arc that opens away from the screen
 * edge the orb is docked to.
 *
 * A right-docked orb fans its icons out to the left (centred on 180°), a
 * left-docked orb to the right (centred on 0°).
 */
export function arcPosition(
  index: number,
  count: number,
  side: "left" | "right",
  radius: number,
): { x: number; y: number } {
  const centre = side === "right" ? 180 : 0;
  // A single icon sits straight out; more than one spreads evenly across the arc.
  const step = count > 1 ? (ARC_SPREAD_DEG * 2) / (count - 1) : 0;
  const angle = centre - ARC_SPREAD_DEG + step * index;
  const radians = (angle * Math.PI) / 180;
  return {
    x: Math.cos(radians) * radius,
    y: Math.sin(radians) * radius,
  };
}
