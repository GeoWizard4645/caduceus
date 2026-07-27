/**
 * Lay the Desktop's icons out in a shape.
 *
 * # You see it before it happens
 *
 * Picking a shape draws it — every icon as a dot, where it is now in grey and
 * where it would go in colour, scaled to your screen. Nothing on the Desktop
 * moves until Apply, and Undo puts every icon back on the exact point it came
 * from.
 *
 * # When Finder will not play along
 *
 * If Finder is keeping the Desktop sorted (View → Sort By, or Stacks), it throws
 * away any position it is given. That is not something this page can turn off
 * for you — the Desktop's view options are not scriptable — so it says so up
 * front rather than appearing to work and changing nothing.
 */

import { useCallback, useEffect, useMemo, useState } from "react";

import * as api from "@/shared/api";
import { permissionFromMessage } from "@/shared/permissions";
import type { DesktopShape, DesktopShapePlan, DesktopSpot } from "@/shared/types";
import { Button, Callout, cx } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";
import { usePermissionGate } from "../../PermissionGate";

const SHAPES: { value: DesktopShape; label: string; closed: boolean; joined: boolean }[] = [
  { value: "circle", label: "Circle", closed: true, joined: true },
  { value: "heart", label: "Heart", closed: true, joined: true },
  { value: "spiral", label: "Spiral", closed: false, joined: true },
  { value: "grid", label: "Grid", closed: false, joined: false },
  { value: "line", label: "Line", closed: false, joined: true },
];

export function DesktopShapesPage({ active, onSetTitle }: ToolPageProps) {
  const [shape, setShape] = useState<DesktopShape>("circle");
  const [plan, setPlan] = useState<DesktopShapePlan | null>(null);
  const [undo, setUndo] = useState<DesktopSpot[] | null>(null);
  const [note, setNote] = useState<{ text: string; ok: boolean } | null>(null);
  const reportPermissionWall = usePermissionGate();
  const [busy, setBusy] = useState(false);

  useEffect(() => onSetTitle("Desktop icon shapes"), [onSetTitle]);

  const fail = useCallback((error: unknown) => {
    const text = api.errorMessage(error);
    const permission = permissionFromMessage(text);
    if (permission) reportPermissionWall(permission);
    else setNote({ text, ok: false });
  }, [reportPermissionWall]);

  const preview = useCallback(async () => {
    setBusy(true);
    setNote(null);
    try {
      setPlan(await api.desktopShapePlan(shape));
    } catch (e) {
      setPlan(null);
      fail(e);
    } finally {
      setBusy(false);
    }
  }, [fail, shape]);

  // Planning moves nothing, so the answer is drawn as soon as the shape
  // changes rather than behind a button.
  useEffect(() => {
    if (active && !undo) void preview();
  }, [active, preview, undo]);

  const apply = async () => {
    setBusy(true);
    try {
      const result = await api.desktopShapeApply(shape);
      setNote({ text: result.message, ok: result.ok });
      // Kept even when it only half worked: a partly rearranged Desktop is
      // exactly when undo matters.
      setUndo(result.previous.length > 0 ? result.previous : null);
    } catch (e) {
      fail(e);
    } finally {
      setBusy(false);
    }
  };

  const revert = async () => {
    if (!undo) return;
    setBusy(true);
    try {
      const result = await api.desktopShapeRevert(undo);
      setNote({ text: result.message, ok: result.ok });
      setUndo(result.previous.length > 0 ? result.previous : null);
    } catch (e) {
      fail(e);
    } finally {
      setBusy(false);
    }
  };

  const count = plan?.spots.length ?? 0;
  const chosen = SHAPES.find((option) => option.value === shape)!;

  return (
    <div className="flex h-full flex-col">
      <div className="shrink-0 border-b border-line px-5 py-3">
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">
          Desktop icon shapes
        </h1>
        <p className="mt-0.5 max-w-prose text-[13px] text-ink-mute">
          Nothing moves until you press Apply, and Undo puts every icon back on the point it came
          from.
        </p>

        <div className="row mt-3 flex-wrap gap-2">
          {SHAPES.map((option) => (
            <button
              key={option.value}
              type="button"
              disabled={busy || !!undo}
              onClick={() => setShape(option.value)}
              className={cx(
                "rounded-full border px-3 py-1 text-2xs transition-colors disabled:opacity-50",
                shape === option.value
                  ? "border-accent/40 bg-accent/12 text-accent"
                  : "border-line text-ink-mute hover:bg-raised hover:text-ink",
              )}
            >
              {option.label}
            </button>
          ))}
        </div>

        {note && (
          <p className={cx("mt-2 text-2xs", note.ok ? "text-ink-mute" : "text-danger")}>
            {note.text}
          </p>
        )}
      </div>

      <div className="min-h-0 flex-1 space-y-3 overflow-y-auto px-5 py-4">
        {plan?.arrangement.blocks && (
          <Callout
            tone="warn"
            title={`Finder is keeping your Desktop arranged by ${plan.arrangement.label}`}
          >
            <p>
              While that is on, Finder throws away any position it is given, so this would change
              nothing. {plan.arrangement.fix}
            </p>
          </Callout>
        )}

        {plan?.arrangement.snaps && (
          <Callout tone="info" title="Snap to Grid is on">
            <p>
              Finder will pull each icon onto the nearest grid position, so the shape comes out a
              little squarer than the preview. {plan.arrangement.fix}
            </p>
          </Callout>
        )}

        {undo ? (
          <div className="rounded-cad border border-positive/30 bg-positive/[0.06] p-4">
            <p className="text-[13px] text-ink">{note?.text}</p>
            <Button className="mt-3" onClick={() => void revert()} disabled={busy}>
              Undo — put every icon back
            </Button>
          </div>
        ) : !plan ? (
          <p className="py-10 text-center text-2xs text-ink-faint">
            {busy ? "Reading your Desktop…" : "Nothing to show."}
          </p>
        ) : count === 0 ? (
          <p className="py-10 text-center text-2xs text-ink-faint">
            There is nothing on your Desktop to arrange.
          </p>
        ) : (
          <Preview plan={plan} closed={chosen.closed} joined={chosen.joined} />
        )}
      </div>

      {plan && count > 0 && !undo && (
        <div className="row shrink-0 justify-between gap-3 border-t border-line px-5 py-3">
          <span className="text-[13px] text-ink">
            {count} icon{count === 1 ? "" : "s"} into a {chosen.label.toLowerCase()}
          </span>
          <Button
            tone="primary"
            onClick={() => void apply()}
            disabled={busy || plan.arrangement.blocks}
          >
            {busy ? "Arranging…" : "Apply"}
          </Button>
        </div>
      )}
    </div>
  );
}

/**
 * The shape, drawn to scale.
 *
 * The viewBox is the usable desktop area widened to hold wherever the icons
 * currently are, so a ghost sitting under the Dock is still visible rather than
 * clipped off the edge of the drawing.
 */
function Preview({
  plan,
  closed,
  joined,
}: {
  plan: DesktopShapePlan;
  closed: boolean;
  joined: boolean;
}) {
  const view = useMemo(() => {
    const pad = 40;
    const xs = [plan.area.x, plan.area.x + plan.area.width, ...plan.current.map((s) => s.x)];
    const ys = [plan.area.y, plan.area.y + plan.area.height, ...plan.current.map((s) => s.y)];
    const x = Math.min(...xs) - pad;
    const y = Math.min(...ys) - pad;
    return { x, y, width: Math.max(...xs) + pad - x, height: Math.max(...ys) + pad - y };
  }, [plan]);

  const path = plan.spots.map((spot) => `${spot.x},${spot.y}`).join(" ");

  return (
    <div className="rounded-cad border border-line bg-surface/40 p-3">
      <svg
        viewBox={`${view.x} ${view.y} ${view.width} ${view.height}`}
        className="h-auto w-full"
        role="img"
        aria-label={`${plan.spots.length} icons arranged in a ${plan.shape}`}
      >
        <rect
          x={plan.area.x}
          y={plan.area.y}
          width={plan.area.width}
          height={plan.area.height}
          rx={12}
          className="fill-none stroke-line"
          strokeWidth={2}
          strokeDasharray="10 10"
        />
        {plan.current.map((spot) => (
          <circle
            key={`from-${spot.name}`}
            cx={spot.x}
            cy={spot.y}
            r={13}
            className="fill-ink-faint/25"
          />
        ))}
        {joined && plan.spots.length > 1 && (
          <polyline
            points={closed ? `${path} ${plan.spots[0].x},${plan.spots[0].y}` : path}
            className="fill-none stroke-accent/35"
            strokeWidth={2}
          />
        )}
        {plan.spots.map((spot) => (
          <circle key={`to-${spot.name}`} cx={spot.x} cy={spot.y} r={15} className="fill-accent">
            <title>{spot.name}</title>
          </circle>
        ))}
      </svg>
      <p className="mt-2 text-2xs text-ink-faint">
        Grey is where each icon is now; the dashed edge is the screen between the menu bar and the
        Dock.
      </p>
    </div>
  );
}
