/**
 * The PopBar: the whole "Highlight & Act" UI, in one small always-on-top
 * window. See `src-tauri/src/popbar.rs` for how it gets opened, positioned
 * and (mostly) dismissed — this file only owns what happens once it is on
 * screen.
 *
 * # Why this is a state machine over one fixed-size window, not six
 *
 * The window's frame never changes size (`popbar.rs::BAR_WIDTH/HEIGHT`) —
 * only what is drawn inside it does. `PopbarView` below is exactly the set
 * of things that can be true at once: the top-level menu, one of the two
 * submenus (Translate's languages, Rewrite's styles — presets standing in
 * for a text field the window is constitutionally unable to host, see the
 * Rust module docs), a running spinner, a done confirmation, or an inline
 * error. Exactly one renders at a time; there is no ambiguous "loading and
 * also showing stale results" state to accidentally reach.
 */

import { useEffect, useRef, useState } from "react";

import { cx, Spinner } from "@/shared/ui";
import type { TextAiAction } from "@/shared/types";

import { onPopbarShow, popbarDismiss, popbarPending, popbarRun } from "./popbarApi";
import type { PopbarMenuItem, PopbarShowPayload, PopbarSubmenuItem, PopbarView } from "./types";

// ---------------------------------------------------------------------------
// Menu contents
// ---------------------------------------------------------------------------

/** The six actions named in the brief, in the order they were named. Two of
 * them (`Translate`, `Rewrite`) are a style/language away from being a
 * single action, so they open a submenu instead of running immediately. */
const TOP_LEVEL: PopbarMenuItem[] = [
  { kind: "action", action: "summarize", label: "Summarise" },
  { kind: "action", action: "reply_politely", label: "Reply politely" },
  { kind: "submenu", id: "translate", label: "Translate" },
  { kind: "action", action: "explain_simply", label: "Explain simply" },
  { kind: "action", action: "fix_grammar", label: "Fix grammar" },
  { kind: "submenu", id: "rewrite", label: "Rewrite" },
];

/** A deliberately short preset list rather than a language picker — this
 * window can never take keyboard focus, so there is nowhere to type one.
 * Four common targets covers the large majority of "translate this" presses
 * without needing a scrolling list inside a 248px-wide bar. */
const TRANSLATE_ITEMS: PopbarSubmenuItem[] = [
  { action: "translate", label: "Spanish", targetLanguage: "Spanish" },
  { action: "translate", label: "French", targetLanguage: "French" },
  { action: "translate", label: "German", targetLanguage: "German" },
  { action: "translate", label: "Japanese", targetLanguage: "Japanese" },
];

const REWRITE_ITEMS: PopbarSubmenuItem[] = [
  { action: "rewrite_professional", label: "Professional" },
  { action: "rewrite_friendly", label: "Friendly" },
  { action: "rewrite_concise", label: "Concise" },
  { action: "rewrite_diplomatic", label: "Diplomatic" },
];

function submenuItems(parent: "translate" | "rewrite"): PopbarSubmenuItem[] {
  return parent === "translate" ? TRANSLATE_ITEMS : REWRITE_ITEMS;
}

function submenuTitle(parent: "translate" | "rewrite"): string {
  return parent === "translate" ? "Translate to" : "Rewrite as";
}

/** How long the "done" confirmation stays up before the bar closes itself. */
const AUTO_DISMISS_MS = 1300;
/** Keep the on-screen preview short — this is a confirmation, not a reader. */
const PREVIEW_MAX_CHARS = 90;

function preview(text: string): string {
  const trimmed = text.trim().replace(/\s+/g, " ");
  return trimmed.length > PREVIEW_MAX_CHARS
    ? `${trimmed.slice(0, PREVIEW_MAX_CHARS - 1)}…`
    : trimmed;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export function PopbarApp() {
  const [payload, setPayload] = useState<PopbarShowPayload | null>(null);
  const [view, setView] = useState<PopbarView>({ kind: "menu" });
  // Guards against handling the same open twice — once from `popbarPending`
  // on mount and again from the live event, if both happen to resolve for
  // the same hotkey press (the ordinary case after the very first open).
  const seenRequestId = useRef<string | null>(null);

  useEffect(() => {
    function apply(next: PopbarShowPayload) {
      if (seenRequestId.current === next.requestId) return;
      seenRequestId.current = next.requestId;
      setPayload(next);
      setView(next.text ? { kind: "menu" } : { kind: "empty" });
    }

    let cancelled = false;
    popbarPending()
      .then((pending) => {
        if (!cancelled && pending) apply(pending);
      })
      .catch(() => {
        // No pending payload yet (e.g. hot-reloaded straight into this page
        // during development) — the live event below is still coming.
      });

    let unlisten: (() => void) | undefined;
    onPopbarShow(apply)
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {});

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // Defensive only: `popbar.rs` dismisses on Escape via a temporary global
  // hotkey, because a window that can never become key never receives a
  // `keydown` in the first place (see the Rust module docs). This listener
  // costs nothing and catches the edge case of that assumption ever
  // changing, but it is not the code path Escape is expected to take.
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") void popbarDismiss();
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  async function run(action: TextAiAction, label: string, targetLanguage?: string) {
    const text = payload?.text;
    if (!text) return;
    setView({ kind: "running", label });
    try {
      const result = await popbarRun(action, text, targetLanguage);
      setView({ kind: "done", label, preview: preview(result) });
      window.setTimeout(() => void popbarDismiss(), AUTO_DISMISS_MS);
    } catch (error) {
      // Shown inline and left up rather than auto-dismissing — a failure the
      // user never sees is worse than a bar that stays open one extra click.
      setView({ kind: "error", message: describeError(error) });
    }
  }

  return (
    <div className="h-full w-full p-2">
      <div
        className={cx(
          "glass shadow-panel flex h-full w-full flex-col overflow-hidden",
          "rounded-cad border border-line",
        )}
      >
        <div
          data-tauri-drag-region
          title="Highlight & Act"
          className="drag-region flex h-6 shrink-0 items-center justify-between px-2.5"
        >
          <span className="pointer-events-none select-none text-2xs font-semibold uppercase tracking-[0.14em] text-ink-faint">
            Highlight &amp; Act
          </span>
          <button
            type="button"
            aria-label="Close"
            title="Close (Esc)"
            onClick={() => void popbarDismiss()}
            className="no-drag flex h-4 w-4 items-center justify-center rounded-full text-[9px] leading-none text-ink-faint transition-colors hover:bg-raised hover:text-ink"
          >
            ✕
          </button>
        </div>

        <div className="no-drag flex flex-1 flex-col overflow-y-auto px-2 pb-2">
          <PopbarBody view={view} payload={payload} onRun={run} onView={setView} />
        </div>
      </div>
    </div>
  );
}

/** Turn whatever `popbarRun` rejected with into one readable line. Tauri
 * rejects `invoke` with the command's `Err` payload — here always a plain
 * string, since every Rust error on this path is `.map_err(|e| e.to_string())`
 * before it crosses the IPC boundary. */
function describeError(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return "Something went wrong and the action did not complete.";
}

// ---------------------------------------------------------------------------
// Body, by view state
// ---------------------------------------------------------------------------

function PopbarBody({
  view,
  payload,
  onRun,
  onView,
}: {
  view: PopbarView;
  payload: PopbarShowPayload | null;
  onRun: (action: TextAiAction, label: string, targetLanguage?: string) => void;
  onView: (view: PopbarView) => void;
}) {
  switch (view.kind) {
    case "empty":
      return <EmptySelection permissionGranted={payload?.permissionGranted ?? true} />;

    case "menu":
      return (
        <div className="flex flex-1 flex-col justify-center gap-1">
          {TOP_LEVEL.map((item) =>
            item.kind === "action" ? (
              <Row key={item.action} label={item.label} onClick={() => onRun(item.action, item.label)} />
            ) : (
              <Row
                key={item.id}
                label={item.label}
                trailing="›"
                onClick={() => onView({ kind: "submenu", parent: item.id })}
              />
            ),
          )}
        </div>
      );

    case "submenu":
      return (
        <div className="flex flex-1 flex-col justify-center gap-1">
          <button
            type="button"
            onClick={() => onView({ kind: "menu" })}
            className="mb-0.5 flex items-center gap-1 self-start text-2xs font-medium text-ink-faint transition-colors hover:text-ink"
          >
            <span aria-hidden="true">‹</span> {submenuTitle(view.parent)}
          </button>
          {submenuItems(view.parent).map((item) => (
            <Row
              key={item.label}
              label={item.label}
              onClick={() => onRun(item.action, `${submenuTitle(view.parent)} ${item.label}`, item.targetLanguage)}
            />
          ))}
        </div>
      );

    case "running":
      return (
        <div className="flex flex-1 flex-col items-center justify-center gap-2.5 py-4 text-center">
          <Spinner className="text-ink-mute" />
          <p className="text-[13px] text-ink-soft">{view.label}…</p>
        </div>
      );

    case "done":
      return (
        <div className="flex flex-1 flex-col items-center justify-center gap-1.5 py-4 text-center">
          <span
            aria-hidden="true"
            className="flex h-6 w-6 items-center justify-center rounded-full bg-positive/20 text-[12px] font-bold text-positive"
          >
            ✓
          </span>
          <p className="text-[13px] font-medium text-ink">Copied to clipboard</p>
          <p className="max-w-full break-words px-1 text-2xs leading-relaxed text-ink-faint">{view.preview}</p>
        </div>
      );

    case "error":
      return (
        <div className="flex flex-1 flex-col items-center justify-center gap-2 py-3 text-center">
          <span
            aria-hidden="true"
            className="flex h-6 w-6 items-center justify-center rounded-full bg-danger/20 text-[12px] font-bold text-danger"
          >
            !
          </span>
          <p className="max-w-full break-words px-1 text-2xs leading-relaxed text-ink-soft">{view.message}</p>
          <button
            type="button"
            onClick={() => onView({ kind: "menu" })}
            className="no-drag rounded-md bg-raised px-2.5 py-1 text-2xs font-medium text-ink transition-colors hover:bg-overlay"
          >
            Back
          </button>
        </div>
      );

    default:
      return null;
  }
}

function EmptySelection({ permissionGranted }: { permissionGranted: boolean }) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-1.5 px-1 py-3 text-center">
      <p className="text-[13px] font-medium text-ink-soft">Nothing selected</p>
      <p className="text-2xs leading-relaxed text-ink-faint">
        {permissionGranted
          ? "Highlight some text in another app, then press the shortcut again."
          : "Caduceus needs Accessibility permission to read your selection — enable it in Settings → Permissions, then try again."}
      </p>
    </div>
  );
}

function Row({ label, trailing, onClick }: { label: string; trailing?: string; onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cx(
        "no-drag flex h-8 shrink-0 items-center justify-between rounded-lg px-2.5 text-left text-[13px] text-ink-soft",
        "transition-colors duration-100 hover:bg-raised hover:text-ink active:bg-overlay",
      )}
    >
      <span className="truncate">{label}</span>
      {trailing && (
        <span aria-hidden="true" className="text-ink-faint">
          {trailing}
        </span>
      )}
    </button>
  );
}
