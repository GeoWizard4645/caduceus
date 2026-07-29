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

import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";

import * as api from "@/shared/api";
import { useDebounced, useSettings, useTauriEvent, useToasts, useUpdateCheck } from "@/shared/hooks";
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
  KeywordGroup,
  ParsedInput,
  VoiceOutcome,
  VoiceState,
} from "@/shared/types";
import { EVENTS } from "@/shared/types";
import { PERMISSIONS, permissionFromMessage } from "@/shared/permissions";
import { hotkeyLabel } from "@/shared/hotkeyLabel";
import { Kbd, Spinner, cx } from "@/shared/ui";
import { ShortcutIcon } from "@/shared/ShortcutIcon";
import { loadUsage, recordUsage } from "@/shared/usage";
import { COMMANDS, COMMAND_GROUPS, type CommandGroupId, type CommandOutput } from "@/shared/commands";

import { AgentPanel } from "./AgentPanel";
import { tabForMode, type Tab } from "@/shared/tabs";

// ---------------------------------------------------------------------------
// Voice: short-utterance auto-open (voice routing rule 6)
// ---------------------------------------------------------------------------
//
// Saying a short, exact thing — "Terminal" — and then pausing should open it,
// without waiting for the mic to be released and the transcript finalised.
// This is deliberately the most conservative piece of voice routing: launching
// the wrong thing off a mis-transcription is far worse than doing nothing, so
// every one of the constants and guards below exists to keep false positives
// as close to zero as the feature can afford. See the effect built from these
// inside `HomeTab` for how they compose, and `router.rs`'s module doc for
// rules 1-5, which this sits alongside.

/** "A word or two" — long enough for "Visual Studio Code" or "System
 * Settings", short enough that an actual sentence never qualifies. */
const AUTO_OPEN_MAX_WORDS = 3;

/** How long the live partial transcript must stop changing before this even
 * starts considering a candidate — the "recogniser has settled" half of rule
 * 6. Comfortably inside the spec's 1-2s window; short pauses mid-sentence are
 * common while thinking, and firing on those would make the feature feel like
 * it is racing the speaker rather than waiting for them. */
const AUTO_OPEN_SETTLE_MS = 1400;

/** How long the visible countdown runs *after* settling, before anything
 * actually happens. This is what makes the feature "visibly counting down
 * rather than firing out of nowhere" — long enough to read the row and hit
 * Cancel, short enough that saying a name and pausing still feels immediate. */
const AUTO_OPEN_COUNTDOWN_MS = 1100;

/**
 * Result groups worth auto-opening. Deliberately an allowlist, not a
 * denylist: a group this list has never heard of is excluded by default
 * rather than included, which is the direction "when in doubt, leave the text
 * in the input" points. Search/AI/calculator/conversion/clipboard rows are
 * left out on purpose — those are answers *about* the text, not things a name
 * launches, and an exact-title match against one of them (a web-search row
 * literally titled after the query) would defeat the whole point of the gate.
 */
const AUTO_OPEN_GROUPS = new Set([
  "Applications",
  "Shortcuts",
  "Commands",
  "Favorites",
  "Files",
  "Extensions",
  "Containers",
  "Bookmarks",
  "Repositories",
  "SSH hosts",
  "Menu bar",
  "Contacts",
  "Browser tabs",
]);

/** Lowercase, trim, collapse whitespace and drop one trailing sentence-ending
 * mark — enough to match "Terminal" against a row titled "Terminal" even when
 * the recogniser appended a period, without being a general fuzzy matcher. */
function normalizeForAutoOpen(text: string): string {
  return text
    .toLowerCase()
    .trim()
    .replace(/[.!?,;:]+$/, "")
    .replace(/\s+/g, " ");
}

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/**
 * A light client-side mirror of `voice::router`'s leading/anywhere matching —
 * not the authoritative implementation, which stays server-side and runs once
 * the transcript is actually finalised. This exists purely to gate auto-open:
 * "no keyword group matched" is one of rule 6's four conditions, and an
 * explicit instruction ("search Terminal") must always beat a guess about
 * what to launch.
 */
function matchesAnyKeywordGroup(transcript: string, groups: KeywordGroup[]): boolean {
  const haystack = normalizeForAutoOpen(transcript);
  if (!haystack) return false;
  return groups.some((group) => {
    if (!group.enabled) return false;
    return group.keywords.some((raw) => {
      const needle = normalizeForAutoOpen(raw);
      if (!needle) return false;
      if (group.matchMode === "anywhere") {
        return new RegExp(`(^|\\s)${escapeRegExp(needle)}(\\s|$)`).test(haystack);
      }
      return haystack === needle || haystack.startsWith(`${needle} `);
    });
  });
}

/**
 * Whether `results[0]` is a safe auto-open candidate for `transcript`.
 *
 * Every condition is a hard "no", not a score threshold: a short transcript,
 * an exact (or near-exact, modulo trailing punctuation) title match in a
 * launchable group, nothing tied with it, and no keyword group already
 * claiming the text. "The best of a weak field" is explicitly not good
 * enough — see the spec note this implements — so this reuses the ranking
 * `results` already carries rather than scoring anything itself.
 */
function pickAutoOpenCandidate(
  results: ResultItem[],
  transcript: string,
  keywordGroups: KeywordGroup[],
): ResultItem | null {
  const words = transcript.trim().split(/\s+/).filter(Boolean);
  if (words.length === 0 || words.length > AUTO_OPEN_MAX_WORDS) return null;
  if (matchesAnyKeywordGroup(transcript, keywordGroups)) return null;

  const top = results[0];
  // No group this feature does not recognise, and never a row that would
  // otherwise ask for confirmation — auto-open is for opening things, not for
  // silently arming a "Shut Down"-style prompt nobody asked for.
  if (!top || !AUTO_OPEN_GROUPS.has(top.group) || top.confirm) return null;

  const query = normalizeForAutoOpen(transcript);
  if (!query || normalizeForAutoOpen(top.title) !== query) return null;

  // A runner-up with the same exact title (two apps of the same name in
  // different locations, say) makes the pick ambiguous, not confident.
  const second = results[1];
  if (second && normalizeForAutoOpen(second.title) === query) return null;

  return top;
}

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
  const update = useUpdateCheck(active);

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
  // A state setter used as a ref callback, not `useRef`, because the list only
  // exists in the DOM while neither `output` nor `session` is showing — a
  // plain `useRef` set once on mount would stay pointed at nothing if the
  // list was not the first thing rendered, and never notice it appearing
  // later. `setListEl` fires every time the node is attached or detached, so
  // the ResizeObserver effect below always has the right node to observe.
  const [listEl, setListEl] = useState<HTMLDivElement | null>(null);
  // The two numbers the hand-rolled virtualiser below needs to work out which
  // rows are actually on screen. See the "virtualised list" section for why
  // there is one at all: with the full command catalogue in view (see
  // `groupEmptyState`), mounting every row as a real DOM node is the kind of
  // thing that makes typing feel sluggish for no reason a user could name.
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(0);
  // Which command produced the message currently being handled, so a permission
  // wall can offer to re-run the thing that hit it rather than just naming the
  // switch and leaving the user to remember what they were doing.
  const lastRun = useRef<string | undefined>(undefined);

  // 90ms: short enough to feel instant, long enough that a fast typist does
  // not fire Spotlight / semantic / bookmark passes on every single letter.
  // (An older 45ms figure assumed every provider was tens of microseconds —
  // that stopped being true once file and system search joined the list.)
  const debouncedInput = useDebounced(input, 90);
  // Tab titles do not need to track the caret. Updating them on every
  // keystroke re-rendered every mounted page (Settings, chat, …) because the
  // shell keeps tabs alive behind CSS `hidden`.
  const debouncedTitle = useDebounced(input, 200);

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
    // A request that names a destination — a staff shortcut set to "clipboard
    // history" — belongs to that tab. This one is still `active` at the moment
    // the event fires, because the Command Center has not re-rendered yet, so
    // without this it would take the focus straight back off the tab that was
    // just opened.
    if (tabForMode(payload?.mode)) return;
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
    const trimmed = debouncedTitle.trim();
    onSetTitle(
      trimmed ? (trimmed.length > 20 ? `${trimmed.slice(0, 19)}…` : trimmed) : undefined,
    );
  }, [debouncedTitle, onSetTitle]);

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
    // A final transcript supersedes any guess auto-open was still counting
    // down on for the same utterance — the two must never both act.
    cancelAutoOpen();
    if (!outcome.ok) {
      // Through `actions.notify`, not the raw toast: a missing microphone or
      // speech grant opens its permission page with Repair one click away,
      // instead of flashing an error the user cannot act on.
      actions.notify(outcome.error ?? "Transcription failed", "error");
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

  const aiPortalOpened = useRef(false);

  const aiPrefix =
    settings?.commandCenter.prefixes.find((p) => p.action === "primary_ai")?.prefix ?? "/";
  const chatOpenPrefix = `${aiPrefix} `;

  useEffect(() => {
    if (!active || !settings) return;
    const opensChat = input === chatOpenPrefix || input.startsWith(chatOpenPrefix);
    if (opensChat && !aiPortalOpened.current) {
      aiPortalOpened.current = true;
      const remainder = input.startsWith(chatOpenPrefix) ? input.slice(chatOpenPrefix.length) : "";
      onOpenTab({ kind: "chat", prefill: remainder, chatMode: "chat" });
      setInput("");
      return;
    }
    if (!opensChat) aiPortalOpened.current = false;
  }, [input, active, onOpenTab, settings, chatOpenPrefix]);

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

      // Only `clipboardProvider` ever reads this — and, per its own comment in
      // providers.ts, only once the input has actually named the clipboard
      // prefix (`parsed.rule.action === "clipboard_search"`); every other
      // query throws the rows away untouched. This used to run unconditionally,
      // which meant an ordinary keystroke — the overwhelmingly common case —
      // paid for a full clipboard-history query, awaited serially *before*
      // `collectResults` even started, purely to discard the answer. That is a
      // real, disk-backed IPC round trip on every settled query, not the few
      // microseconds of in-process JS the rest of this pass costs.
      let rows: ClipboardEntry[] = [];
      if (settings.clipboard.enabled && nextParsed?.rule?.action === "clipboard_search") {
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

  // useCallback, not a plain function: this is handed to every visible
  // `ResultRow` as its `onRun` prop, and `ResultRow` is memoised so a keystroke
  // that does not change *this* row's own props can skip re-rendering it
  // entirely. A fresh closure here every render would defeat that — React.memo
  // compares props by reference, and a new function reference is a prop
  // change — so the one thing standing between "identical props" and "props
  // changed" would be this function's identity, on every single row, on every
  // keystroke.
  const runItem = useCallback(
    async (item: ResultItem, asPage = false) => {
      // Anything that ends the session or deletes something asks once. In a
      // fuzzy list "Shut down" sits a keystroke away from "Sleep", and an undo
      // for that does not exist. Opening a page is never destructive, so it
      // skips this.
      if (!asPage && item.confirm && pendingConfirm?.id !== item.id) {
        setPendingConfirm({ id: item.id, message: item.confirm });
        return;
      }
      setPendingConfirm(null);

      // Counted here rather than inside each command, so applications,
      // shortcuts and commands are all ranked by the same rule.
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
    },
    [pendingConfirm, notify],
  );

  // --- voice: auto-open a short, settled, unambiguous utterance -----------
  //
  // Rule 6 from the voice routing spec. This runs entirely off the *partial*
  // stream — `EVENTS.voicePartial` above already keeps `input` live while
  // dictating — so a short name can open before the mic is ever stopped.
  // `EVENTS.voiceResult`'s handler still owns everything else voice does
  // (keyword routing, auto-submit) and cancels this outright the moment a
  // final transcript lands, so the two never race for the same utterance.
  //
  // The countdown is the visible, cancellable half of the feature: arming it
  // never runs anything by itself, and any further speech, any keystroke, or
  // leaving the "recording" state tears it down before it can.

  const [autoOpen, setAutoOpen] = useState<{ item: ResultItem; armedAt: number } | null>(null);
  const settleTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const fireTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  // The timers below fire well after the render that scheduled them and need
  // whatever `results`/`settings`/`voice` are current *then* — not what they
  // were at schedule time — so they read through this ref instead of closing
  // over the values directly. Kept in sync on every render rather than inside
  // an effect: there is nothing to run when these change, only a snapshot to
  // have ready if a pending timer asks.
  const autoOpenSnapshot = useRef({ results, settings, voice, runItem });
  autoOpenSnapshot.current = { results, settings, voice, runItem };

  // `settings` can briefly be `null` on the very first render (see the guard
  // near the bottom of this component). Nothing auto-opens until they load —
  // the guards this feature relies on all live in settings, so acting before
  // they arrive would be acting without them.
  const autoOpenEnabled = settings?.voice.autoOpenShortUtterance ?? false;

  const clearAutoOpenTimers = useCallback(() => {
    if (settleTimer.current) clearTimeout(settleTimer.current);
    if (fireTimer.current) clearTimeout(fireTimer.current);
    settleTimer.current = undefined;
    fireTimer.current = undefined;
  }, []);

  /** Any explicit action — Escape, a keystroke, clicking Cancel — beats a
   * guess about what to launch. Idempotent, so it is safe to call whether or
   * not anything is actually armed. */
  const cancelAutoOpen = useCallback(() => {
    clearAutoOpenTimers();
    setAutoOpen(null);
  }, [clearAutoOpenTimers]);

  useEffect(() => {
    // Reaching this effect at all — a new partial, or the user typing over
    // one — means speech has not (yet) settled. Tear down whatever was
    // pending and start the settle clock fresh from here.
    clearAutoOpenTimers();
    setAutoOpen(null);
    if (voice !== "recording" || !autoOpenEnabled) return;

    const transcriptAtSettle = input;

    settleTimer.current = setTimeout(() => {
      const snap = autoOpenSnapshot.current;
      // Still recording, and nothing has changed `input` since — this effect
      // would have re-run and cleared this timer otherwise.
      if (snap.voice !== "recording" || !snap.settings) return;

      const candidate = pickAutoOpenCandidate(
        snap.results,
        transcriptAtSettle,
        snap.settings.voice.keywordGroups,
      );
      if (!candidate) return;

      setAutoOpen({ item: candidate, armedAt: Date.now() });
      fireTimer.current = setTimeout(() => {
        setAutoOpen(null);
        const latest = autoOpenSnapshot.current;
        if (latest.voice !== "recording") return;
        // Re-validate right before acting — belt and braces against a race
        // with a final transcript or fresh speech landing in between.
        const stillCandidate = pickAutoOpenCandidate(
          latest.results,
          transcriptAtSettle,
          latest.settings?.voice.keywordGroups ?? [],
        );
        if (!stillCandidate || stillCandidate.id !== candidate.id) return;
        // Close the mic before acting rather than after: the item's own
        // action may itself take a moment, and a live recording sitting
        // behind it would keep listening for no reason. `voice_cancel` does
        // not itself emit `VOICE_STATE_EVENT` (see the recorder HUD, which
        // gets away with that only because it closes its whole window on
        // cancel) — this tab has to drop out of "recording" itself, or the
        // mic indicator would show a recording that has already stopped.
        void api.voiceCancel().catch(() => {});
        setVoice("idle");
        void latest.runItem(stillCandidate);
      }, AUTO_OPEN_COUNTDOWN_MS);
    }, AUTO_OPEN_SETTLE_MS);

    return clearAutoOpenTimers;
  }, [input, voice, autoOpenEnabled, clearAutoOpenTimers]);

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
   *
   * Auto-open's countdown is checked first, ahead of even the confirmation
   * prompt — of everything Escape might need to cancel, an app about to
   * launch itself is the most surprising one to leave hanging.
   */
  const handleEscape = (): boolean => {
    if (autoOpen) {
      cancelAutoOpen();
      return true;
    }
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

    // Any deliberate keypress beats a guess about what to launch — cancel
    // first, then let the key do whatever it would normally do (Escape's own
    // handling of this is above; every other key just falls through).
    if (autoOpen && event.key !== "Escape") cancelAutoOpen();

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

  // Track the list's own rendered height, so the virtualiser knows how many
  // rows actually fit on screen. A `ResizeObserver` rather than a one-off
  // measurement on mount: the Command Center window is user-resizable (see
  // `ResizeGrip` in CommandCenter.tsx), and a stale height would make the
  // window taller than what got rendered — an empty stripe at the bottom that
  // no amount of scrolling fills in.
  useEffect(() => {
    if (!listEl) return;
    const observer = new ResizeObserver((entries) => {
      const height = entries[0]?.contentRect.height;
      if (height !== undefined) setViewportHeight(height);
    });
    observer.observe(listEl);
    setViewportHeight(listEl.clientHeight);
    return () => observer.disconnect();
  }, [listEl]);

  // Grouped once per `results` change rather than on every render of this
  // component (which happens on every keystroke, since `input` lives here) —
  // and the row index alongside it, so the list below never falls back to
  // `results.indexOf(item)`. That lookup is only O(n) per row, but the row
  // count is not small: a single letter is a subsequence of nearly every
  // command's title or keywords, so a query like "s" matches ~200 of the ~200
  // built-in commands, and indexOf-per-row turns "render the list" into an
  // O(n²) pass over it on every keystroke for no reason — the index is known
  // the moment `results` is built.
  //
  // `emptyQuery` reads `debouncedInput`, not `input`: it has to agree with
  // what `results` was actually computed from (see `displayQuery` further
  // down for the same reasoning), or the empty-query grouping below would
  // flash on for a query that already has an answer.
  const emptyQuery = !debouncedInput.trim();

  const { grouped, indexById } = useMemo(() => {
    const byId = new Map<string, number>();
    results.forEach((item, index) => byId.set(item.id, index));
    return {
      grouped: emptyQuery ? groupEmptyState(results) : groupResults(results),
      indexById: byId,
    };
  }, [results, emptyQuery]);

  // Flatten the grouped rows into one array with a known pixel offset each,
  // which is the only thing `visibleRows` below and the keyboard-scroll
  // effect after it actually need. See "Virtualised list" near the bottom of
  // this file for the reasoning.
  const { rows: flatRows, totalHeight, offsetByResultIndex } = useMemo(
    () => buildFlatRows(grouped, indexById),
    [grouped, indexById],
  );

  // The slice of `flatRows` currently within (or just outside) the viewport.
  // Recomputed on scroll and on layout, not on every keystroke — `flatRows`
  // itself only changes when `results` does, same as `grouped` above.
  const visibleRows = useMemo(() => {
    if (viewportHeight === 0) {
      // Before the first `ResizeObserver` callback lands, render a first
      // screenful using a generous guess rather than nothing at all — an
      // empty palette for one frame reads as broken, not as loading.
      return flatRows.filter((row) => row.top < 640);
    }
    const start = Math.max(0, scrollTop - VIRTUAL_OVERSCAN_PX);
    const end = scrollTop + viewportHeight + VIRTUAL_OVERSCAN_PX;
    return flatRows.filter((row) => row.top + row.height >= start && row.top <= end);
  }, [flatRows, scrollTop, viewportHeight]);

  // Keep the highlighted row on screen as the arrow keys move it — the
  // virtualised equivalent of `scrollIntoView`, which needs a mounted DOM
  // node to find and the highlighted row is not always one. `offsetByResultIndex`
  // already knows every row's pixel position without a DOM query.
  useEffect(() => {
    if (!listEl) return;
    const bounds = offsetByResultIndex.get(selected);
    if (!bounds) return;
    const viewTop = listEl.scrollTop;
    const viewBottom = viewTop + listEl.clientHeight;
    if (bounds.top < viewTop) {
      listEl.scrollTop = bounds.top;
    } else if (bounds.top + bounds.height > viewBottom) {
      listEl.scrollTop = bounds.top + bounds.height - listEl.clientHeight;
    }
  }, [selected, listEl, offsetByResultIndex]);

  if (!settings) return null;

  // Names the four things the box actually does, because none of them are
  // discoverable by looking at it. The AI prefix is read from settings rather
  // than hardcoded to "/" — it is rebindable, and a placeholder that advertises
  // a prefix the user has renamed is worse than no placeholder.
  const placeholder =
    `Search apps, search ${hostOf(settings.commandCenter.searchUrlTemplate)}, ` +
    `type ${aiPrefix} then space for AI or your shortcuts, or do maths — 2+2`;

  const wantsAiSpace =
    input.length > 0 &&
    input.trimEnd() === aiPrefix &&
    !input.startsWith(chatOpenPrefix);

  // What `results` was actually ranked and highlighted against — not the live
  // `input`. Typing runs 45ms ahead of the debounce that recomputes `results`,
  // so for most of every keystroke `input` names a query the current rows were
  // *not* matched against; passing it straight to `ResultRow` both highlights
  // stale positions against the wrong text and — because it is a new string on
  // every keystroke — defeats `ResultRow`'s memoisation for all ~200 rows on a
  // query as short as "s", not just the one row that changed.
  const displayQuery = parsed?.remainder ?? debouncedInput.trim();

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

        {/* Dictation had to be started from a global hotkey nobody could be
            expected to discover — this whole file listened for transcripts and
            rendered them, but offered no way to ask for one. macOS puts a
            microphone on the keyboard; the least a search field can do is put
            one at the end of the row.

            Click to toggle rather than hold: the hold gesture belongs to the
            push-to-talk hotkey, and holding a mouse button down while watching
            a palette fill in is an awkward thing to ask of anyone.

            A failure is reported rather than swallowed. "Nothing happens" is
            the single worst outcome here, and the reasons it can fail — voice
            switched off in Settings, a missing microphone grant — are all
            things a sentence can fix. */}
        <button
          type="button"
          onClick={() => {
            void (voice === "idle" ? api.voiceStart() : api.voiceFinish()).catch((error) =>
              notify(api.errorMessage(error), "error"),
            );
          }}
          aria-label={voice === "idle" ? "Start dictation" : "Stop dictation"}
          aria-pressed={voice !== "idle"}
          title={
            voice === "idle"
              ? `Dictate${
                  settings?.voice.pushToTalkHotkey
                    ? ` — or hold ${hotkeyLabel(settings.voice.pushToTalkHotkey)}`
                    : ""
                }`
              : "Stop dictating"
          }
          className={cx(
            "no-drag row h-7 w-7 shrink-0 items-center justify-center rounded-full border transition-colors",
            voice === "recording"
              ? "border-[#ff3b30]/40 bg-[#ff3b30]/12 text-[#ff5f57]"
              : "border-line bg-raised text-ink-mute hover:border-accent/40 hover:text-ink",
          )}
        >
          {/* A microphone, drawn rather than an emoji: an emoji here renders at
              a different weight from every other glyph in this row. */}
          <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" fill="none" aria-hidden="true">
            <rect x="6" y="2" width="4" height="7" rx="2" fill="currentColor" />
            <path
              d="M4 7.5a4 4 0 0 0 8 0M8 11.5V14"
              stroke="currentColor"
              strokeWidth="1.4"
              strokeLinecap="round"
            />
          </svg>
        </button>

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

      {voice === "recording" && input.trim() && !autoOpen && (
        <div className="shrink-0 px-5 pb-2">
          <p className="rounded-lg border border-accent/25 bg-accent/8 px-3 py-2 text-[15px] leading-snug text-ink">
            {input}
          </p>
        </div>
      )}

      {/* Auto-open's countdown: rule 6's "visibly counting down rather than
          firing out of nowhere" requirement. Replaces the plain transcript
          preview above rather than sitting alongside it — the whole point is
          that this state announces the *decision*, not just the words. */}
      {autoOpen && (
        <div className="shrink-0 px-5 pb-2">
          <div className="row items-center gap-2 rounded-lg border border-accent/30 bg-accent/10 px-3 py-2">
            <ShortcutIcon
              icon={autoOpen.item.icon}
              label={autoOpen.item.title}
              className="h-4 w-4 shrink-0"
            />
            <p className="min-w-0 flex-1 truncate text-[15px] leading-snug text-ink">
              Opening <span className="font-semibold">{autoOpen.item.title}</span>…
            </p>
            <button
              type="button"
              onClick={cancelAutoOpen}
              className="no-drag shrink-0 rounded-md border border-line bg-raised px-2 py-1 text-2xs font-medium text-ink-soft transition-colors hover:bg-overlay hover:text-ink"
            >
              Cancel
            </button>
          </div>
          <AutoOpenCountdownBar armedAt={autoOpen.armedAt} durationMs={AUTO_OPEN_COUNTDOWN_MS} />
        </div>
      )}

      {/* Prefix badge: shows which route Enter will take, before you commit. */}
      {wantsAiSpace ? (
        <div className="row shrink-0 px-5 pb-2">
          <span className="rounded-md border border-accent/30 bg-accent/12 px-2 py-0.5 text-2xs font-medium text-accent">
            Caduceus AI
          </span>
          <span className="truncate text-2xs text-ink-faint">
            Press <Kbd>space</Kbd> to open the AI workspace — or keep typing a question after{" "}
            <span className="font-mono text-ink-mute">{chatOpenPrefix}</span>
          </span>
        </div>
      ) : (
        parsed?.rule && (
          <div className="row shrink-0 px-5 pb-2">
            <span className="rounded-md border border-accent/30 bg-accent/12 px-2 py-0.5 text-2xs font-medium text-accent">
              {parsed.rule.label || parsed.rule.prefix}
            </span>
            <span className="truncate text-2xs text-ink-faint">{parsed.rule.description}</span>
          </div>
        )
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
        <div
          ref={setListEl}
          onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
          className="min-h-0 flex-1 overflow-y-auto px-2 py-2"
        >
          {results.length === 0 ? (
            <p className="px-3 py-8 text-center text-2xs text-ink-faint">
              {input.trim() ? "Press ↵ to run this anyway" : "Type to search"}
            </p>
          ) : (
            // The scrollable area is given its full, un-virtualised height so
            // the scrollbar and scroll position behave exactly as they would
            // for a plain list — only the *rows inside it* are windowed, each
            // pinned to the pixel offset `buildFlatRows` already worked out.
            <div style={{ position: "relative", height: totalHeight }}>
              {visibleRows.map((row) =>
                row.kind === "header" ? (
                  <p
                    key={row.key}
                    style={{ position: "absolute", top: row.top, left: 0, right: 0, height: row.height }}
                    className="eyebrow px-3 pb-1 pt-3"
                  >
                    {row.label}
                  </p>
                ) : (
                  <div
                    key={row.key}
                    style={{ position: "absolute", top: row.top, left: 0, right: 0, height: row.height }}
                  >
                    <ResultRow
                      item={row.item}
                      index={row.resultIndex}
                      active={row.resultIndex === selected}
                      query={displayQuery}
                      onHover={setSelected}
                      onRun={runItem}
                    />
                  </div>
                ),
              )}
            </div>
          )}
        </div>
      )}

      {update?.updateAvailable && (
        <div className="no-drag shrink-0 border-t border-accent/30 bg-accent/12 px-4 py-2.5">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <p className="min-w-0 text-[13px] leading-snug text-ink">
              <span className="font-semibold text-accent">
                Update available — Caduceus {update.latestVersion ?? "latest"}
              </span>
              <span className="text-ink-mute"> · you&apos;re on {update.currentVersion}</span>
            </p>
            <div className="row shrink-0 gap-2">
              <button
                type="button"
                onClick={() => onOpenTab({ kind: "settings", section: "help" })}
                className="rounded-lg border border-line bg-surface/80 px-2.5 py-1.5 text-2xs font-medium text-ink-soft transition-colors hover:bg-raised hover:text-ink"
              >
                Details in Settings
              </button>
              {update.releaseUrl && (
                <button
                  type="button"
                  onClick={() => void api.openExternalUrl(update.releaseUrl!)}
                  className="rounded-lg border border-line bg-surface/80 px-2.5 py-1.5 text-2xs font-medium text-ink-soft transition-colors hover:bg-raised hover:text-ink"
                >
                  Release notes
                </button>
              )}
              {/* Runs the website's one-liner in Terminal rather than opening
                  a download. The installer replaces the app and reopens it, so
                  the whole update is one press and a window you can watch. */}
              <button
                type="button"
                onClick={() => {
                  void api
                    .runInstallerUpdate()
                    .then(() => notify("Terminal is running the installer."))
                    .catch((error) => notify(api.errorMessage(error), "error"));
                }}
                className="rounded-lg bg-accent px-3.5 py-1.5 text-[13px] font-semibold text-accent-ink shadow-glow transition-opacity hover:opacity-95"
              >
                Update now
              </button>
            </div>
          </div>
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
            onClick={() => onOpenTab({ kind: "settings", section: update?.updateAvailable ? "help" : undefined })}
            className={cx(
              "no-drag rounded px-1.5 py-0.5 transition-colors hover:bg-raised hover:text-ink",
              update?.updateAvailable && "font-medium text-accent",
            )}
          >
            Settings
            {update?.updateAvailable && (
              <span
                aria-hidden="true"
                className="ml-1.5 inline-block h-1.5 w-1.5 translate-y-[-1px] rounded-full bg-accent shadow-glow"
              />
            )}
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

/**
 * Memoised: this is the thing that made typing feel slow.
 *
 * A one-letter query is a subsequence of nearly every command's title or
 * keywords — see `fuzzy.ts` — so the list routinely holds close to the full
 * ~200-command registry. Without `memo`, every one of those rows re-ran its
 * `highlightSegments` pass and went through full reconciliation on *every*
 * keystroke, because `HomeTab` re-renders on every keystroke (the `<input>`
 * is controlled) while `results` itself only updates once per 45ms debounce.
 * `memo` only pays off if the props below are actually stable across that
 * gap — see `runItem`'s `useCallback` and `displayQuery` in `HomeTab` for the
 * other half of this fix.
 */
const ResultRow = memo(function ResultRow({
  item,
  index,
  active,
  query,
  onHover,
  onRun,
}: {
  item: ResultItem;
  index: number;
  active: boolean;
  query: string;
  onHover: (index: number) => void;
  onRun: (item: ResultItem, asPage: boolean) => void;
}) {
  const segments = item.positions?.length
    ? highlightSegments(item.title, item.positions)
    : [{ text: item.title, match: false }];

  return (
    <div
      data-index={index}
      onMouseMove={() => onHover(index)}
      onClick={(event) => void onRun(item, event.shiftKey && Boolean(item.openPage))}
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
});

/**
 * The visible clock on auto-open's countdown — a thin bar that drains from
 * full to empty over `durationMs`, driven by a CSS transition rather than a
 * polling interval. `armedAt` is only read as a React key: a fresh timestamp
 * remounts the bar (and so restarts the animation) every time a new
 * candidate is armed, which a plain prop change would not reliably do once
 * the width is already mid-transition.
 */
function AutoOpenCountdownBar({ armedAt, durationMs }: { armedAt: number; durationMs: number }) {
  return (
    <div className="mt-1.5 h-0.5 overflow-hidden rounded-full bg-accent/15">
      <AutoOpenCountdownFill key={armedAt} durationMs={durationMs} />
    </div>
  );
}

function AutoOpenCountdownFill({ durationMs }: { durationMs: number }) {
  const barRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = barRef.current;
    if (!el) return;
    // Start full with no transition, force a reflow so the browser treats
    // the next style change as a fresh transition rather than coalescing it
    // with the one just set, then animate to empty.
    el.style.transition = "none";
    el.style.width = "100%";
    void el.offsetWidth;
    el.style.transition = `width ${durationMs}ms linear`;
    el.style.width = "0%";
  }, [durationMs]);

  return <div ref={barRef} className="h-full bg-accent" />;
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

// ---------------------------------------------------------------------------
// Empty-query grouping
// ---------------------------------------------------------------------------
//
// This is the whole change the task is about, so it is worth spelling out
// what it is and is not doing.
//
// `commandProvider`'s empty-query branch (`shared/providers.ts`) already
// answers "what should the palette show before you have typed anything?"
// with *every* command in the registry — nothing here, or anywhere in this
// file, slices that list down. It ranks the full catalogue by usage,
// personalisation and each command's shipped weight, in that order of
// precedence, and hands the result back as one group called "All commands".
// That ranking is exactly the "survey should decide the order" behaviour the
// brief asks to keep, so it is left alone.
//
// What was wrong with showing it as a single "All commands" section is
// readability, not completeness: several hundred rows under one heading is a
// wall, not a catalogue. `groupEmptyState` only relabels that one bucket —
// the first `RECOMMENDED_COUNT` rows (already the top of the ranked order)
// become "Recommended for you", and everything after them is re-sorted into
// its real category using each command's own `group` field from
// `commands.ts`, in the fixed, sensible order `COMMAND_GROUPS` already
// declares for the Settings → Features catalogue. Every other provider's
// rows — Favorites, Shortcuts, Prefixes — pass straight through unchanged,
// keeping their own heading and their own place in the ranking.

/** How many of the top-ranked commands lead the empty state, before the rest
 * fan out into their categories. Small enough to read at a glance, large
 * enough that it is not just restating what "Favorites" already shows. */
const RECOMMENDED_COUNT = 8;

const RECOMMENDED_GROUP = "Recommended for you";

/**
 * The group label `commandProvider`'s empty-query branch (`shared/providers.ts`)
 * gives the full command catalogue. Matched by string rather than importing a
 * shared constant because `ResultItem.group` is typed as a plain `string` —
 * every provider invents its own heading text. If that literal ever changes
 * there, this needs to change with it, or the catalogue would fall back to
 * being treated as an ordinary provider group (see the `default:` case in
 * `groupEmptyState`, which still shows it — just without the split into
 * "Recommended" and per-category sections).
 */
const CATALOGUE_GROUP = "All commands";

/** Every command's category, looked up once — `COMMANDS` does not change at
 * runtime, so there is nothing to gain from rebuilding this per render (or,
 * worse, per keystroke). */
const COMMAND_GROUP_BY_ID: ReadonlyMap<string, CommandGroupId> = new Map(
  COMMANDS.map((command) => [command.id, command.group]),
);

/**
 * Split the empty-query result list into "Recommended for you", the rest of
 * the catalogue bucketed by category, and every other provider's rows
 * untouched.
 *
 * Rows arrive already sorted by score (`collectResults` in `providers.ts`),
 * and nothing here re-sorts them — a `Map` remembers first-appearance order
 * for the non-catalogue groups, and each category's rows keep the relative
 * order they arrived in, so a command a user runs often still sits above one
 * they never touch *within* its own section.
 */
function groupEmptyState(items: ResultItem[]): [string, ResultItem[]][] {
  const groups = new Map<string, ResultItem[]>();
  const byCategory = new Map<CommandGroupId, ResultItem[]>();
  let recommendedRemaining = RECOMMENDED_COUNT;

  for (const item of items) {
    if (item.group !== CATALOGUE_GROUP) {
      const bucket = groups.get(item.group);
      if (bucket) bucket.push(item);
      else groups.set(item.group, [item]);
      continue;
    }

    if (recommendedRemaining > 0) {
      const bucket = groups.get(RECOMMENDED_GROUP);
      if (bucket) bucket.push(item);
      else groups.set(RECOMMENDED_GROUP, [item]);
      recommendedRemaining -= 1;
      continue;
    }

    const commandId = item.id.startsWith("command:") ? item.id.slice("command:".length) : "";
    const categoryId = COMMAND_GROUP_BY_ID.get(commandId) ?? "utilities";
    const bucket = byCategory.get(categoryId);
    if (bucket) bucket.push(item);
    else byCategory.set(categoryId, [item]);
  }

  const sections = [...groups.entries()];
  // Categories are appended in `COMMAND_GROUPS`'s own fixed order rather than
  // first-appearance order — the point of splitting the catalogue up at all
  // is a browsable, predictable shape, and "whichever category happened to
  // contain the highest-ranked leftover command" is not that.
  for (const { id, title } of COMMAND_GROUPS) {
    const rows = byCategory.get(id);
    if (rows && rows.length > 0) sections.push([title, rows]);
  }
  return sections;
}

// ---------------------------------------------------------------------------
// Virtualised list
// ---------------------------------------------------------------------------
//
// The empty-query state above is the reason this exists: with `RECOMMENDED_COUNT`
// plus every category, the full catalogue plus Favorites, Shortcuts and
// Prefixes routinely comes to several hundred rows. Mounting all of them as
// real DOM nodes is not the thing that made typing slow before `ResultRow`
// was memoised — that was every row *re-rendering* on every keystroke — but
// it is still hundreds of nodes for the browser to lay out and paint on the
// very first frame the palette opens, and hundreds more to keep alive for a
// session where the query rarely gets long enough to filter them out. Only
// rendering the rows actually inside (or just outside) the visible area
// keeps that cost roughly constant no matter how large the catalogue grows.
//
// No dependency: the whole thing is fixed-height rows and header rows, which
// is what makes hand-rolling it tractable — a variable-height virtualiser
// earns its keep from a library; this one does not need to.

/** Rendered height of one `ResultRow`, in pixels: the 28px icon badge plus
 * `py-2`'s 8px top and bottom padding. If `ResultRow`'s own layout changes,
 * this has to change with it — there is no way to measure it without
 * defeating the point of not mounting every row. */
const VIRTUAL_ROW_HEIGHT = 44;

/** Rendered height of a section heading (`eyebrow`, `pt-3 pb-1`, one line of
 * small caps text) — a few pixels taller than the padding alone accounts for,
 * which is deliberate: it stands in for the `mb-1` gap the old, non-virtualised
 * `<div className="mb-1">` per group used to add between sections. */
const VIRTUAL_HEADER_HEIGHT = 32;

/** How far past the visible edges to keep rows mounted. Large enough that a
 * fast scroll or a `PageDown`-sized keyboard jump does not show a flash of
 * blank space while new rows mount; small enough that it is still a sliver
 * of the full list rather than most of it. */
const VIRTUAL_OVERSCAN_PX = 320;

interface FlatHeaderRow {
  kind: "header";
  key: string;
  top: number;
  height: number;
  label: string;
}

interface FlatItemRow {
  kind: "item";
  key: string;
  top: number;
  height: number;
  item: ResultItem;
  resultIndex: number;
}

type FlatRow = FlatHeaderRow | FlatItemRow;

/**
 * Turn the grouped sections into one array with a known pixel offset per row.
 *
 * This is the only layout the virtualiser needs: given a scroll position and
 * a viewport height, "which rows are visible" and "where is row N" are both
 * answered by a single pass over fixed-height rows, no measurement required.
 */
function buildFlatRows(
  grouped: [string, ResultItem[]][],
  indexById: Map<string, number>,
): {
  rows: FlatRow[];
  totalHeight: number;
  offsetByResultIndex: Map<number, { top: number; height: number }>;
} {
  const rows: FlatRow[] = [];
  const offsetByResultIndex = new Map<number, { top: number; height: number }>();
  let top = 0;

  for (const [group, items] of grouped) {
    rows.push({ kind: "header", key: `header:${group}`, top, height: VIRTUAL_HEADER_HEIGHT, label: group });
    top += VIRTUAL_HEADER_HEIGHT;

    for (const item of items) {
      const resultIndex = indexById.get(item.id) ?? 0;
      rows.push({ kind: "item", key: item.id, top, height: VIRTUAL_ROW_HEIGHT, item, resultIndex });
      offsetByResultIndex.set(resultIndex, { top, height: VIRTUAL_ROW_HEIGHT });
      top += VIRTUAL_ROW_HEIGHT;
    }
  }

  return { rows, totalHeight: top, offsetByResultIndex };
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
