#!/usr/bin/env python3
"""Draws app-icon.png (the app icon), the sizes tauri.conf.json lists under icons/, and
tray-icon.png (a black-and-alpha template image for the menu bar). Needs Pillow:

    python3 -m pip install pillow && python3 make-icons.py
"""
from PIL import Image, ImageDraw

S = 1024


def tree(d, cx, scale, trunk, foliage):
    """A stylised cedar: a trunk and three tiers of foliage, widest at the bottom."""
    w = 34 * scale
    d.rounded_rectangle([cx - w, 640 * scale, cx + w, 820 * scale], radius=18 * scale, fill=trunk)
    tiers = [(330, 720, 420), (300, 600, 340), (270, 480, 250)]
    for (top, base, half), c in zip(tiers, foliage):
        d.polygon([(cx, top * scale), (cx - half * scale, base * scale), (cx + half * scale, base * scale)], fill=c)


app = Image.new("RGBA", (S, S), (0, 0, 0, 0))
d = ImageDraw.Draw(app)
m = 100  # macOS icons leave a margin inside the canvas
d.rounded_rectangle([m, m, S - m, S - m], radius=190, fill=(26, 74, 52, 255))
tree(d, S / 2, 1, (214, 170, 110, 255),
     [(96, 176, 128, 255), (120, 196, 144, 255), (150, 214, 160, 255)])
app.save("app-icon.png")
for name, px in [("32x32.png", 32), ("128x128.png", 128), ("128x128@2x.png", 256)]:
    app.resize((px, px), Image.LANCZOS).save(f"icons/{name}")
app.save("icons/icon.icns", format="ICNS", sizes=[(16, 16), (32, 32), (64, 64), (128, 128), (256, 256), (512, 512), (1024, 1024)])

# The menu bar wants a template image: black shapes on transparency, macOS tints it.
tray = Image.new("RGBA", (S, S), (0, 0, 0, 0))
d = ImageDraw.Draw(tray)
black = (0, 0, 0, 255)
tree(d, S / 2, 1, black, [black, black, black])
tray = tray.crop((S / 2 - 460, 230, S / 2 + 460, 860))  # tight box around the tree
canvas = Image.new("RGBA", (max(tray.size),) * 2, (0, 0, 0, 0))
canvas.paste(tray, ((canvas.width - tray.width) // 2, (canvas.height - tray.height) // 2))
canvas.resize((44, 44), Image.LANCZOS).save("tray-icon.png")
print("wrote app-icon.png, icons/, tray-icon.png")
