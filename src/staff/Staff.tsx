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
import { CaduceusMark } from "@/shared/CaduceusMark";
import { Onboarding, type OnboardingSignals } from "./Onboarding";
import { OnboardingQuiz } from "./OnboardingQuiz";
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

/**
 * How long a second click still counts as a double-click.
 *
 * Longer than macOS's own default, on purpose: the first click of the pair has
 * already opened the Command Center, and the panel appearing and taking key
 * status costs real milliseconds that come out of the user's budget for the
 * second click. At 280ms the double-click-for-dictation gesture worked about as
 * often as it did not.
 */
const DOUBLE_CLICK_MS = 450;

// Expand time is felt as latency, not as polish: the ring is not usable until
// it lands. 260ms plus a 24ms stagger meant the sixth icon arrived 380ms after
// the pointer did, on top of however long the tracker took to notice.
const POPOUT_EXPAND_MS = 160;
const POPOUT_FADE_MS = 90;
/** Per-icon delay. Six icons, so this multiplies by five on the last one. */
const POPOUT_STAGGER_MS = 10;

export function Staff() {
  const { settings, reload } = useSettings();
  const [hover, setHover] = useState<StaffHoverState>({ hovering: false, expanded: false });
  const [side, setSide] = useState<"left" | "right">("right");
  const [busyId, setBusyId] = useState<string | null>(null);
  const [flash, setFlash] = useState<{ text: string; ok: boolean } | null>(null);
  /** Keep icons on the arc while fading out — never slide them back to the staff. */
  const [arcHeld, setArcHeld] = useState(false);
  // Which pop-out the pointer is over, for the name chip below the arc. Held by
  // id rather than index so re-ordering mid-hover cannot mislabel one.
  const [namedId, setNamedId] = useState<string | null>(null);
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

    // Single click toggles the Command Center; double-click also starts
    // dictation (F1 does the same).
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

    void api.toggleCommandCenter("staff");
    doubleClickWindow.current = setTimeout(() => {
      doubleClickWindow.current = null;
    }, DOUBLE_CLICK_MS);
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
      } else if (outcome.frontendAction?.startsWith("open_feature:")) {
        // The staff has no tabs of its own; the Command Center owns the page.
        await api.openCommandCenter(
          `feature:${outcome.frontendAction.slice("open_feature:".length)}`,
        );
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

  useEffect(() => {
    if (!settings) return;
    void api.refreshStaffLayout();
  }, [settings]);

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

  if (!settings) {
    return (
      <div className="relative h-full w-full overflow-hidden">
        <div className="absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 opacity-90">
          <CaduceusMark height={72} className="drop-shadow-[0_2px_6px_rgb(0_0_0/0.55)]" />
        </div>
      </div>
    );
  }

  const { popoutRadius, popoutIconSize, staffIdleOpacity, staffIdleAnimation } =
    settings.appearance;
  const staffSize = liveStaffSize ?? settings.appearance.staffSize;
  // Not gated on `!hover.expanded`. The pop-out expands on the same tick as
  // hover at the default delay of 0ms, so that condition was never true while
  // the pointer was on the mark and the knobs could not be reached at all. The
  // knob square sits ~72px out at the default size and the pop-out icons ~96px,
  // so both can be on screen without fighting — and the tracker already widens
  // its capture radius to `resize_reach` whenever hovering, which only makes
  // sense if the knobs are visible then.
  const showResize = hover.hovering || resizing;
  const expanded = hover.expanded;
  const onArc = expanded || arcHeld;
  const named = expanded ? staffShortcuts.find((s) => s.id === namedId) : undefined;
  const fadingOut = arcHeld && !expanded;
  const arcSide = onArc ? sideWhileExpanded.current : side;
  const arcRadius = onArc ? popoutRadiusWhileExpanded.current || popoutRadius : popoutRadius;

  return (
    <div className="relative h-full w-full overflow-hidden">
      {/* Everything is positioned from the exact centre of the window. */}
      <div className="absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2">
        {/* --- the hovered shortcut's name ------------------------------
            Below the arc rather than beside the icon: the window is only as
            wide as the arc is, so a label placed outward would be clipped by
            the window edge, and one placed inward would sit on the staff. A
            fixed spot under the arc is always inside, and always in the same
            place, so the eye learns where to look. */}
        <div
          aria-hidden={!named}
          style={{
            // Clamped: the arc radius goes to 132 and the window is only 340
            // tall, so `radius + gap` alone puts the chip's lower half through
            // the bottom edge at the largest setting.
            transform: `translate(-50%, calc(-50% + ${Math.min(arcRadius + 30, 138)}px))`,
            opacity: named ? 1 : 0,
          }}
          className={cx(
            "pointer-events-none absolute left-0 top-0 max-w-[190px] truncate rounded-full px-2.5 py-1",
            "staff-popout shadow-float text-[11px] font-medium leading-none text-ink",
            "transition-opacity duration-150 ease-cad",
          )}
        >
          {named?.label}
        </div>

        {/* --- pop-out icons ------------------------------------------- */}
        {staffShortcuts.map((shortcut, index) => {
          const { x, y } = arcPosition(index, staffShortcuts.length, arcSide, arcRadius);
          const isBusy = busyId === shortcut.id;
          const visible = expanded && !fadingOut;
          const caption = shortcutCaption(shortcut);

          return (
            // The arc transform used to live on the button itself, centred by a
            // `-50%` that is relative to the button's own box. It now lives on
            // this wrapper instead, so the caption rides along with the icon as
            // it flies in and out. Positioning by an explicit pixel offset
            // instead of a `-50%` transform — the arc point minus half the
            // icon's own size — puts the wrapper's top-left exactly where the
            // button's top-left used to land, so the icon is pixel-identical to
            // before. That still matters even though the caption chip below is
            // no longer a flow sibling: the wrapper is deliberately left sized
            // to the button alone (the chip is `absolute`, so it does not add to
            // the wrapper's box), which means a `-50%` transform here would once
            // again be safe today — but pixel maths costs nothing extra and
            // stays correct even if a future flow child changes that. The
            // wrapper never takes pointer events — only the button re-enables
            // them, and only while expanded — so wrapping it can never widen the
            // staff's click-through hole beyond what the cursor tracker in
            // `src-tauri/src/window/mod.rs` already carves out for it.
            <div
              key={shortcut.id}
              style={{
                transform: onArc
                  ? `translate(${x - popoutIconSize / 2}px, ${y - popoutIconSize / 2}px)`
                  : `translate(${-popoutIconSize / 2}px, ${-popoutIconSize / 2}px) scale(0.5)`,
                opacity: visible ? 1 : 0,
                transitionProperty: fadingOut ? "opacity" : "transform, opacity",
                transitionDuration: fadingOut
                  ? `${POPOUT_FADE_MS}ms`
                  : `${POPOUT_EXPAND_MS}ms`,
                transitionDelay: expanded && !fadingOut ? `${index * POPOUT_STAGGER_MS}ms` : "0ms",
                pointerEvents: "none",
              }}
              className="absolute left-0 top-0 ease-cad"
              onTransitionEnd={(e) => {
                if (fadingOut && e.propertyName === "opacity") releaseArcLayout();
              }}
            >
              <button
                type="button"
                title={`${shortcut.label}${shortcut.description ? ` — ${shortcut.description}` : ""}`}
                aria-label={shortcut.label}
                onClick={() => void runShortcut(shortcut)}
                onPointerEnter={() => setNamedId(shortcut.id)}
                onPointerLeave={() => setNamedId((current) => (current === shortcut.id ? null : current))}
                style={{
                  width: popoutIconSize,
                  height: popoutIconSize,
                  pointerEvents: expanded ? "auto" : "none",
                }}
                className={cx(
                  "group flex shrink-0 items-center justify-center rounded-full",
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

              {/* --- destination caption ------------------------------------
                  What clicking this button actually does, in the fewest honest
                  characters: a URL's host, an app's name, or the shortcut's own
                  label for everything else (see `shortcutCaption` below).
                  This window is transparent and click-through, so bare text
                  drawn in normal flow renders straight over the user's
                  wallpaper — muted grey on an arbitrary desktop is unreadable
                  no matter the weight or size. The fix is to give it its own
                  opaque surface: the same `.staff-popout` chip material the
                  button itself is drawn in, so the two read as one object
                  rather than a label floating separately underneath.
                  Absolutely positioned rather than a flow sibling — with the
                  button as the wrapper's only flow child, the wrapper's box is
                  exactly the button's box, so this can hang its top a few
                  pixels above the button's bottom edge and read as "set into"
                  the ring instead of hovering below it. `pointer-events-none`
                  keeps it from ever intercepting a click meant for the button
                  underneath — the button re-enables its own pointer events
                  independently — and the wider `max-w` (roughly a hostname's
                  worth) still truncates rather than wraps, so a long one can
                  never widen the ring or push a neighbour off its spot on the
                  arc. `POPOUT_EDGE_MARGIN` in
                  `src-tauri/src/window/mod.rs::staff_window_side` reserves
                  enough window to keep this chip from clipping at the edge. */}
              <span
                aria-hidden="true"
                title={caption}
                style={{ top: popoutIconSize - 8 }}
                className={cx(
                  "staff-popout shadow-float pointer-events-none absolute left-1/2 z-10",
                  "-translate-x-1/2 max-w-[116px] truncate rounded-full px-2 py-0.5",
                  "text-center text-xs font-medium leading-none text-ink",
                )}
              >
                {caption}
              </span>
            </div>
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

          {(hover.hovering || hover.expanded) && (
            <button
              type="button"
              aria-label="Hide staff"
              title="Hide staff"
              onClick={() => void api.toggleStaff()}
              style={{
                top: staffSize * -0.08,
                right: staffSize * -0.08,
              }}
              className={cx(
                "absolute z-[60] flex h-[18px] w-[18px] items-center justify-center rounded-full",
                "border border-black/25 bg-base/90 text-[10px] leading-none text-ink-soft shadow-sm",
                "transition-opacity hover:bg-raised hover:text-ink",
              )}
            >
              ✕
            </button>
          )}

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

      {settings.general.onboardingQuizDone === false && (
        <OnboardingQuiz
          staffSize={staffSize}
          settings={settings}
          onComplete={() => void reload()}
        />
      )}

      {settings.general.onboardingQuizDone !== false && settings.general.onboardingDone === false && (
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

/** A shortcut's own name, falling back to its Command Center subtitle, falling
 * back to a neutral placeholder for the one case both can be blank: a
 * hand-edited shortcut whose fields were never filled in. */
function shortcutNameOrDescription(shortcut: Shortcut): string {
  return shortcut.label.trim() || shortcut.description.trim() || "Shortcut";
}

/**
 * The host a URL shortcut opens, with the scheme and any path dropped.
 *
 * `target` is not always a well-formed absolute URL by itself — a search
 * shortcut's target is a template like `https://google.com/search?q={query}`,
 * and a handful of legacy shortcuts store a bare host with no scheme at all —
 * so a missing scheme is assumed to be `https://` before parsing, and a
 * `target` the URL parser still rejects (empty, or not URL-shaped) yields
 * `null` rather than throwing, so the caller can fall back to something honest.
 */
function hostnameFromUrl(target: string): string | null {
  const trimmed = target.trim();
  if (!trimmed) return null;
  const withScheme = /^[a-z][a-z0-9+.-]*:\/\//i.test(trimmed) ? trimmed : `https://${trimmed}`;
  try {
    return new URL(withScheme).hostname || null;
  } catch {
    return null;
  }
}

/**
 * The application name a launch shortcut's `target` implies, or `null` when
 * the target does not carry one a client can read honestly.
 *
 * A filesystem path ending `.app` names itself (`/Applications/Google
 * Chrome.app` → `Google Chrome`), and so does a bare executable
 * (`google-chrome`, `chrome.exe`). A macOS bundle id (`com.google.Chrome`)
 * does not — resolving one to "Google Chrome" needs Launch Services, which
 * means a round trip to Rust for every shortcut on every render, and this is
 * a caption, not worth the cost. `null` here means "ask the caller's fallback
 * instead", which is the shortcut's own label — already the human name
 * someone chose when they made the shortcut.
 */
function appNameFromTarget(target: string): string | null {
  const trimmed = target.trim();
  if (!trimmed) return null;
  const appBundle = trimmed.match(/([^/\\]+)\.app[/\\]?$/i);
  if (appBundle) return appBundle[1];
  const looksLikeBundleId = trimmed.includes(".") && !trimmed.includes("/") && !trimmed.includes("\\");
  if (looksLikeBundleId) return null;
  const base = trimmed.split(/[/\\]/).pop() ?? trimmed;
  return base.replace(/\.exe$/i, "");
}

/**
 * The small caption drawn under each pop-out icon: what the button actually
 * links to, not what it is called.
 *
 * A URL shortcut names its host (`gemini.google.com`), an app shortcut names
 * the application, and everything else — a shell command, an AppleScript, a
 * built-in feature page — falls back to the shortcut's own label or its
 * Command Center subtitle, both of which are already short and written by a
 * person for exactly this purpose.
 */
export function shortcutCaption(shortcut: Shortcut): string {
  switch (shortcut.kind) {
    case "open_url":
      return hostnameFromUrl(shortcut.target) ?? shortcutNameOrDescription(shortcut);
    case "open_app":
      return appNameFromTarget(shortcut.target) ?? shortcutNameOrDescription(shortcut);
    default:
      return shortcutNameOrDescription(shortcut);
  }
}
