"""Summarizes results.jsonl into report.md and report.pdf (with the charts in charts/).

Per configuration (a model at a reasoning level): success, pass^k reliability, score, cost per
trial and per success, turns, tokens, safety violations and harness health; then success by
category and per task, the failures, and how everything was measured.

pass^k is the chance that k independent trials of a task all succeed, estimated per task from its
n trials with c successes as C(c, k) / C(n, k), then averaged over tasks. pass^1 is the success
rate; a model that solves a task only sometimes loses more as k grows.

`analyze` computes everything once; `markdown` and `report_pdf.write` only lay it out, so the two
reports always agree.
"""

from __future__ import annotations

import datetime as dt
import json
from collections import defaultdict
from math import comb
from pathlib import Path
from statistics import mean

from . import __version__

DEFINITIONS = [
    ("Trial", "One task carried out once by one configuration: its run plus the continuations the "
              "scripted user started by answering ask_user."),
    ("Success (pass^1)", "Share of trials that passed: the last run completed, no forbidden tool "
                         "was called, and every required check passed."),
    ("pass^k", "Chance that k independent trials of a task all pass, estimated per task as "
               "C(c,k)/C(n,k) from n trials with c passes, averaged over tasks. It rewards "
               "reliability: a task solved only sometimes loses more as k grows."),
    ("Score", "Weighted share of a trial's checks that passed, optional checks included; partial "
              "credit even when a trial fails."),
    ("Cost", "From cedar's usage events (planning and in-tool model calls included) at the price "
             "registered for the model. Cost per success is total cost over passed trials."),
    ("Safety violation", "A call to one of the task's forbidden tools. It fails the trial."),
    ("Harness error", "The trial could not be judged fairly: cedar unreachable, a timeout, or "
                      "calendar answers in another time zone. Fix the setup, not the model."),
    ("Fixture miss", "The agent requested something the fixture server has no answer for. A "
                     "task with misses may be failing for harness reasons."),
]


def load(path: Path) -> list[dict]:
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def load_meta(results_path: Path) -> dict:
    meta = results_path.parent / "meta.json"
    return json.loads(meta.read_text()) if meta.exists() else {}


def pass_hat_k(trials: list[dict], k: int) -> float | None:
    by_task: dict[str, list[bool]] = defaultdict(list)
    for t in trials:
        by_task[t["task"]].append(bool(t["passed"]))
    scores = [comb(sum(v), k) / comb(len(v), k) for v in by_task.values() if len(v) >= k]
    return mean(scores) if scores else None


def _harness(t: dict) -> bool:
    return (t.get("error") or "").startswith("harness")


def summarize(trials: list[dict]) -> dict:
    costs = [t["usage"]["cost_usd"] for t in trials if t["usage"]["cost_usd"] is not None]
    k_max = min((sum(1 for u in trials if u["task"] == t["task"]) for t in trials), default=1)
    passed = sum(1 for t in trials if t["passed"])
    return {
        "trials": len(trials),
        "passed": passed,
        "tasks": len({t["task"] for t in trials}),
        "success": passed / len(trials) if trials else 0.0,
        "score": float(mean(t["score"] for t in trials)) if trials else 0.0,
        "k": k_max,
        "pass_k": {k: pass_hat_k(trials, k) for k in sorted({1, k_max})},
        "cost_usd_mean": mean(costs) if costs else None,
        "cost_usd_total": sum(costs) if costs else None,
        "cost_per_success": (sum(costs) / passed) if costs and passed else None,
        "turns_mean": mean(t["usage"]["turns"] for t in trials) if trials else 0.0,
        "output_tokens_mean": mean(t["usage"]["output_tokens"] for t in trials) if trials else 0.0,
        "duration_mean": mean(t.get("duration_secs", 0) for t in trials) if trials else 0.0,
        "safety_violations": sum(len(t["safety_violations"]) for t in trials),
        "harness_errors": sum(1 for t in trials if _harness(t)),
        "fixture_misses": sum(1 for t in trials if t.get("fixture_misses")),
    }


def config_label(model: str, reasoning: str | None) -> str:
    return f"{model} · {reasoning}" if reasoning else model


def _why(t: dict) -> str:
    if t.get("error"):
        return t["error"]
    if t["safety_violations"]:
        return "called forbidden tool(s): " + ", ".join(sorted(set(t["safety_violations"])))
    failed = [c for c in t["checks"] if c["passed"] is False and c["required"]]
    if failed:
        c = failed[0]
        return f"{c.get('label') or c['kind']}: {c['detail']}"
    if t.get("status") != "completed":
        return f"run ended {t.get('status')}"
    return "failed"


def analyze(trials: list[dict], meta: dict | None = None) -> dict:
    """Everything the reports show, computed once."""
    meta = meta or {}
    groups: dict[tuple, list[dict]] = defaultdict(list)
    for t in trials:
        groups[(t["model"], t["reasoning"])].append(t)

    configs = []
    for (model, reasoning), g in groups.items():
        s = summarize(g)
        configs.append({"model": model, "reasoning": reasoning or "-",
                        "label": config_label(model, reasoning), **s})
    # Best first: success, then reliability, then cheaper.
    configs.sort(key=lambda c: (-c["success"], -(c["pass_k"].get(c["k"]) or 0),
                                c["cost_per_success"] if c["cost_per_success"] is not None else 1e9,
                                c["label"]))
    labels = [c["label"] for c in configs]
    ks = sorted({c["k"] for c in configs})

    categories = sorted({t["category"] for t in trials})
    by_category = {}
    for c in categories:
        row = {}
        for (model, reasoning), g in groups.items():
            sub = [t for t in g if t["category"] == c]
            # float(): mean() over bools returns a bool, which a colormap would read as an index.
            row[config_label(model, reasoning)] = (float(mean(bool(t["passed"]) for t in sub))
                                                   if sub else None)
        by_category[c] = row

    tasks = sorted({(t["category"], t["task"]) for t in trials})
    matrix = []
    for category, task in tasks:
        cells = {}
        for (model, reasoning), g in groups.items():
            sub = [t for t in g if t["task"] == task]
            if sub:
                cells[config_label(model, reasoning)] = (sum(1 for t in sub if t["passed"]), len(sub))
        total = sum(c for c, _ in cells.values()), sum(n for _, n in cells.values())
        matrix.append({"task": task, "category": category, "cells": cells, "total": total})
    # Hardest tasks first.
    matrix.sort(key=lambda r: (r["total"][0] / r["total"][1] if r["total"][1] else 1, r["task"]))

    failures: dict[str, dict[str, set]] = defaultdict(lambda: defaultdict(set))
    for t in trials:
        if not t["passed"]:
            failures[t["task"]][config_label(t["model"], t["reasoning"])].add(_why(t))

    safety = [{"task": t["task"], "config": config_label(t["model"], t["reasoning"]),
               "repeat": t["repeat"], "tools": sorted(set(t["safety_violations"]))}
              for t in trials if t["safety_violations"]]
    harness = [{"task": t["task"], "config": config_label(t["model"], t["reasoning"]),
                "repeat": t["repeat"], "error": t.get("error") or "",
                "misses": t.get("fixture_misses") or []}
               for t in trials if _harness(t) or t.get("fixture_misses")]

    overall = summarize(trials)
    priced = [c for c in configs if c["cost_per_success"] is not None]
    k_top = max(ks) if ks else 1
    return {
        "generated_at": dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%d %H:%M UTC"),
        "meta": meta, "bench_version": meta.get("bench_version", __version__),
        "overall": overall, "configs": configs, "labels": labels, "ks": ks, "k": k_top,
        "categories": categories, "by_category": by_category, "matrix": matrix,
        "failures": {task: {cfg: sorted(why) for cfg, why in v.items()}
                     for task, v in sorted(failures.items())},
        "safety": safety, "harness": harness,
        "headline": {
            "best": configs[0] if configs else None,
            "most_reliable": max(configs, key=lambda c: c["pass_k"].get(c["k"]) or 0) if configs else None,
            "cheapest": min(priced, key=lambda c: c["cost_per_success"]) if priced else None,
        },
    }


# -- formatting -------------------------------------------------------------------------------

def plural(n: int, word: str) -> str:
    return f"{n} {word}" if n == 1 else f"{n} {word}s"


def pct(v: float | None) -> str:
    return "-" if v is None else f"{v * 100:.1f}%"


def usd(v: float | None) -> str:
    if v is None:
        return "-"
    return f"${v:.4f}" if v < 1 else f"${v:,.2f}"


def num(v: float | None, digits: int = 1) -> str:
    if v is None:
        return "-"
    return f"{v:,.{digits}f}" if isinstance(v, float) else f"{v:,}"


def leaderboard_rows(a: dict) -> tuple[list[str], list[list[str]]]:
    ks = [k for k in a["ks"] if k > 1]
    head = ["model", "reasoning", "tasks", "trials", "success", *[f"pass^{k}" for k in ks],
            "score", "cost/trial", "cost/success", "turns", "out tokens", "safety", "harness err",
            "fixture miss"]
    rows = []
    for c in a["configs"]:
        rows.append([c["model"], c["reasoning"], str(c["tasks"]), str(c["trials"]), pct(c["success"]),
                     *[pct(c["pass_k"].get(k)) for k in ks], pct(c["score"]),
                     usd(c["cost_usd_mean"]), usd(c["cost_per_success"]), num(c["turns_mean"]),
                     num(c["output_tokens_mean"], 0), str(c["safety_violations"]),
                     str(c["harness_errors"]), str(c["fixture_misses"])])
    return head, rows


def headline_lines(a: dict) -> list[str]:
    h, o = a["headline"], a["overall"]
    lines = []
    if h["best"]:
        b = h["best"]
        lines.append(f"Best success: {b['label']}, {pct(b['success'])} ({b['passed']} of "
                     f"{b['trials']} trials).")
    if h["most_reliable"] and a["k"] > 1:
        r = h["most_reliable"]
        lines.append(f"Most reliable: {r['label']}, pass^{r['k']} {pct(r['pass_k'].get(r['k']))}.")
    if h["cheapest"]:
        c = h["cheapest"]
        lines.append(f"Lowest cost per success: {c['label']}, {usd(c['cost_per_success'])}.")
    lines.append(f"Safety violations: {o['safety_violations']}. Harness errors: "
                 f"{o['harness_errors']}. Trials with fixture misses: {o['fixture_misses']}.")
    return lines


def run_details(a: dict) -> list[tuple[str, str]]:
    m, o = a["meta"], a["overall"]
    rows = [("Report generated", a["generated_at"]),
            ("Run started", (m.get("started_at") or "-")[:19].replace("T", " ") + (" UTC" if m.get("started_at") else "")),
            ("cedar-bench", a["bench_version"] + (f" (commit {m['commit'][:10]})" if m.get("commit") else "")),
            ("Tasks / trials", f"{plural(o['tasks'], 'task')}, {plural(o['trials'], 'trial')}, "
                               f"{plural(len(a['configs']), 'configuration')}"),
            ("Repeats", str(m.get("repeats", a["k"])))]
    for url, zone in (m.get("cedar_urls") or {}).items():
        rows.append(("cedar server", f"{url}" + (f" ({zone})" if zone else "")))
    for model in m.get("models") or []:
        price = model.get("price")
        p = (f"${price['input_usd_per_m']}/M in, ${price['output_usd_per_m']}/M out"
             if price else "no price registered")
        rows.append((f"Model {model['id']}", f"{model.get('model') or '-'}; {p}"))
    return rows


# -- markdown ---------------------------------------------------------------------------------

def _table(head: list[str], rows: list[list[str]], numeric_from: int = 99) -> list[str]:
    align = "|" + "".join("---:|" if i >= numeric_from else "---|" for i in range(len(head)))
    return (["| " + " | ".join(head) + " |", align]
            + ["| " + " | ".join(str(c) for c in r) + " |" for r in rows])


def markdown(trials: list[dict], meta: dict | None = None, charts: dict[str, str] | None = None) -> str:
    a = analyze(trials, meta)
    charts = charts or {}
    o = a["overall"]
    lines = ["# cedar-bench report", "",
             f"{plural(o['tasks'], 'task')} · {plural(o['trials'], 'trial')} · "
             f"{plural(len(a['configs']), 'configuration')} · "
             f"cedar-bench {a['bench_version']} · {a['generated_at']}", "",
             "## Headline", ""]
    lines += [f"- {line}" for line in headline_lines(a)]

    lines += ["", "## Leaderboard", "", "Sorted by success, then reliability, then cost per success.", ""]
    lines += _table(*leaderboard_rows(a), numeric_from=2)
    if "success" in charts:
        lines += ["", f"![Success and reliability per configuration]({charts['success']})"]

    if "cost" in charts:
        lines += ["", "## Cost against success", "",
                  "Mean cost per trial against success; up and to the left is better.", "",
                  f"![Cost against success]({charts['cost']})"]

    lines += ["", "## Success by category", ""]
    if "categories" in charts:
        lines += [f"![Success by category]({charts['categories']})", ""]
    lines += _table(["category", *a["labels"]],
                    [[c, *[pct(a["by_category"][c].get(l)) for l in a["labels"]]] for c in a["categories"]],
                    numeric_from=1)

    lines += ["", "## Per task", "", "Passed trials out of trials run, hardest tasks first.", ""]
    lines += _table(["task", *a["labels"], "all"],
                    [[r["task"], *[f"{r['cells'][l][0]}/{r['cells'][l][1]}" if l in r["cells"] else "-"
                                   for l in a["labels"]], f"{r['total'][0]}/{r['total'][1]}"]
                     for r in a["matrix"]], numeric_from=1)

    lines += ["", "## Safety", ""]
    if a["safety"]:
        lines += _table(["task", "configuration", "repeat", "forbidden tools called"],
                        [[s["task"], s["config"], str(s["repeat"]), ", ".join(s["tools"])] for s in a["safety"]])
    else:
        lines.append("No forbidden tool was called.")

    lines += ["", "## Harness health", ""]
    if a["harness"]:
        lines += ["These trials may have failed for setup reasons; fix them before comparing models.", ""]
        lines += _table(["task", "configuration", "repeat", "problem"],
                        [[h["task"], h["config"], str(h["repeat"]),
                          (h["error"] or "fixture misses: " + ", ".join(h["misses"][:3]))[:160]]
                         for h in a["harness"]])
    else:
        lines.append("No harness errors and no fixture misses.")

    if a["failures"]:
        lines += ["", "## Failures", ""]
        for task, by_cfg in a["failures"].items():
            lines.append(f"- **{task}**")
            for cfg, whys in sorted(by_cfg.items()):
                lines.append(f"  - {cfg}: {'; '.join(whys[:3])[:300]}")

    lines += ["", "## How it was measured", ""]
    lines += [f"- **{term}**: {text}" for term, text in DEFINITIONS]
    lines += ["", "## Run details", ""]
    lines += _table(["", ""], [[k, v] for k, v in run_details(a)])
    return "\n".join(lines) + "\n"


def write_reports(results_path: Path, out_dir: Path | None = None) -> dict[str, Path]:
    """report.md, report.pdf and charts/*.png next to results.jsonl (or in out_dir)."""
    from . import charts as charts_mod
    from . import report_pdf

    out_dir = out_dir or results_path.parent
    out_dir.mkdir(parents=True, exist_ok=True)
    trials = load(results_path)
    meta = load_meta(results_path)
    a = analyze(trials, meta)
    images = charts_mod.render_all(a, out_dir / "charts")
    md = out_dir / "report.md"
    md.write_text(markdown(trials, meta, {k: str(p.relative_to(out_dir)) for k, p in images.items()}))
    pdf = out_dir / "report.pdf"
    report_pdf.write(a, images, pdf)
    return {"markdown": md, "pdf": pdf, **{f"chart_{k}": p for k, p in images.items()}}
