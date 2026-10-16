#!/usr/bin/env python3
"""Render the first pages of a PDF as JPEG images. cedar runs this inside the sandbox when a PDF
is attached to a prompt for a model that takes images; it is installed in the image as
`cedar-render-pages`.

    cedar-render-pages --pages N --max-side PX --out DIR -- PATH

Renders pages 1..N (fewer if the PDF is shorter) so that the longer side of each image is at most
PX pixels, and never above MAX_DPI. The images are written to DIR as page-1.jpg, page-2.jpg, ...
On success one JSON line is printed to stdout:

    {"page_count": 48, "images": ["DIR/page-1.jpg", ...]}

A failure prints its reason to stderr and exits 1.
"""

import argparse
import json
import os
import sys

MAX_DPI = 150
JPEG_QUALITY = 80


def fail(message):
    print(message, file=sys.stderr)
    sys.exit(1)


def main():
    parser = argparse.ArgumentParser(prog="cedar-render-pages")
    parser.add_argument("--pages", type=int, required=True)
    parser.add_argument("--max-side", type=int, required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("path")
    args = parser.parse_args()
    if args.pages < 1 or args.max_side < 1:
        fail("--pages and --max-side must be at least 1")

    try:
        import pypdfium2 as pdfium
    except ImportError:
        fail("pypdfium2 is not installed in the sandbox image")

    try:
        pdf = pdfium.PdfDocument(args.path)
    except Exception as e:  # pdfium raises its own error types for unreadable or encrypted files
        fail(f"cannot open {args.path} as a PDF: {e}")

    os.makedirs(args.out, exist_ok=True)
    count = len(pdf)
    images = []
    for i in range(min(count, args.pages)):
        page = pdf[i]
        width, height = page.get_size()  # in points, 1/72 inch
        scale = min(args.max_side / max(width, height, 1), MAX_DPI / 72)
        try:
            image = page.render(scale=scale).to_pil().convert("RGB")
        except Exception as e:
            fail(f"cannot render page {i + 1}: {e}")
        out = os.path.join(args.out, f"page-{i + 1}.jpg")
        image.save(out, "JPEG", quality=JPEG_QUALITY, optimize=True)
        images.append(out)
    print(json.dumps({"page_count": count, "images": images}))


if __name__ == "__main__":
    main()
