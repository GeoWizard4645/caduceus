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
      return [row];
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
  searchFallbackProvider,
  clipboardProvider,
  prefixHintProvider,
];
