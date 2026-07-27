/**
 * The Manage window: browser-style tabs over the app's stateful surfaces.
 *
 * A palette row is the right shape for "do this once"; it is the wrong shape
 * for anything with state you want to keep an eye on — a keep-awake countdown,
 * which sound device is live, what is holding port 3000. Those get pages here,
 * and the pages get tabs, so a countdown can stay open while you use another
 * page — exactly the reason browsers grew tabs.
 *
 * # Tab model
 *
 * One tab per page, ever. Opening a page that already has a tab focuses it
 * rather than duplicating it: two tabs both showing the same live state invite
 * the question of which one is right, and the answer ("both, they poll") is
 * worse than the question. `⌘1`–`⌘5` jump between tabs, `⌘W` closes one —
 * closing the last hides the window, since an empty tab bar manages nothing.
 */

import { type ReactElement, useCallback, useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { useTauriEvent } from "@/shared/hooks";
import { EVENTS } from "@/shared/types";
import { ThemeToggle } from "@/shared/ThemeToggle";
import { cx } from "@/shared/ui";

import { AwakePage } from "./pages/AwakePage";
import { SoundPage } from "./pages/SoundPage";
import { PortsPage } from "./pages/PortsPage";
import { DockerPage } from "./pages/DockerPage";
import { MachinePage } from "./pages/MachinePage";

export type PageId = "awake" | "sound" | "ports" | "docker" | "machine";

interface PageDef {
  id: PageId;
  title: string;
  icon: string;
  render: () => ReactElement;
}

export const PAGES: PageDef[] = [
  { id: "awake", title: "Keep Awake", icon: "☀", render: () => <AwakePage /> },
  { id: "sound", title: "Sound", icon: "◐", render: () => <SoundPage /> },
  { id: "ports", title: "Ports", icon: "◈", render: () => <PortsPage /> },
  { id: "docker", title: "Docker", icon: "◉", render: () => <DockerPage /> },
  { id: "machine", title: "This Mac", icon: "◍", render: () => <MachinePage /> },
];

function isPageId(value: string | null | undefined): value is PageId {
  return PAGES.some((page) => page.id === value);
}

export function Manage() {
  const [open, setOpen] = useState<PageId[]>(["awake"]);
  const [active, setActive] = useState<PageId>("awake");

  const show = useCallback((page: PageId) => {
    setOpen((current) => (current.includes(page) ? current : [...current, page]));
    setActive(page);
  }, []);

  // The palette (or a deep link) asks for a page by name.
  useTauriEvent<string | null>(EVENTS.manageOpen, (page) => {
    if (isPageId(page)) show(page);
  });

  const close = useCallback(
    (page: PageId) => {
      setOpen((current) => {
        const next = current.filter((id) => id !== page);
        if (next.length === 0) {
          // An empty tab bar manages nothing; the window state is kept for the
          // next open, so this is a hide rather than a reset.
          void getCurrentWindow().hide();
          return current;
        }
        if (active === page) {
          // Focus the neighbour, the way browsers do.
          const index = current.indexOf(page);
          setActive(next[Math.max(0, index - 1)]);
        }
        return next;
      });
    },
    [active],
  );

  // ⌘1–⌘9 jump, ⌘W closes the active tab.
  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if (!event.metaKey) return;
      if (event.key === "w") {
        event.preventDefault();
        close(active);
        return;
      }
      const digit = Number.parseInt(event.key, 10);
      if (digit >= 1 && digit <= open.length) {
        event.preventDefault();
        setActive(open[digit - 1]);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [open, active, close]);

  const unopened = PAGES.filter((page) => !open.includes(page.id));

  return (
    <div className="flex h-screen flex-col bg-base text-ink">
      {/* --- tab bar ------------------------------------------------------ */}
      <div
        data-tauri-drag-region
        className="flex shrink-0 items-end gap-1 border-b border-line px-3 pt-2"
        // Room for the traffic lights under the overlay title bar.
        style={{ paddingLeft: 84 }}
      >
        {open.map((id) => {
          const page = PAGES.find((entry) => entry.id === id)!;
          const isActive = id === active;
          return (
            <div
              key={id}
              role="tab"
              aria-selected={isActive}
              onClick={() => setActive(id)}
              className={cx(
                "group flex cursor-default items-center gap-2 rounded-t-lg border border-b-0 px-3 py-1.5 text-[13px] transition-colors",
                isActive
                  ? "border-line bg-surface text-ink"
                  : "border-transparent text-ink-mute hover:bg-raised/60 hover:text-ink-soft",
              )}
            >
              <span aria-hidden="true" className="text-[12px]">
                {page.icon}
              </span>
              {page.title}
              <button
                type="button"
                aria-label={`Close ${page.title}`}
                onClick={(event) => {
                  event.stopPropagation();
                  close(id);
                }}
                className={cx(
                  "rounded px-1 text-[11px] leading-none text-ink-faint transition-opacity hover:bg-overlay hover:text-ink",
                  isActive ? "opacity-100" : "opacity-0 group-hover:opacity-100",
                )}
              >
                ✕
              </button>
            </div>
          );
        })}

        {unopened.length > 0 && (
          <NewTabMenu pages={unopened} onPick={(page) => show(page)} />
        )}

        <span className="ml-auto pb-1.5">
          <ThemeToggle />
        </span>
      </div>

      {/* --- page body ---------------------------------------------------- */}
      {/* Every open page stays mounted; hiding is CSS. Unmounting would reset
          a countdown's local state or a filter every time you switch tabs,
          which defeats the reason tabs exist. */}
      <div className="min-h-0 flex-1 overflow-hidden">
        {open.map((id) => {
          const page = PAGES.find((entry) => entry.id === id)!;
          return (
            <div
              key={id}
              role="tabpanel"
              className={cx("h-full overflow-y-auto", id !== active && "hidden")}
            >
              {page.render()}
            </div>
          );
        })}
      </div>
    </div>
  );
}

/** The "+" button: a small menu of pages not yet open. */
function NewTabMenu({
  pages,
  onPick,
}: {
  pages: PageDef[];
  onPick: (page: PageId) => void;
}) {
  const [openMenu, setOpenMenu] = useState(false);

  return (
    <div className="relative pb-1">
      <button
        type="button"
        aria-label="Open a page"
        onClick={() => setOpenMenu((value) => !value)}
        className="rounded-md px-2 py-1 text-[15px] leading-none text-ink-faint transition-colors hover:bg-raised hover:text-ink"
      >
        +
      </button>
      {openMenu && (
        <>
          {/* Click-away layer. */}
          <div className="fixed inset-0 z-10" onClick={() => setOpenMenu(false)} />
          <div className="absolute left-0 top-full z-20 mt-1 min-w-[180px] rounded-lg border border-line bg-surface py-1 shadow-float">
            {pages.map((page) => (
              <button
                key={page.id}
                type="button"
                onClick={() => {
                  setOpenMenu(false);
                  onPick(page.id);
                }}
                className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-[13px] text-ink-soft transition-colors hover:bg-raised hover:text-ink"
              >
                <span aria-hidden="true">{page.icon}</span>
                {page.title}
              </button>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
