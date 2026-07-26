/**
 * The Command Center — Caduceus's palette.
 *
 * One input, one keyboard-navigable list, and a prefix router. Results come
 * from {@link defaultProviders}; the routing decision (what Enter actually
 * does) is made in Rust so the same rules apply to voice input.
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
import { Kbd, Spinner, cx } from "@/shared/ui";
import { ShortcutIcon } from "@/shared/ShortcutIcon";

import { AgentPanel } from "./AgentPanel";
import { ClipboardView } from "./ClipboardView";

type Mode = "default" | "clipboard";

interface ChatReply {
  prompt: string;
  text: string;
}

export function CommandCenter() {
  const { settings } = useSettings();
  const { toasts, notify } = useToasts();

  const [input, setInput] = useState("");
  const [mode, setMode] = useState<Mode>("default");
  const [selected, setSelected] = useState(0);
  const [results, setResults] = useState<ResultItem[]>([]);
  const [parsed, setParsed] = useState<ParsedInput | null>(null);
  const [busy, setBusy] = useState(false);
  const [chat, setChat] = useState<ChatReply | null>(null);
  const [session, setSession] = useState<{ id: string; task: string } | null>(null);
  const [voice, setVoice] = useState<VoiceState>("idle");
  const [clipboardCount, setClipboardCount] = useState(0);

  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const activateClipboard = useRef<() => void>(() => {});

  const debouncedInput = useDebounced(input, 90);

  // --- palette actions handed to providers --------------------------------
  const actions = useMemo<PaletteActions>(
    () => ({
      close: () => void api.hideCommandCenter(),
      setInput: (value) => {
        setInput(value);
        inputRef.current?.focus();
      },
      setMode: (next) => setMode(next),
      notify,
    }),
    [notify],
  );

  // --- window open --------------------------------------------------------
  useTauriEvent<CommandCenterOpenPayload>(EVENTS.commandCenterOpen, (payload) => {
    setMode(payload.mode === "clipboard" ? "clipboard" : "default");
    setInput(payload.prefill);
    setChat(null);
    setSelected(0);
    // The webview needs a tick to become focusable after the window is shown.
    setTimeout(() => {
      inputRef.current?.focus();
      if (payload.selectAll) inputRef.current?.select();
    }, 40);
  });

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // --- voice --------------------------------------------------------------
  useTauriEvent<VoiceState>(EVENTS.voiceState, setVoice);

  useTauriEvent<string>(EVENTS.voicePartial, (text) => {
    setInput(text);
    inputRef.current?.focus();
  });

  useTauriEvent<VoiceOutcome>(EVENTS.voiceResult, (outcome) => {
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

  // --- results ------------------------------------------------------------
  useEffect(() => {
    if (mode !== "default" || !settings) return;
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
  }, [debouncedInput, mode, settings, actions]);

  // --- submit -------------------------------------------------------------
  const submit = useCallback(
    async (raw?: string) => {
      const text = (raw ?? input).trim();
      if (!text) return;

      setBusy(true);
      setChat(null);
      try {
        const outcome = await api.dispatchInput(text);

        if (outcome.action === "primary_ai") {
          if (outcome.ok) setChat({ prompt: text, text: outcome.message });
          else notify(outcome.message, "error");
        } else if (outcome.action === "computer_use") {
          if (outcome.ok && outcome.sessionId) {
            setSession({ id: outcome.sessionId, task: parsedRemainder(text) });
          } else {
            notify(outcome.message, "error");
          }
        } else if (outcome.action === "clipboard_search") {
          setMode("clipboard");
          setInput(outcome.clipboardQuery ?? "");
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
    [input, notify],
  );

  // --- keyboard -----------------------------------------------------------
  const onKeyDown = (e: React.KeyboardEvent) => {
    const count = mode === "clipboard" ? clipboardCount : results.length;

    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        setSelected((i) => (count === 0 ? 0 : (i + 1) % count));
        break;

      case "ArrowUp":
        e.preventDefault();
        setSelected((i) => (count === 0 ? 0 : (i - 1 + count) % count));
        break;

      case "Enter": {
        e.preventDefault();
        if (mode === "clipboard") {
          activateClipboard.current();
          return;
        }
        // A prefixed input always dispatches, even if a shortcut happens to be
        // highlighted — typing "/c open mail" must never launch a bookmark.
        const item = results[selected];
        if (item && !parsed?.rule) void runItem(item);
        else void submit();
        break;
      }

      case "Escape":
        e.preventDefault();
        if (session) setSession(null);
        else if (chat) setChat(null);
        else if (mode === "clipboard") {
          setMode("default");
          setInput("");
        } else if (input) setInput("");
        else void api.hideCommandCenter();
        break;

      case "Tab":
        // Completes to the highlighted row instead of moving focus out of the
        // input — there is nowhere else for focus to usefully go.
        e.preventDefault();
        if (mode === "default" && results[selected]) {
          setInput(results[selected].title);
        }
        break;
    }
  };

  const runItem = async (item: ResultItem) => {
    setBusy(true);
    try {
      const keepOpen = (await item.run()) === false;
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
    mode === "clipboard"
      ? "Search your clipboard history…"
      : `Search apps, search ${hostOf(settings.commandCenter.searchUrlTemplate)}, ` +
        `type ${aiPrefix} for AI or your own shortcuts, or do maths — 2+2`;

  const grouped = groupResults(results);

  return (
    // `h-full` rather than `h-screen`: html/body/#root are already 100% tall, so
    // this is identical in the real window but also correct when the component
    // is embedded in the UI preview harness.
    <div className="relative flex h-full w-full flex-col overflow-hidden rounded-cad-lg glass shadow-float">
      {/* --- input row -------------------------------------------------- */}
      <div className="drag-region flex shrink-0 items-center gap-3 px-5 pb-3 pt-4">
        <span aria-hidden="true" className="shrink-0 text-ink-faint">
          {busy ? <Spinner className="text-accent" /> : mode === "clipboard" ? "❐" : "⌕"}
        </span>

        <input
          ref={inputRef}
          value={input}
          onChange={(e) => {
            setInput(e.target.value);
            setSelected(0);
          }}
          onKeyDown={onKeyDown}
          placeholder={placeholder}
          spellCheck={false}
          autoComplete="off"
          className="no-drag min-w-0 flex-1 bg-transparent text-[17px] font-normal tracking-[-0.01em] text-ink placeholder:text-ink-faint focus:outline-none"
        />

        {voice !== "idle" && (
          <span className="row shrink-0 text-2xs text-accent">
            <span className="relative flex h-2 w-2">
              <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-accent opacity-60" />
              <span className="relative inline-flex h-2 w-2 rounded-full bg-accent" />
            </span>
            {voice === "recording" ? (input.trim() ? "Dictating…" : "Listening…") : "Finishing…"}
          </span>
        )}

        {mode === "clipboard" && (
          <button
            type="button"
            onClick={() => {
              setMode("default");
              setInput("");
              inputRef.current?.focus();
            }}
            className="no-drag shrink-0 rounded-md px-2 py-1 text-2xs text-ink-mute transition-colors hover:bg-raised hover:text-ink"
          >
            Clipboard · Esc to exit
          </button>
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
      {parsed?.rule && mode === "default" && (
        <div className="row shrink-0 px-5 pb-2">
          <span className="rounded-md border border-accent/30 bg-accent/12 px-2 py-0.5 text-2xs font-medium text-accent">
            {parsed.rule.label || parsed.rule.prefix}
          </span>
          <span className="truncate text-2xs text-ink-faint">{parsed.rule.description}</span>
        </div>
      )}

      <div className="hairline shrink-0" />

      {/* --- body -------------------------------------------------------- */}
      {session ? (
        <AgentPanel
          sessionId={session.id}
          task={session.task}
          onClose={() => setSession(null)}
        />
      ) : chat ? (
        <ChatResult reply={chat} onDismiss={() => setChat(null)} onNotify={notify} />
      ) : mode === "clipboard" ? (
        <ClipboardView
          query={debouncedInput}
          selectedIndex={selected}
          onCountChange={setClipboardCount}
          onNotify={notify}
          onClose={() => void api.hideCommandCenter()}
          registerActivate={(fn) => {
            activateClipboard.current = fn;
          }}
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
                      onClick={() => void runItem(item)}
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
          <span>{mode === "clipboard" ? "copy" : "run"}</span>
          {mode === "clipboard" && (
            <>
              <Kbd>⌘P</Kbd>
              <span>pin</span>
            </>
          )}
          <Kbd>esc</Kbd>
          <span>close</span>
        </div>
        <button
          type="button"
          onClick={() => void api.openSettingsWindow()}
          className="no-drag rounded px-1.5 py-0.5 transition-colors hover:bg-raised hover:text-ink"
        >
          Settings
        </button>
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
  onClick: () => void;
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

function ChatResult({
  reply,
  onDismiss,
  onNotify,
}: {
  reply: ChatReply;
  onDismiss: () => void;
  onNotify: (message: string, tone?: "info" | "error") => void;
}) {
  return (
    <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">
      <p className="eyebrow mb-2">Reply</p>
      <div className="selectable whitespace-pre-wrap text-[13px] leading-relaxed text-ink-soft">
        {reply.text}
      </div>
      <div className="row mt-4">
        <button
          type="button"
          onClick={() => {
            navigator.clipboard
              .writeText(reply.text)
              .then(() => onNotify("Copied"))
              .catch(() => onNotify("Could not copy", "error"));
          }}
          className="rounded-md border border-line bg-raised px-2.5 py-1 text-2xs text-ink-soft transition-colors hover:bg-overlay hover:text-ink"
        >
          Copy
        </button>
        <button
          type="button"
          onClick={onDismiss}
          className="rounded-md px-2.5 py-1 text-2xs text-ink-faint transition-colors hover:text-ink"
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
