#!/usr/bin/env python3
"""Rebrands a copy of the source tree for a white-label build. scripts/build-dmg.sh runs it on a
staged copy before compiling; never point it at your checkout, it edits files in place.

    scripts/rebrand.py <tree> [--assistant-name NAME] [--app-name NAME] [--identifier ID]
                              [--logo SVG] [--logo-on-dark SVG] [--icon PNG|SVG]
                              [--icon-background COLOR] [--dry-run]

--assistant-name  replaces "Cedar" and the word "cedar" where people read them: the text of the
                  chat pages and the setup page, and the string literals of the chat server and the
                  app (notifications, menus, error messages). Code is left alone: identifiers, CSS
                  classes, storage keys, file names, URLs, CEDAR_* variables, and a lone lowercase
                  "cedar" in a string without spaces (a key, not prose).
--app-name        the app's name (bundle, menu bar, DMG); --identifier its bundle identifier.
--logo            an SVG mark for light backgrounds: the logo in the chat's top bar and the favicon.
--logo-on-dark    the same for dark backgrounds (default: --logo).
--icon            the glyph for the app icon, the menu bar and the favicons' PNGs: a PNG with a
                  transparent background, or an SVG: rendered with cairosvg or rsvg-convert where
                  they work, else by this script itself, which draws flat logos (filled paths and
                  shapes, no transforms, strokes, gradients or text),
                  drawn on --icon-background (default white). Default: --logo-on-dark if the
                  background is dark, else --logo.

Needs Pillow for the icons (python3 -m pip install pillow), like desktop/make-icons.py.
"""
import argparse
import io
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

# A whole-word "Cedar" or "cedar", not part of an identifier, path, class, id, URL or file name
# ("cedar-chat", "cedar.db", "crate::cedar", "Cedar.app"), but "Cedar." and "cedar's" are words.
# "Cedar style" is the name of a front end (v2), not of the assistant.
NAME = re.compile(r"(?<![\w\-./@:#$])(Cedar|cedar)(?![\w\-/]|\.\w| style)")


def replace_words(text, name, prose):
    """Replaces the name in a piece of text; a lowercase "cedar" only if `prose`."""
    return NAME.sub(lambda m: name if (m.group(1) == "Cedar" or prose) else m.group(0), text)


def rebrand_strings(src, name, lang):
    """Replaces the name inside the string literals of JavaScript or Rust source, leaving code
    and comments alone. Returns the new source and the strings it changed."""
    out, changes, i, n = [], [], 0, len(src)
    while i < n:
        c = src[i]
        if src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j < 0 else j
            out.append(src[i:j]); i = j
        elif src.startswith("/*", i):
            j = src.find("*/", i + 2)
            j = n if j < 0 else j + 2
            out.append(src[i:j]); i = j
        elif lang == "rs" and c == "r" and re.match(r'r#*"', src[i:]) and not (i and (src[i - 1].isalnum() or src[i - 1] == "_")):
            hashes = re.match(r'r(#*)"', src[i:]).group(1)
            start = i + 2 + len(hashes)
            end = src.find('"' + hashes, start)
            end = n if end < 0 else end
            out.append(src[i:start]); out.append(_lit(src[start:end], name, changes)); i = end
        elif lang == "rs" and c == "'":
            # A char literal ('"', '\'') or a lifetime ('a): neither holds a string.
            m = re.match(r"'(\\.|[^\\'])'", src[i:])
            j = i + (len(m.group(0)) if m else 1)
            out.append(src[i:j]); i = j
        elif c == '"' or (lang == "js" and c in "'`"):
            j = i + 1
            while j < n and src[j] != c:
                j += 2 if src[j] == "\\" else 1
            out.append(c); out.append(_lit(src[i + 1:j], name, changes)); out.append(src[j:j + 1]); i = j + 1
        elif lang == "js" and c == "/" and _regex_may_start(src, i):
            # A regular expression literal: copy it as it is, so its quotes don't open a string.
            j, in_class = i + 1, False
            while j < n and (in_class or src[j] != "/") and src[j] != "\n":
                if src[j] == "\\":
                    j += 1
                elif src[j] == "[":
                    in_class = True
                elif src[j] == "]":
                    in_class = False
                j += 1
            out.append(src[i:j + 1]); i = j + 1
        else:
            out.append(c); i += 1
    return "".join(out), changes


def _regex_may_start(src, i):
    prev = src[:i].rstrip()
    return not prev or prev[-1] in "(,=:[!&|?{};+-*%<>~^" or re.search(r"\b(return|typeof|case|in|of)$", prev)


def _lit(body, name, changes):
    new = replace_words(body, name, prose=bool(re.search(r"\s", body)))
    if new != body:
        changes.append((body, new))
    return new


TEXT_ATTRS = re.compile(r'\b(title|aria-label|placeholder|alt|content)="([^"]*)"')


def rebrand_html(src, name):
    """Replaces the name in an HTML page's text and its readable attributes; inline scripts go
    through the JavaScript rules, styles are left alone."""
    out, changes = [], []
    for part in re.split(r"(<script\b[^>]*>.*?</script>|<style\b[^>]*>.*?</style>|<[^>]+>)", src, flags=re.S | re.I):
        if part.lower().startswith("<script"):
            head, body, tail = re.match(r"(<script\b[^>]*>)(.*?)(</script>)", part, re.S | re.I).groups()
            body, ch = rebrand_strings(body, name, "js")
            changes += ch
            out.append(head + body + tail)
        elif part.lower().startswith("<style"):
            out.append(part)
        elif part.startswith("<"):
            new = TEXT_ATTRS.sub(lambda m: f'{m.group(1)}="{replace_words(m.group(2), name, True)}"', part)
            if new != part:
                changes.append((part, new))
            out.append(new)
        else:
            new = replace_words(part, name, True)
            if new.strip() != part.strip():
                changes.append((part.strip(), new.strip()))
            out.append(new)
    return "".join(out), changes


# Where people read the name. src/ (cedar itself) only speaks to the chat server and the app.
TEXT_FILES = {
    "html": ["chat/static/index.html", "chat/static/v*/index.html", "desktop/ui/*.html"],
    "js": ["chat/static/*.js", "chat/static/v*/app.js", "desktop/ui/*.js"],
    "rs": ["chat/src/*.rs", "desktop/src/*.rs", "pairing/src/*.rs"],
}


def rebrand_text(root, name, dry_run):
    total = 0
    for kind, globs in TEXT_FILES.items():
        for pattern in globs:
            for path in sorted(root.glob(pattern)):
                src = path.read_text()
                new, changes = rebrand_html(src, name) if kind == "html" else rebrand_strings(src, name, kind)
                total += len(changes)
                if dry_run:
                    for old, repl in changes:
                        print(f"{path.relative_to(root)}: {old[:90]!r} -> {repl[:90]!r}")
                elif new != src:
                    path.write_text(new)
    print(f"rebrand: {total} strings name {name!r}", file=sys.stderr)


def rebrand_bundle(root, app_name, identifier, assistant_name):
    conf_path = root / "desktop" / "tauri.conf.json"
    conf = json.loads(conf_path.read_text())
    if app_name:
        conf["productName"] = app_name
    if identifier:
        conf["identifier"] = identifier
    if assistant_name:
        for key in ("shortDescription", "longDescription"):
            conf["bundle"][key] = replace_words(conf["bundle"][key], assistant_name, True)
    conf_path.write_text(json.dumps(conf, indent=2) + "\n")


# -- the logo and the icons ------------------------------------------------------------------

def rasterize(path, px):
    """A PNG or SVG as a px-wide RGBA image. An SVG goes through cairosvg or rsvg-convert when
    either works here, else through `draw_svg`, which knows the flat shapes logos are made of."""
    from PIL import Image
    if path.suffix.lower() != ".svg":
        return Image.open(path).convert("RGBA")
    try:
        import cairosvg
        return Image.open(io.BytesIO(cairosvg.svg2png(url=str(path), output_width=px))).convert("RGBA")
    except (ImportError, OSError):
        pass  # not installed, or installed without the Cairo library (as on a Mac without Homebrew's)
    if shutil.which("rsvg-convert"):
        data = subprocess.run(["rsvg-convert", "-w", str(px), str(path)], check=True, capture_output=True).stdout
        return Image.open(io.BytesIO(data)).convert("RGBA")
    return draw_svg(path, px)


# -- a small SVG renderer for flat logos -----------------------------------------------------

SVG_UNSUPPORTED = re.compile(r"\btransform=|<(image|text|use|mask|clipPath|pattern|filter|\w*[Gg]radient)\b|\bstroke=\"(?!none)")
SVG_SHAPE = re.compile(r"<(path|rect|circle|ellipse|polygon)\b([^>]*)>")
SVG_ATTR = re.compile(r'([\w-]+)="([^"]*)"')
PATH_TOKEN = re.compile(r"[MmLlHhVvCcSsQqTtAaZz]|[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?")


def draw_svg(path, px):
    """Draws an SVG of filled paths, rects, circles, ellipses and polygons (no transforms,
    strokes, gradients, text or images: the shapes of a flat logo) as a px-wide RGBA image.
    Overlapping subpaths of one shape cut holes (the even-odd rule)."""
    from PIL import Image, ImageChops, ImageColor, ImageDraw
    text = re.sub(r"<metadata>.*?</metadata>", "", path.read_text(), flags=re.S)
    if SVG_UNSUPPORTED.search(text):
        sys.exit(f"{path}: this SVG uses transforms, strokes, gradients, text or images; render it with "
                 "cairosvg (brew install cairo && python3 -m pip install cairosvg) or rsvg-convert "
                 "(brew install librsvg), or pass --icon as a PNG")
    vb = re.search(r'viewBox="([^"]+)"', text)
    if not vb:
        sys.exit(f"{path}: no viewBox")
    x0, y0, w, h = (float(v) for v in vb.group(1).replace(",", " ").split())
    ss = 4
    k = px * ss / w
    size = (px * ss, max(1, round(h * k)))
    img = Image.new("RGBA", size, (0, 0, 0, 0))
    pt = lambda x, y: ((x - x0) * k, (y - y0) * k)
    for tag, attrs in SVG_SHAPE.findall(text):
        a = dict(SVG_ATTR.findall(attrs))
        style = dict(kv.split(":", 1) for kv in a.get("style", "").replace(" ", "").split(";") if ":" in kv)
        fill = style.get("fill", a.get("fill", "#000"))
        if fill == "none":
            continue
        mask = Image.new("L", size, 0)
        if tag == "path":
            for poly in path_polygons(a.get("d", "")):
                if len(poly) > 2:
                    sub = Image.new("L", size, 0)
                    ImageDraw.Draw(sub).polygon([pt(x, y) for x, y in poly], fill=255)
                    mask = ImageChops.difference(mask, sub)
        else:
            num = lambda n: float(a.get(n, 0))
            d = ImageDraw.Draw(mask)
            if tag == "rect":
                d.rounded_rectangle([pt(num("x"), num("y")), pt(num("x") + num("width"), num("y") + num("height"))],
                                    radius=max(num("rx"), num("ry")) * k, fill=255)
            elif tag == "circle":
                d.ellipse([pt(num("cx") - num("r"), num("cy") - num("r")), pt(num("cx") + num("r"), num("cy") + num("r"))], fill=255)
            elif tag == "ellipse":
                d.ellipse([pt(num("cx") - num("rx"), num("cy") - num("ry")), pt(num("cx") + num("rx"), num("cy") + num("ry"))], fill=255)
            else:
                v = [float(t) for t in re.findall(r"[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?", a.get("points", ""))]
                d.polygon([pt(x, y) for x, y in zip(v[::2], v[1::2])], fill=255)
        opacity = float(style.get("fill-opacity", a.get("fill-opacity", 1))) * float(style.get("opacity", a.get("opacity", 1)))
        color = ImageColor.getrgb(fill if fill.startswith(("#", "rgb")) or fill.isalpha() else "#000")[:3]
        layer = Image.new("RGBA", size, color + (0,))
        layer.putalpha(mask.point(lambda v: round(v * opacity)))
        img.alpha_composite(layer)
    return img.resize((px, max(1, round(size[1] / ss))), Image.LANCZOS)


def path_polygons(d):
    """The subpaths of SVG path data as lists of points, curves and arcs flattened."""
    import math
    tokens = PATH_TOKEN.findall(d)
    polys, cur, i = [], [], 0
    x = y = sx = sy = 0.0
    last_ctrl, cmd, prev = None, None, None

    def nums(n):
        nonlocal i
        vals = [float(t) for t in tokens[i:i + n]]
        i += n
        return vals

    def curve(points, steps=24):
        # Bezier through the control points, by de Casteljau.
        for s in range(1, steps + 1):
            t, ps = s / steps, list(points)
            while len(ps) > 1:
                ps = [(a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t) for a, b in zip(ps, ps[1:])]
            cur.append(ps[0])

    while i < len(tokens):
        if tokens[i].isalpha():
            cmd = tokens[i]; i += 1
        elif cmd is None:
            break
        rel = cmd.islower()
        c = cmd.upper()
        ox, oy = (x, y) if rel else (0.0, 0.0)
        if c == "Z":
            if cur:
                polys.append(cur)
            cur, (x, y) = [], (sx, sy)
            last_ctrl = None
            prev = c
            continue
        if c == "M":
            if cur:
                polys.append(cur)
            px_, py_ = nums(2)
            x, y = ox + px_, oy + py_
            sx, sy, cur = x, y, [(x, y)]
            cmd = "l" if rel else "L"  # coordinates after a moveto are linetos
        elif c == "L":
            px_, py_ = nums(2); x, y = ox + px_, oy + py_; cur.append((x, y))
        elif c == "H":
            (v,) = nums(1); x = (x if rel else 0) + v; cur.append((x, y))
        elif c == "V":
            (v,) = nums(1); y = (y if rel else 0) + v; cur.append((x, y))
        elif c in "CS":
            if c == "C":
                x1, y1, x2, y2, ex, ey = nums(6); c1 = (ox + x1, oy + y1)
            else:
                x2, y2, ex, ey = nums(4)
                c1 = (2 * x - last_ctrl[0], 2 * y - last_ctrl[1]) if last_ctrl and prev in "CS" else (x, y)
            c2, end = (ox + x2, oy + y2), (ox + ex, oy + ey)
            curve([(x, y), c1, c2, end]); last_ctrl, (x, y) = c2, end
        elif c in "QT":
            if c == "Q":
                x1, y1, ex, ey = nums(4); c1 = (ox + x1, oy + y1)
            else:
                ex, ey = nums(2)
                c1 = (2 * x - last_ctrl[0], 2 * y - last_ctrl[1]) if last_ctrl and prev in "QT" else (x, y)
            end = (ox + ex, oy + ey)
            curve([(x, y), c1, end]); last_ctrl, (x, y) = c1, end
        elif c == "A":
            rx, ry, rot, large, sweep, ex, ey = nums(7)
            ex, ey = ox + ex, oy + ey
            cur.extend(arc_points(x, y, rx, ry, rot, large, sweep, ex, ey, math))
            x, y = ex, ey
        if c not in "CSQT":
            last_ctrl = None
        prev = c
    if cur:
        polys.append(cur)
    return polys


def arc_points(x1, y1, rx, ry, rot, large, sweep, x2, y2, math, steps=32):
    """An SVG elliptical arc as points, by the endpoint-to-centre conversion of SVG 1.1 F.6.5."""
    if rx == 0 or ry == 0 or (x1, y1) == (x2, y2):
        return [(x2, y2)]
    rx, ry, phi = abs(rx), abs(ry), math.radians(rot)
    cos, sin = math.cos(phi), math.sin(phi)
    dx, dy = (x1 - x2) / 2, (y1 - y2) / 2
    x1p, y1p = cos * dx + sin * dy, -sin * dx + cos * dy
    scale = (x1p / rx) ** 2 + (y1p / ry) ** 2
    if scale > 1:
        rx, ry = rx * math.sqrt(scale), ry * math.sqrt(scale)
    num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p
    co = math.sqrt(max(0, num / (rx * rx * y1p * y1p + ry * ry * x1p * x1p)))
    if large == sweep:
        co = -co
    cxp, cyp = co * rx * y1p / ry, -co * ry * x1p / rx
    cx, cy = cos * cxp - sin * cyp + (x1 + x2) / 2, sin * cxp + cos * cyp + (y1 + y2) / 2
    ang = lambda ux, uy, vx, vy: math.atan2(ux * vy - uy * vx, ux * vx + uy * vy)
    t1 = ang(1, 0, (x1p - cxp) / rx, (y1p - cyp) / ry)
    dt = ang((x1p - cxp) / rx, (y1p - cyp) / ry, (-x1p - cxp) / rx, (-y1p - cyp) / ry)
    if not sweep and dt > 0:
        dt -= 2 * math.pi
    elif sweep and dt < 0:
        dt += 2 * math.pi
    out = []
    for s in range(1, steps + 1):
        t = t1 + dt * s / steps
        out.append((cx + rx * math.cos(t) * cos - ry * math.sin(t) * sin, cy + rx * math.cos(t) * sin + ry * math.sin(t) * cos))
    return out


def fit(glyph, box):
    """The glyph trimmed to its opaque pixels and scaled to fit a box-by-box square, centred."""
    from PIL import Image
    bbox = glyph.getchannel("A").getbbox()
    if not bbox:
        sys.exit("the icon has no opaque pixels; it needs a transparent background")
    glyph = glyph.crop(bbox)
    scale = box / max(glyph.size)
    glyph = glyph.resize((max(1, round(glyph.width * scale)), max(1, round(glyph.height * scale))), Image.LANCZOS)
    square = Image.new("RGBA", (box, box), (0, 0, 0, 0))
    square.paste(glyph, ((box - glyph.width) // 2, (box - glyph.height) // 2), glyph)
    return square


def tile(glyph, px, background, radius):
    """The glyph on a rounded square of the background colour, full bleed, px wide."""
    from PIL import Image, ImageDraw
    big = px * 4
    img = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    ImageDraw.Draw(img).rounded_rectangle([0, 0, big - 1, big - 1], radius=radius * big, fill=background)
    g = fit(glyph, round(big * 0.6))
    img.alpha_composite(g, ((big - g.width) // 2, (big - g.height) // 2))
    return img.resize((px, px), Image.LANCZOS)


def is_dark(color):
    from PIL import ImageColor
    r, g, b = ImageColor.getrgb(color)[:3]
    return 0.2126 * r + 0.7152 * g + 0.0722 * b < 128


def rebrand_icons(root, icon, background):
    from PIL import Image
    desktop, assets = root / "desktop", root / "docs" / "assets"
    glyph = rasterize(icon, 1024)
    radius = 11 / 48  # the corner of cedar-app-icon.svg
    # macOS: the tile on 824 of the 1024 px canvas, as in desktop/make-icons.py.
    shape = tile(glyph, 824, background, radius)
    app = Image.new("RGBA", (1024, 1024), (0, 0, 0, 0))
    app.paste(shape, (100, 100), shape)
    app.save(desktop / "app-icon.png")
    for name, px in [("32x32.png", 32), ("128x128.png", 128), ("128x128@2x.png", 256)]:
        app.resize((px, px), Image.LANCZOS).save(desktop / "icons" / name)
    app.save(desktop / "icons" / "icon.icns", format="ICNS",
             sizes=[(16, 16), (32, 32), (64, 64), (128, 128), (256, 256), (512, 512), (1024, 1024)])
    shape.save(desktop / "icons" / "icon.ico", format="ICO",
               sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
    tile(glyph, 32, background, radius).save(desktop / "tray-icon-color.png")
    # The menu bar's template image: the glyph's shape in black; macOS tints it.
    mono = fit(glyph, 44)
    black = Image.new("RGBA", mono.size, (0, 0, 0, 255))
    black.putalpha(mono.getchannel("A"))
    black.save(desktop / "tray-icon.png")
    # The chat's favicons and home-screen icon.
    tile(glyph, 32, background, radius).save(assets / "favicon-32.png")
    tile(glyph, 16, background, radius).save(assets / "favicon-16.png")
    tile(glyph, 180, background, 0).save(assets / "apple-touch-icon.png")


def rebrand_logo(root, logo, logo_on_dark):
    assets = root / "docs" / "assets"
    shutil.copyfile(logo, assets / "cedar-mark.svg")
    shutil.copyfile(logo_on_dark or logo, assets / "cedar-mark-on-dark.svg")
    shutil.copyfile(logo, assets / "favicon.svg")


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("root", type=Path)
    p.add_argument("--assistant-name")
    p.add_argument("--app-name")
    p.add_argument("--identifier")
    p.add_argument("--logo", type=Path)
    p.add_argument("--logo-on-dark", type=Path)
    p.add_argument("--icon", type=Path)
    p.add_argument("--icon-background", default="#ffffff")
    p.add_argument("--dry-run", action="store_true", help="print the text changes, change nothing")
    a = p.parse_args()

    for f in (a.logo, a.logo_on_dark, a.icon):
        if f and not f.is_file():
            sys.exit(f"{f}: no such file")
    for f in (a.logo, a.logo_on_dark):
        if f and f.suffix.lower() != ".svg":
            sys.exit(f"{f}: the logo must be an SVG")
    if a.assistant_name and not re.fullmatch(r"[^\"'\\<>`${}]{1,40}", a.assistant_name):
        sys.exit("--assistant-name: 1 to 40 characters, without quotes, backslashes, braces, < > ` or $")
    if a.assistant_name:
        rebrand_text(a.root, a.assistant_name, a.dry_run)
    if a.dry_run:
        return
    if a.app_name or a.identifier or a.assistant_name:
        rebrand_bundle(a.root, a.app_name, a.identifier, a.assistant_name)
    if a.logo:
        rebrand_logo(a.root, a.logo, a.logo_on_dark)
    icon = a.icon or ((a.logo_on_dark or a.logo) if is_dark(a.icon_background) else a.logo)
    if icon:
        rebrand_icons(a.root, icon, a.icon_background)


if __name__ == "__main__":
    main()
