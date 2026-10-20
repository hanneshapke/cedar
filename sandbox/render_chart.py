#!/usr/bin/env python3
"""Draw a chart from a JSON spec. cedar's `generate_chart` tool runs this inside the sandbox; it is
installed in the image as `cedar-render-chart`.

    cedar-render-chart SPEC.json

The spec (checked by cedar before it gets here) looks like:

    {"path": "out/sales.png", "kind": "bar", "labels": ["Q1", "Q2"],
     "series": [{"name": "2025", "values": [1, 2]}], "title": "...", "x_label": "...",
     "y_label": "...", "stacked": false, "bins": 20, "log_scale": false}

`kind` is one of line, bar, barh, area, scatter, pie, histogram. The output format follows the
extension of `path` (.png, .svg, or .pdf). On success one JSON line, {"bytes": N}, is printed to
stdout; a failure prints its reason to stderr and exits 1.
"""

import json
import math
import os
import sys

WIDTH_IN, HEIGHT_IN, DPI = 10, 6, 150

# The same look as the chat's chart card (chat/static/chart-view.css): a fixed categorical order,
# neutral grey past it, recessive grid and axes, and text in greys rather than series colors.
PALETTE = ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#6250d6", "#e34948"]
OTHER = "#a3a19a"
INK = "#1f1e1b"
MUTED = "#6b685f"
GRID = "#e6e4dd"
STYLE = {
    "font.family": ["DejaVu Sans"],
    "font.size": 10.5,
    "text.color": INK,
    "axes.edgecolor": "#cfccc3",
    "axes.labelcolor": MUTED,
    "axes.labelsize": 10.5,
    "axes.labelweight": "normal",
    "axes.titlesize": 15,
    "axes.titleweight": "bold",
    "axes.titlelocation": "left",
    "axes.titlepad": 16,
    "axes.titlecolor": INK,
    "axes.spines.top": False,
    "axes.spines.right": False,
    "axes.spines.left": False,
    "axes.grid": False,
    "axes.prop_cycle": None,  # set in main(), once cycler is importable
    "grid.color": GRID,
    "grid.linewidth": 0.8,
    "xtick.color": MUTED,
    "ytick.color": MUTED,
    "xtick.labelsize": 9.5,
    "ytick.labelsize": 9.5,
    "xtick.major.size": 0,
    "ytick.major.size": 0,
    "xtick.major.pad": 6,
    "ytick.major.pad": 6,
    "legend.frameon": False,
    "legend.fontsize": 10,
    "lines.linewidth": 2.2,
    "lines.solid_capstyle": "round",
    "lines.solid_joinstyle": "round",
    "patch.linewidth": 0,
    "figure.facecolor": "white",
    "savefig.facecolor": "white",
}


def fail(message):
    print(message, file=sys.stderr)
    sys.exit(1)


def values_of(series):
    """Values with gaps (null) as NaN, which matplotlib leaves out of lines and bars."""
    return [math.nan if v is None else float(v) for v in series["values"]]


def legend_wanted(spec):
    return len(spec["series"]) > 1 or any("name" in s for s in spec["series"])


def color(i):
    return PALETTE[i] if i < len(PALETTE) else OTHER


def pie_slices(labels, values, limit=len(PALETTE)):
    """The slices with a value, the smallest folded into "Other" past `limit`, and their colors."""
    slices = [(str(l), v) for l, v in zip(labels, values) if v > 0]
    colors = [color(i) for i in range(len(slices))]
    if len(slices) > limit:
        kept = set(sorted(range(len(slices)), key=lambda i: -slices[i][1])[: limit - 1])
        rest = [s for i, s in enumerate(slices) if i not in kept]
        slices = [s for i, s in enumerate(slices) if i in kept]
        colors = [color(i) for i in range(len(slices))] + [OTHER]
        slices.append((f"Other ({len(rest)})", sum(v for _, v in rest)))
    return slices, colors


def draw(ax, spec):
    kind = spec["kind"]
    series = spec["series"]
    labels = spec.get("labels", [])
    stacked = spec.get("stacked", False)
    names = [s.get("name", f"series {i + 1}") for i, s in enumerate(series)]

    if kind == "pie":
        # A donut, with its slices named in a legend beside it, each with its share.
        slices, colors = pie_slices(labels, values_of(series[0]))
        total = sum(v for _, v in slices)
        wedges, _ = ax.pie([v for _, v in slices], colors=colors, startangle=90, counterclock=False,
                           wedgeprops={"width": 0.38, "linewidth": 2, "edgecolor": "white"})
        ax.text(0, 0.06, f"{total:,.4g}", ha="center", va="center", fontsize=20, fontweight="bold", color=INK)
        ax.text(0, -0.14, "Total", ha="center", va="center", fontsize=10, color=MUTED)
        ax.legend(wedges, [f"{name}  {v / total:.1%}" for name, v in slices], loc="center left",
                  bbox_to_anchor=(1.0, 0.5), handlelength=0.9, handleheight=0.9, labelspacing=0.8)
        ax.set_aspect("equal")
        return

    if kind == "histogram":
        data = [[v for v in values_of(s) if not math.isnan(v)] for s in series]
        ax.hist(data, bins=spec.get("bins", "auto"), label=names, stacked=stacked,
                color=[color(i) for i in range(len(data))], edgecolor="white", linewidth=1.2)
    elif kind == "scatter":
        for i, (name, s) in enumerate(zip(names, series)):
            ax.scatter(labels, values_of(s), label=name, color=color(i), alpha=0.85, s=36,
                       edgecolors="white", linewidths=0.8)
    elif kind in ("line", "area"):
        numeric = all(isinstance(l, (int, float)) for l in labels)
        x = labels if numeric else list(range(len(labels)))
        if kind == "area" and stacked:
            ax.stackplot(x, *[values_of(s) for s in series], labels=names, alpha=0.8,
                         colors=[color(i) for i in range(len(series))], edgecolor="white", linewidth=1)
        else:
            for i, (name, s) in enumerate(zip(names, series)):
                ys = values_of(s)
                if kind == "area":
                    ax.fill_between(x, ys, color=color(i), alpha=0.16, linewidth=0)
                    ax.plot(x, ys, label=name, color=color(i))
                else:
                    ax.plot(x, ys, label=name, color=color(i), marker="o" if len(x) <= 30 else None,
                            markersize=4.5, markeredgecolor="white", markeredgewidth=1)
        if not numeric:
            ax.set_xticks(x, [str(l) for l in labels])
    else:  # bar, barh
        n = len(series)
        positions = list(range(len(labels)))
        horizontal = kind == "barh"
        bar = ax.barh if horizontal else ax.bar
        if stacked:
            base = [0.0] * len(labels)
            for i, (name, s) in enumerate(zip(names, series)):
                vals = [0.0 if math.isnan(v) else v for v in values_of(s)]
                style = {"label": name, "color": color(i), "edgecolor": "white", "linewidth": 1}
                if horizontal:
                    bar(positions, vals, 0.62, left=base, **style)
                else:
                    bar(positions, vals, 0.62, bottom=base, **style)
                base = [b + v for b, v in zip(base, vals)]
        else:
            group = 0.78 if n > 1 else 0.62
            width = group / n
            for i, (name, s) in enumerate(zip(names, series)):
                offset = (i - (n - 1) / 2) * width
                # A thin white edge leaves a gap between neighbouring bars of a group.
                bar([p + offset for p in positions], values_of(s), width, label=name, color=color(i),
                    edgecolor="white", linewidth=1.5 if n > 1 else 0)
        ticks = [str(l) for l in labels]
        if horizontal:
            ax.set_yticks(positions, ticks)
            ax.invert_yaxis()  # first label on top, as in a table
        else:
            ax.set_xticks(positions, ticks)

    if spec.get("log_scale"):
        (ax.set_xscale if kind == "barh" else ax.set_yscale)("log")
    ax.grid(True, axis="x" if kind == "barh" else "y")
    ax.set_axisbelow(True)
    if kind == "barh":
        # The bars stand on the left axis; the value axis is the grid.
        ax.spines["left"].set_visible(True)
        ax.spines["bottom"].set_visible(False)
    if legend_wanted(spec):
        # Above the plot, in a row, rather than over the data.
        ax.legend(loc="lower left", bbox_to_anchor=(0, 1.0), ncol=min(len(series), 6),
                  handlelength=1.2, handleheight=0.9, columnspacing=1.4, borderaxespad=0.4)


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "--":
        spec_path = sys.argv[2]
    elif len(sys.argv) == 2:
        spec_path = sys.argv[1]
    else:
        fail("usage: cedar-render-chart SPEC.json")

    try:
        with open(spec_path, encoding="utf-8") as f:
            spec = json.load(f)
    except (OSError, ValueError) as e:
        fail(f"cannot read the chart spec: {e}")

    try:
        import matplotlib
        matplotlib.use("Agg")
        import matplotlib.pyplot as plt
        from cycler import cycler
    except ImportError as e:  # pragma: no cover - depends on the image
        fail(f"chart rendering is not available in this sandbox image ({e})")
    plt.rcParams.update({**STYLE, "axes.prop_cycle": cycler(color=PALETTE)})

    out = spec["path"]
    out_dir = os.path.dirname(out)
    if out_dir:
        try:
            os.makedirs(out_dir, exist_ok=True)
        except OSError as e:
            fail(f"cannot create {out_dir}: {e}")

    try:
        fig, ax = plt.subplots(figsize=(WIDTH_IN, HEIGHT_IN), dpi=DPI)
        draw(ax, spec)
        if spec.get("title"):
            # Clear of a legend row above the plot.
            ax.set_title(spec["title"], pad=34 if legend_wanted(spec) and spec["kind"] != "pie" else 16)
        if spec.get("x_label"):
            ax.set_xlabel(spec["x_label"])
        if spec.get("y_label"):
            ax.set_ylabel(spec["y_label"])
        # Long category names: tilt them rather than let them overlap.
        if spec["kind"] in ("bar", "line", "area") and any(
            len(str(l)) > 6 for l in spec.get("labels", [])
        ) and len(spec.get("labels", [])) > 4:
            plt.setp(ax.get_xticklabels(), rotation=30, ha="right")
        fig.tight_layout()
        fig.savefig(out)
    except OSError as e:
        fail(f"cannot write {out}: {e}")
    except Exception as e:  # noqa: BLE001 - whatever matplotlib raises is the reason to report
        fail(f"drawing failed: {e}")

    print(json.dumps({"bytes": os.path.getsize(out)}))


if __name__ == "__main__":
    main()
