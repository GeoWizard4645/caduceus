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
import { fuzzyMatch, fuzzyScore } from "./fuzzy";
import type {
  CalcResult,
  ClipboardEntry,
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
  /** Runs when the row is chosen. Return `false` to keep the palette open. */
  run: () => void | boolean | Promise<void | boolean>;
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
  setMode: (mode: "default" | "clipboard" | "system") => void;
  notify: (message: string, tone?: "info" | "error") => void;
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
        actions.setMode("clipboard");
        actions.setInput("");
        return false;
      }
      if (outcome.frontendAction === "system_monitor") {
        actions.setMode("system");
        actions.setInput("");
        return false;
      }
      if (!outcome.ok) {
        actions.notify(outcome.message, "error");
        return false;
      }
      return true;
    },
  };
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
    case "clipboard_view":
      return "Browse clipboard history";
    case "system_monitor":
      return "CPU, memory, disks and processes";
  }
}

/** Recent clipboard entries. */
export const clipboardProvider: ResultProvider = {
  id: "clipboard",
  title: "Clipboard",
  search({ clipboard, query, actions, settings }) {
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
      // Below shortcuts on an empty query, competitive once you are searching.
      score: (query ? 300 : 200) - index,
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
        title: app.name,
        subtitle: "Application",
        icon: "▣",
        group: "Applications",
        // Launching an app you named is almost always what you meant, so this
        // outranks a web search for the same word.
        score: match.score + 40,
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
      {
        id: "capture-record-mic",
        title: "Start screen recording (mic on)",
        subtitle: "Screen + microphone → Downloads as .mov",
        icon: "⏺",
        group: "Capture",
        score: /record|video/.test(q) ? 850 : 350,
        run: async () => {
          try {
            const state = await api.captureRecordStart(true, false);
            actions.notify(state.message);
          } catch (e) {
            actions.notify(api.errorMessage(e), "error");
          }
        },
      },
      {
        id: "capture-record-system",
        title: "Start screen recording (system audio)",
        subtitle: "Screen + system sound (macOS 13+); mic off",
        icon: "🔊",
        group: "Capture",
        score: /system|audio/.test(q) ? 880 : 300,
        run: async () => {
          try {
            const state = await api.captureRecordStart(false, true);
            actions.notify(state.message);
          } catch (e) {
            actions.notify(api.errorMessage(e), "error");
          }
        },
      },
      {
        id: "capture-record-both",
        title: "Start screen recording (mic + system audio)",
        subtitle: "Opens Screenshot.app — use Options for audio sources",
        icon: "🎬",
        group: "Capture",
        score: 320,
        run: async () => {
          try {
            const state = await api.captureRecordStart(true, true);
            actions.notify(state.message);
          } catch (e) {
            actions.notify(api.errorMessage(e), "error");
          }
        },
      },
      {
        id: "capture-record-stop",
        title: "Stop screen recording",
        subtitle: "Use the menu bar control in Screenshot / macOS",
        icon: "⏹",
        group: "Capture",
        score: q.includes("stop") ? 950 : 280,
        run: async () => {
          try {
            const state = await api.captureRecordStop();
            actions.notify(state.message);
          } catch (e) {
            actions.notify(api.errorMessage(e), "error");
          }
        },
      },
    ];
    return items;
  },
};

export const defaultProviders: ResultProvider[] = [
  calculatorProvider,
  shortcutProvider,
  appLauncherProvider,
  captureProvider,
  searchFallbackProvider,
  clipboardProvider,
  prefixHintProvider,
];
