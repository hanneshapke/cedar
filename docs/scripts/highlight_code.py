#!/usr/bin/env python3
"""Syntax-highlight the code blocks of the landing page.

    python3 docs/scripts/highlight_code.py [docs/landing/index.html]

The page has no highlighting library: the colors are written into it as spans with a few
classes (`hl-c` comments, `hl-s` strings, `hl-k` keywords, `hl-p` JSON keys, `hl-n` numbers,
`hl-f` commands and calls, `hl-o` flags, `hl-v` variables), styled by one rule set in the page's
<style>. Run this after changing a code sample. It reads each <pre> block's text (dropping any
earlier highlighting), picks the language from its first line (`# ...tool.json` is JSON,
`# ...tool.py` is Python up to a `# share it` shell part, anything else is shell), and writes it
back highlighted. Running it again on a highlighted page changes nothing.
"""
import html
import re
import sys
from pathlib import Path

PATH = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parents[1] / "landing" / "index.html"
src = PATH.read_text(encoding="utf-8")


def span(cls, text):
    return f'<span class="hl-{cls}">{html.escape(text, quote=False)}</span>' if text else ""


def esc(text):
    return html.escape(text, quote=False)


# --- shell -----------------------------------------------------------------------------------
SH = re.compile(r"""(?P<c>(?:(?<=\s)|^)\#[^\n]*)|(?P<s>'[^']*'|"[^"]*")|(?P<v>\$\(?[A-Za-z_][A-Za-z0-9_]*\)?)|(?P<o>(?<=\s)--?[A-Za-z][\w-]*)|(?P<w>[^\s'"$#]+)|(?P<x>\s+|.)""")


def shell(code):
    out, start_of_command = [], True
    for line in code.split("\n"):
        start = start_of_command
        for m in SH.finditer(line):
            kind, text = m.lastgroup, m.group()
            if kind == "w" and start:
                out.append(span("f", text))
                start = False
            elif kind in ("c", "s", "v", "o"):
                out.append(span(kind, text))
                start = False
            else:
                out.append(esc(text))
        start_of_command = not line.rstrip().endswith("\\")
        out.append("\n")
    return "".join(out)[:-1]


# --- JSON ------------------------------------------------------------------------------------
JS = re.compile(r"""(?P<c>^\#[^\n]*)|(?P<s>"(?:\\.|[^"\\])*")(?P<key>\s*:)?|(?P<n>-?\d+(?:\.\d+)?|true|false|null)|(?P<x>.)""", re.S | re.M)


def json_(code):
    out = []
    for m in JS.finditer(code):
        if m.group("c"):
            out.append(span("c", m.group("c")))
        elif m.group("s"):
            out.append(span("p" if m.group("key") else "s", m.group("s")))
            if m.group("key"):
                out.append(esc(m.group("key")))
        elif m.group("n"):
            out.append(span("n", m.group("n")))
        else:
            out.append(esc(m.group("x")))
    return "".join(out)


# --- Python ----------------------------------------------------------------------------------
KEYWORDS = {"import", "from", "if", "not", "else", "elif", "for", "in", "return", "def", "and", "or",
            "with", "as", "try", "except", "None", "True", "False"}
PY = re.compile(r"""(?P<c>\#[^\n]*)|(?P<s>[fbr]?"(?:\\.|[^"\\])*"|[fbr]?'(?:\\.|[^'\\])*')|(?P<n>\b\d+(?:\.\d+)?\b)|(?P<w>[A-Za-z_][A-Za-z0-9_]*)(?P<call>\()?|(?P<x>.)""", re.S)


def python(code):
    out = []
    for m in PY.finditer(code):
        if m.group("c"):
            out.append(span("c", m.group("c")))
        elif m.group("s"):
            out.append(span("s", m.group("s")))
        elif m.group("n"):
            out.append(span("n", m.group("n")))
        elif m.group("w"):
            w = m.group("w")
            if w in KEYWORDS:
                out.append(span("k", w))
            elif m.group("call"):
                out.append(span("f", w))
            else:
                out.append(esc(w))
            if m.group("call"):
                out.append("(")
        else:
            out.append(esc(m.group("x")))
    return "".join(out)


# --- apply -----------------------------------------------------------------------------------
PRE = re.compile(r'(<pre style="[^"]*")(?: class="code")?>(.*?)</pre>', re.S)
TAG = re.compile(r"<[^>]+>")


def highlight(m):
    attrs, body = m.group(1), m.group(2)
    code = html.unescape(TAG.sub("", body))
    first = code.split("\n", 1)[0]
    if first.endswith("tool.json"):
        new = json_(code)
    elif first.endswith("tool.py"):
        py, _, tail = code.partition("\n\n# share it\n")
        new = python(py) + ("\n\n" + shell("# share it\n" + tail) if tail else "")
    else:
        new = shell(code)
    if "font-variant-ligatures" not in attrs:
        attrs = attrs[:-1] + ';font-variant-ligatures:none"'
    return f'{attrs} class="code">{new}</pre>'


out, n = PRE.subn(highlight, src)
if not n:
    sys.exit(f"no <pre style=...> code blocks in {PATH}")
CSS = (".code .hl-c{color:#a39a8c}.code .hl-s{color:#b8d4a3}.code .hl-k{color:#e8a98c}"
       ".code .hl-p{color:#e6c27a}.code .hl-n{color:#d9aed0}.code .hl-f{color:#8fc4d6}"
       ".code .hl-o{color:#c3b8e0}.code .hl-v{color:#e6c27a}")
# The color rules go first in the page's <style>, replacing an earlier copy.
out, had = re.subn(r"<style>\.code \.hl-[^\n]*\n", f"<style>{CSS}\n", out, count=1)
if not had:
    if out.count("<style>") != 1:
        sys.exit("expected exactly one <style> in the page for the color rules")
    out = out.replace("<style>", f"<style>{CSS}\n", 1)
PATH.write_text(out, encoding="utf-8")
print(f"highlighted {n} code blocks in {PATH}")
