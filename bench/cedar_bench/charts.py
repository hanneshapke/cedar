"""The report's charts, as PNGs for report.md and report.pdf.

Light theme for print, from the reference data-viz palette: categorical slots in fixed order
(slot 1 blue, slot 2 orange), one sequential blue ramp for magnitude, ink tokens for every piece of
text (never the series color), hairline recessive grids, one axis per chart. Every chart has a
table beside it in the report, so no value depends on reading color alone.
"""

from __future__ import annotations

from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
from matplotlib.colors import LinearSegmentedColormap, to_rgb  # noqa: E402

SURFACE = "#ffffff"  # the page is white; the screen surface #fcfcfb would print as a faint box
INK = "#0b0b0b"
INK_2 = "#52514e"
MUTED = "#898781"
GRID = "#e1e0d9"
BASELINE = "#c3c2b7"
SERIES = ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#6250d6", "#e34948"]
SEQUENTIAL = ["#cde2fb", "#b7d3f6", "#9ec5f4", "#86b6ef", "#6da7ec", "#5598e7", "#3987e5",
              "#2a78d6", "#256abf", "#1c5cab", "#184f95", "#104281", "#0d366b"]
WIDTH_IN = 6.85  # the PDF's text width (A4 less 18 mm margins)
DPI = 220


def _style():
    plt.rcParams.update({
        "font.family": "DejaVu Sans", "font.size": 8.5, "text.color": INK,
        "axes.facecolor": SURFACE, "figure.facecolor": SURFACE, "savefig.facecolor": SURFACE,
        "axes.edgecolor": BASELINE, "axes.labelcolor": INK_2, "axes.linewidth": 0.8,
        "xtick.color": MUTED, "ytick.color": INK_2, "xtick.labelsize": 8, "ytick.labelsize": 8,
        "axes.grid": False, "legend.frameon": False, "legend.fontsize": 8,
    })


def _clean(ax, grid_axis: str):
    for side in ("top", "right"):
        ax.spines[side].set_visible(False)
    ax.spines["left" if grid_axis == "x" else "bottom"].set_visible(False)
    ax.grid(True, axis=grid_axis, color=GRID, linewidth=0.6)
    ax.set_axisbelow(True)
    ax.tick_params(length=0, pad=4)


def _save(fig, path: Path) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(path, dpi=DPI, bbox_inches="tight", pad_inches=0.08)
    plt.close(fig)
    return path


def success_chart(a: dict, path: Path) -> Path:
    """Success (pass^1) and, with repeats, pass^k per configuration: horizontal bars, best on top."""
    configs = a["configs"]
    k = a["k"]
    two = k > 1
    n = len(configs)
    # A fixed height per configuration and a fixed axis range keep every bar the same thin
    # thickness (about 0.15 in) however many configurations there are.
    band = 0.62 if two else 0.38
    fig, ax = plt.subplots(figsize=(WIDTH_IN, 0.55 + band * n))
    ys = list(range(n))[::-1]
    h = 0.32 if two else 0.5
    series = [("Success (pass^1)", [c["success"] for c in configs])]
    if two:
        series.append((f"pass^{k} (all {k} repeats pass)", [c["pass_k"].get(k) or 0 for c in configs]))
    for i, (name, values) in enumerate(series):
        offset = (h / 2 + 0.02) * (1 if i == 0 else -1) if two else 0
        pos = [y + offset for y in ys]
        ax.barh(pos, [v * 100 for v in values], height=h, color=SERIES[i], label=name,
                edgecolor=SURFACE, linewidth=1)
        for y, v in zip(pos, values):
            ax.text(v * 100 + 1.2, y, f"{v * 100:.0f}%", va="center", ha="left", fontsize=7.5,
                    color=INK_2)
    ax.set_yticks(ys, [c["label"] for c in configs])
    ax.set_ylim(-0.6, n - 0.4)
    ax.set_xlim(0, 108)
    ax.set_xticks([0, 25, 50, 75, 100], ["0%", "25%", "50%", "75%", "100%"])
    _clean(ax, "x")
    if two:
        ax.legend(loc="lower center", bbox_to_anchor=(0.45, 1.0), ncol=2, handlelength=1.2,
                  borderaxespad=0.3)
    return _save(fig, path)


def cost_chart(a: dict, path: Path) -> Path | None:
    """Cost per trial against success, one dot per configuration, each labeled directly."""
    pts = [c for c in a["configs"] if c["cost_usd_mean"] is not None]
    if not pts:
        return None
    fig, ax = plt.subplots(figsize=(WIDTH_IN, 3.0))
    xs = [c["cost_usd_mean"] for c in pts]
    ys = [c["success"] * 100 for c in pts]
    ax.scatter(xs, ys, s=64, color=SERIES[0], edgecolors=SURFACE, linewidths=2, zorder=3)
    for c, x, y in zip(pts, xs, ys):
        ax.annotate(c["label"], (x, y), xytext=(7, 4), textcoords="offset points", fontsize=7.5,
                    color=INK_2)
    hi = max(xs) or 1
    ax.set_xlim(0, hi * 1.35)
    ax.set_ylim(0, 105)
    # No "0%" label: it would collide with the x axis's first label at the origin.
    ax.set_yticks([25, 50, 75, 100], ["25%", "50%", "75%", "100%"])
    ax.xaxis.set_major_formatter(lambda v, _: f"${v:.3f}" if hi < 1 else f"${v:,.2f}")
    ax.set_xlabel("Mean cost per trial (USD)")
    ax.set_ylabel("Success")
    _clean(ax, "y")
    ax.spines["left"].set_visible(True)
    return _save(fig, path)


def _ink_on(fill: str) -> str:
    r, g, b = to_rgb(fill)
    return "#ffffff" if (0.2126 * r + 0.7152 * g + 0.0722 * b) < 0.45 else INK


def category_heatmap(a: dict, path: Path) -> Path | None:
    """Success per configuration and category on one sequential ramp, values in the cells."""
    cats, labels = a["categories"], a["labels"]
    if not cats or not labels:
        return None
    cmap = LinearSegmentedColormap.from_list("seq", SEQUENTIAL)
    fig, ax = plt.subplots(figsize=(WIDTH_IN, 0.6 + 0.36 * len(labels)))
    for i, label in enumerate(labels):
        for j, cat in enumerate(cats):
            v = a["by_category"][cat].get(label)
            fill = "#f0efec" if v is None else matplotlib.colors.to_hex(cmap(v))
            ax.add_patch(plt.Rectangle((j, i), 1, 1, facecolor=fill, edgecolor=SURFACE, linewidth=2))
            ax.text(j + 0.5, i + 0.5, "-" if v is None else f"{v * 100:.0f}%", ha="center",
                    va="center", fontsize=7.5, color=INK_2 if v is None else _ink_on(fill))
    ax.set_xlim(0, len(cats))
    ax.set_ylim(len(labels), 0)
    ax.set_xticks([j + 0.5 for j in range(len(cats))], [c.replace("_", " ") for c in cats])
    ax.set_yticks([i + 0.5 for i in range(len(labels))], labels)
    ax.xaxis.tick_top()
    for side in ax.spines.values():
        side.set_visible(False)
    ax.tick_params(length=0, pad=4)
    ax.tick_params(axis="x", colors=INK_2)
    return _save(fig, path)


def render_all(a: dict, out_dir: Path) -> dict[str, Path]:
    _style()
    out = {"success": success_chart(a, out_dir / "success.png")}
    for key, fn in (("cost", cost_chart), ("categories", category_heatmap)):
        p = fn(a, out_dir / f"{key}.png")
        if p:
            out[key] = p
    return out
