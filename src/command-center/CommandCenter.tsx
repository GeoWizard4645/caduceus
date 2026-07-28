/**
 * The Command Center: one window, tabs for everything.
 *
 * Caduceus used to open four windows — the palette, Settings, chat, and a
 * management window. That is four things to find, four to arrange, and four to
 * close. Now there is one window and everything in it is a tab, the way pages
 * are tabs in a browser: a new tab is the palette, and running something with
 * state worth keeping puts it in a tab beside whatever you were already doing.
 *
 * # Two personalities, one window
 *
 * A lone Home tab is a *palette*: it floats over full-screen apps and has no
 * Dock icon. The moment a second tab exists it grows into a larger *window*
 * layout, but both dismiss when you click another app (unless you turn that off
 * in Settings).
 *
 * {@link isFloating} still tracks “palette vs pages” for sizing; blur dismissal
 * is independent of tab count.
 */

import { Suspense, lazy, useCallback, useEffect, useRef, useState } from "react";

import * as api from "@/shared/api";
import { WORKFLOW_PENDING_EVENT } from "@/shared/api";
import { useTauriEvent, useToasts } from "@/shared/hooks";
import { EVENTS, type CommandCenterOpenPayload } from "@/shared/types";
import { Spinner, cx } from "@/shared/ui";
import { ThemeToggle } from "@/shared/ThemeToggle";

import { HomeTab } from "./HomeTab";
import { TabBoundary } from "./TabBoundary";
import { ClipboardTabPage } from "./pages/ClipboardTabPage";
import { SystemTabPage } from "./pages/SystemTabPage";
import { AwakePage } from "./pages/AwakePage";
import { SoundPage } from "./pages/SoundPage";
import { PortsPage } from "./pages/PortsPage";
import { DockerPage } from "./pages/DockerPage";
import { MachinePage } from "./pages/MachinePage";
import { PermissionPage } from "./pages/PermissionPage";
import { ToolPage } from "./pages/ToolPage";
import { ExtensionPage } from "./pages/ExtensionPage";
import { WorkflowImportPage } from "./pages/WorkflowImportPage";
import {
  MAX_TABS,
  closeTab as closeTabIn,
  homeTab,
  isFloating,
  isTabKind,
  loadTabs,
  openTab as openTabIn,
  saveTabs,
  takeResume,
  tabForMode,
  tabIcon,
  tabLabel,
  type Tab,
} from "@/shared/tabs";

// Settings and chat are large and rarely the first thing opened; loading them
// with the palette would cost every hotkey press the price of a page most
// presses never reach.
const SettingsPage = lazy(() =>
  import("@/settings/Settings").then((m) => ({ default: m.Settings })),
);
const ChatPage = lazy(() => import("@/chat/Chat").then((m) => ({ default: m.Chat })));

/** Size the window grows to the first time a real page opens in it. */
const PAGE_MIN_WIDTH = 1080;
const PAGE_MIN_HEIGHT = 720;

export function CommandCenter() {
  // Restored during the first render, so a reopened window *is* the window you
  // left rather than a blank one that fills in a frame later. Read through a
  // lazy initialiser: `useRef(loadTabs())` would re-read storage on every
  // render and throw the result away.
  const [restored] = useState(loadTabs);
  const [tabs, setTabs] = useState<Tab[]>(() => restored?.tabs ?? [homeTab()]);
  const [activeId, setActiveId] = useState<string>(() => restored?.activeId ?? "");
  const { toasts, notify } = useToasts();

  // The first tab's id is generated during the initial state, so adopt it once.
  useEffect(() => {
    if (!activeId && tabs[0]) setActiveId(tabs[0].id);
  }, [activeId, tabs]);

  // Granting Screen Recording restarts the app, because macOS only re-reads
  // that grant at process start. Coming back to a blank palette makes the
  // restart feel like a crash and quietly loses the thing that was asked for,
  // so the page that triggered it is reopened and focused here.
  //
  // `takeResume` clears as it reads: this must fire once, on the launch that
  // followed the grant, and never again.
  useEffect(() => {
    const resume = takeResume();
    if (!resume) return;
    setTabs((current) => {
      const result = openTabIn(current, resume);
      setActiveId(result.activeId);
      return result.tabs;
    });
  }, []);

  // Write them down as they change. Hiding the window keeps React state alive,
  // so this is really about the times it does not survive — an app restart, an
  // update, a reloaded webview — after which an empty window is
  // indistinguishable from having lost your work.
  //
  // Debounced because a Home tab renames itself on every keystroke, and
  // serialising the whole set per character typed is a lot of work to record
  // something nobody is going to read until the next launch.
  useEffect(() => {
    if (!activeId) return;
    const timer = setTimeout(() => saveTabs(tabs, activeId), 300);
    return () => clearTimeout(timer);
  }, [tabs, activeId]);

  const floating = isFloating(tabs);

  // Keeps the palette/page flag in sync for window sizing and future behaviour.
  useEffect(() => {
    void api.setPaletteFloating(floating).catch(() => {});
  }, [floating]);

  // Grow once when the window stops being a palette. Only ever grows, and only
  // if it is currently smaller — resizing a window the user has sized by hand is
  // rude, and shrinking it back on every tab close would be worse.
  const grown = useRef(false);
  useEffect(() => {
    if (floating || grown.current) return;
    grown.current = true;
    void (async () => {
      try {
        const { getCurrentWindow, LogicalSize } = await import("@tauri-apps/api/window");
        const window = getCurrentWindow();
        const scale = await window.scaleFactor();
        const size = (await window.outerSize()).toLogical(scale);
        if (size.width < PAGE_MIN_WIDTH || size.height < PAGE_MIN_HEIGHT) {
          await window.setSize(
            new LogicalSize(
              Math.max(size.width, PAGE_MIN_WIDTH),
              Math.max(size.height, PAGE_MIN_HEIGHT),
            ),
          );
          await window.center();
        }
      } catch {
        // A window that will not resize is still perfectly usable.
      }
    })();
  }, [floating]);

  const openTab = useCallback((request: Omit<Tab, "id">) => {
    setTabs((current) => {
      const result = openTabIn(current, request);
      if (result.refused) {
        notify(result.refused, "error");
        return current;
      }
      setActiveId(result.activeId);
      return result.tabs;
    });
  }, [notify]);

  const closeTab = useCallback((id: string) => {
    setTabs((current) => {
      const result = closeTabIn(current, activeId, id);
      setActiveId(result.activeId);
      // Closing the last tab puts the window away rather than leaving an empty
      // frame; the fresh Home tab is what it reopens on.
      if (result.emptied) void api.hideCommandCenter();
      return result.tabs;
    });
  }, [activeId]);

  const setTabTitle = useCallback((id: string, title: string | undefined) => {
    setTabs((current) =>
      current.some((tab) => tab.id === id && tab.title === title)
        ? current
        : current.map((tab) => (tab.id === id ? { ...tab, title } : tab)),
    );
  }, []);

  // --- requests from Rust --------------------------------------------------
  useTauriEvent<{ kind: string; section?: string | null; conversationId?: number | null }>(
    EVENTS.tabOpen,
    (request) => {
      if (!isTabKind(request.kind)) return;
      openTab({
        kind: request.kind,
        section: request.section ?? undefined,
        conversationId: request.conversationId ?? undefined,
      });
    },
  );

  // A workflow link arrived. Bring the review page up.
  //
  // Without this the import is staged and completely invisible: nothing in the
  // palette mentions it, and the only way to find it is to guess that Settings
  // has a Workflows tab. A link the user clicked deliberately should land
  // somewhere they can see — and just as importantly, one they did *not* click
  // should announce itself rather than sit in a queue waiting to be confirmed
  // by someone who never learns it is there.
  //
  // Showing the page runs nothing. It is a description of what would be added,
  // with the commands quoted in full; committing is a separate, explicit act.
  useTauriEvent<unknown>(WORKFLOW_PENDING_EVENT, () => {
    openTab({ kind: "workflow-import", title: "Workflow import" });
    void api.openCommandCenter();
  });

  // Reopening keeps the tab you left. The window stays mounted while hidden, so
  // "open" is show-and-focus, not a reset — jumping back to Home every time made
  // Settings, chat, and tool pages feel like they vanished the moment you
  // dismissed the window.
  //
  // A named destination still wins. A staff shortcut set to "clipboard history"
  // calls `open_command_center` with `mode: "clipboard"`, and that must land on
  // clipboard rather than whatever was open before. ⌘T is how you ask for a
  // fresh Home tab deliberately.
  useTauriEvent<CommandCenterOpenPayload>(EVENTS.commandCenterOpen, (payload) => {
    const destination = tabForMode(payload?.mode);
    if (!destination) return;

    setTabs((current) => {
      const result = openTabIn(current, destination);
      setActiveId(result.activeId);
      return result.tabs;
    });
  });

  // --- keyboard ------------------------------------------------------------
  //
  // Two listeners, deliberately, because they want opposite priorities.
  //
  // The browser keys are registered in the **capture** phase: ⌘T means "new
  // tab" everywhere in this window, and no page inside it gets to eat it first.
  // They are also matched on `event.code`, not `event.key` — with a non-Latin
  // layout active, `key` for the T key is not "t", and the shortcut simply
  // stopped existing.
  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if (!event.metaKey || event.ctrlKey || event.altKey) return;

      if (event.code === "KeyT") {
        event.preventDefault();
        openTab({ kind: "home" });
        return;
      }
      if (event.code === "KeyW") {
        event.preventDefault();
        if (activeId) closeTab(activeId);
        return;
      }
      // ⌘9 is the last tab, as in every browser; ⌘1–⌘8 are positional.
      const digit = /^Digit([1-9])$/.exec(event.code)?.[1];
      if (digit) {
        event.preventDefault();
        const index = Number(digit);
        const target = index === 9 ? tabs[tabs.length - 1] : tabs[index - 1];
        if (target) setActiveId(target.id);
      }
    };
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, [tabs, activeId, openTab, closeTab]);

  // Escape is the opposite: the page in front gets first refusal, because
  // inside a page it means "dismiss this output" or "clear the search". Only
  // once nothing has claimed it does it mean "put the window away".
  //
  // `window` is the last stop in the bubble path, after the page's own listener
  // on `document` and after React's handlers on the root container inside it —
  // so anything that dealt with the key has already had the chance to say so by
  // calling `preventDefault`. Before this, Escape existed only on the palette's
  // `<input>`: click a button, press Escape, nothing happened.
  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      event.preventDefault();
      // Closing the last tab hides the window, which is what Escape in a bare
      // palette has always done; with pages open it closes the page in front.
      if (activeId) closeTab(activeId);
      else void api.hideCommandCenter();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [activeId, closeTab]);

  return (
    <div
      className={cx(
        "has-backdrop relative flex h-full w-full flex-col overflow-hidden shadow-float",
        floating ? "glass" : "bg-base",
      )}
      // The radius is a setting, so it cannot be a Tailwind class.
      style={{ borderRadius: "var(--cad-radius)", fontSize: "calc(1em * var(--cad-scale))" }}
    >
      <DragHandle />

      {/* Always. The tab strip used to appear only once a second tab existed,
          which meant the window you opened with the hotkey had no visible tabs,
          no + button and nothing to suggest ⌘T would do anything — and then
          grew a tab bar out of nowhere the first time you opened Settings. One
          window that behaves like a browser has to look like one from the
          first frame. */}
      <TabBar
        tabs={tabs}
        activeId={activeId}
        onSelect={setActiveId}
        onClose={closeTab}
        onNew={() => openTab({ kind: "home" })}
      />

      {/* Every tab stays mounted; hiding is CSS. Unmounting would restart a
          countdown, drop a half-written message and reset a filter every time
          you switched away — which defeats the reason tabs exist. */}
      <div className="min-h-0 flex-1">
        {tabs.map((tab) => {
          const isActive = tab.id === activeId;
          return (
            <div key={tab.id} className={cx("h-full", !isActive && "hidden")}>
              {/* Per tab, not around the lot: a page that throws should lose
                  its own tab and nothing else. */}
              <TabBoundary label={tabLabel(tab)} onClose={() => closeTab(tab.id)}>
                <TabContent
                  tab={tab}
                  active={isActive}
                  onOpenTab={openTab}
                  onSetTitle={(title) => setTabTitle(tab.id, title)}
                />
              </TabBoundary>
            </div>
          );
        })}
      </div>

      <ResizeGrip />

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
// Window chrome
// ---------------------------------------------------------------------------

/**
 * The grab bar above the tabs.
 *
 * The window is borderless — no title bar, which is what makes the palette look
 * like a palette — so there is nothing that says "you can move me". The whole
 * strip has always been draggable; this is the two millimetres of dots that
 * admit it.
 */
function DragHandle() {
  return (
    <div
      data-tauri-drag-region
      title="Drag to move"
      className="drag-region flex h-4 shrink-0 cursor-grab items-center justify-center active:cursor-grabbing"
    >
      <span
        aria-hidden="true"
        className="pointer-events-none flex gap-[3px] opacity-40 transition-opacity hover:opacity-70"
      >
        {[0, 1, 2].map((i) => (
          <span key={i} className="h-[3px] w-[3px] rounded-full bg-ink-faint" />
        ))}
      </span>
    </div>
  );
}

/**
 * The corner you drag to resize.
 *
 * Same problem as the drag handle: a borderless window has resizable edges and
 * no way of saying so. macOS draws no grow box on a window with no frame, so
 * this is one.
 *
 * `startResizeDragging` hands the gesture to the window manager, which is what
 * makes the resize track the pointer at the compositor's frame rate instead of
 * ours — dragging a corner by setting the window size from mouse events looks
 * like it is lagging, because it is.
 */
function ResizeGrip() {
  const onPointerDown = async (event: React.PointerEvent) => {
    // Left button only: a right-drag on the corner should open nothing and
    // resize nothing.
    if (event.button !== 0) return;
    event.preventDefault();
    try {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      await getCurrentWindow().startResizeDragging("SouthEast");
    } catch {
      // Not in a Tauri window, or the runtime refused. The window is still
      // resizable from its edges either way.
    }
  };

  return (
    <div
      onPointerDown={(event) => void onPointerDown(event)}
      title="Drag to resize"
      aria-hidden="true"
      className="no-drag absolute bottom-0 right-0 z-50 h-4 w-4 cursor-nwse-resize"
    >
      {/* Three lines, shortest at the corner — the shape every resize grip has
          had since the classic Mac OS grow box. */}
      <svg viewBox="0 0 16 16" className="h-4 w-4 stroke-ink-faint opacity-50">
        <path d="M15 5 L5 15 M15 9 L9 15 M15 13 L13 15" strokeWidth="1.2" strokeLinecap="round" />
      </svg>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Tab bar
// ---------------------------------------------------------------------------

function TabBar({
  tabs,
  activeId,
  onSelect,
  onClose,
  onNew,
}: {
  tabs: Tab[];
  activeId: string;
  onSelect: (id: string) => void;
  onClose: (id: string) => void;
  onNew: () => void;
}) {
  return (
    <div
      data-tauri-drag-region
      className="drag-region flex shrink-0 items-end gap-0.5 overflow-x-auto border-b border-line px-3 pt-2"
    >
      {tabs.map((tab) => {
        const isActive = tab.id === activeId;
        return (
          <div
            key={tab.id}
            role="tab"
            aria-selected={isActive}
            title={tabLabel(tab)}
            onClick={() => onSelect(tab.id)}
            onAuxClick={(event) => {
              // Middle-click closes, as in a browser.
              if (event.button === 1) onClose(tab.id);
            }}
            className={cx(
              "no-drag group flex max-w-[190px] shrink-0 cursor-default items-center gap-2 rounded-t-lg border border-b-0 px-3 py-1.5 text-[13px] transition-colors",
              isActive
                ? "border-line bg-surface text-ink"
                : "border-transparent text-ink-mute hover:bg-raised/60 hover:text-ink-soft",
            )}
          >
            <span aria-hidden="true" className="shrink-0 text-[12px]">
              {tabIcon(tab)}
            </span>
            <span className="truncate">{tabLabel(tab)}</span>
            <button
              type="button"
              aria-label={`Close ${tabLabel(tab)}`}
              onClick={(event) => {
                event.stopPropagation();
                onClose(tab.id);
              }}
              className={cx(
                "shrink-0 rounded px-1 text-[11px] leading-none text-ink-faint transition-opacity hover:bg-overlay hover:text-ink",
                isActive ? "opacity-100" : "opacity-0 group-hover:opacity-100",
              )}
            >
              ✕
            </button>
          </div>
        );
      })}

      {tabs.length < MAX_TABS && (
        <button
          type="button"
          aria-label="New tab"
          title="New tab (⌘T)"
          onClick={onNew}
          className="no-drag mb-1 shrink-0 rounded-md px-2 py-1 text-[15px] leading-none text-ink-faint transition-colors hover:bg-raised hover:text-ink"
        >
          +
        </button>
      )}

      <span className="no-drag ml-auto shrink-0 pb-1.5 pl-2">
        <ThemeToggle />
      </span>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Tab contents
// ---------------------------------------------------------------------------

function TabContent({
  tab,
  active,
  onOpenTab,
  onSetTitle,
}: {
  tab: Tab;
  active: boolean;
  onOpenTab: (request: Omit<Tab, "id">) => void;
  onSetTitle: (title: string | undefined) => void;
}) {
  switch (tab.kind) {
    case "home":
      return <HomeTab active={active} onOpenTab={onOpenTab} onSetTitle={onSetTitle} />;
    case "clipboard":
      return <ClipboardTabPage active={active} />;
    case "system":
      return <SystemTabPage active={active} />;
    case "awake":
      return <AwakePage active={active} />;
    case "sound":
      return <SoundPage active={active} />;
    case "ports":
      return <PortsPage active={active} />;
    case "docker":
      return <DockerPage active={active} />;
    case "machine":
      return <MachinePage active={active} />;
    case "tool":
      return (
        <ToolPage
          active={active}
          commandId={tab.commandId ?? ""}
          prefill={tab.prefill}
          onOpenTab={onOpenTab}
          onSetTitle={onSetTitle}
        />
      );
    case "extension":
      return (
        <ExtensionPage
          active={active}
          extensionId={tab.extensionId ?? ""}
          prefill={tab.prefill}
          onSetTitle={onSetTitle}
        />
      );
    case "workflow-import":
      return <WorkflowImportPage active={active} token={tab.token} onSetTitle={onSetTitle} />;
    case "permission":
      return (
        <PermissionPage
          active={active}
          permission={tab.permission ?? "accessibility"}
          retryCommandId={tab.retryCommandId}
          onOpenTab={onOpenTab}
        />
      );
    case "settings":
      return (
        <LazyPage>
          <SettingsPage initialSection={tab.section} />
        </LazyPage>
      );
    case "chat":
      return (
        <LazyPage>
          <ChatPage
            initialConversationId={tab.conversationId}
            initialPrefill={tab.prefill}
            initialMode={tab.chatMode ?? "chat"}
            onOpenTab={onOpenTab}
          />
        </LazyPage>
      );
  }
}

function LazyPage({ children }: { children: React.ReactNode }) {
  return (
    <Suspense
      fallback={
        <div className="flex h-full items-center justify-center">
          <Spinner className="text-accent" />
        </div>
      }
    >
      {children}
    </Suspense>
  );
}
