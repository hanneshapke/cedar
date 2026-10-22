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
                  transparent background (or an SVG, rendered with cairosvg or rsvg-convert),
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
    "rs": ["chat/src/*.rs", "desktop/src/*.rs"],
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
    """A PNG or SVG as a px-wide RGBA image (an SVG through cairosvg, else rsvg-convert)."""
    from PIL import Image
    if path.suffix.lower() != ".svg":
        return Image.open(path).convert("RGBA")
    try:
        import cairosvg
        data = cairosvg.svg2png(url=str(path), output_width=px)
    except ImportError:
        if not shutil.which("rsvg-convert"):
            sys.exit(f"{path}: rendering an SVG needs cairosvg (python3 -m pip install cairosvg) or "
                     "rsvg-convert (brew install librsvg); or pass --icon as a PNG")
        data = subprocess.run(["rsvg-convert", "-w", str(px), str(path)], check=True, capture_output=True).stdout
    return Image.open(io.BytesIO(data)).convert("RGBA")


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
