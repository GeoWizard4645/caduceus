#!/usr/bin/env python3
"""Generate every Caduceus icon from one pixel-art grid.

The mark is a caduceus — Hermes' staff — drawn on a small pixel grid so it stays
crisp at menu-bar sizes instead of turning to mush. The grid is defined once,
here, and everything else is derived from it:

    assets/icon-source.png        1024x1024 master, fed to `npm run tauri icon`
    src-tauri/icons/tray.png      monochrome macOS template image (menu bar)
    src-tauri/icons/tray@2x.png
    src/shared/caduceusPixels.ts  the same grid as data, so the floating staff
                                  in the app renders identical pixels as SVG
    website/caduceus-mark.png     transparent mark for the landing page

Re-run after editing the grid:

    python3 scripts/make-icons.py

Requires Pillow (`pip install pillow`). Generated files are committed, so this
is a design-time tool, not a build step.
"""

from __future__ import annotations

import math
import os

from PIL import Image, ImageDraw, ImageFilter

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# --- Grid ------------------------------------------------------------------
# Kept small and odd-width so the staff has a true centre column. Every
# measurement below is in grid cells, not pixels.
GRID_W, GRID_H = 13, 20
CENTRE_X = 6

# Vertical bands.
KNOB_TOP_ROW, KNOB_ROW = 0, 1
STAFF_TOP, STAFF_BOTTOM = 2, 19
SNAKE_TOP, SNAKE_BOTTOM = 7, 18

# Snake helix, spanning exactly one period so each snake shows one full coil.
#
# The constraint that matters is amplitude <= period / 2*pi: any steeper and the
# path moves more than one cell per row, which breaks the line into disconnected
# dashes that read as zigzag noise instead of a snake wrapping a staff.
SNAKE_AMPLITUDE = 2.0
SNAKE_PERIOD = 12.0

# Left wing, as {row: [columns]}. Mirrored to the right automatically.
#
# A diagonal sweep: the tip sits high and outboard, and the wing narrows as it
# runs down to meet the staff. A flat top row reads as a crossbar and turns the
# whole mark into a winged sword, which is the failure mode to avoid.
WING_SHAPE = {
    3: [0, 1, 2],
    4: [1, 2, 3, 4],
    5: [3, 4, 5],
    6: [5],
}

# --- Palette ---------------------------------------------------------------
# The in-app mark swaps snake_a for the user's accent colour; these are the
# values baked into the raster icons.
COLOURS = {
    "knob": (255, 233, 168),
    "staff": (232, 196, 104),
    "wing": (237, 239, 247),
    "snake_a": (124, 124, 255),
    "snake_b": (79, 227, 208),
}

BG_TOP = (24, 25, 37)
BG_BOTTOM = (9, 10, 15)

SS = 4  # supersampling for the non-pixel-art parts (tile, glow)


def build_grid() -> dict:
    """Return {(x, y): kind} for every filled cell.

    Later writes win, so the order below is also the z-order: staff first, then
    wings, then the snakes on top of both.
    """
    cells = {}

    # Staff.
    for y in range(STAFF_TOP, STAFF_BOTTOM + 1):
        cells[(CENTRE_X, y)] = "staff"

    # Knob.
    cells[(CENTRE_X, KNOB_TOP_ROW)] = "knob"
    for x in (CENTRE_X - 1, CENTRE_X, CENTRE_X + 1):
        cells[(x, KNOB_ROW)] = "knob"

    # Wings, left then mirrored.
    for row, columns in WING_SHAPE.items():
        for x in columns:
            cells[(x, row)] = "wing"
            cells[(GRID_W - 1 - x, row)] = "wing"

    # Snakes: two sine waves half a period out of phase.
    crossing = 0
    for y in range(SNAKE_TOP, SNAKE_BOTTOM + 1):
        offset = SNAKE_AMPLITUDE * math.cos(2 * math.pi * (y - SNAKE_TOP) / SNAKE_PERIOD)
        xa = round(CENTRE_X + offset)
        xb = round(CENTRE_X - offset)

        if xa == xb:
            # A crossing. Drawing both would put one pixel on the staff and lose
            # any sense of depth, so only the snake passing *in front* is drawn,
            # and they alternate — which is what sells the helix.
            cells[(xa, y)] = "snake_a" if crossing % 2 == 0 else "snake_b"
            crossing += 1
        else:
            cells[(xa, y)] = "snake_a"
            cells[(xb, y)] = "snake_b"

    # Heads: a two-cell blunt tip angled outward, sitting directly above where
    # each snake's body starts so the two connect. No eye — at 13x18 a single
    # dark cell next to a one-cell head just reads as a longer dash.
    reach = round(SNAKE_AMPLITUDE)
    for kind, out in (("snake_a", 1), ("snake_b", -1)):
        x = CENTRE_X + reach * out
        cells[(x, SNAKE_TOP - 1)] = kind
        cells[(x + out, SNAKE_TOP - 1)] = kind

    return cells


GRID = build_grid()


# ---------------------------------------------------------------------------
# Raster output
# ---------------------------------------------------------------------------


def render_mark(cell_px, palette=None, alpha=255):
    """Draw the grid at `cell_px` pixels per cell, on transparency."""
    colours = palette or COLOURS
    img = Image.new("RGBA", (GRID_W * cell_px, GRID_H * cell_px), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    for (x, y), kind in GRID.items():
        colour = colours.get(kind, (255, 255, 255))
        draw.rectangle(
            [x * cell_px, y * cell_px, (x + 1) * cell_px - 1, (y + 1) * cell_px - 1],
            fill=(colour[0], colour[1], colour[2], alpha),
        )
    return img


def lerp(a, b, t):
    return tuple(round(p + (q - p) * t) for p, q in zip(a, b))


def vertical_gradient(size, top, bottom):
    img = Image.new("RGB", (1, size), top)
    px = img.load()
    for y in range(size):
        px[0, y] = lerp(top, bottom, y / max(size - 1, 1))
    return img.resize((size, size), Image.BILINEAR)


def rounded_mask(size, radius):
    m = Image.new("L", (size, size), 0)
    ImageDraw.Draw(m).rounded_rectangle((0, 0, size - 1, size - 1), radius=radius, fill=255)
    return m


def build_app_icon(size=1024):
    """The mark on a rounded, gradient-filled tile."""
    s = size * SS
    inset = round(s * 0.085)  # macOS icons sit inside a safe area
    tile = s - inset * 2

    bg = vertical_gradient(tile, BG_TOP, BG_BOTTOM).convert("RGBA")
    bg.putalpha(rounded_mask(tile, radius=round(tile * 0.225)))

    # The same faint top hairline the UI uses to fake a light source.
    hair = Image.new("RGBA", (tile, tile), (0, 0, 0, 0))
    ImageDraw.Draw(hair).rounded_rectangle(
        (0, 0, tile - 1, tile - 1),
        radius=round(tile * 0.225),
        outline=(255, 255, 255, 30),
        width=max(2, round(tile * 0.004)),
    )
    bg.alpha_composite(hair)

    # A soft accent glow behind the mark, so the icon has some depth.
    glow = Image.new("RGBA", (tile, tile), (0, 0, 0, 0))
    ImageDraw.Draw(glow).ellipse(
        (tile * 0.24, tile * 0.16, tile * 0.76, tile * 0.84), fill=(124, 124, 255, 62)
    )
    bg.alpha_composite(glow.filter(ImageFilter.GaussianBlur(tile * 0.075)))

    # Size the mark to ~76% of the tile, snapped to whole cells so the pixel
    # grid stays perfectly square.
    cell = max(1, round(tile * 0.76 / GRID_H))
    mark = render_mark(cell)
    bg.alpha_composite(mark, ((tile - mark.width) // 2, (tile - mark.height) // 2))

    canvas = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    canvas.alpha_composite(bg, (inset, inset))
    return canvas.resize((size, size), Image.LANCZOS)


def build_tray(height_px):
    """macOS template image: a pure-black silhouette that the OS tints itself.

    Rendered at a whole number of pixels per cell and never resampled — a scaled
    pixel mark in a 22px menu bar turns into grey mush.
    """
    cell = max(1, height_px // GRID_H)
    silhouette = {k: (0, 0, 0) for k in COLOURS}
    mark = render_mark(cell, palette=silhouette)

    canvas = Image.new("RGBA", (height_px, height_px), (0, 0, 0, 0))
    canvas.alpha_composite(mark, ((height_px - mark.width) // 2, (height_px - mark.height) // 2))
    return canvas


# ---------------------------------------------------------------------------
# TypeScript output
# ---------------------------------------------------------------------------

TS_KIND = {"snake_a": "snakeA", "snake_b": "snakeB"}


def build_typescript():
    """Emit the grid for the frontend, so the app and the icons cannot drift."""
    rows = []
    for y in range(GRID_H):
        for x in range(GRID_W):
            kind = GRID.get((x, y))
            if kind:
                rows.append('  [%d, %d, "%s"],' % (x, y, TS_KIND.get(kind, kind)))

    newline = "\n"
    return f'''// GENERATED by scripts/make-icons.py — do not edit by hand.
//
// The caduceus mark as pixel data. The floating staff renders these as SVG
// rects, which keeps it crisp at any size and lets each part take its colour
// from a CSS variable — so the snakes pick up the user's accent. The raster app
// and tray icons come from the same grid, so the two can never drift apart.

export const CADUCEUS_WIDTH = {GRID_W};
export const CADUCEUS_HEIGHT = {GRID_H};

export type PixelKind = "knob" | "staff" | "wing" | "snakeA" | "snakeB";

/** `[x, y, kind]`, one entry per filled cell. */
export const CADUCEUS_PIXELS: readonly (readonly [number, number, PixelKind])[] = [
{newline.join(rows)}
];

/**
 * Fill for each part. Wings stay fixed white (same as the app icons); the staff
 * keeps a warm gold; one snake follows the user's accent.
 */
export const PIXEL_FILL: Record<PixelKind, string> = {{
  knob: "#ffe9a8",
  staff: "#e8c468",
  wing: "#ffffff",
  snakeA: "rgb(var(--c-accent))",
  snakeB: "#4fe3d0",
}};
'''


# ---------------------------------------------------------------------------


def main():
    for directory in ("assets", "src-tauri/icons", "website", "src/shared"):
        os.makedirs(os.path.join(ROOT, directory), exist_ok=True)

    build_app_icon(1024).save(os.path.join(ROOT, "assets", "icon-source.png"))
    print("wrote assets/icon-source.png")

    mark = render_mark(cell_px=28)
    mark.save(os.path.join(ROOT, "website", "caduceus-mark.png"))
    mark.save(os.path.join(ROOT, "assets", "caduceus-mark.png"))
    print("wrote website/caduceus-mark.png + assets/caduceus-mark.png")

    for name, px in (("tray.png", 22), ("tray@2x.png", 44)):
        build_tray(px).save(os.path.join(ROOT, "src-tauri", "icons", name))
        print("wrote src-tauri/icons/" + name)

    with open(os.path.join(ROOT, "src", "shared", "caduceusPixels.ts"), "w") as f:
        f.write(build_typescript())
    print("wrote src/shared/caduceusPixels.ts")

    # A quick ASCII proof, so you can sanity-check the grid without opening a
    # single PNG.
    legend = {"knob": "O", "staff": "|", "wing": "W", "snake_a": "a", "snake_b": "b"}
    print()
    for y in range(GRID_H):
        print("    " + "".join(legend.get(GRID.get((x, y)), " ") for x in range(GRID_W)))


if __name__ == "__main__":
    main()
