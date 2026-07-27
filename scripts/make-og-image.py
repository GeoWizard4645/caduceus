#!/usr/bin/env python3
"""Generate the social card — `website/og-image.png`, 1200x630.

# Why this is generated rather than designed

Every other image in this project comes out of one pixel grid in
`make-icons.py`, so that redrawing the mark redraws the app icon, the tray
template and the staff together. A social card drawn by hand in some other tool
would be the one place the mark could drift out of step — and it is the copy
most people see first, because it is what renders when the link is pasted into
Slack, X, Discord or a Reddit comment.

So it imports the same grid. Change the mark, re-run both scripts, and the
preview card changes with everything else.

Run with `python3 scripts/make-og-image.py`. Needs Pillow.
"""

import importlib.util
import os

from PIL import Image, ImageDraw, ImageFont

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)

# `make-icons.py` has a hyphen in it, so it is not importable by name. Loading
# it by path is still better than copying the grid: a second copy of the mark is
# exactly the thing this script exists to avoid.
_spec = importlib.util.spec_from_file_location("make_icons", os.path.join(HERE, "make-icons.py"))
_icons = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_icons)

BG_TOP, BG_BOTTOM = _icons.BG_TOP, _icons.BG_BOTTOM
COLOURS = _icons.COLOURS
render_mark = _icons.render_mark

# The size every scraper expects. Facebook, X, LinkedIn, Slack and Discord all
# crop toward 1.91:1; 1200x630 is the size that survives all of them uncropped.
W, H = 1200, 630

TITLE = "Caduceus"
TAGLINE = "A fast, local-first command centre for your Mac"
FOOTNOTE = "Open source  ·  macOS 11+  ·  No tracking  ·  ~10 MB"

# San Francisco, because the card is advertising a Mac app and the system font
# is the one the screenshots are already in.
FONT_STACK = [
    "/System/Library/Fonts/SFNS.ttf",
    "/System/Library/Fonts/HelveticaNeue.ttc",
    "/Library/Fonts/Arial.ttf",
]


def font(size: int, weight: int = 0):
    """A font at `size`, falling back until something loads.

    Pillow cannot select a weight out of a variable font, so the heavier title
    is faked by drawing it twice a pixel apart. That is invisible at card sizes
    and avoids shipping a font file with the repo.
    """
    for path in FONT_STACK:
        try:
            return ImageFont.truetype(path, size)
        except OSError:
            continue
    return ImageFont.load_default()


def background() -> Image.Image:
    """The same vertical gradient the app icon sits on."""
    img = Image.new("RGB", (W, H), BG_BOTTOM)
    draw = ImageDraw.Draw(img)
    for y in range(H):
        t = y / (H - 1)
        draw.line(
            [(0, y), (W, y)],
            fill=tuple(round(a + (b - a) * t) for a, b in zip(BG_TOP, BG_BOTTOM)),
        )
    return img


def glow(img: Image.Image) -> None:
    """A soft accent wash behind the mark, so it does not float on flat navy."""
    layer = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    draw = ImageDraw.Draw(layer)
    cx, cy = 250, H // 2
    for radius in range(320, 0, -8):
        alpha = round(16 * (1 - radius / 320))
        draw.ellipse(
            [cx - radius, cy - radius, cx + radius, cy + radius],
            fill=COLOURS["snake_a"] + (alpha,),
        )
    img.paste(Image.alpha_composite(img.convert("RGBA"), layer).convert("RGB"), (0, 0))


def text(draw, xy, body, fnt, fill, bold=False):
    draw.text(xy, body, font=fnt, fill=fill)
    if bold:
        draw.text((xy[0] + 1, xy[1]), body, font=fnt, fill=fill)


def main() -> None:
    img = background()
    glow(img)

    # The mark, at whatever cell size fits the left third with room to breathe.
    mark = render_mark(cell_px=22)
    img.paste(mark, (250 - mark.width // 2, (H - mark.height) // 2), mark)

    draw = ImageDraw.Draw(img)
    left = 470

    text(draw, (left, 214), TITLE, font(96), COLOURS["wing"], bold=True)
    text(draw, (left, 336), TAGLINE, font(34), (176, 182, 204))
    text(draw, (left, 404), FOOTNOTE, font(24), (124, 132, 158))

    # A rule in the accent, tying the card to the site's own hero.
    draw.rectangle([left, 386, left + 62, 389], fill=COLOURS["snake_b"])

    out = os.path.join(ROOT, "website", "og-image.png")
    img.save(out, optimize=True)
    print(f"wrote website/og-image.png ({os.path.getsize(out) // 1024} KB)")


if __name__ == "__main__":
    main()
