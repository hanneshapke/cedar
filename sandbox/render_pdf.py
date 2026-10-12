#!/usr/bin/env python3
"""Render a Markdown or HTML document to a PDF. cedar's `generate_pdf` tool runs this inside the
sandbox; it is installed in the image as `cedar-render-pdf`.

    cedar-render-pdf [--format markdown|html] [--title TITLE] INPUT OUTPUT

Markdown is converted with python-markdown (tables, fenced code, footnotes, definition lists, and
the other "extra" extensions), wrapped in a page template with a stylesheet, and rendered with
WeasyPrint. HTML input is wrapped in the same template unless it is already a full document (has an
`<html>` tag), in which case it is rendered as-is and --title is ignored. Relative image URLs
resolve against the current directory, so `![chart](chart.png)` picks up a workspace file.

On success one JSON line, {"pages": N, "bytes": M}, is printed to stdout. Warnings from the
renderer (a missing image, say) go to stderr; a failure prints its reason to stderr and exits 1.
"""

import argparse
import html
import json
import logging
import os
import re
import sys

STYLESHEET = """
@page {
    size: A4;
    margin: 22mm 20mm 24mm 20mm;
    @bottom-center {
        content: counter(page) " / " counter(pages);
        font-family: "DejaVu Sans", sans-serif;
        font-size: 9pt;
        color: #777;
    }
}
html { font-size: 11pt; }
body {
    font-family: "DejaVu Serif", serif;
    line-height: 1.5;
    color: #1a1a1a;
    margin: 0;
}
h1, h2, h3, h4, h5, h6 {
    font-family: "DejaVu Sans", sans-serif;
    line-height: 1.25;
    margin: 1.4em 0 0.5em;
    page-break-after: avoid;
}
h1 { font-size: 2em; margin-top: 0; }
h2 { font-size: 1.5em; border-bottom: 1px solid #ddd; padding-bottom: 0.2em; }
h3 { font-size: 1.2em; }
h4, h5, h6 { font-size: 1em; }
p, ul, ol, blockquote, pre, table, figure, dl { margin: 0 0 0.9em; }
li > p { margin-bottom: 0.3em; }
a { color: #1a4d8f; text-decoration: none; }
code, pre, kbd, samp {
    font-family: "DejaVu Sans Mono", monospace;
    font-size: 0.88em;
}
code { background: #f3f3f3; padding: 0.1em 0.3em; border-radius: 3px; }
pre {
    background: #f6f6f6;
    border: 1px solid #e3e3e3;
    border-radius: 4px;
    padding: 0.7em 0.9em;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
}
pre code { background: none; padding: 0; font-size: inherit; }
blockquote {
    border-left: 3px solid #ccc;
    color: #555;
    margin-left: 0;
    padding-left: 1em;
}
table { border-collapse: collapse; width: 100%; font-size: 0.95em; }
th, td { border: 1px solid #ccc; padding: 0.35em 0.6em; text-align: left; vertical-align: top; }
th { background: #f0f0f0; font-family: "DejaVu Sans", sans-serif; }
tr { page-break-inside: avoid; }
img { max-width: 100%; height: auto; }
hr { border: 0; border-top: 1px solid #ccc; margin: 1.5em 0; }
dt { font-weight: bold; }
dd { margin: 0 0 0.5em 1.5em; }
.footnote { font-size: 0.9em; border-top: 1px solid #ddd; margin-top: 2em; }
"""

TEMPLATE = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{title}</title>
</head>
<body>
{body}
</body>
</html>
"""

MARKDOWN_EXTENSIONS = ["extra", "sane_lists", "smarty", "admonition"]


def fail(message):
    print(message, file=sys.stderr)
    sys.exit(1)


def first_heading(body):
    m = re.search(r"<h1[^>]*>(.*?)</h1>", body, re.S | re.I)
    if not m:
        return ""
    text = re.sub(r"<[^>]+>", "", m.group(1))
    return html.unescape(text).strip()


def is_full_document(text):
    return re.search(r"<html[\s>]", text, re.I) is not None


def main():
    parser = argparse.ArgumentParser(description="Render Markdown or HTML to PDF.")
    parser.add_argument("--format", choices=["markdown", "html"], default="markdown")
    parser.add_argument("--title", default="")
    parser.add_argument("input")
    parser.add_argument("output")
    args = parser.parse_args()

    try:
        import markdown
        import weasyprint
    except ImportError as e:  # pragma: no cover - depends on the image
        fail(f"PDF rendering is not available in this sandbox image ({e})")

    # WeasyPrint reports a missing image or an unsupported CSS rule through logging; keep those
    # visible on stderr, but without the chatter below WARNING.
    logging.basicConfig(stream=sys.stderr, level=logging.WARNING, format="%(message)s")
    logging.getLogger("fontTools").setLevel(logging.ERROR)

    try:
        with open(args.input, encoding="utf-8") as f:
            source = f.read()
    except OSError as e:
        fail(f"cannot read {args.input}: {e}")
    except UnicodeDecodeError:
        fail(f"{args.input} is not valid UTF-8 text")

    if args.format == "markdown":
        body = markdown.markdown(
            source, extensions=MARKDOWN_EXTENSIONS, output_format="html5"
        )
        document = TEMPLATE.format(
            title=html.escape(args.title or first_heading(body)), body=body
        )
    elif is_full_document(source):
        document = source
    else:
        document = TEMPLATE.format(
            title=html.escape(args.title or first_heading(source)), body=source
        )

    out_dir = os.path.dirname(args.output)
    if out_dir:
        try:
            os.makedirs(out_dir, exist_ok=True)
        except OSError as e:
            fail(f"cannot create {out_dir}: {e}")

    try:
        rendered = weasyprint.HTML(string=document, base_url=os.getcwd() + "/").render(
            stylesheets=[weasyprint.CSS(string=STYLESHEET)]
        )
        rendered.write_pdf(args.output)
    except OSError as e:
        fail(f"cannot write {args.output}: {e}")
    except Exception as e:  # noqa: BLE001 - whatever WeasyPrint raises is the reason to report
        fail(f"rendering failed: {e}")

    print(json.dumps({"pages": len(rendered.pages), "bytes": os.path.getsize(args.output)}))


if __name__ == "__main__":
    main()
