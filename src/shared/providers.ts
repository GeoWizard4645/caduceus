/**
 * The Command Center's result-source interface.
 *
 * This is Caduceus's frontend extension point. A provider takes the current query
 * and returns rows; the palette merges, ranks and renders them. Adding a new
 * source — open browser tabs, a project list, calculator results — means
 * writing one object and appending it to {@link defaultProviders}.
 *
 * See `docs/PLUGIN_GUIDE.md` for a worked example.
 */

import { invoke } from "@tauri-apps/api/core";

import * as api from "./api";
import {
  COMMANDS,
  commandWeight,
  matchTrigger,
  type CommandDef,
  type CommandOutput,
} from "./commands";
import { usageBoost } from "./usage";
import { personalizationBoost } from "./personalization";
import { convert as convertUnits, formatValue, parseConversion } from "./units";
import { fuzzyMatch, fuzzyScore } from "./fuzzy";
import type { Tab } from "./tabs";
import type {
  CalcResult,
  ClipboardEntry,
  Extension,
  InstalledApp,
  ParsedInput,
  Settings,
  Shortcut,
} from "./types";

/** One row in the palette. */
export interface ResultItem {
  /** Unique within a render pass. Prefix with your provider id. */
  id: string;
  title: string;
  subtitle?: string;
  /** Emoji or short string; rendered in the leading badge. */
  icon: string;
  /** Section heading this row appears under. */
  group: string;
  /** Higher sorts first. Roughly 0–1000; see `fuzzy.ts`. */
  score: number;
  /** Right-aligned hint, e.g. a keyboard shortcut or a timestamp. */
  accessory?: string;
  /** Character indices in `title` to highlight. */
  positions?: number[];
  /**
   * Stable id used to count how often this row is run.
   *
   * Set it for anything whose identity survives a restart — a command, an
   * application, a shortcut. Left unset for rows that are one-offs (a clipboard
   * entry, a web search), where a count would mean nothing.
   */
  usageKey?: string;
  /**
   * Ask before running, showing this sentence. Enter a second time confirms.
   *
   * Used for the handful of rows that end the session or delete something: in a
   * fuzzy list, "Shut down" is one keystroke away from "Sleep".
   */
  confirm?: string;
  /** Runs when the row is chosen. Return `false` to keep the palette open. */
  run: () => void | boolean | Promise<void | boolean>;
  /**
   * Opens this row's own page instead of running it, on ⇧↵.
   *
   * Set by anything that *has* a page — every built-in command does. Rows
   * without one (a clipboard entry, a web search) simply do not offer it.
   */
  openPage?: () => void | boolean | Promise<void | boolean>;
}

/** Everything a provider is given to answer a query. */
export interface ProviderContext {
  /** Text after any prefix has been stripped. */
  query: string;
  /** The raw input, including the prefix. */
  raw: string;
  /** How the input parsed, from the Rust side. */
  parsed: ParsedInput | null;
  settings: Settings;
  /** Clipboard rows already fetched for this query. */
  clipboard: ClipboardEntry[];
  /** Ask the palette to do something (close, switch mode, show a message). */
  actions: PaletteActions;
}

export interface PaletteActions {
  close: () => void;
  setInput: (value: string) => void;
  /** Open (or focus) a tab. Anything with state worth keeping goes in one. */
  openTab: (request: Omit<Tab, "id">) => void;
  notify: (message: string, tone?: "info" | "error") => void;
  /** Show text in the palette's output panel, with a copy button. */
  showOutput: (output: CommandOutput) => void;
}

export interface ResultProvider {
  id: string;
  /** Section heading used for this provider's rows. */
  title: string;
  search(ctx: ProviderContext): ResultItem[] | Promise<ResultItem[]>;
}

// ---------------------------------------------------------------------------
// Built-in providers
// ---------------------------------------------------------------------------

/** Shortcuts from Settings, fuzzy-matched on label, description and keywords. */
export const shortcutProvider: ResultProvider = {
  id: "shortcuts",
  title: "Shortcuts",
  search({ query, settings, actions }) {
    const visible = settings.shortcuts.filter((s) => !s.hidden);

    // An empty query shows pinned (staff) shortcuts in their configured order,
    // so the palette is useful before you have typed anything.
    if (!query) {
      return visible
        .filter((s) => s.showInStaff)
        .sort((a, b) => a.orderIndex - b.orderIndex)
        .map((shortcut, index) => toItem(shortcut, 500 - index, undefined, actions));
    }

    return visible
      .map((shortcut) => {
        const match = fuzzyMatch(query, shortcut.label);
        const score = fuzzyScore(query, [
          shortcut.label,
          shortcut.description,
          ...shortcut.keywords,
        ]);
        return score === null ? null : toItem(shortcut, score, match?.positions, actions);
      })
      .filter((x): x is ResultItem => x !== null);
  },
};

function toItem(
  shortcut: Shortcut,
  score: number,
  positions: number[] | undefined,
  actions: PaletteActions,
): ResultItem {
  return {
    id: `shortcut:${shortcut.id}`,
    usageKey: `shortcut:${shortcut.id}`,
    title: shortcut.label,
    subtitle: shortcut.description || describeTarget(shortcut),
    icon: shortcut.icon || shortcut.label.charAt(0).toUpperCase(),
    group: "Shortcuts",
    score,
    positions,
    run: async () => {
      const outcome = await api.runShortcut(shortcut.id);
      // These are handled here rather than in Rust because "switch the palette
      // into another mode" is a UI concept with no backend meaning.
      if (outcome.frontendAction === "clipboard_view") {
        actions.openTab({ kind: "clipboard" });
        return false;
      }
      if (outcome.frontendAction === "system_monitor") {
        actions.openTab({ kind: "system" });
        return false;
      }
      const feature = featureFromAction(outcome.frontendAction);
      if (feature) {
        const command = COMMANDS.find((c) => c.id === feature);
        if (!command) {
          actions.notify(`This shortcut points at a feature that no longer exists.`, "error");
          return false;
        }
        return command.run({ input: "", values: {}, actions });
      }
      if (!outcome.ok) {
        actions.notify(outcome.message, "error");
        return false;
      }
      return true;
    },
  };
}

/**
 * The command id inside an `open_feature:<id>` action, if that is what it is.
 *
 * Rust sends the id back as part of the action string rather than as its own
 * field because `ExecOutcome.frontend_action` is a single string that several
 * kinds already share, and widening it would mean touching every caller.
 */
function featureFromAction(action: string | null | undefined): string | null {
  const prefix = "open_feature:";
  return action?.startsWith(prefix) ? action.slice(prefix.length) : null;
}

function describeTarget(shortcut: Shortcut): string {
  switch (shortcut.kind) {
    case "open_url":
      return shortcut.target.replace(/^https?:\/\//, "");
    case "open_app":
      return shortcut.target || "No application set";
    case "run_command":
      return shortcut.target;
    case "run_applescript":
      return "AppleScript";
    case "open_feature":
      return COMMANDS.find((c) => c.id === shortcut.target)?.title ?? "A Caduceus feature";
    case "clipboard_view":
      return "Browse clipboard history";
    case "system_monitor":
      return "CPU, memory, disks and processes";
  }
}

/**
 * Recent clipboard entries — **only** when you are asking for them.
 *
 * These used to appear in every result list. That is wrong in a way that took a
 * while to see: the clipboard is a history of text, so *something* in it fuzzy-
 * matches almost anything you type, and the matches are meaningless. Searching
 * for "chrome" would surface the word "chrome" from a message you pasted last
 * Tuesday, ranked alongside the application.
 *
 * So they are gated behind the clipboard prefix (`/v` by default) and the
 * clipboard tab, which is where somebody who wants their clipboard history goes.
 */
export const clipboardProvider: ResultProvider = {
  id: "clipboard",
  title: "Clipboard",
  search({ clipboard, query, parsed, actions, settings }) {
    // The prefix router hands this provider the query with `/v` stripped, and
    // sets `rule` to the clipboard action. No rule means the user was searching
    // for something else and these rows are noise.
    if (parsed?.rule?.action !== "clipboard_search") return [];

    const limit = settings.commandCenter.maxResultsPerSource;
    return clipboard.slice(0, limit).map((entry, index) => ({
      id: `clipboard:${entry.id}`,
      title: entry.preview || "(empty)",
      subtitle: [
        entry.sourceApp,
        entry.kind === "image" ? `${entry.width}×${entry.height}` : null,
        relativeTime(entry.createdAt),
      ]
        .filter(Boolean)
        .join(" · "),
      icon: entry.pinned ? "★" : entry.kind === "image" ? "▣" : entry.kind === "files" ? "⌥" : "≡",
      group: "Clipboard",
      // Top of the list: reaching this provider at all means the user asked
      // for their clipboard specifically, so nothing else should outrank it.
      score: 900 - index,
      positions: query ? (fuzzyMatch(query, entry.preview)?.positions ?? undefined) : undefined,
      run: async () => {
        try {
          await api.clipboardCopy(entry.id);
          actions.notify("Copied to clipboard");
          return true;
        } catch (error) {
          actions.notify(api.errorMessage(error), "error");
          return false;
        }
      },
    }));
  },
};

/**
 * Hints for the configured prefixes, shown on an empty query so the routing
 * rules are discoverable instead of being something you have to read the README
 * to find out about.
 */
export const prefixHintProvider: ResultProvider = {
  id: "prefixes",
  title: "Prefixes",
  search({ raw, settings, actions }) {
    if (raw.trim()) return [];

    const hints: ResultItem[] = settings.commandCenter.prefixes
      .filter((rule) => rule.showHint && rule.prefix.trim())
      .map((rule, index) => ({
        id: `prefix:${rule.id}`,
        title: rule.label || rule.prefix,
        subtitle: rule.description,
        icon: rule.prefix,
        group: "Prefixes",
        // Above clipboard on the empty query: prefixes are how you discover
        // that Caduceus does anything beyond opening bookmarks, and burying them
        // under a scroll means nobody finds them.
        score: 300 - index,
        accessory: `${rule.prefix} …`,
        run: () => {
          // Selecting a hint types the prefix for you rather than running
          // anything: there is no query yet.
          actions.setInput(`${rule.prefix} `);
          return false;
        },
      }));

    hints.push({
      id: "prefix:settings",
      title: "Settings",
      subtitle: "Shortcuts, prefixes, AI backends, clipboard, appearance",
      icon: "⚙",
      group: "Prefixes",
      score: 280,
      accessory: "⌘,",
      run: async () => {
        await api.openSettingsWindow();
      },
    });

    return hints;
  },
};

/**
 * The fallback row: whatever you typed, as a web search. Always present so
 * Enter never does nothing.
 */
export const searchFallbackProvider: ResultProvider = {
  id: "search",
  title: "Search",
  search({ query, parsed, settings }) {
    // Only for the no-prefix path; a prefixed input has its own destination.
    if (!query || (parsed && parsed.rule)) return [];

    const engine = hostOf(settings.commandCenter.searchUrlTemplate);
    return [
      {
        id: "search:web",
        title: query,
        subtitle: `Search ${engine}`,
        icon: "⌕",
        group: "Search",
        // Below a strong shortcut match, above a weak one.
        score: 260,
        accessory: "↵",
        run: async () => {
          await api.dispatchInput(query);
          return true;
        },
      },
    ];
  },
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

export function hostOf(urlTemplate: string): string {
  try {
    return new URL(urlTemplate.replace("{query}", "x")).hostname.replace(/^www\./, "");
  } catch {
    return "the web";
  }
}

export function relativeTime(timestampMs: number): string {
  const seconds = Math.max(0, Math.round((Date.now() - timestampMs) / 1000));
  if (seconds < 45) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.round(hours / 24);
  if (days < 7) return `${days}d ago`;
  return new Date(timestampMs).toLocaleDateString();
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}

/** Run every provider and return rows sorted by score. */
export async function collectResults(
  providers: ResultProvider[],
  ctx: ProviderContext,
): Promise<ResultItem[]> {
  const settled = await Promise.allSettled(providers.map((p) => p.search(ctx)));

  const items: ResultItem[] = [];
  settled.forEach((result, index) => {
    if (result.status === "fulfilled") {
      items.push(...result.value);
    } else {
      // A broken provider must not take the palette down with it.
      console.error(`result provider "${providers[index].id}" failed:`, result.reason);
    }
  });

  return items.sort((a, b) => b.score - a.score);
}

// ---------------------------------------------------------------------------
// Launcher
// ---------------------------------------------------------------------------

/**
 * Every installed application, fuzzy-matched by name.
 *
 * The list is fetched from Rust once and cached in the module, because a
 * filesystem scan per keystroke would make the palette feel awful. Rust caches
 * it too (with a TTL), so the worst case after a cold start is one ~100ms call.
 */
let appCache: InstalledApp[] | null = null;
let appLoad: Promise<InstalledApp[]> | null = null;

async function installedApps(): Promise<InstalledApp[]> {
  if (appCache) return appCache;
  // Concurrent callers share one in-flight request.
  appLoad ??= api
    .listInstalledApps()
    .then((apps) => {
      appCache = apps;
      return apps;
    })
    .catch(() => [] as InstalledApp[])
    .finally(() => {
      appLoad = null;
    });
  return appLoad;
}

/** Drop the cache — call after the user installs something. */
export function invalidateAppCache(): void {
  appCache = null;
}

export const appLauncherProvider: ResultProvider = {
  id: "apps",
  title: "Applications",
  async search({ query, settings, actions, parsed }) {
    // Only for the plain path: "/c open mail" is a task, not a launch.
    if (!query || parsed?.rule) return [];

    const apps = await installedApps();
    const limit = settings.commandCenter.maxResultsPerSource;

    return apps
      .map((app) => {
        const match = fuzzyMatch(query, app.name);
        return match ? { app, match } : null;
      })
      .filter((x): x is { app: InstalledApp; match: NonNullable<ReturnType<typeof fuzzyMatch>> } => x !== null)
      .sort((a, b) => b.match.score - a.match.score)
      .slice(0, limit)
      .map(({ app, match }) => ({
        id: `app:${app.path}`,
        usageKey: `app:${app.path}`,
        title: app.name,
        subtitle: "Application",
        icon: "▣",
        group: "Applications",
        // Applications lead the list.
        //
        // Typing a name is overwhelmingly a request to launch that thing, and
        // when it is not, the command underneath is one arrow key away. The
        // reverse — a command winning because its description happened to
        // contain the word — means typing "docker" and getting a lecture about
        // containers instead of Docker. `APP_LEAD` is what buys that: enough to
        // clear a strong command match, not so much that a two-letter fuzzy
        // hit on an app you have never opened beats an exact command name.
        score: match.score + APP_LEAD + usageBoost(`app:${app.path}`),
        positions: match.positions,
        accessory: "↵",
        run: async () => {
          const outcome = await api.launchApp(app.path);
          if (!outcome.ok) {
            actions.notify(outcome.message, "error");
            return false;
          }
        },
      }));
  },
};

/**
 * Local files, matched on every plain query — not just behind `file`/`find`.
 *
 * `liveListProvider`'s file search only fires once you have already typed the
 * word "file", which is right for a deliberate browse but wrong for the far
 * more common case: typing a filename because you want that file, the same way
 * typing an app's name means you want that app. So this runs unconditionally,
 * scored to land where that expectation puts it — after Applications, before
 * Commands, and never as its own tab or page.
 *
 * Gated at two characters rather than one: `mdfind` is Spotlight's own index,
 * not a directory walk, so it is fast, but a single letter still matches
 * enough of the disk to be noise on every keystroke.
 */
export const fileSearchProvider: ResultProvider = {
  id: "files-inline",
  title: "Files",
  async search({ query, parsed, settings, actions }) {
    if (parsed?.rule) return [];
    const trimmed = query.trim();
    if (trimmed.length < 2) return [];

    // Capped well under the source limit: files are a supporting result here,
    // not the reason the palette is open, and five is enough to catch "the
    // thing I meant" without pushing every command off the visible list.
    const limit = Math.min(5, settings.commandCenter.maxResultsPerSource);

    let hits: Awaited<ReturnType<typeof api.searchFiles>>;
    try {
      hits = await api.searchFiles(trimmed, limit);
    } catch {
      // A slow or failing Spotlight index must not empty the palette.
      return [];
    }

    return hits.map((hit, index) => {
      const match = fuzzyMatch(trimmed, hit.name);
      return {
        id: `file-inline:${hit.path}`,
        title: hit.name,
        subtitle: hit.path.replace(/^\/Users\/[^/]+/, "~"),
        icon: "▤",
        group: "Files",
        score: (match?.score ?? 500 - index) + FILE_LEAD,
        positions: match?.positions ?? undefined,
        accessory: "↵ reveal",
        run: () => rowOutcome(actions, () => api.revealPath(hit.path)),
      };
    });
  },
};

// ---------------------------------------------------------------------------
// Calculator
// ---------------------------------------------------------------------------

/**
 * Arithmetic typed into the bar, evaluated in Rust.
 *
 * Returns nothing unless the input really is a calculation, so ordinary
 * searches never grow a spurious result row.
 */
export const calculatorProvider: ResultProvider = {
  id: "calculator",
  title: "Calculator",
  async search({ query, parsed, actions }) {
    if (!query || parsed?.rule) return [];

    let result: CalcResult | null = null;
    try {
      result = await api.calculate(query);
    } catch {
      return [];
    }
    if (!result) return [];

    return [
      {
        id: "calc:result",
        title: result.display,
        subtitle: result.expression,
        icon: "=",
        group: "Calculator",
        // Above everything: if it parsed as maths, it is what you meant.
        score: 900,
        accessory: "↵ copy",
        run: async () => {
          try {
            await navigator.clipboard.writeText(result.display);
            actions.notify(`Copied ${result.display}`);
          } catch {
            actions.notify("Could not copy the result", "error");
          }
        },
      },
    ];
  },
};

/**
 * `12 km to miles` answered inline, the way `2+2` already is.
 *
 * Conversion is the other thing people type into a search box expecting a
 * number back, and until now Caduceus sent it to Google. It is pure arithmetic
 * on definitions — see `shared/units.ts` — so it runs here with no network and
 * cannot be stale.
 *
 * Currency is deliberately *not* here. Its answer depends on what day it is, so
 * it lives on the Convert page where the source and the date can be shown.
 */
export const conversionProvider: ResultProvider = {
  id: "conversion",
  title: "Conversion",
  search({ raw, parsed, actions }) {
    if (parsed?.rule) return [];

    const conversion = parseConversion(raw);
    if (!conversion) return [];

    const result = convertUnits(conversion.value, conversion.from, conversion.to);
    if (result === null) return [];

    const answer = `${formatValue(result)} ${conversion.to.symbol}`;
    return [
      {
        id: "conversion:result",
        title: answer,
        subtitle: `${formatValue(conversion.value)} ${conversion.from.name} in ${conversion.to.name}`,
        icon: "⇄",
        group: "Conversion",
        // Above everything. Typing a conversion is unambiguous, and it should
        // never lose to an app whose name happens to share a letter with "km".
        score: 980,
        accessory: "↵ copies",
        run: async () => {
          try {
            await navigator.clipboard.writeText(formatValue(result));
            actions.notify(`Copied ${answer}`);
          } catch {
            actions.notify("Could not copy the result", "error");
          }
        },
      },
    ];
  },
};

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/**
 * Providers the palette queries, in declaration order. Ordering here is only
 * cosmetic — rows are ranked by score — but it keeps related sources together
 * when several tie.
 */
/** Screenshots and screen recordings. */
export const captureProvider: ResultProvider = {
  id: "capture",
  title: "Capture",
  search({ query, actions }) {
    const q = query.trim().toLowerCase();
    const wantsCapture =
      q.length === 0 ||
      /screenshot|screen shot|screen record|recording|capture/.test(q);
    if (!wantsCapture) return [];

    const items: ResultItem[] = [
      {
        id: "capture-screenshot",
        title: "Screenshot the display",
        subtitle: "Copies to the clipboard and saves a PNG to Downloads",
        icon: "📷",
        group: "Capture",
        score: q.includes("screenshot") ? 900 : 400,
        run: async () => {
          try {
            const result = await api.captureScreenshot(true);
            actions.notify(result.message);
          } catch (e) {
            actions.notify(api.errorMessage(e), "error");
          }
        },
      },
      // Recording is a page, not four palette rows.
      //
      // These used to be three "start recording" rows and a "stop" row, and
      // every one of them shelled out to Screenshot.app — the ⇧⌘5 overlay,
      // which cannot capture system audio at all. One of them said so in its
      // own subtitle. Caduceus has a real recorder now (ScreenCaptureKit, see
      // `capture::recorder`), and it has controls, a clock and a level meter,
      // none of which fit in a palette row.
      {
        id: "capture-record",
        title: "Record the screen",
        subtitle: "With the audio your Mac is playing — the thing ⇧⌘5 cannot do",
        icon: "⏺",
        group: "Capture",
        score: /record|video|screencast/.test(q) ? 880 : 350,
        run: () => {
          actions.openTab({
            kind: "tool",
            commandId: "page.screen-record",
            title: "Record the screen",
          });
          return false;
        },
      },
      {
        id: "capture-record-meeting",
        title: "Record a meeting",
        subtitle: "Both sides of the call, transcribed on-device, notes beside it",
        icon: "◉",
        group: "Capture",
        score: /meeting|call|transcri|notes/.test(q) ? 900 : 300,
        run: () => {
          actions.openTab({ kind: "tool", commandId: "page.meeting", title: "Meeting notes" });
          return false;
        },
      },
    ];
    return items;
  },
};


// ---------------------------------------------------------------------------
// Built-in commands
// ---------------------------------------------------------------------------

/**
 * Every entry in the command registry, ranked alongside apps and shortcuts.
 *
 * The registry is deliberately searched with the *same* fuzzy scorer the
 * shortcut provider uses, over the same three fields (title, description,
 * keywords). That is what makes "half" find the window snap, "hash" find
 * SHA-256, and a command sit in one ranked list with everything else rather
 * than in a submenu you have to know exists.
 */
/** Opens the Caduceus AI tab from plain search terms (ai, chat, local, …). */
export const aiWorkspaceProvider: ResultProvider = {
  id: "ai",
  title: "AI",
  search({ query, parsed, settings, actions }) {
    if (parsed?.rule) return [];

    const score = fuzzyScore(query, [
      "Caduceus AI",
      "AI chat assistant",
      "chat with AI",
      "local models",
      "local AI",
      "ollama",
      "hermes",
      "llm",
    ]);
    if (score === null) return [];

    const aiPrefix =
      settings.commandCenter.prefixes.find((p) => p.action === "primary_ai")?.prefix ?? "/";

    return [
      {
        id: "ai:workspace",
        usageKey: "ai:workspace",
        title: "Caduceus AI",
        subtitle: `Chat, Cowork, and local models · ${aiPrefix} then space in Search`,
        icon: "⚕",
        group: "AI",
        score: score + 120 + usageBoost("ai:workspace"),
        positions: fuzzyMatch(query, "Caduceus AI")?.positions,
        accessory: "↵",
        openPage: () => {
          actions.openTab({ kind: "settings", section: "ai" });
          return false;
        },
        run: () => {
          actions.openTab({ kind: "chat", chatMode: "chat" });
          return false;
        },
      },
    ];
  },
};

export const favoritesProvider: ResultProvider = {
  id: "favorites",
  title: "Favorites",
  search({ query, settings, actions }) {
    if (query) return [];

    const ids = settings.general.personalization?.favoriteCommandIds ?? [];
    if (ids.length === 0) return [];

    const byId = new Map(COMMANDS.map((c) => [c.id, c]));
    const rows: ResultItem[] = [];

    ids.forEach((commandId, index) => {
      const command = byId.get(commandId);
      if (!command) return;
      rows.push({
        id: `favorite:${command.id}`,
        usageKey: `command:${command.id}`,
        title: command.title,
        subtitle: command.detail,
        icon: command.icon,
        group: "Favorites",
        score: 540 - index,
        accessory: accessoryFor(command),
        confirm: command.argument ? undefined : command.confirm,
        openPage: () => openCommandPage(command, "", actions),
        run: () => runCommand(command, "", actions),
      });
    });

    return rows;
  },
};

export const commandProvider: ResultProvider = {
  id: "commands",
  title: "Commands",
  search({ query, raw, parsed, actions, settings }) {
    // A prefixed input has its own destination; "/c open mail" is a task.
    if (parsed?.rule) return [];

    // `sha256 hello` — the first word names a command, the rest is its input.
    const triggered = matchTrigger(raw);
    if (triggered) {
      const { command, input } = triggered;
      const row: ResultItem = {
        id: `command:${command.id}`,
        title: command.title,
        subtitle: input
          ? `${command.detail.split(".")[0]} — on “${truncate(input, 48)}”`
          : needsItsPage(command, input)
            ? `Opens its page, with a box for the ${command.argument}`
            : command.detail,
        icon: command.icon,
        group: "Commands",
        // Above everything: naming a command by its trigger word is
        // unambiguous, and outranking the calculator here is the point.
        score: 950,
        accessory: "↵",
        confirm: input ? command.confirm : undefined,
        openPage: () => openCommandPage(command, input, actions),
        run: () => runCommand(command, input, actions),
      };

      // With an argument — `sha256 hello` — the trigger row is the whole
      // answer. You have named a command and given it something to work on,
      // and fuzzy-matching the argument as well would bury it in noise.
      if (input) return [row];

      // On its own, the trigger word is a *search*, and this used to return
      // only the triggered row. That silently hid every other command with the
      // same name in it: typing "color" matched `tool.color_convert`'s trigger
      // and so the Colors page — a better answer — never appeared at all,
      // while "colour" worked because nothing claims it as a trigger.
      //
      // The trigger row still outranks everything, because naming a command
      // exactly is unambiguous. It just no longer deletes the alternatives.
      const rest = COMMANDS.filter((entry) => entry.id !== command.id)
        .map((entry) => {
          const score = fuzzyScore(query, [entry.title, entry.detail, ...entry.keywords]);
          return score === null ? null : { entry, score };
        })
        .filter((hit): hit is { entry: CommandDef; score: number } => hit !== null)
        .map(({ entry, score }) => ({
          id: `command:${entry.id}`,
          usageKey: `command:${entry.id}`,
          title: entry.title,
          subtitle: entry.detail,
          icon: entry.icon,
          group: "Commands",
          // Capped below the trigger row's 950 so the exact match stays first.
          score: Math.min(
            score - 10 + usageBoost(`command:${entry.id}`) + personalizationBoost(settings, entry.id),
            900,
          ),
          positions: fuzzyMatch(query, entry.title)?.positions,
          accessory: accessoryFor(entry),
          confirm: entry.argument ? undefined : entry.confirm,
          openPage: () => openCommandPage(entry, "", actions),
          run: () => runCommand(entry, "", actions),
        }));

      return [row, ...rest];
    }

    // Nothing typed: show the whole catalogue, ranked. Commands that are only
    // discoverable once you already know their name are not discoverable, and
    // the empty palette is the one place with room to list them.
    if (!query) {
      const ranked = [...COMMANDS].sort(
        (a, b) =>
          usageBoost(`command:${b.id}`) +
          commandWeight(b) +
          personalizationBoost(settings, b.id) -
          (usageBoost(`command:${a.id}`) +
            commandWeight(a) +
            personalizationBoost(settings, a.id)),
      );

      return ranked.map((command, index) => ({
        id: `command:${command.id}`,
        usageKey: `command:${command.id}`,
        title: command.title,
        subtitle: command.detail,
        icon: command.icon,
        group: "All commands",
        // Below the prefix hints and the clipboard, which is where a list this
        // long belongs — it is a reference, not a suggestion.
        score: EMPTY_STATE_BASE - index,
        accessory: accessoryFor(command),
        confirm: command.argument ? undefined : command.confirm,
        openPage: () => openCommandPage(command, "", actions),
        run: () => runCommand(command, "", actions),
      }));
    }

    return COMMANDS.map((command): ResultItem | null => {
      const match = fuzzyMatch(query, command.title);
      const score = fuzzyScore(query, [command.title, command.detail, ...command.keywords]);
      if (score === null) return null;

      return {
        id: `command:${command.id}`,
        usageKey: `command:${command.id}`,
        title: command.title,
        subtitle: command.detail,
        icon: command.icon,
        group: "Commands",
        // Just under a shortcut the user configured themselves, and under an
        // exact app-name match, but above a web search. The usage boost is what
        // lets a command you run daily overtake a closer textual match you never
        // touch.
        score: score - 10 + usageBoost(`command:${command.id}`) + personalizationBoost(settings, command.id),
        positions: match?.positions,
        accessory: accessoryFor(command),
        confirm: command.argument ? undefined : command.confirm,
        openPage: () => openCommandPage(command, "", actions),
        run: () => runCommand(command, "", actions),
      };
    }).filter((item): item is ResultItem => item !== null);
  },
};

/** Open a command's own page, optionally with what has already been typed. */
function openCommandPage(command: CommandDef, input: string, actions: PaletteActions): false {
  actions.openTab({
    kind: "tool",
    commandId: command.id,
    title: command.title,
    icon: command.icon,
    prefill: input || undefined,
  });
  // The palette stays; the page opened beside it.
  return false;
}

/**
 * Run a command from the palette — or open its page when running is not what
 * the user can have meant.
 *
 * A command that takes an argument, chosen from the list with nothing typed
 * after it, used to run on an empty string and come back with "Type something
 * after the command first." Nobody picked the row wanting that sentence: they
 * picked it wanting the tool. So they get the tool, with a box in it.
 */
function runCommand(command: CommandDef, input: string, actions: PaletteActions) {
  if (needsItsPage(command, input)) {
    return openCommandPage(command, input, actions);
  }
  // No form was filled in — this came straight from the palette — so the one
  // value there is goes in under the id every default form uses.
  return command.run({ input, actions, values: { input } });
}

/** Whether picking this row with this input should open the page instead. */
function needsItsPage(command: CommandDef, input: string): boolean {
  return Boolean(command.argument) && !command.argumentOptional && !input.trim();
}

/** The right-hand hint: what Enter is about to do. */
function accessoryFor(command: CommandDef): string {
  if (command.argument && !command.argumentOptional) return "opens ▸";
  return command.trigger ? `${command.trigger} …` : "↵";
}

/**
 * Where the full command list starts on an empty query.
 *
 * Below the prefix hints so the catalogue sits at the bottom, and far enough
 * above nothing that a hundred and fifty descending scores never collide with
 * another provider.
 */
const EMPTY_STATE_BASE = 180;

/**
 * How far an application's fuzzy score is lifted above a command's.
 *
 * The one number that decides "type a name, get the app". Chosen against the
 * command provider's `score - 10`: a command needs to beat an app by more than
 * 55 points of fuzzy match to lead, which in practice means the app has to be a
 * poor match and the command an excellent one.
 */
const APP_LEAD = 45;

/**
 * How far a matched file's fuzzy score is lifted above a command's.
 *
 * Between the two: an app is overwhelmingly a request to launch it, but a file
 * on your Mac named close to what you typed is still more likely to be what you
 * meant than a command whose *description* happens to contain the word — so
 * files sit above `commandProvider`'s `score - 10` and below `APP_LEAD`.
 */
const FILE_LEAD = 15;

function truncate(text: string, max: number): string {
  return text.length <= max ? text : `${text.slice(0, max - 1)}…`;
}

// ---------------------------------------------------------------------------
// Live lists
// ---------------------------------------------------------------------------

/**
 * Sources that have to ask the system what exists before they can answer.
 *
 * Each one is gated behind a leading keyword, so the cost — an `lsof`, a
 * directory scan, a `docker ps` — is only ever paid when the query actually
 * begins with the word that asks for it. Typing "safari" runs none of them.
 */
type LiveKind = "ports" | "repos" | "ssh" | "docker" | "audio" | "files" | "big";

const LIVE_TRIGGERS: { kind: LiveKind; words: string[] }[] = [
  { kind: "ports", words: ["port", "ports", "listening"] },
  { kind: "repos", words: ["repo", "repos", "git", "project"] },
  { kind: "ssh", words: ["ssh", "host"] },
  { kind: "docker", words: ["docker", "container", "containers"] },
  { kind: "audio", words: ["audio", "output", "input", "speaker", "mic", "microphone"] },
  { kind: "files", words: ["file", "files", "find"] },
  { kind: "big", words: ["large", "big", "biggest", "space"] },
];

function liveKind(raw: string): { kind: LiveKind; rest: string } | null {
  const trimmed = raw.trim();
  if (!trimmed) return null;
  const space = trimmed.search(/\s/);
  const head = (space === -1 ? trimmed : trimmed.slice(0, space)).toLowerCase();
  const rest = space === -1 ? "" : trimmed.slice(space + 1).trim();

  const entry = LIVE_TRIGGERS.find((candidate) => candidate.words.includes(head));
  return entry ? { kind: entry.kind, rest } : null;
}

export const liveListProvider: ResultProvider = {
  id: "live",
  title: "System",
  async search({ raw, parsed, settings, actions }) {
    if (parsed?.rule) return [];
    const matched = liveKind(raw);
    if (!matched) return [];

    const limit = settings.commandCenter.maxResultsPerSource;

    try {
      switch (matched.kind) {
        case "ports":
          return await portRows(matched.rest, limit, actions);
        case "repos":
          return await repoRows(matched.rest, limit, actions);
        case "ssh":
          return await sshRows(matched.rest, limit, actions);
        case "docker":
          return await dockerRows(matched.rest, limit, actions);
        case "audio":
          return await audioRows(matched.rest, limit, actions);
        case "files":
          return await fileRows(matched.rest, limit, actions);
        case "big":
          return await bigFileRows(limit, actions);
      }
    } catch (error) {
      // A live source that cannot answer must not empty the palette.
      console.error("live list failed:", error);
      return [];
    }
  },
};

/** Run a `ToolOutcome` call from a row and report what happened. */
async function rowOutcome(
  actions: PaletteActions,
  call: () => Promise<{ ok: boolean; message: string; copied: string | null }>,
): Promise<boolean> {
  try {
    const result = await call();
    actions.notify(result.message, result.ok ? "info" : "error");
    return false;
  } catch (error) {
    actions.notify(api.errorMessage(error), "error");
    return false;
  }
}

async function portRows(rest: string, limit: number, actions: PaletteActions) {
  const wanted = Number.parseInt(rest, 10);
  const ports = await api.listeningPorts(Number.isFinite(wanted) ? wanted : undefined);

  return ports.slice(0, limit).map((entry, index) => ({
    id: `port:${entry.port}:${entry.pid}`,
    title: `Port ${entry.port} — ${entry.process}`,
    subtitle: `pid ${entry.pid} · ↵ stops it with SIGTERM`,
    icon: "◈",
    group: "Ports",
    score: 700 - index,
    accessory: "↵ free",
    confirm: `Stop ${entry.process} (pid ${entry.pid}) on port ${entry.port}?`,
    run: () => rowOutcome(actions, () => api.freePort(entry.port)),
  }));
}

async function repoRows(rest: string, limit: number, actions: PaletteActions) {
  const repos = await api.gitRepos(80);
  const filtered = rest
    ? repos.filter((repo) => fuzzyMatch(rest, repo.name) !== null)
    : repos;

  return filtered.slice(0, limit).map((repo, index) => ({
    id: `repo:${repo.path}`,
    title: repo.name,
    subtitle: `${repo.branch} · ${repo.path.replace(/^\/Users\/[^/]+/, "~")}`,
    icon: "⑂",
    group: "Repositories",
    score: 700 - index,
    positions: rest ? (fuzzyMatch(rest, repo.name)?.positions ?? undefined) : undefined,
    accessory: "↵ terminal",
    run: () => rowOutcome(actions, () => api.openPathInTerminal(repo.path)),
  }));
}

async function sshRows(rest: string, limit: number, actions: PaletteActions) {
  const hosts = await api.sshHosts();
  const filtered = rest ? hosts.filter((host) => fuzzyMatch(rest, host.alias) !== null) : hosts;

  return filtered.slice(0, limit).map((host, index) => ({
    id: `ssh:${host.alias}`,
    title: host.alias,
    subtitle: host.user ? `${host.user}@${host.hostname}` : host.hostname,
    icon: "⌁",
    group: "SSH hosts",
    score: 700 - index,
    positions: rest ? (fuzzyMatch(rest, host.alias)?.positions ?? undefined) : undefined,
    accessory: "↵ connect",
    run: () => rowOutcome(actions, () => api.sshConnect(host.alias)),
  }));
}

async function dockerRows(rest: string, limit: number, actions: PaletteActions) {
  const containers = await api.dockerContainers();
  const filtered = rest
    ? containers.filter((container) => fuzzyMatch(rest, container.name) !== null)
    : containers;

  return filtered.slice(0, limit).map((container, index) => ({
    id: `docker:${container.id}`,
    title: container.name,
    subtitle: `${container.image} · ${container.status}`,
    icon: container.running ? "◉" : "◌",
    group: "Containers",
    score: 700 - index,
    accessory: container.running ? "↵ stop" : "↵ start",
    run: () =>
      rowOutcome(actions, () =>
        api.dockerAction(container.id, container.running ? "stop" : "start"),
      ),
  }));
}

async function audioRows(rest: string, limit: number, actions: PaletteActions) {
  const devices = await api.audioDevices();
  // "mic"/"input" asks about the input side; everything else means output.
  const wantsInput = /^(in|input|mic|microphone)/i.test(rest) || false;
  const relevant = devices.filter((device) => (wantsInput ? device.isInput : device.isOutput));

  return relevant.slice(0, limit).map((device, index) => {
    const current = wantsInput ? device.isDefaultInput : device.isDefaultOutput;
    return {
      id: `audio:${device.uid}`,
      title: device.name,
      subtitle: current
        ? `Current ${wantsInput ? "microphone" : "sound output"}`
        : `Make this the ${wantsInput ? "microphone" : "sound output"}`,
      icon: wantsInput ? "◍" : "◐",
      group: wantsInput ? "Microphones" : "Sound output",
      score: (current ? 720 : 700) - index,
      accessory: current ? "✓" : "↵",
      run: () => rowOutcome(actions, () => api.setAudioDevice(device.uid, wantsInput)),
    };
  });
}

async function fileRows(rest: string, limit: number, actions: PaletteActions) {
  if (!rest) return [];
  const hits = await api.searchFiles(rest, limit);

  return hits.map((hit, index) => ({
    id: `file:${hit.path}`,
    title: hit.name,
    subtitle: hit.path.replace(/^\/Users\/[^/]+/, "~"),
    icon: "▤",
    group: "Files",
    score: 700 - index,
    positions: fuzzyMatch(rest, hit.name)?.positions ?? undefined,
    accessory: "↵ reveal",
    run: () => rowOutcome(actions, () => api.revealPath(hit.path)),
  }));
}

async function bigFileRows(limit: number, actions: PaletteActions) {
  const files = await api.largestFiles(undefined, limit);

  return files.map((file, index) => ({
    id: `big:${file.path}`,
    title: `${file.size} — ${file.name}`,
    subtitle: file.path.replace(/^\/Users\/[^/]+/, "~"),
    icon: "▣",
    group: "Largest files",
    score: 700 - index,
    accessory: "↵ reveal",
    run: () => rowOutcome(actions, () => api.revealPath(file.path)),
  }));
}

// ---------------------------------------------------------------------------
// Extensions
// ---------------------------------------------------------------------------

/**
 * The installed list, cached for the length of a window session.
 *
 * Reading it is a directory scan plus a header parse per file, and this runs on
 * every keystroke. Installing or removing one goes through Settings, which calls
 * {@link forgetExtensions} — so the cache is only ever stale for a file someone
 * dropped into the folder by hand, and it comes back on the next window.
 */
let extensionCache: Promise<Extension[]> | null = null;

function installedExtensions(): Promise<Extension[]> {
  extensionCache ??= api.listExtensions().catch(() => [] as Extension[]);
  return extensionCache;
}

/** Drop the cache after an install or a removal. */
export function forgetExtensions(): void {
  extensionCache = null;
}

/**
 * How much of the query names the extension, and how much is its input.
 *
 * Longest match first: for "word count hello", `Word Count` should take two
 * tokens and be handed "hello", not take one and be handed "count hello".
 * Fuzzy matching is subsequence-based, so trying short prefixes first would
 * almost always win with the wrong answer.
 */
function splitExtensionQuery(
  tokens: string[],
  ext: Extension,
): { score: number; positions?: number[]; input: string } | null {
  for (let take = tokens.length; take >= 1; take -= 1) {
    const head = tokens.slice(0, take).join(" ");
    const match = fuzzyMatch(head, ext.name) ?? fuzzyMatch(head, ext.id);
    if (!match) continue;
    return {
      score: match.score,
      positions: fuzzyMatch(head, ext.name)?.positions,
      input: tokens.slice(take).join(" "),
    };
  }
  return null;
}

/**
 * Installed extensions, as palette rows.
 *
 * Enter opens the extension's page rather than running it in place. An
 * extension can return a list whose rows carry an `action` closure, and that
 * closure only exists while the worker that made it does — a palette row that
 * vanishes on the next keystroke cannot own a running program. The page can,
 * and it is also where an extension's errors can be read.
 */
export const extensionProvider: ResultProvider = {
  id: "extensions",
  title: "Extensions",
  async search({ query, actions }) {
    const trimmed = query.trim();
    if (!trimmed) return [];

    const installed = await installedExtensions();
    if (installed.length === 0) return [];

    const tokens = trimmed.split(/\s+/);
    const rows: ResultItem[] = [];

    for (const ext of installed) {
      const match = splitExtensionQuery(tokens, ext);
      if (!match) continue;

      rows.push({
        id: `extension:${ext.id}`,
        title: ext.name,
        subtitle: match.input
          ? `${ext.description || "Extension"} — “${match.input}”`
          : ext.description || "Extension",
        icon: "⊞",
        group: "Extensions",
        // Below a built-in command of the same name: a third-party file should
        // not be able to take a keystroke away from something that ships.
        score: match.score - 40 + usageBoost(`extension:${ext.id}`),
        positions: match.positions,
        usageKey: `extension:${ext.id}`,
        accessory: "↵",
        run: () => {
          actions.openTab({
            kind: "extension",
            extensionId: ext.id,
            prefill: match.input,
            title: ext.name,
            icon: "⊞",
          });
          return false;
        },
      });
    }

    return rows;
  },
};

// ---------------------------------------------------------------------------
// Search-shaped system providers: browser tabs, bookmarks, semantic file
// search, contacts, menu bar items
// ---------------------------------------------------------------------------
//
// Five capabilities that were built, tested and registered over IPC
// (`browser_search_tabs`/`browser_switch_tab`/`browser_search_bookmarks` in
// `tools/browser_cmds.rs`; `semantic_search` in `commands.rs`; `contacts_search`/
// `contacts_copy`/`menu_bar_items`/`menu_bar_invoke`, also `commands.rs`) but had
// no caller anywhere in the frontend — so none of them were reachable. All five
// are the same *shape* of thing: you search for a tab, a bookmark, a file, a
// person or a menu command the same way you search for an app, and the palette
// is where that happens. None of them get their own page.
//
// The shared risk across all five, and the reason this section exists rather
// than five one-line `invoke` calls, is that every provider's `search` runs on
// **every keystroke** (see `HomeTab.tsx`'s 45ms debounce — short enough that
// ordinary typing clears it on nearly every letter). `clipboardProvider`'s own
// comment above tells the story of what happens when that is forgotten: a
// Keychain + SQLite round trip fired on every keystroke, its answer thrown
// away unread, because nothing gated it behind the one case where it mattered.
// Three of these five are worse than that clipboard case, not equivalent to
// it — they shell out to `osascript`, and a real `osascript -e` round trip on
// this machine measures 150–420ms even for a trivial one-line script (timed
// with `time osascript -e 'tell application "System Events" to get name of
// every process'`), climbing well past 300ms for even a *shallow*, non-
// recursive menu-bar read. `frontmost_menu_items` walks every submenu
// recursively. None of that can be allowed to sit behind a plain fuzzy match.
//
// So each provider below picks its gate to fit what it costs:
//
//   - Browser tabs, contacts and menu bar items shell out to `osascript` and
//     are gated behind an explicit leading trigger word (`leadingWord`,
//     mirroring `liveKind` above) — typing "s" does not run any of them,
//     because "s" is not "tab", "contact" or "menu". Only spelling out the
//     word opts in, the same contract `liveListProvider`'s ports/repos/ssh/
//     docker/audio/files/big already use for exactly this reason.
//   - Tabs and menu items *additionally* cache their unfiltered IPC result
//     for a few seconds and fuzzy-filter the cache in JS on every keystroke
//     after that, rather than re-shelling `osascript` per letter. Typing
//     "tab docs" after the trigger fires one AppleScript enumeration for
//     "tab ", then four essentially-free in-process filters for "d", "o",
//     "c", "s" — measured at ~0.04ms/call over 200 cached tabs and
//     ~0.09ms/call over 400 cached menu items (see the scratchpad harness
//     referenced in this task's report). Contacts cannot be cached this way
//     — Contacts.app's own AppleScript search refuses an empty "list
//     everyone" query — so it stays a live call per keystroke, but only
//     while the explicit trigger keeps it opted in.
//   - Bookmarks read a plist/JSON/SQLite file rather than shelling out, but
//     doing that per keystroke is the identical "SQLite round trip on every
//     keystroke" shape the clipboard bug already named, so it gets the same
//     fetch-once-cache-and-filter treatment as tabs, without needing a
//     trigger word: reading files is cheap enough that a bare 3-character
//     gate is "sensible" on its own. Measured at ~0.44ms/call over 2000
//     cached bookmarks.
//   - Semantic search hits a real BM25 + embedding index and its ranking
//     itself changes as you keep typing, so there is nothing to cache; a
//     3-character floor is the only gate it needs, matching
//     `fileSearchProvider`'s two-character floor over plain Spotlight.
//
// All five fail silently: a browser that is not running, a missing
// Automation/Accessibility grant, an unbuilt semantic index, or Contacts.app
// never having been opened are ordinary, expected outcomes — never an error
// that should reach the user or take another provider down with it.

/** Mirrors `browsertabs::TabHit` (Rust) — see the file header on scope. */
interface BrowserTabHit {
  browser: string;
  windowId: number;
  tabIndex: number;
  title: string;
  url: string;
}

/** Mirrors `browsertabs::BookmarkHit` (Rust). */
interface BookmarkHit {
  source: string;
  title: string;
  url: string;
  folder: string | null;
}

/** Mirrors `knowledge::LabeledValue` / `knowledge::ContactHit` (Rust). */
interface LabeledValue {
  label: string;
  value: string;
}
interface ContactHit {
  name: string;
  phones: LabeledValue[];
  emails: LabeledValue[];
}

/** Mirrors `knowledge::MenuItem` (Rust) — renamed to avoid any suggestion this
 * is a DOM menu item. */
interface MenuBarEntry {
  path: string[];
}

/** The shape `rowOutcome` (defined above, in the live-lists section) expects
 * back from a Rust call — `ToolOutcome`'s fields, written out locally since
 * `ToolOutcome` itself is not imported into this file. */
type ToolOutcomeLike = { ok: boolean; message: string; copied: string | null };

/**
 * Split `raw` into its first whitespace-delimited word and everything after.
 *
 * Identical in spirit to `liveKind`'s own head/rest split above — kept as a
 * separate helper because these five providers are registered independently
 * rather than dispatched through `liveListProvider`'s single switch, so they
 * need the split without the rest of that function's trigger table.
 */
function leadingWord(raw: string): { head: string; rest: string } {
  const trimmed = raw.trim();
  const space = trimmed.search(/\s/);
  const head = (space === -1 ? trimmed : trimmed.slice(0, space)).toLowerCase();
  const rest = space === -1 ? "" : trimmed.slice(space + 1).trim();
  return { head, rest };
}

// --- Browser tabs ------------------------------------------------------------

const TAB_TRIGGERS = ["tab", "tabs"];
const TABS_CACHE_MS = 4_000;
let tabsCache: { at: number; data: BrowserTabHit[] } | null = null;

/**
 * Every open tab, fetched with an empty query (`browser_search_tabs`'s own
 * doc comment: an empty query "lists everything open") and reused for a
 * short window.
 *
 * `search_tabs` on the Rust side always enumerates every scriptable browser
 * over `osascript` regardless of what query it is given — the query only
 * trims the result afterwards, in Rust, not before. So calling it once per
 * keystroke costs exactly as much as calling it once per *word*: there is no
 * cheaper "narrow" call to make. Caching the one AppleScript walk and
 * fuzzy-filtering the cached list in JS is the whole saving.
 */
async function allBrowserTabs(): Promise<BrowserTabHit[]> {
  const now = Date.now();
  if (tabsCache && now - tabsCache.at < TABS_CACHE_MS) return tabsCache.data;
  const data = await invoke<BrowserTabHit[]>("browser_search_tabs", { query: "" }).catch(
    () => [] as BrowserTabHit[],
  );
  tabsCache = { at: now, data };
  return data;
}

export const browserTabsProvider: ResultProvider = {
  id: "browser-tabs",
  title: "Browser tabs",
  async search({ raw, parsed, settings, actions }) {
    if (parsed?.rule) return [];
    const { head, rest } = leadingWord(raw);
    if (!TAB_TRIGGERS.includes(head)) return [];

    const tabs = await allBrowserTabs();
    if (tabs.length === 0) return [];

    const limit = settings.commandCenter.maxResultsPerSource;
    const scored: { tab: BrowserTabHit; match: ReturnType<typeof fuzzyMatch> }[] = rest
      ? tabs
          .map((tab) => ({ tab, match: fuzzyMatch(rest, tab.title) ?? fuzzyMatch(rest, tab.url) }))
          .filter((x): x is { tab: BrowserTabHit; match: NonNullable<ReturnType<typeof fuzzyMatch>> } =>
            x.match !== null,
          )
          .sort((a, b) => b.match.score - a.match.score)
      : tabs.map((tab) => ({ tab, match: null }));

    return scored.slice(0, limit).map(({ tab, match }, index) => ({
      id: `tab:${tab.browser}:${tab.windowId}:${tab.tabIndex}`,
      title: tab.title || tab.url,
      subtitle: `${tab.browser} · ${tab.url.replace(/^https?:\/\//, "")}`,
      icon: "◫",
      group: "Browser tabs",
      score: 700 - index,
      positions: match?.positions,
      accessory: "↵ switch",
      run: () =>
        rowOutcome(actions, () =>
          invoke<ToolOutcomeLike>("browser_switch_tab", {
            browser: tab.browser,
            windowId: tab.windowId,
            tabIndex: tab.tabIndex,
          }),
        ),
    }));
  },
};

// --- Bookmarks -----------------------------------------------------------------

const BOOKMARKS_CACHE_MS = 30_000;
let bookmarksCache: { at: number; data: BookmarkHit[] } | null = null;

/**
 * Every bookmark across Safari, the Chromium family and Firefox, fetched
 * once and reused for the length of a short session.
 *
 * A longer TTL than tabs (30s vs 4s): a bookmark list barely changes minute
 * to minute the way open tabs do, and re-reading Safari's binary plist plus
 * every installed Chromium profile's `Bookmarks` JSON plus Firefox's SQLite
 * file on every keystroke is the exact "SQLite round trip on every
 * keystroke" shape the clipboard bug is named after in this section's header.
 */
async function allBookmarks(): Promise<BookmarkHit[]> {
  const now = Date.now();
  if (bookmarksCache && now - bookmarksCache.at < BOOKMARKS_CACHE_MS) return bookmarksCache.data;
  const data = await invoke<BookmarkHit[]>("browser_search_bookmarks", { query: "", limit: 2000 }).catch(
    () => [] as BookmarkHit[],
  );
  bookmarksCache = { at: now, data };
  return data;
}

export const bookmarksProvider: ResultProvider = {
  id: "bookmarks",
  title: "Bookmarks",
  async search({ query, parsed, settings, actions }) {
    if (parsed?.rule) return [];
    const trimmed = query.trim();
    // No trigger word here, unlike tabs/contacts/menu: reading cached files
    // is cheap enough that a plain length floor is "sensible" gating on its
    // own, the same call `fileSearchProvider` makes for Spotlight.
    if (trimmed.length < 3) return [];

    const bookmarks = await allBookmarks();
    if (bookmarks.length === 0) return [];

    // A supporting result, not the reason the palette is open — capped well
    // under the source limit for the same reason `fileSearchProvider` caps
    // itself at 5.
    const limit = Math.min(5, settings.commandCenter.maxResultsPerSource);

    return bookmarks
      .map((bookmark) => {
        const titleMatch = fuzzyMatch(trimmed, bookmark.title);
        const match = titleMatch ?? fuzzyMatch(trimmed, bookmark.url);
        return match ? { bookmark, match, positions: titleMatch?.positions } : null;
      })
      .filter(
        (
          x,
        ): x is {
          bookmark: BookmarkHit;
          match: NonNullable<ReturnType<typeof fuzzyMatch>>;
          positions: number[] | undefined;
        } => x !== null,
      )
      .sort((a, b) => b.match.score - a.match.score)
      .slice(0, limit)
      .map(({ bookmark, match, positions }) => ({
        id: `bookmark:${bookmark.source}:${bookmark.url}`,
        title: bookmark.title || bookmark.url,
        subtitle: [bookmark.source, bookmark.folder].filter(Boolean).join(" · "),
        icon: "☆",
        group: "Bookmarks",
        score: match.score,
        positions,
        accessory: "↵ open",
        run: async () => {
          try {
            const outcome = await api.openExternalUrl(bookmark.url);
            if (!outcome.ok) actions.notify(outcome.message, "error");
          } catch (error) {
            actions.notify(api.errorMessage(error), "error");
          }
        },
      }));
  },
};

// --- Semantic file search --------------------------------------------------------

/**
 * Local BM25 + (optionally) embedding search over whatever has been indexed
 * — see `api.ts`'s note on `semanticSearch`, written back when
 * `semantic_search` was built and tested but not yet registered over IPC.
 *
 * Unlike `fileSearchProvider` (Spotlight, effectively instant and gated at
 * two characters) this is real work server-side and its *ranking* changes as
 * you keep typing — there is nothing stable to cache between keystrokes the
 * way tabs/bookmarks/menu items have, so the only gate is the length floor.
 */
export const semanticSearchProvider: ResultProvider = {
  id: "semantic",
  title: "Semantic search",
  async search({ query, parsed, settings, actions }) {
    if (parsed?.rule) return [];
    const trimmed = query.trim();
    if (trimmed.length < 3) return [];

    const limit = Math.min(5, settings.commandCenter.maxResultsPerSource);

    let hits: api.SemanticSearchHit[];
    try {
      hits = await api.semanticSearch(trimmed, limit);
    } catch {
      // No backend configured, an unbuilt index, or Ollama unreachable are
      // all "nothing to show" — never a broken palette.
      return [];
    }

    return hits.map((hit, index) => ({
      id: `semantic:${hit.path}`,
      title: hit.title || hit.path.split("/").pop() || hit.path,
      subtitle: hit.snippet || hit.path.replace(/^\/Users\/[^/]+/, "~"),
      // A small visual tell for *why* this matched: embeddings found it with
      // no shared words at all, term overlap found it with no meaning check,
      // or both agreed.
      icon: hit.matchedVia === "semantic" ? "✦" : hit.matchedVia === "hybrid" ? "✧" : "▤",
      group: "Semantic search",
      // Below a typed filename match (`fileSearchProvider`'s FILE_LEAD band)
      // and below commands: the backend's own ranking decides order within
      // this list, not a fuzzy score against the query, since a semantic hit
      // may share no text with what was typed at all.
      score: 480 - index * 15,
      accessory: "↵ reveal",
      run: () => rowOutcome(actions, () => api.revealPath(hit.path)),
    }));
  },
};

// --- Contacts --------------------------------------------------------------------

const CONTACT_TRIGGERS = ["contact", "contacts"];

export const contactsProvider: ResultProvider = {
  id: "contacts",
  title: "Contacts",
  async search({ raw, parsed, settings, actions }) {
    if (parsed?.rule) return [];
    const { head, rest } = leadingWord(raw);
    if (!CONTACT_TRIGGERS.includes(head)) return [];

    const name = rest.trim();
    // The Rust side refuses an empty query outright ("Type a name to search
    // for."), and there is no "list everyone" mode to cache the way
    // bookmarks or tabs have — every hit is a fresh `whose name contains
    // "…"` AppleScript round trip. Two characters rather than three: this
    // only runs at all once the explicit "contact " trigger has already been
    // typed, so the corpus a short query is matched against is one name
    // field via Contacts' own filter, not this file's whole disk.
    if (name.length < 2) return [];

    let hits: ContactHit[];
    try {
      hits = await invoke<ContactHit[]>("contacts_search", { query: name });
    } catch {
      // Contacts isn't running, Automation permission is missing, or the
      // AppleScript failed outright — all "no rows", never a broken palette.
      return [];
    }

    const limit = settings.commandCenter.maxResultsPerSource;
    const rows: ResultItem[] = [];

    outer: for (const contact of hits) {
      const values: { label: string; value: string; kind: "phone" | "email" }[] = [
        ...contact.phones.map((p) => ({ label: p.label, value: p.value, kind: "phone" as const })),
        ...contact.emails.map((e) => ({ label: e.label, value: e.value, kind: "email" as const })),
      ];

      if (values.length === 0) {
        rows.push({
          id: `contact:${contact.name}`,
          title: contact.name,
          subtitle: "No phone or email on file",
          icon: "◌",
          group: "Contacts",
          score: 700 - rows.length,
          run: () => false,
        });
        if (rows.length >= limit) break outer;
        continue;
      }

      // One row per number/address rather than one per person: "choosing
      // copies a phone or email" only makes sense once you have picked
      // *which* one, and collapsing them into a single row would need a
      // second menu this palette has no mechanism for.
      for (const entry of values) {
        rows.push({
          id: `contact:${contact.name}:${entry.kind}:${entry.value}`,
          title: contact.name,
          subtitle: `${entry.label || (entry.kind === "phone" ? "Phone" : "Email")} · ${entry.value}`,
          icon: entry.kind === "phone" ? "☎" : "✉",
          group: "Contacts",
          score: 700 - rows.length,
          accessory: "↵ copy",
          run: () =>
            rowOutcome(actions, () => invoke<ToolOutcomeLike>("contacts_copy", { value: entry.value })),
        });
        if (rows.length >= limit) break outer;
      }
    }

    return rows;
  },
};

// --- Menu bar items ----------------------------------------------------------------

const MENU_TRIGGERS = ["menu", "menubar"];
const MENU_CACHE_MS = 6_000;
let menuCache: { at: number; data: MenuBarEntry[] } | null = null;

/**
 * The frontmost app's whole menu bar — every menu and submenu, recursively —
 * fetched once and reused briefly.
 *
 * This is the single slowest call reachable from the palette: `osascript`
 * walking a full, potentially deeply-nested menu tree, measured even for a
 * *shallow*, non-recursive top-level read at ~340ms on this machine (`time
 * osascript -e '…menu bar items of menu bar 1…'`). It is also the one this
 * task's brief calls out by name: "must require an explicit opt-in trigger
 * word, never fire speculatively." `MENU_TRIGGERS` is that word; the cache
 * on top of it is what keeps continuing to type after it from re-walking the
 * whole tree on every letter.
 */
async function frontmostMenuItems(): Promise<MenuBarEntry[]> {
  const now = Date.now();
  if (menuCache && now - menuCache.at < MENU_CACHE_MS) return menuCache.data;
  const data = await invoke<MenuBarEntry[]>("menu_bar_items").catch(() => [] as MenuBarEntry[]);
  // Cached even when empty (no frontmost app, no Automation/Accessibility
  // grant, or the script failed) so a permission dialog the user is
  // ignoring is not re-triggered on every subsequent keystroke either.
  menuCache = { at: now, data };
  return data;
}

export const menuBarProvider: ResultProvider = {
  id: "menu-bar",
  title: "Menu bar",
  async search({ raw, parsed, settings, actions }) {
    if (parsed?.rule) return [];
    const { head, rest } = leadingWord(raw);
    if (!MENU_TRIGGERS.includes(head)) return [];

    const items = await frontmostMenuItems();
    if (items.length === 0) return [];

    const limit = settings.commandCenter.maxResultsPerSource;
    const scored: { item: MenuBarEntry; match: ReturnType<typeof fuzzyMatch> }[] = rest
      ? items
          .map((item) => ({ item, match: fuzzyMatch(rest, item.path.join(" ")) }))
          .filter(
            (x): x is { item: MenuBarEntry; match: NonNullable<ReturnType<typeof fuzzyMatch>> } =>
              x.match !== null,
          )
          .sort((a, b) => b.match.score - a.match.score)
      : items.map((item) => ({ item, match: null }));

    return scored.slice(0, limit).map(({ item, match }, index) => ({
      id: `menu:${item.path.join(" ")}`,
      title: item.path[item.path.length - 1] ?? "",
      subtitle: item.path.slice(0, -1).join(" ▸ ") || "Menu bar",
      icon: "⌘",
      group: "Menu bar",
      score: 700 - index,
      positions: match?.positions,
      accessory: "↵ run",
      run: async () => {
        try {
          await invoke<void>("menu_bar_invoke", { path: item.path });
        } catch (error) {
          actions.notify(api.errorMessage(error), "error");
          return false;
        }
      },
    }));
  },
};

export const defaultProviders: ResultProvider[] = [
  calculatorProvider,
  favoritesProvider,
  aiWorkspaceProvider,
  commandProvider,
  liveListProvider,
  shortcutProvider,
  conversionProvider,
  appLauncherProvider,
  fileSearchProvider,
  captureProvider,
  extensionProvider,
  browserTabsProvider,
  bookmarksProvider,
  semanticSearchProvider,
  contactsProvider,
  menuBarProvider,
  searchFallbackProvider,
  clipboardProvider,
  prefixHintProvider,
];
