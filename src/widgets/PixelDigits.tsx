/**
 * A tiny dot-matrix font, rendered as crisp pixel squares — the same
 * `shapeRendering="crispEdges"` trick `StaffResize.tsx`'s resize knob uses,
 * so a widget's numerals read as the same pixel-art language as the mark and
 * the rest of the chrome instead of as ordinary system type.
 */

/** `[row][col]` bitmaps, one string of `0`/`1` per row. Digits are 3 wide,
 * `:` and the space are 1 — every glyph is 5 rows tall. */
const GLYPHS: Record<string, string[]> = {
  "0": ["111", "101", "101", "101", "111"],
  "1": ["010", "110", "010", "010", "111"],
  "2": ["111", "001", "111", "100", "111"],
  "3": ["111", "001", "111", "001", "111"],
  "4": ["101", "101", "111", "001", "001"],
  "5": ["111", "100", "111", "001", "111"],
  "6": ["111", "100", "111", "101", "111"],
  "7": ["111", "001", "010", "010", "010"],
  "8": ["111", "101", "111", "101", "111"],
  "9": ["111", "101", "111", "001", "111"],
  ":": ["0", "1", "0", "1", "0"],
  " ": ["0", "0", "0", "0", "0"],
};

const GLYPH_HEIGHT = 5;

export function PixelText({
  text,
  cell = 4,
  gap = 1,
  color = "rgb(var(--c-ink))",
  className,
}: {
  text: string;
  /** Rendered size of one grid cell, in CSS px. */
  cell?: number;
  /** Gap between glyphs, in grid cells. */
  gap?: number;
  color?: string;
  className?: string;
}) {
  const glyphs = text.split("").map((ch) => GLYPHS[ch] ?? GLYPHS[" "]);

  let cursor = 0;
  const cells: { x: number; y: number }[] = [];
  for (const rows of glyphs) {
    rows.forEach((row, y) => {
      row.split("").forEach((bit, x) => {
        if (bit === "1") cells.push({ x: cursor + x, y });
      });
    });
    cursor += rows[0].length + gap;
  }
  // The trailing gap was added after the last glyph too; drop it so the
  // canvas is exactly as wide as what is actually drawn.
  const totalWidth = Math.max(0, cursor - gap);

  return (
    <svg
      width={totalWidth * cell}
      height={GLYPH_HEIGHT * cell}
      viewBox={`0 0 ${totalWidth} ${GLYPH_HEIGHT}`}
      shapeRendering="crispEdges"
      aria-hidden="true"
      className={className ?? "block"}
    >
      {cells.map((c) => (
        <rect key={`${c.x}-${c.y}`} x={c.x} y={c.y} width={1} height={1} fill={color} />
      ))}
    </svg>
  );
}
