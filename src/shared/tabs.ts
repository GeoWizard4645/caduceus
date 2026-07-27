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
  | "machine";

export interface Tab {
  /** Unique for the lifetime of the tab, so React keys stay stable. */
  id: string;
  kind: TabKind;
  /** Overrides the kind's default label — a chat tab shows its thread name. */
  title?: string;
  /** For `settings`: which pane to select on open. */
  section?: string;
  /** For `chat`: which conversation to show. */
  conversationId?: number;
}

interface KindMeta {
  label: string;
  icon: string;
  /** Whether a second tab of this kind is allowed. */
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
};

/** The most tabs one window session will hold. */
export const MAX_TABS = 24;

export function isTabKind(value: unknown): value is TabKind {
  return typeof value === "string" && value in TAB_KINDS;
}

export function tabLabel(tab: Tab): string {
  return tab.title ?? TAB_KINDS[tab.kind].label;
}

export function tabIcon(tab: Tab): string {
  return TAB_KINDS[tab.kind].icon;
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
    const existing = tabs.find((tab) => tab.kind === request.kind);
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
