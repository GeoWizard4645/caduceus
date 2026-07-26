/**
 * The floating staff and its radial pop-out.
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
import { StaffMark } from "@/shared/StaffMark";
import { Onboarding, type OnboardingSignals } from "./Onboarding";
import { StaffResizeFrame } from "./StaffResize";
import { useSettings, useTauriEvent } from "@/shared/hooks";
import { ShortcutIcon } from "@/shared/ShortcutIcon";
import { cx } from "@/shared/ui";
import type { StaffHoverState, Shortcut, VoiceState } from "@/shared/types";
import { EVENTS, STAFF_POPOUT_LIMIT } from "@/shared/types";

/** How far the arc spreads either side of straight-out-from-the-edge. */
const ARC_SPREAD_DEG = 76;

/** Pointer travel, in px, before a press becomes a drag rather than a click. */
const DRAG_THRESHOLD = 4;

// Expand time is felt as latency, not as polish: the ring is not usable until
// it lands. 260ms plus a 24ms stagger meant the sixth icon arrived 380ms after
// the pointer did, on top of however long the tracker took to notice.
const POPOUT_EXPAND_MS = 160;
const POPOUT_FADE_MS = 90;
/** Per-icon delay. Six icons, so this multiplies by five on the last one. */
const POPOUT_STAGGER_MS = 10;

export function Staff() {
  const { settings } = useSettings();
  const [hover, setHover] = useState<StaffHoverState>({ hovering: false, expanded: false });
  const [side, setSide] = useState<"left" | "right">("right");
  const [busyId, setBusyId] = useState<string | null>(null);
  const [flash, setFlash] = useState<{ text: string; ok: boolean } | null>(null);
  /** Keep icons on the arc while fading out — never slide them back to the staff. */
  const [arcHeld, setArcHeld] = useState(false);
  const [voice, setVoice] = useState<VoiceState>("idle");
  /** Live size while a corner-drag is in progress; null means use settings. */
  const [liveStaffSize, setLiveStaffSize] = useState<number | null>(null);
  const [resizing, setResizing] = useState(false);
  // First-run walkthrough. Steps complete on the real interaction, so the staff
  // records what has actually happened rather than what has been read.
  const [signals, setSignals] = useState<OnboardingSignals>({
    hovered: false,
    expanded: false,
    commandCenterOpened: false,
    hotkeyUsed: false,
  });
  const sideWhileExpanded = useRef<"left" | "right">("right");
  const popoutRadiusWhileExpanded = useRef(0);
  const settingsRef = useRef(settings);
  settingsRef.current = settings;

  useTauriEvent<StaffHoverState>(EVENTS.staffHover, setHover);
  // The staff is the only Caduceus surface always on screen, so it is where
  // "your microphone is live" has to be visible — the Command Center can be
  // behind another window or scrolled off a second display.
  useTauriEvent<VoiceState>(EVENTS.voiceState, setVoice);

  useTauriEvent<string>(EVENTS.commandCenterShown, (source) => {
    setSignals((current) => ({
      ...current,
      commandCenterOpened: true,
      hotkeyUsed: current.hotkeyUsed || source === "hotkey",
    }));
  });

  useEffect(() => {
    if (!hover.hovering && !hover.expanded) return;
    setSignals((current) =>
      current.hovered && current.expanded
        ? current
        : { ...current, hovered: true, expanded: current.expanded || hover.expanded },
    );
  }, [hover.hovering, hover.expanded]);

  useEffect(() => {
    if (hover.expanded) {
      setArcHeld(true);
      sideWhileExpanded.current = side;
      if (settings) popoutRadiusWhileExpanded.current = settings.appearance.popoutRadius;
    }
  }, [hover.expanded, side, settings]);

  // Which way the arc opens depends on where the staff currently sits, not on
  // the saved edge preference — the user may have dragged it across the screen.
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

  const staffShortcuts = useMemo<Shortcut[]>(() => {
    if (!settings) return [];
    return settings.shortcuts
      .filter((s) => s.showInStaff)
      .sort((a, b) => a.orderIndex - b.orderIndex)
      .slice(0, STAFF_POPOUT_LIMIT);
  }, [settings]);

  // --- drag vs click -------------------------------------------------------
  const pressOrigin = useRef<{ x: number; y: number } | null>(null);
  const dragging = useRef(false);
  /** Open while a second click would still count as a double-click. */
  const doubleClickWindow = useRef<ReturnType<typeof setTimeout> | null>(null);

  const onPointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0 || resizing) return;
    pressOrigin.current = { x: e.screenX, y: e.screenY };
    dragging.current = false;
  };

  const onPointerMove = (e: React.PointerEvent) => {
    if (resizing) return;
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
            void api.saveStaffPosition();
            void recomputeSide();
          }, 120);
        });
    }
  };

  const onPointerUp = () => {
    const wasDrag = dragging.current;
    pressOrigin.current = null;
    dragging.current = false;
    if (wasDrag || resizing) return;

    // Single click opens the Command Center; double-click also starts dictation
    // (F1 does the same).
    //
    // The first click acts immediately rather than waiting out a double-click
    // window. Deferring it made every single click feel ~280ms slow, and there
    // is nothing to undo: starting dictation opens the Command Center too, so a
    // second click just adds the microphone to a window that is already up.
    if (doubleClickWindow.current) {
      clearTimeout(doubleClickWindow.current);
      doubleClickWindow.current = null;
      void api.toggleDictation();
      return;
    }

    void api.openCommandCenter(undefined, undefined, "staff");
    doubleClickWindow.current = setTimeout(() => {
      doubleClickWindow.current = null;
    }, 280);
  };

  useEffect(
    () => () => {
      if (doubleClickWindow.current) clearTimeout(doubleClickWindow.current);
    },
    [],
  );

  const runShortcut = async (shortcut: Shortcut) => {
    void api.collapseStaffPopout();
    setBusyId(shortcut.id);
    try {
      const outcome = await api.runShortcut(shortcut.id);
      if (outcome.frontendAction === "clipboard_view") {
        await api.openCommandCenter("clipboard");
      } else if (outcome.frontendAction === "system_monitor") {
        await api.openCommandCenter("system");
      } else if (!outcome.ok) {
        setFlash({ text: outcome.message, ok: false });
      }
    } catch (error) {
      setFlash({ text: api.errorMessage(error), ok: false });
    } finally {
      setBusyId(null);
    }
  };

  const releaseArcLayout = () => {
    setArcHeld(false);
  };

  // Errors are shown briefly on the staff itself: there is no other surface
  // here, and silently doing nothing is the worst possible outcome.
  useEffect(() => {
    if (!flash) return;
    const timer = setTimeout(() => setFlash(null), 4200);
    return () => clearTimeout(timer);
  }, [flash]);

  useEffect(() => {
    if (!arcHeld || hover.expanded) return;
    const timer = setTimeout(releaseArcLayout, POPOUT_FADE_MS + 50);
    return () => clearTimeout(timer);
  }, [arcHeld, hover.expanded]);

  // During a resize drag the pointer can leave the mark's hit circle; keep the
  // window capturing until the gesture ends. (Hover alone uses a wider radius
  // in Rust so the corner knobs stay hittable without swallowing the whole pad.)
  useEffect(() => {
    if (!settings || !resizing) return;
    void api.setStaffInteractive(true);
    return () => {
      void api.setStaffInteractive(false);
    };
  }, [resizing, settings]);

  const commitStaffSize = useCallback((next: number) => {
    const current = settingsRef.current;
    if (!current || current.appearance.staffSize === next) {
      setLiveStaffSize(null);
      return;
    }
    void api
      .updateSettings({
        ...current,
        appearance: { ...current.appearance, staffSize: next },
      })
      .finally(() => setLiveStaffSize(null));
  }, []);

  if (!settings) return null;

  const { popoutRadius, popoutIconSize, staffIdleOpacity, staffIdleAnimation } =
    settings.appearance;
  const staffSize = liveStaffSize ?? settings.appearance.staffSize;
  const showResize = (hover.hovering || resizing) && !hover.expanded;
  const expanded = hover.expanded;
  const onArc = expanded || arcHeld;
  const fadingOut = arcHeld && !expanded;
  const arcSide = onArc ? sideWhileExpanded.current : side;
  const arcRadius = onArc ? popoutRadiusWhileExpanded.current || popoutRadius : popoutRadius;

  return (
    <div className="relative h-full w-full overflow-hidden">
      {/* Everything is positioned from the exact centre of the window. */}
      <div className="absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2">
        {/* --- pop-out icons ------------------------------------------- */}
        {staffShortcuts.map((shortcut, index) => {
          const { x, y } = arcPosition(index, staffShortcuts.length, arcSide, arcRadius);
          const isBusy = busyId === shortcut.id;
          const visible = expanded && !fadingOut;

          return (
            <button
              key={shortcut.id}
              type="button"
              title={`${shortcut.label}${shortcut.description ? ` — ${shortcut.description}` : ""}`}
              aria-label={shortcut.label}
              onClick={() => void runShortcut(shortcut)}
              onTransitionEnd={(e) => {
                if (fadingOut && e.propertyName === "opacity") releaseArcLayout();
              }}
              style={{
                width: popoutIconSize,
                height: popoutIconSize,
                transform: onArc
                  ? `translate(calc(-50% + ${x}px), calc(-50% + ${y}px))`
                  : "translate(-50%, -50%) scale(0.5)",
                opacity: visible ? 1 : 0,
                transitionProperty: fadingOut ? "opacity" : "transform, opacity",
                transitionDuration: fadingOut
                  ? `${POPOUT_FADE_MS}ms`
                  : `${POPOUT_EXPAND_MS}ms`,
                transitionDelay: expanded && !fadingOut ? `${index * POPOUT_STAGGER_MS}ms` : "0ms",
                pointerEvents: expanded ? "auto" : "none",
              }}
              className={cx(
                "group absolute left-0 top-0 flex items-center justify-center rounded-full",
                "staff-popout shadow-float",
                "text-[15px] leading-none text-ink ease-cad",
                "focus-visible:ring-2 focus-visible:ring-accent",
                isBusy && "animate-pulse",
              )}
            >
              <span
                className={cx(
                  "pointer-events-none flex h-[62%] w-[62%] select-none items-center justify-center",
                  expanded && "transition-transform duration-150 ease-cad group-hover:scale-110",
                )}
              >
                <ShortcutIcon
                  icon={shortcut.icon}
                  label={shortcut.label}
                  className="h-full w-full text-[15px]"
                  imgClassName="rounded-sm"
                />
              </span>
            </button>
          );
        })}

        {/* --- the staff ------------------------------------------------ */}
        <div className="absolute left-0 top-0 z-50 -translate-x-1/2 -translate-y-1/2">
          <StaffResizeFrame
            size={staffSize}
            visible={showResize}
            onLiveSize={setLiveStaffSize}
            onCommit={commitStaffSize}
            onResizingChange={setResizing}
          />

          <button
            type="button"
            aria-label="Open the Caduceus Command Center"
            onPointerDown={onPointerDown}
            onPointerMove={onPointerMove}
            onPointerUp={onPointerUp}
            onContextMenu={(e) => {
              e.preventDefault();
              void api.openSettingsWindow();
            }}
            style={{ opacity: expanded || hover.hovering || voice !== "idle" ? 1 : staffIdleOpacity }}
            className={cx(
              "group relative flex items-center justify-center",
              "transition-[opacity,transform] duration-150 ease-cad",
              !resizing && "hover:scale-[1.08] active:scale-[0.96]",
              "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2",
            )}
          >
            {/* Accent bloom behind the mark, so it stays visible against a busy
                desktop without the mark itself having to be opaque. */}
            <span
              aria-hidden="true"
              className={cx(
                "absolute rounded-full transition-opacity duration-200 ease-cad",
                expanded ? "opacity-100" : "opacity-70",
                staffIdleAnimation && !expanded && !resizing && "animate-staff-pulse",
              )}
              style={{
                width: staffSize * 1.5,
                height: staffSize * 1.5,
                background:
                  "radial-gradient(circle, rgb(var(--c-accent) / 0.30) 0%, rgb(var(--c-accent) / 0) 68%)",
              }}
            />

            <StaffMark
              height={staffSize}
              icon={settings.appearance.staffMarkIcon}
              className="relative drop-shadow-[0_2px_6px_rgb(0_0_0/0.55)]"
            />

            {/* Recording tell. Red rather than the accent colour on purpose: the
                accent is used all over the staff for ordinary state, and "the mic
                is on" should never be mistakable for any of it. */}
            {voice !== "idle" && (
              <span
                aria-hidden="true"
                className="absolute -right-0.5 -top-0.5 flex h-3 w-3"
                style={{ transform: `translate(${staffSize * 0.18}px, ${staffSize * -0.18}px)` }}
              >
                {voice === "recording" && (
                  <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-[#ff3b30] opacity-75" />
                )}
                <span
                  className={cx(
                    "relative inline-flex h-3 w-3 rounded-full border border-black/30 bg-[#ff3b30]",
                    "shadow-[0_0_8px_rgb(255_59_48/0.9)]",
                    voice === "transcribing" && "animate-pulse opacity-70",
                  )}
                />
              </span>
            )}
          </button>
        </div>
      </div>

      {settings.general.onboardingDone === false && (
        <Onboarding
          signals={signals}
          settings={settings}
          staffSize={staffSize}
          onFinish={() =>
            void api.updateSettings({
              ...settings,
              general: { ...settings.general, onboardingDone: true },
            })
          }
        />
      )}

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
 * edge the staff is docked to.
 *
 * A right-docked staff fans its icons out to the left (centred on 180°), a
 * left-docked staff to the right (centred on 0°).
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
