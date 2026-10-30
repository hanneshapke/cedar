"""A minimal text-only PDF writer, so a task can declare a PDF attachment as lines of text in its
YAML (`text_pdf`) instead of committing a binary file."""

from __future__ import annotations

LINES_PER_PAGE = 50


def _escape(text: str) -> str:
    text = text.encode("latin-1", "replace").decode("latin-1")
    return text.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")


def make_pdf(lines: list[str], title: str = "") -> bytes:
    pages = [lines[i:i + LINES_PER_PAGE] for i in range(0, len(lines), LINES_PER_PAGE)] or [[]]
    objects: list[bytes] = []

    def add(body: str | bytes) -> int:
        objects.append(body.encode("latin-1") if isinstance(body, str) else body)
        return len(objects)

    catalog = add("")  # filled in once the page tree's number is known
    tree = add("")
    font = add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>")
    kids = []
    for page_lines in pages:
        ops = ["BT", "/F1 11 Tf", "14 TL", "56 790 Td"]
        for line in page_lines:
            ops.append(f"({_escape(line)}) Tj T*")
        ops.append("ET")
        stream = "\n".join(ops).encode("latin-1", "replace")
        content = add(b"<< /Length %d >>\nstream\n" % len(stream) + stream + b"\nendstream")
        kids.append(add(f"<< /Type /Page /Parent {tree} 0 R /MediaBox [0 0 595 842] "
                        f"/Resources << /Font << /F1 {font} 0 R >> >> /Contents {content} 0 R >>"))
    objects[catalog - 1] = f"<< /Type /Catalog /Pages {tree} 0 R >>".encode()
    objects[tree - 1] = (f"<< /Type /Pages /Kids [{' '.join(f'{k} 0 R' for k in kids)}] "
                         f"/Count {len(kids)} >>").encode()
    info = add(f"<< /Title ({_escape(title)}) /Producer (cedar-bench) >>")

    out = bytearray(b"%PDF-1.4\n")
    offsets = []
    for i, body in enumerate(objects, 1):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % i + body + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objects) + 1)
    out += b"".join(b"%010d 00000 n \n" % o for o in offsets)
    out += (b"trailer\n<< /Size %d /Root %d 0 R /Info %d 0 R >>\nstartxref\n%d\n%%%%EOF\n"
            % (len(objects) + 1, catalog, info, xref))
    return bytes(out)
