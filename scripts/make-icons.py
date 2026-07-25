#!/usr/bin/env python3
"""Render Orbit's icon set from code.

Everything Orbit ships as a raster icon is generated here rather than checked in
as an opaque binary, so a contributor can restyle the brand by editing numbers in
one file and re-running:

    python3 scripts/make-icons.py

Outputs
  assets/icon-source.png     1024x1024 master, fed to `npm run tauri icon`
  src-tauri/icons/tray.png   monochrome macOS template image (menu bar)
  src-tauri/icons/tray@2x.png
  website/orbit-mark.png     transparent mark for the landing page / README

Requires Pillow (`pip install pillow`). This script is a build-time convenience,
not a runtime dependency — the generated PNGs are committed.
"""

from __future__ import annotations

import math
import os

from PIL import Image, ImageDraw, ImageFilter

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# --- Brand ------------------------------------------------------------------
# Keep these in sync with the --o-accent tokens in src/styles.css.
BG_TOP = (24, 25, 37)
BG_BOTTOM = (9, 10, 15)
ORB_LIGHT = (201, 200, 255)
ORB_MID = (140, 139, 255)
ORB_DEEP = (91, 87, 232)
ORB_SHADOW = (42, 36, 120)
RING_A = (124, 124, 255)
RING_B = (79, 227, 208)

SS = 4  # supersampling factor; everything is drawn big and downsampled once


def lerp(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


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


def radial_sphere(size, cx, cy, radius):
    """A lit sphere with a soft terminator, rendered per-pixel."""
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    px = img.load()
    lx, ly = cx - radius * 0.34, cy - radius * 0.40  # light position
    for y in range(size):
        for x in range(size):
            dx, dy = x - cx, y - cy
            d = math.hypot(dx, dy)
            if d > radius + 1.5:
                continue
            # Distance from the light, normalised across the sphere.
            t = min(math.hypot(x - lx, y - ly) / (radius * 1.65), 1.0)
            if t < 0.34:
                c = lerp(ORB_LIGHT, ORB_MID, t / 0.34)
            elif t < 0.72:
                c = lerp(ORB_MID, ORB_DEEP, (t - 0.34) / 0.38)
            else:
                c = lerp(ORB_DEEP, ORB_SHADOW, (t - 0.72) / 0.28)
            # 1.5px feathered edge so the sphere doesn't alias against the ring.
            a = 255 if d <= radius - 1.5 else round(255 * max(0.0, (radius + 1.5 - d) / 3.0))
            px[x, y] = (*c, a)
    return img


def orbit_ring(size, cx, cy, rx, ry, tilt_deg, width, occlude_r=None):
    """Ellipse stroked with a two-stop gradient, optionally occluded by the orb."""
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    px = img.load()
    tilt = math.radians(tilt_deg)
    cos_t, sin_t = math.cos(tilt), math.sin(tilt)
    steps = 3600
    for i in range(steps):
        th = 2 * math.pi * i / steps
        ex, ey = rx * math.cos(th), ry * math.sin(th)
        x = cx + ex * cos_t - ey * sin_t
        y = cy + ex * sin_t + ey * cos_t
        # Fade the far half of the orbit so the ring reads as 3D.
        depth = (math.sin(th) + 1) / 2
        col = lerp(RING_A, RING_B, (math.cos(th) + 1) / 2)
        alpha = round(255 * (0.16 + 0.84 * depth))
        if occlude_r is not None and math.hypot(x - cx, y - cy) < occlude_r:
            # Behind the orb: keep a faint trace only where it peeks out.
            continue
        r = width / 2
        for oy in range(-int(r) - 1, int(r) + 2):
            for ox in range(-int(r) - 1, int(r) + 2):
                d = math.hypot(ox, oy)
                if d > r:
                    continue
                sx, sy = int(x) + ox, int(y) + oy
                if not (0 <= sx < size and 0 <= sy < size):
                    continue
                a = round(alpha * min(1.0, (r - d) / 1.5 + 0.35))
                if a > px[sx, sy][3]:
                    px[sx, sy] = (*col, a)
    return img


def build_mark(size):
    """The mark on a transparent background: orbit ring + orb + satellite."""
    s = size * SS
    c = s / 2
    orb_r = s * 0.180
    layer = Image.new("RGBA", (s, s), (0, 0, 0, 0))

    # Halo.
    halo = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    ImageDraw.Draw(halo).ellipse(
        (c - s * 0.30, c - s * 0.30, c + s * 0.30, c + s * 0.30), fill=(*RING_A, 46)
    )
    layer.alpha_composite(halo.filter(ImageFilter.GaussianBlur(s * 0.055)))

    ring = orbit_ring(s, c, c, s * 0.405, s * 0.164, -27, s * 0.027, occlude_r=orb_r * 1.02)
    layer.alpha_composite(ring)

    layer.alpha_composite(radial_sphere(s, c, c, orb_r))

    # Specular highlight on the orb.
    spec = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    ImageDraw.Draw(spec).ellipse(
        (c - orb_r * 0.62, c - orb_r * 0.68, c - orb_r * 0.02, c - orb_r * 0.24),
        fill=(255, 255, 255, 96),
    )
    layer.alpha_composite(spec.filter(ImageFilter.GaussianBlur(s * 0.012)))

    # Satellite, with its own bloom.
    sat = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    sx, sy, sr = c + s * 0.344, c - s * 0.188, s * 0.039
    ImageDraw.Draw(sat).ellipse((sx - sr, sy - sr, sx + sr, sy + sr), fill=(*RING_B, 255))
    glow = sat.filter(ImageFilter.GaussianBlur(s * 0.022))
    layer.alpha_composite(glow)
    layer.alpha_composite(sat)

    return layer.resize((size, size), Image.LANCZOS)


def build_app_icon(size=1024):
    """The mark inside a rounded, gradient-filled tile (macOS/Windows app icon)."""
    s = size * SS
    inset = round(s * 0.085)  # macOS icons sit inside a safe area
    tile_size = s - inset * 2

    bg = vertical_gradient(tile_size, BG_TOP, BG_BOTTOM).convert("RGBA")
    bg.putalpha(rounded_mask(tile_size, radius=round(tile_size * 0.225)))

    # A faint top hairline, the same trick the UI uses to fake a light source.
    hair = Image.new("RGBA", (tile_size, tile_size), (0, 0, 0, 0))
    ImageDraw.Draw(hair).rounded_rectangle(
        (0, 0, tile_size - 1, tile_size - 1),
        radius=round(tile_size * 0.225),
        outline=(255, 255, 255, 30),
        width=max(2, round(tile_size * 0.004)),
    )
    bg.alpha_composite(hair)

    mark = build_mark(round(tile_size * 0.86))
    canvas = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    canvas.alpha_composite(bg, (inset, inset))
    canvas.alpha_composite(
        mark, (inset + (tile_size - mark.width) // 2, inset + (tile_size - mark.height) // 2)
    )
    return canvas.resize((size, size), Image.LANCZOS)


def build_tray(size):
    """macOS template image: pure black silhouette + alpha, tinted by the OS."""
    s = size * SS
    c = s / 2
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    stroke = max(2, round(s * 0.055))

    # Orbit ellipse.
    d.ellipse((c - s * 0.44, c - s * 0.18, c + s * 0.44, c + s * 0.18), outline=(0, 0, 0, 255), width=stroke)
    ring = img.rotate(27, resample=Image.BICUBIC, center=(c, c))

    out = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    out.alpha_composite(ring)
    ImageDraw.Draw(out).ellipse(
        (c - s * 0.185, c - s * 0.185, c + s * 0.185, c + s * 0.185), fill=(0, 0, 0, 255)
    )
    return out.resize((size, size), Image.LANCZOS)


def main():
    os.makedirs(os.path.join(ROOT, "assets"), exist_ok=True)
    os.makedirs(os.path.join(ROOT, "src-tauri", "icons"), exist_ok=True)
    os.makedirs(os.path.join(ROOT, "website"), exist_ok=True)

    icon = build_app_icon(1024)
    icon.save(os.path.join(ROOT, "assets", "icon-source.png"))
    print("wrote assets/icon-source.png")

    mark = build_mark(512)
    mark.save(os.path.join(ROOT, "website", "orbit-mark.png"))
    mark.save(os.path.join(ROOT, "assets", "orbit-mark.png"))
    print("wrote website/orbit-mark.png + assets/orbit-mark.png")

    for name, px in (("tray.png", 22), ("tray@2x.png", 44)):
        build_tray(px).save(os.path.join(ROOT, "src-tauri", "icons", name))
        print(f"wrote src-tauri/icons/{name}")


if __name__ == "__main__":
    main()
