/**
 * The Command Center — Caduceus's palette.
 *
 * One input, one keyboard-navigable list, and a prefix router. Results come
 * from {@link defaultProviders}; the routing decision (what Enter actually
 * does) is made in Rust so the same rules apply to voice input.
 */

import { getCurrentWindow } from "@tauri-apps/api/window";
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
import type { CommandOutput } from "@/shared/commands";
import { loadUsage, recordUsage } from "@/shared/usage";
import type {
  ChatMessage,
  ClipboardEntry,
  CommandCenterOpenPayload,
  ParsedInput,
  VoiceOutcome,
  VoiceState,
} from "@/shared/types";
import { EVENTS } from "@/shared/types";
import { Kbd, Spinner, cx } from "@/shared/ui";
import { ThemeToggle } from "@/shared/ThemeToggle";
import { ShortcutIcon } from "@/shared/ShortcutIcon";
import { Thread } from "@/chat/Thread";

import { AgentPanel } from "./AgentPanel";
import { ClipboardView } from "./ClipboardView";
import { SystemView } from "./SystemView";

type Mode = "default" | "clipboard" | "system";

export function CommandCenter() {
  const { settings } = useSettings();
  const { toasts, notify } = useToasts();

  const [input, setInput] = useState("");
  const [mode, setMode] = useState<Mode>("default");
  const [selected, setSelected] = useState(0);
  const [results, setResults] = useState<ResultItem[]>([]);
  const [parsed, setParsed] = useState<ParsedInput | null>(null);
  const [busy, setBusy] = useState(false);
  const [chat, setChat] = useState<{ conversationId: number } | null>(null);
  const [session, setSession] = useState<{ id: string; task: string } | null>(null);
  const [voice, setVoice] = useState<VoiceState>("idle");
  const [clipboardCount, setClipboardCount] = useState(0);
  const [output, setOutput] = useState<CommandOutput | null>(null);
  // The row that has been chosen once and is waiting for a second Enter. Held
  // by id rather than by index, so moving the selection cancels it instead of
  // silently re-aiming the confirmation at a different row.
  const [pendingConfirm, setPendingConfirm] = useState<{ id: string; message: string } | null>(
    null,
  );

  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const activateClipboard = useRef<() => void>(() => {});
  /** After Esc closes inline chat, do not auto-reopen until the query changes. */
  const skipChatAutoOpen = useRef<string | null>(null);

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
      setMode: (next) => setMode(next),
      notify,
      showOutput: (next) => setOutput(next),
    }),
    [notify],
  );

  // --- window open --------------------------------------------------------
  useTauriEvent<CommandCenterOpenPayload>(EVENTS.commandCenterOpen, (payload) => {
    setMode(
      payload.mode === "clipboard" || payload.mode === "system" ? payload.mode : "default",
    );
    setInput(payload.prefill);
    setChat(null);
    setSelected(0);
    setOutput(null);
    setPendingConfirm(null);
    skipChatAutoOpen.current = null;
    // The webview needs a tick to become focusable after the window is shown.
    setTimeout(() => {
      inputRef.current?.focus();
      if (payload.selectAll) inputRef.current?.select();
    }, 40);
  });

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // Ranking reads these synchronously inside `search()`, so they have to be in
  // memory before the first result pass rather than awaited during it.
  const [usageReady, setUsageReady] = useState(false);
  useEffect(() => {
    void loadUsage().finally(() => setUsageReady(true));
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
  }, [debouncedInput, mode, settings, actions, usageReady]);

  // Open the inline chat as soon as the input routes to `/`, before Enter.
  useEffect(() => {
    if (skipChatAutoOpen.current !== null && skipChatAutoOpen.current !== debouncedInput) {
      skipChatAutoOpen.current = null;
    }
  }, [debouncedInput]);

  useEffect(() => {
    if (mode !== "default" || session || output) return;

    const action = parsed?.rule?.action;
    if (action !== "primary_ai") {
      setChat(null);
      return;
    }

    if (skipChatAutoOpen.current === debouncedInput) return;

    let cancelled = false;
    void (async () => {
      try {
        const list = await api.chatConversations();
        let id = list[0]?.id;
        if (id == null) id = await api.chatNewConversation();
        if (!cancelled) setChat({ conversationId: id });
      } catch {
        // Chat store unavailable — Enter dispatch will surface the error.
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [parsed?.rule?.action, parsed?.rule?.id, debouncedInput, mode, session, output]);

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
            setChat({ conversationId: outcome.conversationId });
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
        setPendingConfirm(null);
        setSelected((i) => (count === 0 ? 0 : (i + 1) % count));
        break;

      case "ArrowUp":
        e.preventDefault();
        setPendingConfirm(null);
        setSelected((i) => (count === 0 ? 0 : (i - 1 + count) % count));
        break;

      case "Enter": {
        e.preventDefault();
        // While an output panel is up, Enter must not silently re-run the row
        // that produced it. Escape is the way back.
        if (output) return;
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
        if (pendingConfirm) setPendingConfirm(null);
        else if (output) setOutput(null);
        else if (session) setSession(null);
        else if (chat) {
          skipChatAutoOpen.current = input;
          setChat(null);
        }
        else if (mode === "clipboard" || mode === "system") {
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
    // Anything that ends the session or deletes something asks once. In a fuzzy
    // list "Shut down" sits a keystroke away from "Sleep", and an undo for that
    // does not exist.
    if (item.confirm && pendingConfirm?.id !== item.id) {
      setPendingConfirm({ id: item.id, message: item.confirm });
      return;
    }
    setPendingConfirm(null);

    // Counted here rather than inside each command, so applications, shortcuts
    // and commands are all ranked by the same rule.
    if (item.usageKey) recordUsage(item.usageKey);

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
          {busy ? <Spinner className="text-accent" /> : mode === "clipboard" ? "❐" : mode === "system" ? "◔" : "⌕"}
        </span>

        <input
          ref={inputRef}
          value={input}
          onChange={(e) => {
            setInput(e.target.value);
            setSelected(0);
            setPendingConfirm(null);
          }}
          onKeyDown={onKeyDown}
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

        {(mode === "clipboard" || mode === "system") && (
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

      {/* Confirmation: the second Enter runs it, Escape or a keystroke cancels. */}
      {pendingConfirm && (
        <div className="row shrink-0 gap-2 px-5 pb-2">
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
      ) : chat ? (
        <InlineChat
          conversationId={chat.conversationId}
          onDismiss={() => setChat(null)}
          onNotify={notify}
        />
      ) : mode === "system" ? (
        <SystemView query={debouncedInput} onNotify={notify} />
      ) : mode === "clipboard" ? (
        <ClipboardView
          query={debouncedInput}
          selectedIndex={selected}
          onCountChange={setClipboardCount}
          onNotify={notify}
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
        <div className="row">
          <ThemeToggle />
          <button
            type="button"
            onClick={() => void api.openSettingsWindow()}
            className="no-drag rounded px-1.5 py-0.5 transition-colors hover:bg-raised hover:text-ink"
          >
            Settings
          </button>
          <ResizeGrip onHide={() => void api.hideCommandCenter()} />
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

/**
 * The `/` conversation, inline in the palette.
 *
 * Renders the same {@link Thread} the chat window does, so a fix to one is a fix
 * to both. The palette shows the tail of the thread; "Open in window" hands the
 * same conversation to the full view for reading back through it.
 */
function InlineChat({
  conversationId,
  onDismiss,
  onNotify,
}: {
  conversationId: number;
  onDismiss: () => void;
  onNotify: (message: string, tone?: "info" | "error") => void;
}) {
  const [messages, setMessages] = useState<ChatMessage[]>([]);

  const reload = useCallback(async () => {
    try {
      setMessages(await api.chatMessages(conversationId));
    } catch {
      setMessages([]);
    }
  }, [conversationId]);

  useEffect(() => {
    void reload();
  }, [reload]);

  // The chat window writes to the same thread.
  useTauriEvent<number>(EVENTS.chatChanged, (id) => {
    if (id === conversationId) void reload();
  });

  const last = [...messages].reverse().find((m) => m.role === "assistant");

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <Thread className="min-h-0 flex-1" messages={messages} />

      <div className="row shrink-0 gap-1 px-5 pb-3">
        <button
          type="button"
          onClick={() => void api.openChatWindow(conversationId)}
          className="rounded-md border border-line bg-raised px-2.5 py-1 text-2xs text-ink-soft transition-colors hover:bg-overlay hover:text-ink"
        >
          Open in window
        </button>
        {last && (
          <>
            <button
              type="button"
              onClick={() => {
                navigator.clipboard
                  .writeText(last.text)
                  .then(() => onNotify("Copied"))
                  .catch(() => onNotify("Could not copy", "error"));
              }}
              className="rounded-md px-2.5 py-1 text-2xs text-ink-faint transition-colors hover:text-ink"
            >
              Copy
            </button>
            <button
              type="button"
              onClick={() => {
                api
                  .addToNotes(last.text)
                  .then((out) => onNotify(out.message))
                  .catch((e) => onNotify(api.errorMessage(e), "error"));
              }}
              className="rounded-md px-2.5 py-1 text-2xs text-ink-faint transition-colors hover:text-ink"
            >
              Save to Notes
            </button>
          </>
        )}
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

/**
 * Bottom-right corner: drag to resize, double-click to hide.
 *
 * The window is undecorated, so macOS draws no resize affordance of its own —
 * without a visible grip the edges are draggable but undiscoverable. Resizing
 * is handed to the window manager via `startResizeDragging` rather than being
 * tracked in JS, so it stays smooth and respects the min/max in tauri.conf.json.
 */
function ResizeGrip({ onHide }: { onHide: () => void }) {
  return (
    <span
      role="button"
      tabIndex={-1}
      aria-label="Resize the Command Center. Double-click to hide."
      title="Drag to resize · double-click to hide"
      onPointerDown={(e) => {
        if (e.button !== 0 || e.detail > 1) return;
        void getCurrentWindow().startResizeDragging("SouthEast");
      }}
      onDoubleClick={onHide}
      className={cx(
        "no-drag ml-1 flex h-4 w-4 cursor-nwse-resize items-center justify-center",
        "text-ink-faint transition-colors hover:text-ink-mute",
      )}
    >
      <svg viewBox="0 0 10 10" className="h-2.5 w-2.5" aria-hidden="true">
        <path
          d="M9 1 1 9 M9 5 5 9 M9 9 9 9"
          stroke="currentColor"
          strokeWidth="1.4"
          strokeLinecap="round"
          fill="none"
        />
      </svg>
    </span>
  );
}
