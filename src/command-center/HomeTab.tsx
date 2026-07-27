/**
 * The Home tab: Caduceus's palette.
 *
 * One input, one keyboard-navigable list, and a prefix router. Results come
 * from {@link defaultProviders}; the routing decision (what Enter actually
 * does) is made in Rust so the same rules apply to voice input.
 *
 * This is what the global hotkey opens and what ⌘T gives you. Anything with
 * state worth keeping — the clipboard, a conversation, Settings, a keep-awake
 * countdown — opens as its own tab through {@link PaletteActions.openTab}
 * rather than taking this one over, so a half-typed search survives.
 *
 * Two things still render *inside* the palette rather than as tabs:
 *
 * * **Command output** — a hash, a decoded token, a colour. Transient by
 *   nature; a tab per SHA-256 would be twenty-four tabs of nothing by lunchtime.
 * * **An agent session** — it belongs to the line just typed, and reads as part
 *   of having submitted it.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import * as api from "@/shared/api";
import { useDebounced, useSettings, useTauriEvent, useToasts } from "@/shared/hooks";
import { highlightSegments } from "@/shared/fuzzy";
import {
  collectResults,
  defaultProviders,
  hostOf,
  type PaletteActions,
  type ResultItem,
} from "@/shared/providers";
import type {
  ClipboardEntry,
  CommandCenterOpenPayload,
  ParsedInput,
  VoiceOutcome,
  VoiceState,
} from "@/shared/types";
import { EVENTS } from "@/shared/types";
import { PERMISSIONS, permissionFromMessage } from "@/shared/permissions";
import { Kbd, Spinner, cx } from "@/shared/ui";
import { ShortcutIcon } from "@/shared/ShortcutIcon";
import { loadUsage, recordUsage } from "@/shared/usage";
import type { CommandOutput } from "@/shared/commands";

import { AgentPanel } from "./AgentPanel";
import type { Tab } from "@/shared/tabs";

export function HomeTab({
  active,
  onOpenTab,
  onSetTitle,
}: {
  /** False while another tab is in front. A background palette takes no keys. */
  active: boolean;
  onOpenTab: (request: Omit<Tab, "id">) => void;
  /** Names this tab after what is being searched, so several are tellable apart. */
  onSetTitle: (title: string | undefined) => void;
}) {
  const { settings } = useSettings();
  const { toasts, notify } = useToasts();

  const [input, setInput] = useState("");
  const [selected, setSelected] = useState(0);
  const [results, setResults] = useState<ResultItem[]>([]);
  const [parsed, setParsed] = useState<ParsedInput | null>(null);
  const [busy, setBusy] = useState(false);
  const [session, setSession] = useState<{ id: string; task: string } | null>(null);
  const [voice, setVoice] = useState<VoiceState>("idle");
  const [output, setOutput] = useState<CommandOutput | null>(null);
  // The row chosen once and waiting for a second Enter. Held by id rather than
  // by index, so moving the selection cancels it instead of silently re-aiming
  // the confirmation at a different row.
  const [pendingConfirm, setPendingConfirm] = useState<{ id: string; message: string } | null>(
    null,
  );

  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  // Which command produced the message currently being handled, so a permission
  // wall can offer to re-run the thing that hit it rather than just naming the
  // switch and leaving the user to remember what they were doing.
  const lastRun = useRef<string | undefined>(undefined);

  // 45ms, not 90: every provider behind this was measured in the tens of
  // microseconds (parse 0.1ms, calculator 0.3ms, app list cached in-process),
  // so the debounce was costing more than the work it was protecting.
  const debouncedInput = useDebounced(input, 45);

  // --- palette actions handed to providers --------------------------------
  const actions = useMemo<PaletteActions>(
    () => ({
      close: () => void api.hideCommandCenter(),
      setInput: (value) => {
        setInput(value);
        inputRef.current?.focus();
      },
      openTab: onOpenTab,
      // A missing macOS permission is not an error message, it is a task. It
      // opens the page that walks through granting it — with the command that
      // hit the wall attached, so it can be run again in place.
      notify: (message, tone) => {
        const permission = tone === "error" ? permissionFromMessage(message) : null;
        if (permission) {
          onOpenTab({
            kind: "permission",
            permission,
            retryCommandId: lastRun.current,
            title: `${PERMISSIONS[permission].title} permission`,
          });
          return;
        }
        notify(message, tone);
      },
      showOutput: (next) => setOutput(next),
    }),
    [notify, onOpenTab],
  );

  // --- window open --------------------------------------------------------
  useTauriEvent<CommandCenterOpenPayload>(EVENTS.commandCenterOpen, (payload) => {
    // Only the tab in front reacts. Otherwise every Home tab would swallow the
    // same keystroke, and the one you can see might not be the one that got it.
    if (!active) return;
    setInput(payload.prefill);
    setSelected(0);
    setOutput(null);
    setPendingConfirm(null);
    // Three attempts, not one. The webview needs a tick to become focusable
    // after the window is shown, and how many ticks depends on whether macOS
    // was also moving the panel into a full-screen Space at the time. A single
    // 40ms shot won that race most of the time, which is the worst kind of
    // reliable — the palette would come up and quietly ignore the keyboard.
    for (const delay of [0, 60, 180]) {
      setTimeout(() => {
        if (document.activeElement === inputRef.current) return;
        inputRef.current?.focus();
        if (payload.selectAll) inputRef.current?.select();
      }, delay);
    }
  });

  useEffect(() => {
    if (active) inputRef.current?.focus();
  }, [active]);

  // The panel can be handed key status a frame after it is ordered in, and a
  // webview that was not focusable when we asked becomes focusable then.
  useEffect(() => {
    if (!active) return;
    const refocus = () => inputRef.current?.focus();
    window.addEventListener("focus", refocus);
    return () => window.removeEventListener("focus", refocus);
  }, [active]);

  // Label the tab with the query, truncated. An untouched palette stays "Search".
  useEffect(() => {
    const trimmed = input.trim();
    onSetTitle(
      trimmed ? (trimmed.length > 20 ? `${trimmed.slice(0, 19)}…` : trimmed) : undefined,
    );
  }, [input, onSetTitle]);

  // Ranking reads these synchronously inside `search()`, so they have to be in
  // memory before the first result pass rather than awaited during it.
  const [usageReady, setUsageReady] = useState(false);
  useEffect(() => {
    void loadUsage().finally(() => setUsageReady(true));
  }, []);

  // --- voice --------------------------------------------------------------
  useTauriEvent<VoiceState>(EVENTS.voiceState, (next) => {
    if (active) setVoice(next);
  });

  useTauriEvent<string>(EVENTS.voicePartial, (text) => {
    if (!active) return;
    setInput(text);
    inputRef.current?.focus();
  });

  useTauriEvent<VoiceOutcome>(EVENTS.voiceResult, (outcome) => {
    if (!active) return;
    if (!outcome.ok) {
      notify(outcome.error ?? "Transcription failed", "error");
      return;
    }
    const routed = outcome.routed;
    if (!routed) return;

    // Rebuild the equivalent typed input, so a spoken command and a typed one
    // travel through exactly the same dispatch path.
    const prefix = prefixForRoute(routed.route, settings?.commandCenter.prefixes ?? []);
    const text = prefix ? `${prefix} ${routed.text}` : routed.text;
    setInput(text);

    if (outcome.autoSubmit && routed.text.trim()) void submit(text);
    else inputRef.current?.focus();
  });

const CHAT_OPEN_PREFIX = "/ ";
const aiPortalOpened = useRef(false);

  useEffect(() => {
    if (!active) return;
    const opensChat = input === CHAT_OPEN_PREFIX || input.startsWith(CHAT_OPEN_PREFIX);
    if (opensChat && !aiPortalOpened.current) {
      aiPortalOpened.current = true;
      const remainder = input.startsWith(CHAT_OPEN_PREFIX) ? input.slice(2) : "";
      onOpenTab({ kind: "chat", prefill: remainder, chatMode: "chat" });
      setInput("");
      return;
    }
    if (!opensChat) aiPortalOpened.current = false;
  }, [input, active, onOpenTab]);

  // --- results ------------------------------------------------------------
  useEffect(() => {
    if (!settings) return;
    let cancelled = false;

    void (async () => {
      // Parsing happens in Rust so the palette and the router never disagree
      // about what a prefix means.
      let nextParsed: ParsedInput | null = null;
      try {
        nextParsed = await api.parseInput(debouncedInput);
      } catch {
        // A parse failure is not worth blocking results over.
      }
      if (cancelled) return;
      setParsed(nextParsed);

      const query = nextParsed?.remainder ?? debouncedInput.trim();

      let rows: ClipboardEntry[] = [];
      if (settings.clipboard.enabled) {
        try {
          rows = await api.clipboardList(query, settings.commandCenter.maxResultsPerSource);
        } catch {
          // History being unavailable must not empty the palette.
        }
      }
      if (cancelled) return;

      const items = await collectResults(defaultProviders, {
        query,
        raw: debouncedInput,
        parsed: nextParsed,
        settings,
        clipboard: rows,
        actions,
      });
      if (cancelled) return;

      setResults(items);
      setSelected((current) => Math.min(current, Math.max(items.length - 1, 0)));
    })();

    return () => {
      cancelled = true;
    };
  }, [debouncedInput, settings, actions, usageReady]);

  // --- submit -------------------------------------------------------------
  const submit = useCallback(
    async (raw?: string) => {
      // Trim only trailing whitespace so `/ ` still parses as the AI prefix with
      // an empty remainder; `.trim()` would collapse it to `/` before dispatch.
      const text = (raw ?? input).trimEnd();
      if (!text.trim()) return;

      setBusy(true);
      try {
        const outcome = await api.dispatchInput(text);

        if (outcome.action === "primary_ai") {
          if (outcome.conversationId != null) {
            // The reply belongs with the rest of the thread, in the chat tab,
            // where it survives the next thing typed here.
            onOpenTab({ kind: "chat", conversationId: outcome.conversationId });
            setInput("");
          } else if (!outcome.ok) {
            notify(outcome.message, "error");
          }
        } else if (outcome.action === "computer_use") {
          if (outcome.ok && outcome.sessionId) {
            setSession({ id: outcome.sessionId, task: parsedRemainder(text) });
          } else if (outcome.ok && outcome.message) {
            notify(outcome.message, "info");
          } else if (!outcome.ok) {
            notify(outcome.message, "error");
          }
        } else if (outcome.action === "clipboard_search") {
          onOpenTab({ kind: "clipboard" });
          setInput("");
        } else if (!outcome.ok) {
          notify(outcome.message, "error");
        }

        if (outcome.closeWindow) {
          setInput("");
          await api.hideCommandCenter();
        }
      } catch (error) {
        notify(api.errorMessage(error), "error");
      } finally {
        setBusy(false);
      }
    },
    [input, notify, onOpenTab],
  );

  const runItem = async (item: ResultItem, asPage = false) => {
    // Anything that ends the session or deletes something asks once. In a fuzzy
    // list "Shut down" sits a keystroke away from "Sleep", and an undo for that
    // does not exist. Opening a page is never destructive, so it skips this.
    if (!asPage && item.confirm && pendingConfirm?.id !== item.id) {
      setPendingConfirm({ id: item.id, message: item.confirm });
      return;
    }
    setPendingConfirm(null);

    // Counted here rather than inside each command, so applications, shortcuts
    // and commands are all ranked by the same rule.
    if (item.usageKey) recordUsage(item.usageKey);
    lastRun.current = item.usageKey?.startsWith("command:")
      ? item.usageKey.slice("command:".length)
      : undefined;

    setBusy(true);
    try {
      const action = asPage && item.openPage ? item.openPage : item.run;
      const keepOpen = (await action()) === false;
      if (!keepOpen) {
        setInput("");
        await api.hideCommandCenter();
      }
    } catch (error) {
      notify(api.errorMessage(error), "error");
    } finally {
      setBusy(false);
    }
  };

  // --- keyboard -----------------------------------------------------------
  //
  // On `window`, not on the `<input>`.
  //
  // The palette is an Accessory app's panel. It takes key status without the
  // application ever becoming active, and in that state the caret does not
  // always land in the search field — most visibly over a full-screen app,
  // where the window would come up with ⌘T working (a window listener) and
  // Escape, the arrows and Enter doing nothing at all, because every one of
  // those lived on an element that did not have focus. Clicking any button in
  // the palette had the same effect, permanently.
  //
  // One listener on the window fixes both: these keys belong to the palette
  // wherever focus happens to be sitting.

  /** Whether focus is somewhere other than the search field that wants the raw key. */
  const focusIsInAField = () => {
    const element = document.activeElement;
    if (!element || element === inputRef.current) return false;
    return element.tagName === "TEXTAREA" || element.tagName === "INPUT";
  };

  /**
   * Escape, in order of what is on screen: cancel the confirmation, dismiss the
   * output, leave the agent session, clear the query.
   *
   * Returns whether it found something to do. **False when the palette is
   * already empty** — and that matters, because `document` is reached before
   * `window` in the bubble phase, so claiming the key unconditionally meant the
   * shell's handler never ran. Escape on an empty search box hid the entire
   * window, taking every other open tab with it, which directly contradicts
   * what `CommandCenter` says it does ("with pages open it closes the page in
   * front"). Declining here is what lets the shell decide.
   */
  const handleEscape = (): boolean => {
    if (pendingConfirm) {
      setPendingConfirm(null);
      return true;
    }
    if (output) {
      setOutput(null);
      return true;
    }
    if (session) {
      setSession(null);
      return true;
    }
    if (input) {
      setInput("");
      inputRef.current?.focus();
      return true;
    }
    return false;
  };

  // Reassigned on every render so the listener below can stay registered once
  // and still never see a stale `results` or `selected`.
  const onPaletteKey = (event: KeyboardEvent) => {
    // ⌘-anything belongs to the tab shell: new tab, close tab, switch tab.
    if (event.metaKey || event.ctrlKey || event.defaultPrevented) return;

    const count = results.length;

    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        setPendingConfirm(null);
        setSelected((i) => (count === 0 ? 0 : (i + 1) % count));
        return;

      case "ArrowUp":
        event.preventDefault();
        setPendingConfirm(null);
        setSelected((i) => (count === 0 ? 0 : (i - 1 + count) % count));
        return;

      case "Enter": {
        if (focusIsInAField()) return;
        event.preventDefault();
        // While an output panel is up, Enter must not silently re-run the row
        // that produced it. Escape is the way back.
        if (output) return;
        // A prefixed input always dispatches, even if a shortcut happens to be
        // highlighted — typing "/c open mail" must never launch a bookmark.
        const item = results[selected];
        // ⇧↵ asks for the page rather than the action: a way to open the tool
        // and keep it open, for the times you are about to use it twice.
        if (item && !parsed?.rule) void runItem(item, event.shiftKey && Boolean(item.openPage));
        else void submit();
        return;
      }

      case "Escape":
        // Only claimed when it was used for something. An unclaimed Escape
        // falls through to the shell, which closes this tab — or hides the
        // window, when this is the only tab there is.
        if (handleEscape()) event.preventDefault();
        return;

      case "Tab":
        if (focusIsInAField()) return;
        // Completes to the highlighted row instead of moving focus out of the
        // input — there is nowhere else for focus to usefully go.
        event.preventDefault();
        if (results[selected]) setInput(results[selected].title);
        return;
    }

    // Anything printable while the caret is adrift: put it back in the search
    // field. The keystroke itself still lands there, because the default action
    // runs against whatever has focus once dispatch finishes.
    if (
      event.key.length === 1 &&
      !event.altKey &&
      document.activeElement !== inputRef.current &&
      !focusIsInAField()
    ) {
      inputRef.current?.focus();
    }
  };

  const paletteKey = useRef(onPaletteKey);
  paletteKey.current = onPaletteKey;

  // On `document`, while the shell's fallback is on `window`. Both are
  // bubble-phase, so the propagation path — not the order the two effects
  // happened to run in — is what guarantees the palette gets first refusal and
  // the shell only sees what the palette left alone.
  useEffect(() => {
    if (!active) return;
    const listener = (event: KeyboardEvent) => paletteKey.current(event);
    document.addEventListener("keydown", listener);
    return () => document.removeEventListener("keydown", listener);
  }, [active]);

  // Keep the highlighted row visible.
  useEffect(() => {
    listRef.current
      ?.querySelector<HTMLElement>(`[data-index="${selected}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  if (!settings) return null;

  // Names the four things the box actually does, because none of them are
  // discoverable by looking at it. The AI prefix is read from settings rather
  // than hardcoded to "/" — it is rebindable, and a placeholder that advertises
  // a prefix the user has renamed is worse than no placeholder.
  const aiPrefix =
    settings.commandCenter.prefixes.find((p) => p.action === "primary_ai")?.prefix ?? "/";
  const placeholder =
    `Search apps, search ${hostOf(settings.commandCenter.searchUrlTemplate)}, ` +
    `type ${aiPrefix} for AI or your own shortcuts, or do maths — 2+2`;

  const grouped = groupResults(results);

  return (
    <div className="relative flex h-full w-full flex-col overflow-hidden">
      {/* --- input row -------------------------------------------------- */}
      <div className="drag-region flex shrink-0 items-center gap-3 px-5 pb-3 pt-4">
        <span aria-hidden="true" className="shrink-0 text-ink-faint">
          {busy ? <Spinner className="text-accent" /> : "⌕"}
        </span>

        <input
          ref={inputRef}
          value={input}
          onChange={(e) => {
            setInput(e.target.value);
            setSelected(0);
            setPendingConfirm(null);
          }}
          placeholder={placeholder}
          spellCheck={false}
          autoComplete="off"
          className="no-drag min-w-0 flex-1 bg-transparent text-[17px] font-normal tracking-[-0.01em] text-ink placeholder:text-ink-faint focus:outline-none"
        />

        {/* Red, not accent: the accent colour means "ordinary Caduceus state"
            everywhere else in this window, and a live microphone should not be
            mistakable for any of it. */}
        {voice !== "idle" && (
          <span
            className={cx(
              "row shrink-0 rounded-full border px-2 py-0.5 text-2xs font-medium",
              voice === "recording"
                ? "border-[#ff3b30]/40 bg-[#ff3b30]/12 text-[#ff5f57]"
                : "border-line bg-raised text-ink-mute",
            )}
          >
            <span className="relative flex h-2 w-2">
              {voice === "recording" && (
                <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-[#ff3b30] opacity-75" />
              )}
              <span
                className={cx(
                  "relative inline-flex h-2 w-2 rounded-full",
                  voice === "recording" ? "bg-[#ff3b30]" : "bg-ink-faint",
                )}
              />
            </span>
            {voice === "recording" ? (input.trim() ? "Recording…" : "Listening…") : "Transcribing…"}
          </span>
        )}
      </div>

      {voice === "recording" && input.trim() && (
        <div className="shrink-0 px-5 pb-2">
          <p className="rounded-lg border border-accent/25 bg-accent/8 px-3 py-2 text-[15px] leading-snug text-ink">
            {input}
          </p>
        </div>
      )}

      {/* Prefix badge: shows which route Enter will take, before you commit. */}
      {parsed?.rule && (
        <div className="row shrink-0 px-5 pb-2">
          <span className="rounded-md border border-accent/30 bg-accent/12 px-2 py-0.5 text-2xs font-medium text-accent">
            {parsed.rule.label || parsed.rule.prefix}
          </span>
          <span className="truncate text-2xs text-ink-faint">{parsed.rule.description}</span>
        </div>
      )}

      <div className="hairline shrink-0" />

      {/* Confirmation: the second Enter runs it, Escape or a keystroke cancels. */}
      {pendingConfirm && (
        <div className="row shrink-0 gap-2 px-5 pb-2 pt-2">
          <span className="rounded-md border border-danger/40 bg-danger/12 px-2 py-0.5 text-2xs font-medium text-danger">
            Confirm
          </span>
          <span className="truncate text-2xs text-ink-soft">{pendingConfirm.message}</span>
          <span className="ml-auto shrink-0 text-2xs text-ink-faint">↵ again · esc cancels</span>
        </div>
      )}

      {/* --- body -------------------------------------------------------- */}
      {output ? (
        <OutputPanel output={output} onDismiss={() => setOutput(null)} onNotify={notify} />
      ) : session ? (
        <AgentPanel
          sessionId={session.id}
          task={session.task}
          onClose={() => setSession(null)}
        />
      ) : (
        <div ref={listRef} className="min-h-0 flex-1 overflow-y-auto px-2 py-2">
          {results.length === 0 ? (
            <p className="px-3 py-8 text-center text-2xs text-ink-faint">
              {input.trim() ? "Press ↵ to run this anyway" : "Type to search"}
            </p>
          ) : (
            grouped.map(([group, items]) => (
              <div key={group} className="mb-1">
                <p className="eyebrow px-3 pb-1 pt-2">{group}</p>
                {items.map((item) => {
                  const index = results.indexOf(item);
                  return (
                    <ResultRow
                      key={item.id}
                      item={item}
                      index={index}
                      active={index === selected}
                      query={parsed?.remainder ?? input.trim()}
                      onHover={() => setSelected(index)}
                      onClick={(event) =>
                        void runItem(item, event.shiftKey && Boolean(item.openPage))
                      }
                    />
                  );
                })}
              </div>
            ))
          )}
        </div>
      )}

      {/* --- footer ------------------------------------------------------ */}
      <div className="flex shrink-0 items-center justify-between border-t border-line px-4 py-2 text-2xs text-ink-faint">
        <div className="row">
          <Kbd>↑</Kbd>
          <Kbd>↓</Kbd>
          <span>navigate</span>
          <Kbd>↵</Kbd>
          <span>run</span>
          <Kbd>⇧↵</Kbd>
          <span>open its page</span>
          <Kbd>⌘T</Kbd>
          <span>new tab</span>
          <Kbd>esc</Kbd>
          <span>close</span>
        </div>
        <div className="row">
          <button
            type="button"
            onClick={() => onOpenTab({ kind: "settings" })}
            className="no-drag rounded px-1.5 py-0.5 transition-colors hover:bg-raised hover:text-ink"
          >
            Settings
          </button>
        </div>
      </div>

      {/* --- toasts ------------------------------------------------------ */}
      <div className="pointer-events-none absolute bottom-12 left-1/2 z-50 flex -translate-x-1/2 flex-col items-center gap-2">
        {toasts.map((toast) => (
          <div
            key={toast.id}
            className={cx(
              "animate-fade-rise glass-raised max-w-[420px] rounded-lg px-3.5 py-2 text-2xs shadow-float",
              toast.tone === "error" ? "text-danger" : "text-ink-soft",
            )}
          >
            {toast.message}
          </div>
        ))}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Pieces
// ---------------------------------------------------------------------------

function ResultRow({
  item,
  index,
  active,
  query,
  onHover,
  onClick,
}: {
  item: ResultItem;
  index: number;
  active: boolean;
  query: string;
  onHover: () => void;
  onClick: (event: React.MouseEvent) => void;
}) {
  const segments = item.positions?.length
    ? highlightSegments(item.title, item.positions)
    : [{ text: item.title, match: false }];

  return (
    <div
      data-index={index}
      onMouseMove={onHover}
      onClick={onClick}
      className={cx(
        "flex cursor-default items-center gap-3 rounded-lg px-3 py-2 transition-colors duration-100",
        active ? "bg-accent/12" : "hover:bg-raised/60",
      )}
    >
      <span
        aria-hidden="true"
        className={cx(
          "flex h-7 w-7 shrink-0 items-center justify-center overflow-hidden rounded-md border text-[13px] leading-none",
          active ? "border-accent/40 bg-accent/15 text-accent" : "border-line bg-raised text-ink-mute",
        )}
      >
        <ShortcutIcon icon={item.icon} label={item.title} className="h-5 w-5" />
      </span>

      <div className="min-w-0 flex-1">
        <p className="truncate text-[13px] text-ink">
          {query
            ? segments.map((segment, i) => (
                <span key={i} className={segment.match ? "font-semibold text-accent" : undefined}>
                  {segment.text}
                </span>
              ))
            : item.title}
        </p>
        {item.subtitle && <p className="truncate text-2xs text-ink-faint">{item.subtitle}</p>}
      </div>

      {item.accessory && (
        <span className="shrink-0 text-2xs text-ink-faint">{item.accessory}</span>
      )}
    </div>
  );
}

/**
 * Text a command produced, shown in the palette rather than in a toast.
 *
 * Anything longer than a few words — a formatted JSON document, a decoded JWT,
 * a machine summary — is unreadable as a toast that disappears. This keeps it on
 * screen, scrollable and selectable, until it is dismissed.
 */
function OutputPanel({
  output,
  onDismiss,
  onNotify,
}: {
  output: CommandOutput;
  onDismiss: () => void;
  onNotify: (message: string, tone?: "info" | "error") => void;
}) {
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="row shrink-0 gap-2 px-5 pb-1 pt-2">
        <span className="text-[13px] font-medium text-ink">{output.title}</span>
        {output.message && (
          <span className="truncate text-2xs text-ink-faint">{output.message}</span>
        )}
      </div>

      <pre className="min-h-0 flex-1 overflow-auto whitespace-pre-wrap break-words px-5 py-2 font-mono text-2xs leading-relaxed text-ink-soft">
        {output.text}
      </pre>

      <div className="row shrink-0 gap-1 px-5 pb-3">
        <button
          type="button"
          onClick={() => {
            navigator.clipboard
              .writeText(output.text)
              .then(() => onNotify("Copied"))
              .catch(() => onNotify("Could not copy", "error"));
          }}
          className="rounded-md border border-line bg-raised px-2.5 py-1 text-2xs text-ink-soft transition-colors hover:bg-overlay hover:text-ink"
        >
          Copy
        </button>
        <button
          type="button"
          onClick={() => {
            api
              .addToNotes(output.text, output.title)
              .then((result) => onNotify(result.message))
              .catch((error) => onNotify(api.errorMessage(error), "error"));
          }}
          className="rounded-md px-2.5 py-1 text-2xs text-ink-faint transition-colors hover:text-ink"
        >
          Save to Notes
        </button>
        <button
          type="button"
          onClick={onDismiss}
          className="ml-auto rounded-md px-2.5 py-1 text-2xs text-ink-faint transition-colors hover:text-ink"
        >
          Back · Esc
        </button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Preserve overall score ordering while grouping rows under their headings. */
function groupResults(items: ResultItem[]): [string, ResultItem[]][] {
  const groups = new Map<string, ResultItem[]>();
  for (const item of items) {
    const bucket = groups.get(item.group);
    if (bucket) bucket.push(item);
    else groups.set(item.group, [item]);
  }
  return [...groups.entries()];
}

/** The prefix that reproduces a voice route as typed input. */
function prefixForRoute(
  route: string,
  prefixes: { prefix: string; action: string }[],
): string | null {
  const wanted =
    route === "computer_use" ? "computer_use"
    : route === "primary_ai" ? "primary_ai"
    : route === "clipboard_search" ? "clipboard_search"
    : null;
  if (!wanted) return null;
  return prefixes.find((p) => p.action === wanted)?.prefix ?? null;
}

/** Strip a leading prefix token for display in the agent panel's title. */
function parsedRemainder(text: string): string {
  const match = text.match(/^\s*\S+\s+(.*)$/);
  return match?.[1] ?? text;
}
