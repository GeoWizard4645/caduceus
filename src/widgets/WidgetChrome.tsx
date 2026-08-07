import { getCurrentWindow } from "@tauri-apps/api/window";

import { cx } from "@/shared/ui";

import { destroyWidget } from "./widgetApi";

/**
 * The strip you drag a widget by.
 *
 * Copies the Command Center's `DragHandle` almost exactly (see
 * `src/command-center/CommandCenter.tsx`) rather than the staff's own
 * threshold-based `startDragging()` — a widget's whole window *is* its
 * content, so there is no small round mark to distinguish a click on from a
 * drag the way the staff has to. `data-tauri-drag-region` hands the gesture
 * to the window manager directly; nothing here tracks pointer movement.
 *
 * The remove button rides in the same strip because a widget, unlike the
 * palette, can be closed for good — see `widgets_destroy` in
 * `src-tauri/src/widgets.rs` for why that is a `destroy()`, not a `close()`.
 */
export function WidgetChrome({ id }: { id: string }) {
  return (
    <div
      data-tauri-drag-region
      title="Drag to move"
      className="drag-region flex h-5 shrink-0 cursor-grab items-center justify-between px-1.5 active:cursor-grabbing"
    >
      <span aria-hidden="true" className="pointer-events-none flex gap-[3px] opacity-40">
        {[0, 1, 2].map((i) => (
          <span key={i} className="h-[3px] w-[3px] rounded-full bg-ink-faint" />
        ))}
      </span>
      <button
        type="button"
        aria-label="Remove widget"
        title="Remove widget"
        onClick={() => void destroyWidget(id)}
        className={cx(
          "no-drag flex h-[16px] w-[16px] items-center justify-center rounded-full",
          "text-[9px] leading-none text-ink-faint transition-colors hover:bg-raised hover:text-ink",
        )}
      >
        ✕
      </button>
    </div>
  );
}

/**
 * The edges and corners you drag to resize — same trick as the Command
 * Center's `ResizeGrip`: hand the gesture to the window manager so it tracks
 * the pointer at the compositor's frame rate instead of ours, which is what
 * makes a resize feel immediate rather than laggy.
 *
 * Widgets are small, so a single corner grip was an even worse target here
 * than it was on the Command Center. The whole perimeter is draggable now:
 * 5px edge strips and 12px corners, with the corners stacked on top so they
 * win the overlap.
 */
const WIDGET_RESIZE_HANDLES = [
  { dir: "North", className: "left-3 right-3 top-0 h-1.5 cursor-ns-resize" },
  { dir: "South", className: "bottom-0 left-3 right-3 h-1.5 cursor-ns-resize" },
  { dir: "West", className: "bottom-3 left-0 top-3 w-1.5 cursor-ew-resize" },
  { dir: "East", className: "bottom-3 right-0 top-3 w-1.5 cursor-ew-resize" },
  { dir: "NorthWest", className: "left-0 top-0 h-3 w-3 cursor-nwse-resize" },
  { dir: "NorthEast", className: "right-0 top-0 h-3 w-3 cursor-nesw-resize" },
  { dir: "SouthWest", className: "bottom-0 left-0 h-3 w-3 cursor-nesw-resize" },
  { dir: "SouthEast", className: "bottom-0 right-0 h-3 w-3 cursor-nwse-resize" },
] as const;

export function WidgetResizeGrip() {
  const startResize = async (event: React.PointerEvent, direction: string) => {
    // Left button only: a right-drag on an edge should open nothing and
    // resize nothing.
    if (event.button !== 0) return;
    event.preventDefault();
    try {
      // The string cast is the API's own union of the eight compass points,
      // which `WIDGET_RESIZE_HANDLES` is written against.
      await getCurrentWindow().startResizeDragging(direction as never);
    } catch {
      // Not in a Tauri window, or the runtime refused. Nothing to fall back
      // to, and nothing that should surface to the user.
    }
  };

  return (
    <>
      {WIDGET_RESIZE_HANDLES.map(({ dir, className }) => (
        <div
          key={dir}
          onPointerDown={(event) => void startResize(event, dir)}
          aria-hidden="true"
          className={`no-drag absolute z-50 ${className}`}
        />
      ))}
      {/* Three lines, shortest at the corner — the same grow-box shape the
          Command Center's grip and classic Mac OS both use. Purely the visual
          hint; the south-east handle above is what catches the drag. */}
      <svg
        viewBox="0 0 16 16"
        aria-hidden="true"
        className="pointer-events-none absolute bottom-0 right-0 z-50 h-3.5 w-3.5 stroke-ink-faint opacity-45"
      >
        <path d="M15 5 L5 15 M15 9 L9 15 M15 13 L13 15" strokeWidth="1.4" strokeLinecap="round" />
      </svg>
    </>
  );
}
