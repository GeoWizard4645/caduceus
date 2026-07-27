/**
 * What a tab is.
 *
 * Caduceus is one window. Everything it can show — the palette, your clipboard,
 * a conversation, Settings, the keep-awake countdown — is a tab in it, the way
 * pages are tabs in a browser. Nothing opens a second window.
 *
 * # The rules that make that work
 *
 * * **A new tab is the palette.** ⌘T gives you the same thing the hotkey does:
 *   an empty search. That is the home page.
 * * **Most kinds are singletons.** Opening the clipboard twice focuses the tab
 *   you already have. Two tabs showing the same live state raise the question of
 *   which one is right, and "both, they poll" is a worse answer than the
 *   question. Home is the exception — several searches at once is the point.
 * * **Twenty-four, then stop.** Past that a tab bar is a worse list than the
 *   thing it is listing, and every open tab is a page holding state.
 */

export type TabKind =
  | "home"
  | "clipboard"
  | "chat"
  | "settings"
  | "system"
  | "awake"
  | "sound"
  | "ports"
  | "docker"
  | "machine"
  | "tool"
  | "permission";

export interface Tab {
  /** Unique for the lifetime of the tab, so React keys stay stable. */
  id: string;
  kind: TabKind;
  /** Overrides the kind's default label — a chat tab shows its thread name. */
  title?: string;
  /** Overrides the kind's default icon, for kinds that are one page per thing. */
  icon?: string;
  /** For `settings`: which pane to select on open. */
  section?: string;
  /** For `chat`: which conversation to show. */
  conversationId?: number;
  /** For `tool`: which command's page this is. */
  commandId?: string;
  /** For `tool`: text to start the page's input with. */
  prefill?: string;
  /** For `permission`: which grant is missing. */
  permission?: PermissionId;
  /** For `permission`: the command to offer to re-run once it is granted. */
  retryCommandId?: string;
}

/**
 * The macOS grants Caduceus can walk someone through.
 *
 * Kept in step with `window::grants::Grant` on the Rust side, whose
 * `rename_all = "kebab-case"` produces exactly these strings.
 */
export type PermissionId =
  | "accessibility"
  | "screen-recording"
  | "microphone"
  | "automation"
  | "speech-recognition";

interface KindMeta {
  label: string;
  icon: string;
  /**
   * Whether a second tab of this kind is allowed.
   *
   * "Singleton" is per {@link singletonKey}, not per kind: two different tool
   * pages are two different things, and opening the same one twice is not.
   */
  singleton: boolean;
  /**
   * Whether this tab makes the window a *window* rather than an overlay.
   *
   * Home does not: a lone palette should still float over full-screen apps and
   * dismiss when you click away. Everything else does.
   */
  page: boolean;
}

export const TAB_KINDS: Record<TabKind, KindMeta> = {
  home: { label: "Search", icon: "⌕", singleton: false, page: false },
  clipboard: { label: "Clipboard", icon: "❐", singleton: true, page: true },
  chat: { label: "AI Chat", icon: "✳", singleton: true, page: true },
  settings: { label: "Settings", icon: "⚙", singleton: true, page: true },
  system: { label: "System Monitor", icon: "◔", singleton: true, page: true },
  awake: { label: "Keep Awake", icon: "☀", singleton: true, page: true },
  sound: { label: "Sound", icon: "◐", singleton: true, page: true },
  ports: { label: "Ports", icon: "◈", singleton: true, page: true },
  docker: { label: "Docker", icon: "◉", singleton: true, page: true },
  machine: { label: "This Mac", icon: "◍", singleton: true, page: true },
  tool: { label: "Tool", icon: "⌂", singleton: true, page: true },
  permission: { label: "Permission", icon: "⚠", singleton: true, page: true },
};

/**
 * What makes two tabs "the same tab" for singleton purposes.
 *
 * Clipboard is one page, so its key is its kind. A tool page is one page *per
 * command*, so `sha256` and `slugify` coexist while asking for `sha256` twice
 * lands you back on the one you already have.
 */
export function singletonKey(tab: Pick<Tab, "kind" | "commandId" | "permission">): string {
  switch (tab.kind) {
    case "tool":
      return `tool:${tab.commandId ?? ""}`;
    case "permission":
      return `permission:${tab.permission ?? ""}`;
    default:
      return tab.kind;
  }
}

/** The most tabs one window session will hold. */
export const MAX_TABS = 24;

export function isTabKind(value: unknown): value is TabKind {
  return typeof value === "string" && value in TAB_KINDS;
}

/** Every grant the permission page knows how to render. */
const PERMISSION_IDS: PermissionId[] = [
  "accessibility",
  "screen-recording",
  "microphone",
  "automation",
  "speech-recognition",
];

export function isPermissionId(value: unknown): value is PermissionId {
  return typeof value === "string" && (PERMISSION_IDS as string[]).includes(value);
}

export function tabLabel(tab: Tab): string {
  return tab.title ?? TAB_KINDS[tab.kind].label;
}

export function tabIcon(tab: Tab): string {
  return tab.icon ?? TAB_KINDS[tab.kind].icon;
}

/** Whether this set of tabs should behave as an overlay rather than a window. */
export function isFloating(tabs: Tab[]): boolean {
  return tabs.length === 1 && tabs[0].kind === "home";
}

let counter = 0;
export function newTabId(kind: TabKind): string {
  counter += 1;
  return `${kind}-${counter}`;
}

export function homeTab(): Tab {
  return { id: newTabId("home"), kind: "home" };
}

export interface OpenResult {
  tabs: Tab[];
  activeId: string;
  /** Set when the request was refused, for the caller to show. */
  refused?: string;
}

/**
 * Add a tab, or focus the existing one.
 *
 * Pure so the rules above are testable without a webview.
 */
export function openTab(tabs: Tab[], request: Omit<Tab, "id">): OpenResult {
  const meta = TAB_KINDS[request.kind];

  if (meta.singleton) {
    const key = singletonKey(request);
    const existing = tabs.find((tab) => singletonKey(tab) === key);
    if (existing) {
      // Focusing carries the new payload across: asking for Settings → Voice
      // while Settings is already open should move to Voice, not ignore you.
      const updated = tabs.map((tab) =>
        tab.id === existing.id ? { ...tab, ...request, id: tab.id } : tab,
      );
      return { tabs: updated, activeId: existing.id };
    }
  }

  if (tabs.length >= MAX_TABS) {
    return {
      tabs,
      activeId: tabs[tabs.length - 1]?.id ?? "",
      refused: `That is ${MAX_TABS} tabs — close one first.`,
    };
  }

  const tab: Tab = { ...request, id: newTabId(request.kind) };
  return { tabs: [...tabs, tab], activeId: tab.id };
}

/**
 * Remove a tab and pick what to focus next.
 *
 * Focus moves left, which is what every browser does and what the muscle memory
 * of closing several in a row expects. Closing the last tab leaves one fresh
 * home tab, so reopening the window is never a blank rectangle.
 */
export function closeTab(
  tabs: Tab[],
  activeId: string,
  closingId: string,
): OpenResult & { emptied: boolean } {
  const index = tabs.findIndex((tab) => tab.id === closingId);
  if (index === -1) return { tabs, activeId, emptied: false };

  const remaining = tabs.filter((tab) => tab.id !== closingId);

  if (remaining.length === 0) {
    const fresh = homeTab();
    return { tabs: [fresh], activeId: fresh.id, emptied: true };
  }

  const nextActive =
    activeId === closingId
      ? (remaining[Math.max(0, index - 1)] ?? remaining[0]).id
      : activeId;

  return { tabs: remaining, activeId: nextActive, emptied: false };
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

const STORAGE_KEY = "caduceus.tabs.v1";

/**
 * What gets written down.
 *
 * Tabs are the window. Closing it puts it away rather than throwing it out —
 * that has always been true of the *window*, but the tabs lived only in React
 * state, so anything that reloaded the webview (an update, a crash, a restart)
 * silently emptied it. A tab you deliberately left open is a thing you meant to
 * come back to.
 *
 * Only the identifying fields are kept, never anything live: a chat tab is
 * saved as "this conversation", not as its messages, so it reloads rather than
 * restores something stale.
 */
interface StoredTabs {
  tabs: Tab[];
  activeId: string;
}

export function saveTabs(tabs: Tab[], activeId: string): void {
  try {
    const payload: StoredTabs = { tabs: tabs.map(forStorage), activeId };
    localStorage.setItem(STORAGE_KEY, JSON.stringify(payload));
  } catch {
    // A window that cannot remember its tabs is still a working window.
  }
}

/**
 * Strip what should not come back.
 *
 * A Home tab is labelled with whatever is in its search box. Restoring that
 * label without the query it describes gives you a tab called "sha256 hello"
 * containing an empty palette — a tab that lies about itself.
 */
function forStorage(tab: Tab): Tab {
  if (tab.kind !== "home") return tab;
  const { title: _title, ...rest } = tab;
  return rest;
}

/**
 * Read the tabs back, or `null` if there is nothing usable to read.
 *
 * Validated rather than trusted: a kind that no longer exists (a page dropped
 * in an update) is skipped instead of rendering as `undefined`, and an id
 * collision with a freshly generated one is made impossible by advancing the
 * counter past everything restored.
 */
export function loadTabs(): { tabs: Tab[]; activeId: string } | null {
  let stored: unknown;
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    stored = JSON.parse(raw);
  } catch {
    return null;
  }

  if (typeof stored !== "object" || stored === null) return null;
  const { tabs, activeId } = stored as Partial<StoredTabs>;
  if (!Array.isArray(tabs)) return null;

  const restored = tabs
    .filter((tab): tab is Tab => Boolean(tab) && isTabKind((tab as Tab).kind))
    // Kind alone is not enough. A `permission` tab whose `permission` is a
    // string no longer in the union, or a `tool` tab with no `commandId`, is a
    // page that cannot render — and a crash during render takes the whole
    // window with it, not just the tab.
    .filter((tab) => tab.kind !== "permission" || isPermissionId(tab.permission))
    .filter((tab) => tab.kind !== "tool" || typeof tab.commandId === "string")
    .slice(0, MAX_TABS);
  if (restored.length === 0) return null;

  // Ids are `${kind}-${n}`; keep the counter above every restored one so a new
  // tab in this session can never collide with a tab from the last.
  for (const tab of restored) {
    const suffix = Number.parseInt(String(tab.id).split("-").pop() ?? "", 10);
    if (Number.isFinite(suffix)) counter = Math.max(counter, suffix);
  }

  const active = restored.some((tab) => tab.id === activeId) ? activeId! : restored[0].id;
  return { tabs: restored, activeId: active };
}
