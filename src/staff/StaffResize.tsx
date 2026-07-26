/**
 * Corner resize for the staff mark.
 *
 * On hover, a light rectangle frames the mark and each corner shows a pixel
 * circle with three menu lines. Drag any corner toward or away from the centre
 * to grow or shrink; the size is written to Appearance settings on release.
 */

import { useEffect, useRef, useState } from "react";

import { cx } from "@/shared/ui";

export const STAFF_SIZE_MIN = 28;
export const STAFF_SIZE_MAX = 160;

type Corner = "nw" | "ne" | "sw" | "se";

const CORNERS: { id: Corner; className: string; cursor: string }[] = [
  { id: "nw", className: "left-0 top-0 -translate-x-1/2 -translate-y-1/2", cursor: "nwse-resize" },
  { id: "ne", className: "right-0 top-0 translate-x-1/2 -translate-y-1/2", cursor: "nesw-resize" },
  { id: "sw", className: "bottom-0 left-0 -translate-x-1/2 translate-y-1/2", cursor: "nesw-resize" },
  { id: "se", className: "bottom-0 right-0 translate-x-1/2 translate-y-1/2", cursor: "nwse-resize" },
];

function clampSize(n: number): number {
  return Math.round(Math.min(STAFF_SIZE_MAX, Math.max(STAFF_SIZE_MIN, n)));
}

/** Pixel-art circle with three horizontal menu lines. */
function ResizeKnob({ active }: { active: boolean }) {
  const rim = active ? "rgb(var(--c-accent))" : "rgb(var(--c-line-strong))";
  const fill = "rgb(var(--c-surface))";
  const line = "rgb(var(--c-ink-soft))";
  // Stepped circle on a 14×14 grid (drawn at 16px with 1px padding).
  const disc = [
    [4, 1, 6, 1],
    [2, 2, 10, 1],
    [1, 3, 12, 1],
    [1, 4, 12, 6],
    [1, 10, 12, 1],
    [2, 11, 10, 1],
    [4, 12, 6, 1],
  ] as const;
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 14 14"
      shapeRendering="crispEdges"
      aria-hidden="true"
      className="block drop-shadow-[0_1px_2px_rgb(0_0_0/0.45)]"
    >
      {disc.map(([x, y, w, h], i) => (
        <rect key={`d${i}`} x={x} y={y} width={w} height={h} fill={fill} />
      ))}
      {/* Rim — outer ring cells. */}
      <rect x="4" y="1" width="6" height="1" fill={rim} />
      <rect x="2" y="2" width="2" height="1" fill={rim} />
      <rect x="10" y="2" width="2" height="1" fill={rim} />
      <rect x="1" y="3" width="1" height="8" fill={rim} />
      <rect x="12" y="3" width="1" height="8" fill={rim} />
      <rect x="2" y="11" width="2" height="1" fill={rim} />
      <rect x="10" y="11" width="2" height="1" fill={rim} />
      <rect x="4" y="12" width="6" height="1" fill={rim} />
      {/* Three menu lines. */}
      <rect x="4" y="4" width="6" height="1" fill={line} />
      <rect x="4" y="6" width="6" height="1" fill={line} />
      <rect x="4" y="8" width="6" height="1" fill={line} />
    </svg>
  );
}

export function StaffResizeFrame({
  size,
  visible,
  onLiveSize,
  onCommit,
  onResizingChange,
}: {
  size: number;
  visible: boolean;
  onLiveSize: (size: number) => void;
  onCommit: (size: number) => void;
  onResizingChange: (resizing: boolean) => void;
}) {
  const [activeCorner, setActiveCorner] = useState<Corner | null>(null);
  const drag = useRef<{
    corner: Corner;
    startSize: number;
    startDist: number;
    centreX: number;
    centreY: number;
  } | null>(null);

  useEffect(() => {
    if (!activeCorner) return;
    const onMove = (e: PointerEvent) => {
      const d = drag.current;
      if (!d || d.startDist < 4) return;
      const dist = Math.hypot(e.clientX - d.centreX, e.clientY - d.centreY);
      onLiveSize(clampSize(d.startSize * (dist / d.startDist)));
    };
    const onUp = (e: PointerEvent) => {
      const d = drag.current;
      drag.current = null;
      setActiveCorner(null);
      onResizingChange(false);
      if (!d) return;
      const dist = Math.hypot(e.clientX - d.centreX, e.clientY - d.centreY);
      const next =
        d.startDist < 4 ? d.startSize : clampSize(d.startSize * (dist / d.startDist));
      onLiveSize(next);
      onCommit(next);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
    };
  }, [activeCorner, onCommit, onLiveSize, onResizingChange]);

  const box = size + 10;

  return (
    <div
      className="pointer-events-none absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2"
      style={{ width: box, height: box }}
      aria-hidden={!visible}
    >
      <div
        className={cx(
          "absolute inset-0 rounded-[3px] border border-dashed transition-opacity duration-150",
          visible ? "opacity-100" : "opacity-0",
        )}
        style={{
          borderColor: "rgb(var(--c-accent) / 0.55)",
          boxShadow: visible ? "0 0 0 1px rgb(var(--c-base) / 0.35)" : undefined,
        }}
      />

      {CORNERS.map((corner) => (
        <button
          key={corner.id}
          type="button"
          tabIndex={visible ? 0 : -1}
          aria-label={`Resize staff from the ${corner.id} corner`}
          title="Drag to resize"
          onPointerDown={(e) => {
            if (e.button !== 0) return;
            e.preventDefault();
            e.stopPropagation();
            const rect = e.currentTarget.parentElement?.getBoundingClientRect();
            if (!rect) return;
            const centreX = rect.left + rect.width / 2;
            const centreY = rect.top + rect.height / 2;
            drag.current = {
              corner: corner.id,
              startSize: size,
              startDist: Math.hypot(e.clientX - centreX, e.clientY - centreY),
              centreX,
              centreY,
            };
            setActiveCorner(corner.id);
            onResizingChange(true);
            e.currentTarget.setPointerCapture(e.pointerId);
          }}
          style={{ cursor: corner.cursor }}
          className={cx(
            "pointer-events-auto absolute z-[60] rounded-full",
            "transition-[opacity,transform] duration-150 ease-cad",
            "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent",
            corner.className,
            visible ? "opacity-100 scale-100" : "pointer-events-none opacity-0 scale-75",
            activeCorner === corner.id && "scale-110",
          )}
        >
          <ResizeKnob active={activeCorner === corner.id} />
        </button>
      ))}
    </div>
  );
}
