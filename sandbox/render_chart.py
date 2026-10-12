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


def fail(message):
    print(message, file=sys.stderr)
    sys.exit(1)


def values_of(series):
    """Values with gaps (null) as NaN, which matplotlib leaves out of lines and bars."""
    return [math.nan if v is None else float(v) for v in series["values"]]


def legend_wanted(spec):
    return len(spec["series"]) > 1 or any("name" in s for s in spec["series"])


def draw(ax, spec):
    kind = spec["kind"]
    series = spec["series"]
    labels = spec.get("labels", [])
    stacked = spec.get("stacked", False)
    names = [s.get("name", f"series {i + 1}") for i, s in enumerate(series)]

    if kind == "pie":
        values = values_of(series[0])
        ax.pie(values, labels=[str(l) for l in labels], autopct="%1.1f%%", startangle=90,
               counterclock=False, wedgeprops={"linewidth": 1, "edgecolor": "white"})
        ax.set_aspect("equal")
        return

    if kind == "histogram":
        data = [[v for v in values_of(s) if not math.isnan(v)] for s in series]
        ax.hist(data, bins=spec.get("bins", "auto"), label=names, stacked=stacked,
                alpha=0.85 if len(data) == 1 or stacked else 0.6, edgecolor="white")
    elif kind == "scatter":
        for name, s in zip(names, series):
            ax.scatter(labels, values_of(s), label=name, alpha=0.8)
    elif kind in ("line", "area"):
        numeric = all(isinstance(l, (int, float)) for l in labels)
        x = labels if numeric else list(range(len(labels)))
        if kind == "area" and stacked:
            ax.stackplot(x, *[values_of(s) for s in series], labels=names, alpha=0.85)
        else:
            for name, s in zip(names, series):
                ys = values_of(s)
                if kind == "area":
                    ax.fill_between(x, ys, alpha=0.3)
                    ax.plot(x, ys, label=name)
                else:
                    ax.plot(x, ys, label=name, marker="o" if len(x) <= 30 else None)
        if not numeric:
            ax.set_xticks(x, [str(l) for l in labels])
    else:  # bar, barh
        n = len(series)
        positions = list(range(len(labels)))
        horizontal = kind == "barh"
        bar = ax.barh if horizontal else ax.bar
        if stacked:
            base = [0.0] * len(labels)
            for name, s in zip(names, series):
                vals = [0.0 if math.isnan(v) else v for v in values_of(s)]
                if horizontal:
                    bar(positions, vals, left=base, label=name)
                else:
                    bar(positions, vals, bottom=base, label=name)
                base = [b + v for b, v in zip(base, vals)]
        else:
            width = 0.8 / n
            for i, (name, s) in enumerate(zip(names, series)):
                offset = (i - (n - 1) / 2) * width
                bar([p + offset for p in positions], values_of(s), width, label=name)
        ticks = [str(l) for l in labels]
        if horizontal:
            ax.set_yticks(positions, ticks)
            ax.invert_yaxis()  # first label on top, as in a table
        else:
            ax.set_xticks(positions, ticks)

    if spec.get("log_scale"):
        (ax.set_xscale if kind == "barh" else ax.set_yscale)("log")
    ax.grid(True, axis="x" if kind == "barh" else "y", alpha=0.3)
    ax.set_axisbelow(True)
    if legend_wanted(spec):
        ax.legend()


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
    except ImportError as e:  # pragma: no cover - depends on the image
        fail(f"chart rendering is not available in this sandbox image ({e})")

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
            ax.set_title(spec["title"], fontsize=14, pad=12)
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
