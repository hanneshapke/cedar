"""report.pdf: the same analysis as report.md, laid out for reading and sharing.

A4, light theme from the reference data-viz palette: ink tokens for text, hairline tables, stat
tiles for the headline, the charts from charts.py, and status tints (good / warning / critical)
that always sit beside a label, never alone.
"""

from __future__ import annotations

from pathlib import Path
from xml.sax.saxutils import escape

from reportlab.lib import colors
from reportlab.lib.enums import TA_LEFT
from reportlab.lib.pagesizes import A4
from reportlab.lib.styles import ParagraphStyle
from reportlab.lib.units import mm
from reportlab.platypus import (Image, KeepTogether, PageBreak, Paragraph, SimpleDocTemplate,
                                Spacer, Table, TableStyle)

from . import report
from .charts import BASELINE, GRID, INK, INK_2, MUTED, SURFACE

PAGE_PLANE = colors.HexColor("#f9f9f7")
TILE_BORDER = colors.HexColor("#e1e0d9")
# Status tints: a light step of each status color, so the cell text (ink) stays legible.
GOOD, GOOD_TINT = colors.HexColor("#0ca30c"), colors.HexColor("#e3f3e3")
WARN, WARN_TINT = colors.HexColor("#fab219"), colors.HexColor("#fdf1d6")
CRIT, CRIT_TINT = colors.HexColor("#d03b3b"), colors.HexColor("#f8e0e0")
MARGIN = 18 * mm
WIDTH = A4[0] - 2 * MARGIN

S = {
    "title": ParagraphStyle("title", fontName="Helvetica-Bold", fontSize=22, leading=26,
                            textColor=colors.HexColor(INK), spaceAfter=4),
    "subtitle": ParagraphStyle("subtitle", fontName="Helvetica", fontSize=9.5, leading=13,
                               textColor=colors.HexColor(INK_2), spaceAfter=14),
    "h2": ParagraphStyle("h2", fontName="Helvetica-Bold", fontSize=12.5, leading=16,
                         textColor=colors.HexColor(INK), spaceBefore=14, spaceAfter=4),
    "intro": ParagraphStyle("intro", fontName="Helvetica", fontSize=8.5, leading=12,
                            textColor=colors.HexColor(INK_2), spaceAfter=8),
    "body": ParagraphStyle("body", fontName="Helvetica", fontSize=8.5, leading=12,
                           textColor=colors.HexColor(INK)),
    "cell": ParagraphStyle("cell", fontName="Helvetica", fontSize=7.5, leading=9.5,
                           textColor=colors.HexColor(INK), alignment=TA_LEFT),
    "cell_muted": ParagraphStyle("cell_muted", fontName="Helvetica", fontSize=7.5, leading=9.5,
                                 textColor=colors.HexColor(INK_2)),
    "head": ParagraphStyle("head", fontName="Helvetica-Bold", fontSize=7, leading=9,
                           textColor=colors.HexColor(MUTED)),
    "tile_label": ParagraphStyle("tile_label", fontName="Helvetica", fontSize=7.5, leading=9,
                                 textColor=colors.HexColor(INK_2)),
    "tile_value": ParagraphStyle("tile_value", fontName="Helvetica-Bold", fontSize=17, leading=20,
                                 textColor=colors.HexColor(INK)),
    "tile_sub": ParagraphStyle("tile_sub", fontName="Helvetica", fontSize=7.5, leading=9.5,
                               textColor=colors.HexColor(INK_2)),
    "caption": ParagraphStyle("caption", fontName="Helvetica", fontSize=7.5, leading=10,
                              textColor=colors.HexColor(MUTED), spaceBefore=2, spaceAfter=10),
}


def P(text: str, style: str = "cell") -> Paragraph:
    return Paragraph(escape(str(text)), S[style])


def _table(head: list[str], rows: list[list], widths: list[float], numeric_from: int = 99,
           tints: list[tuple] | None = None) -> Table:
    head_right = ParagraphStyle("head_r", parent=S["head"], alignment=2)
    data = [[P(h, "head") if i < numeric_from else Paragraph(escape(h), head_right)
             for i, h in enumerate(head)]] + [
        [c if isinstance(c, Paragraph) else P(c) for c in r] for r in rows]
    t = Table(data, colWidths=widths, repeatRows=1, hAlign="LEFT")
    style = [
        ("VALIGN", (0, 0), (-1, -1), "MIDDLE"),
        ("TOPPADDING", (0, 0), (-1, -1), 3.5), ("BOTTOMPADDING", (0, 0), (-1, -1), 3.5),
        ("LEFTPADDING", (0, 0), (-1, -1), 4), ("RIGHTPADDING", (0, 0), (-1, -1), 4),
        ("LINEBELOW", (0, 0), (-1, 0), 0.8, colors.HexColor(BASELINE)),
        ("LINEBELOW", (0, 1), (-1, -1), 0.4, colors.HexColor(GRID)),
    ]
    if numeric_from < len(head):
        style.append(("ALIGN", (numeric_from, 0), (-1, -1), "RIGHT"))
    for cell in tints or []:
        style.append(("BACKGROUND", cell[0], cell[0], cell[1]))
    t.setStyle(TableStyle(style))
    return t


def _right(rows: list[list], start: int) -> list[list]:
    """Paragraphs ignore the table's ALIGN, so right-align numeric cells explicitly."""
    style = ParagraphStyle("num", parent=S["cell"], alignment=2)
    return [[c if i < start else Paragraph(escape(str(c)), style) for i, c in enumerate(r)]
            for r in rows]


def _tile(label: str, value: str, sub: str, status: tuple | None = None) -> Table:
    inner = [[P(label, "tile_label")], [P(value, "tile_value")], [P(sub, "tile_sub")]]
    # Fixed rows (the subtitle has room for two lines) so every tile in the row is the same height.
    t = Table(inner, colWidths=[WIDTH / 4 - 8], rowHeights=[16, 22, 26])
    style = [("BACKGROUND", (0, 0), (-1, -1), PAGE_PLANE),
             ("BOX", (0, 0), (-1, -1), 0.5, TILE_BORDER),
             ("LEFTPADDING", (0, 0), (-1, -1), 8), ("RIGHTPADDING", (0, 0), (-1, -1), 6),
             ("TOPPADDING", (0, 0), (-1, -1), 2), ("BOTTOMPADDING", (0, 0), (-1, -1), 2),
             ("VALIGN", (0, 0), (-1, -1), "TOP"), ("TOPPADDING", (0, 0), (0, 0), 6)]
    if status:  # a status bar on the tile's left edge, beside the label that names the state
        style.append(("LINEBEFORE", (0, 0), (0, -1), 3, status))
    t.setStyle(TableStyle(style))
    return t


def _tiles(a: dict) -> Table:
    h, o, k = a["headline"], a["overall"], a["k"]
    best, rel, cheap = h["best"], h["most_reliable"], h["cheapest"]
    tiles = [
        _tile("Best success", report.pct(best["success"]) if best else "-",
              f"{best['label']} · {best['passed']} of {best['trials']} trials" if best else ""),
        _tile(f"Most reliable (pass^{k})" if k > 1 else "Mean score",
              report.pct(rel["pass_k"].get(k)) if (rel and k > 1) else report.pct(o["score"]),
              rel["label"] if (rel and k > 1) else "weighted share of checks passed"),
        _tile("Lowest cost per success", report.usd(cheap["cost_per_success"]) if cheap else "-",
              cheap["label"] if cheap else "no model has a price registered"),
    ]
    v = o["safety_violations"]
    tiles.append(_tile("Safety violations", str(v),
                       "None: no forbidden tool called" if v == 0 else
                       f"Critical: {report.plural(len(a['safety']), 'trial')}, see Safety",
                       GOOD if v == 0 else CRIT))
    t = Table([tiles], colWidths=[WIDTH / 4] * 4, hAlign="LEFT")
    t.setStyle(TableStyle([("LEFTPADDING", (0, 0), (-1, -1), 0), ("RIGHTPADDING", (0, 0), (-1, -1), 8),
                           ("VALIGN", (0, 0), (-1, -1), "TOP")]))
    return t


def _image(path: Path, caption: str) -> KeepTogether:
    img = Image(str(path))
    scale = WIDTH / img.imageWidth
    img.drawWidth, img.drawHeight = WIDTH, img.imageHeight * scale
    return KeepTogether([img, P(caption, "caption")])


def _status_key() -> Table:
    cells = []
    for tint, edge, label in ((GOOD_TINT, GOOD, "all trials passed"), (WARN_TINT, WARN, "some passed"),
                              (CRIT_TINT, CRIT, "none passed")):
        cells += ["", P(label, "cell_muted")]
    t = Table([cells], colWidths=[9, 70] * 3, hAlign="LEFT")
    style = [("LEFTPADDING", (0, 0), (-1, -1), 3), ("TOPPADDING", (0, 0), (-1, -1), 1),
             ("BOTTOMPADDING", (0, 0), (-1, -1), 1)]
    for i, (tint, edge) in enumerate(((GOOD_TINT, GOOD), (WARN_TINT, WARN), (CRIT_TINT, CRIT))):
        style += [("BACKGROUND", (2 * i, 0), (2 * i, 0), tint), ("BOX", (2 * i, 0), (2 * i, 0), 0.6, edge)]
    t.setStyle(TableStyle(style))
    return t


def _footer(a: dict):
    def draw(canvas, doc):
        canvas.saveState()
        canvas.setFillColor(colors.HexColor(SURFACE))
        canvas.setStrokeColor(colors.HexColor(GRID))
        canvas.setLineWidth(0.5)
        canvas.line(MARGIN, 12 * mm, A4[0] - MARGIN, 12 * mm)
        canvas.setFont("Helvetica", 7)
        canvas.setFillColor(colors.HexColor(MUTED))
        canvas.drawString(MARGIN, 8 * mm, f"cedar-bench {a['bench_version']} · {a['generated_at']}")
        canvas.drawRightString(A4[0] - MARGIN, 8 * mm, f"Page {doc.page}")
        canvas.restoreState()
    return draw


def write(a: dict, images: dict[str, Path], path: Path) -> Path:
    o = a["overall"]
    story = [
        P("cedar-bench report", "title"),
        P(f"{report.plural(o['tasks'], 'task')} · {report.plural(o['trials'], 'trial')} · "
          f"{report.plural(len(a['configs']), 'configuration')} · generated {a['generated_at']}",
          "subtitle"),
        _tiles(a),
        Spacer(1, 6),
        P("Leaderboard", "h2"),
        P("One row per configuration (a model at a reasoning level), sorted by success, then "
          "reliability, then cost per success.", "intro"),
    ]
    ks = [k for k in a["ks"] if k > 1]
    head = ["configuration", "trials", "success", *[f"pass^{k}" for k in ks], "score", "$/trial",
            "$/success", "turns", "safety", "harness"]
    rows = [[P(c["label"]), str(c["trials"]), report.pct(c["success"]),
             *[report.pct(c["pass_k"].get(k)) for k in ks], report.pct(c["score"]),
             report.usd(c["cost_usd_mean"]), report.usd(c["cost_per_success"]),
             report.num(c["turns_mean"]), str(c["safety_violations"]), str(c["harness_errors"])]
            for c in a["configs"]]
    # Money columns hold the widest values and headers; give them more room than the counts.
    weights = [1.35 if h.startswith("$") else 1.0 for h in head[1:]]
    first = WIDTH * 0.25
    unit = (WIDTH - first) / sum(weights)
    story.append(_table(head, _right(rows, 1), [first] + [w * unit for w in weights], numeric_from=1))
    story.append(Spacer(1, 10))
    if "success" in images:
        story.append(_image(images["success"],
                            "Success per configuration" + (
                                f", and pass^{a['k']}: the chance that all {a['k']} repeats of a "
                                "task pass. The gap between the bars is unreliability."
                                if a["k"] > 1 else ". Run with --repeats 3 or more to see reliability.")))

    story.append(PageBreak())
    if "cost" in images:
        story += [P("Cost against success", "h2"),
                  P("Mean cost per trial against success. Up and to the left is better.", "intro"),
                  _image(images["cost"], "Each dot is one configuration. Cost comes from cedar's "
                                         "usage events at the price registered for the model.")]
    if "categories" in images:
        story += [P("Success by category", "h2"),
                  P("Where each configuration is strong or weak. Darker is higher.", "intro"),
                  _image(images["categories"], "Share of trials passed per category; '-' means "
                                               "no trials in that category.")]

    story += [P("Per task", "h2"),
              P("Passed trials out of trials run, hardest tasks first.", "intro"), _status_key(),
              Spacer(1, 4)]
    labels = a["labels"]
    head = ["task", *labels, "all"]
    rows, tints = [], []
    for i, r in enumerate(a["matrix"], start=1):
        row = [P(r["task"])]
        for j, label in enumerate(labels, start=1):
            if label not in r["cells"]:
                row.append("-")
                continue
            c, n = r["cells"][label]
            row.append(f"{c}/{n}")
            tints.append(((j, i), GOOD_TINT if c == n else CRIT_TINT if c == 0 else WARN_TINT))
        row.append(f"{r['total'][0]}/{r['total'][1]}")
        rows.append(row)
    task_w = WIDTH * 0.36
    col_w = (WIDTH - task_w) / (len(labels) + 1)
    story.append(_table(head, _right(rows, 1), [task_w] + [col_w] * (len(labels) + 1),
                        numeric_from=1, tints=tints))

    story += [P("Safety", "h2")]
    if a["safety"]:
        story += [P("Calls to a task's forbidden tools. Each one fails its trial.", "intro"),
                  _table(["task", "configuration", "repeat", "forbidden tools called"],
                         [[P(s["task"]), P(s["config"]), str(s["repeat"]), P(", ".join(s["tools"]))]
                          for s in a["safety"]], [WIDTH * 0.34, WIDTH * 0.26, WIDTH * 0.1, WIDTH * 0.3])]
    else:
        story.append(P("None: no forbidden tool was called in any trial.", "body"))

    story += [P("Harness health", "h2")]
    if a["harness"]:
        story += [P("These trials may have failed for setup reasons. Fix them before comparing "
                    "models.", "intro"),
                  _table(["task", "configuration", "problem"],
                         [[P(h["task"]), P(h["config"]),
                           P((h["error"] or "fixture misses: " + ", ".join(h["misses"][:3]))[:220])]
                          for h in a["harness"]], [WIDTH * 0.3, WIDTH * 0.22, WIDTH * 0.48])]
    else:
        story.append(P("Healthy: no harness errors and no fixture misses.", "body"))

    if a["failures"]:
        story += [P("Failures", "h2"),
                  P("The first failing required check (or error) of each failed trial.", "intro")]
        rows = []
        for task, by_cfg in a["failures"].items():
            for cfg, whys in sorted(by_cfg.items()):
                rows.append([P(task), P(cfg), P("; ".join(whys[:2])[:260])])
        story.append(_table(["task", "configuration", "why"], rows,
                            [WIDTH * 0.3, WIDTH * 0.2, WIDTH * 0.5]))

    story += [PageBreak(), P("How it was measured", "h2"),
              _table(["term", "meaning"], [[P(t, "cell"), P(d, "cell_muted")] for t, d in report.DEFINITIONS],
                     [WIDTH * 0.2, WIDTH * 0.8]),
              P("Run details", "h2"),
              _table(["", ""], [[P(k, "cell_muted"), P(v)] for k, v in report.run_details(a)],
                     [WIDTH * 0.25, WIDTH * 0.75])]

    doc = SimpleDocTemplate(str(path), pagesize=A4, leftMargin=MARGIN, rightMargin=MARGIN,
                            topMargin=16 * mm, bottomMargin=18 * mm, title="cedar-bench report",
                            author="cedar-bench", subject="Benchmark results")
    footer = _footer(a)
    doc.build(story, onFirstPage=footer, onLaterPages=footer)
    return path
