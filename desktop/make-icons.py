#!/usr/bin/env python3
"""Renders the app's icons from the brand assets in ../docs/assets (the landing page's), which
are the source of truth:

    cedar-app-icon.svg   -> app-icon.png (1024 px, with the margin a macOS icon carries itself),
                            icons/32x32.png, icons/128x128.png, icons/128x128@2x.png, icons/icon.icns,
                            icons/icon.ico (full bleed, as Windows draws icons),
                            tray-icon-color.png (32 px, full bleed: the tray icon on Windows)
    cedar-mark-mono.svg  -> tray-icon.png (44 px, black on transparency: a menu bar template image)

Those SVGs are nothing but <rect> elements with rounded corners, so they are drawn here exactly
at any size without an SVG renderer. Needs Pillow:

    python3 -m pip install pillow && python3 make-icons.py
"""
import re
import sys
from pathlib import Path

from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent
ASSETS = HERE.parent / "docs" / "assets"
# Apple's icon template: the shape sits on 824 of the 1024 px canvas.
CANVAS = 1024
SHAPE = 824
SUPERSAMPLE = 4

RECT = re.compile(r"<rect\b([^>]*)>")
ATTR = re.compile(r'([\w-]+)="([^"]*)"')


def read_svg(name):
    path = ASSETS / name
    if not path.is_file():
        sys.exit(f"{path} is missing; the brand assets live in docs/assets")
    text = path.read_text()
    text = re.sub(r"<metadata>.*?</metadata>", "", text, flags=re.S)  # c2pa manifest
    size = re.search(r'viewBox="0 0 ([\d.]+) ([\d.]+)"', text)
    if not size:
        sys.exit(f"{path}: no viewBox")
    rects = []
    for m in RECT.finditer(text):
        a = dict(ATTR.findall(m.group(1)))
        rects.append({
            "x": float(a.get("x", 0)), "y": float(a.get("y", 0)),
            "w": float(a["width"]), "h": float(a["height"]),
            "rx": float(a.get("rx", 0)), "fill": a.get("fill", "#000"),
        })
    if not rects:
        sys.exit(f"{path}: no <rect> elements; this script only knows how to draw those")
    return float(size.group(1)), rects


def render(units, rects, px, color=None):
    """Draws the rects of a `units`-wide SVG as a px-by-px RGBA image, supersampled for clean
    edges. `color` replaces every fill (for the single-colour tray image)."""
    big = px * SUPERSAMPLE
    k = big / units
    img = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    for r in rects:
        box = [r["x"] * k, r["y"] * k, (r["x"] + r["w"]) * k, (r["y"] + r["h"]) * k]
        d.rounded_rectangle(box, radius=r["rx"] * k, fill=color or r["fill"])
    return img.resize((px, px), Image.LANCZOS)


# App icon: the full-bleed square, centred with the macOS margin on a transparent canvas.
units, rects = read_svg("cedar-app-icon.svg")
shape = render(units, rects, SHAPE)
app = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
app.paste(shape, ((CANVAS - SHAPE) // 2, (CANVAS - SHAPE) // 2), shape)
app.save(HERE / "app-icon.png")
(HERE / "icons").mkdir(exist_ok=True)
for name, px in [("32x32.png", 32), ("128x128.png", 128), ("128x128@2x.png", 256)]:
    app.resize((px, px), Image.LANCZOS).save(HERE / "icons" / name)
app.save(
    HERE / "icons" / "icon.icns",
    format="ICNS",
    sizes=[(16, 16), (32, 32), (64, 64), (128, 128), (256, 256), (512, 512), (1024, 1024)],
)

# Windows: the exe, the installer and the Start Menu draw the icon as it is, without the macOS margin.
shape.save(
    HERE / "icons" / "icon.ico",
    format="ICO",
    sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
)
# The Windows tray shows icons in colour, on light and dark taskbars alike.
render(units, rects, 32).save(HERE / "tray-icon-color.png")

# Menu bar: the mono mark in black; macOS tints a template image to the menu bar's colour.
units, rects = read_svg("cedar-mark-mono.svg")
render(units, rects, 44, color=(0, 0, 0, 255)).save(HERE / "tray-icon.png")

print("wrote app-icon.png, icons/, tray-icon.png from", ASSETS)
