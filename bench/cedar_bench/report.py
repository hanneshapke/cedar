"""Summarizes results.jsonl: success rate, pass^k reliability, cost, turns and safety violations,
per model and reasoning level, and broken down by category.

pass^k is the chance that k independent trials of a task all succeed, estimated per task from its
n trials with c successes as C(c, k) / C(n, k), then averaged over tasks. pass^1 is the success
rate; a model that solves a task only sometimes loses more as k grows.
"""

from __future__ import annotations

import json
from collections import defaultdict
from math import comb
from pathlib import Path
from statistics import mean


def load(path: Path) -> list[dict]:
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def pass_hat_k(trials: list[dict], k: int) -> float | None:
    by_task: dict[str, list[bool]] = defaultdict(list)
    for t in trials:
        by_task[t["task"]].append(bool(t["passed"]))
    scores = [comb(sum(v), k) / comb(len(v), k) for v in by_task.values() if len(v) >= k]
    return mean(scores) if scores else None


def summarize(trials: list[dict]) -> dict:
    costs = [t["usage"]["cost_usd"] for t in trials if t["usage"]["cost_usd"] is not None]
    k_max = min((sum(1 for u in trials if u["task"] == t["task"]) for t in trials), default=1)
    return {
        "trials": len(trials),
        "tasks": len({t["task"] for t in trials}),
        "success": mean(bool(t["passed"]) for t in trials) if trials else 0.0,
        "score": mean(t["score"] for t in trials) if trials else 0.0,
        "pass_k": {k: pass_hat_k(trials, k) for k in sorted({1, k_max})},
        "cost_usd_mean": mean(costs) if costs else None,
        "cost_usd_total": sum(costs) if costs else None,
        "turns_mean": mean(t["usage"]["turns"] for t in trials) if trials else 0.0,
        "output_tokens_mean": mean(t["usage"]["output_tokens"] for t in trials) if trials else 0.0,
        "safety_violations": sum(len(t["safety_violations"]) for t in trials),
        "harness_errors": sum(1 for t in trials if (t.get("error") or "").startswith("harness")),
        "fixture_misses": sum(1 for t in trials if t.get("fixture_misses")),
    }


def _fmt(v, kind=""):
    if v is None:
        return "-"
    if kind == "pct":
        return f"{v * 100:.1f}%"
    if kind == "usd":
        return f"${v:.4f}"
    if isinstance(v, float):
        return f"{v:.1f}"
    return str(v)


def markdown(trials: list[dict]) -> str:
    groups: dict[tuple, list[dict]] = defaultdict(list)
    for t in trials:
        groups[(t["model"], t["reasoning"] or "-")].append(t)
    lines = ["# cedar-bench results", ""]
    ks = sorted({k for g in groups.values() for k in summarize(g)["pass_k"]})
    head = ["model", "reasoning", "tasks", "trials", "success", *[f"pass^{k}" for k in ks if k > 1],
            "score", "cost/trial", "turns", "safety", "harness err", "fixture miss"]
    lines += ["| " + " | ".join(head) + " |", "|" + "---|" * len(head)]
    for (model, reasoning), g in sorted(groups.items()):
        s = summarize(g)
        row = [model, reasoning, s["tasks"], s["trials"], _fmt(s["success"], "pct"),
               *[_fmt(s["pass_k"].get(k), "pct") for k in ks if k > 1],
               _fmt(s["score"], "pct"), _fmt(s["cost_usd_mean"], "usd"), _fmt(s["turns_mean"]),
               s["safety_violations"], s["harness_errors"], s["fixture_misses"]]
        lines.append("| " + " | ".join(str(c) for c in row) + " |")

    categories = sorted({t["category"] for t in trials})
    lines += ["", "## Success by category", "",
              "| model | reasoning | " + " | ".join(categories) + " |",
              "|" + "---|" * (len(categories) + 2)]
    for (model, reasoning), g in sorted(groups.items()):
        cells = []
        for c in categories:
            sub = [t for t in g if t["category"] == c]
            cells.append(_fmt(mean(bool(t["passed"]) for t in sub), "pct") if sub else "-")
        lines.append(f"| {model} | {reasoning} | " + " | ".join(cells) + " |")

    failing = defaultdict(list)
    for t in trials:
        if not t["passed"]:
            why = t.get("error") or next((c["detail"] for c in t["checks"]
                                          if c["passed"] is False and c["required"]), "")
            failing[t["task"]].append(f"{t['model']}/{t['reasoning'] or '-'}: {why}")
    if failing:
        lines += ["", "## Failures", ""]
        for task, whys in sorted(failing.items()):
            lines.append(f"- **{task}**")
            lines += [f"  - {w}" for w in sorted(set(whys))[:5]]
    return "\n".join(lines) + "\n"
