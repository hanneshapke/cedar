"""Checks: how a trial is scored, from what cedar and the fixture server recorded.

A trial is one task carried out once by one model at one reasoning level: the run, and the
continuations the scripted user started by answering `ask_user`. Checks read

- outcome: the final run's workspace files (`GET /v1/runs/{id}/files`) and the fixture server's
  state (drafts created, pages requested);
- process: the `tool.call` events of every run in the trial;
- answer: the final run's `final_text`.

Every check takes `weight` (default 1) and `required` (default true). A trial passes when its last
run completed, no forbidden tool was called, and every required check passed; its score is the
weighted share of checks passed. A check that can't be graded here (an `llm_judge` without a judge
configured) is skipped: it counts toward neither.

Patterns are Python regular expressions searched anywhere in the text; add `(?i)` to ignore case.
"""

from __future__ import annotations

import fnmatch
import io
import json
import os
import re
import urllib.request
from dataclasses import dataclass, field
from typing import Any, Callable


@dataclass
class Trial:
    """What the runner collected; everything a check may read."""

    prompt: str
    runs: list[dict]  # the run objects, first to last
    events: list[dict]  # every event of every run, in order, each with "run_id"
    files: list[dict]  # the last run's workspace listing
    fetch_file: Callable[[str], bytes]
    fixture_state: dict
    error: str | None = None

    @property
    def final(self) -> dict:
        return self.runs[-1] if self.runs else {}

    @property
    def final_text(self) -> str:
        return self.final.get("final_text") or ""

    def tool_calls(self) -> list[dict]:
        return [e["data"] for e in self.events if e["type"] == "tool.call"]

    def call_index(self, pattern: str) -> int | None:
        for i, c in enumerate(self.tool_calls()):
            if fnmatch.fnmatchcase(c["name"], pattern):
                return i
        return None


@dataclass
class Result:
    kind: str
    passed: bool | None  # None: skipped
    detail: str
    weight: float = 1.0
    required: bool = True
    label: str = ""

    def as_dict(self) -> dict:
        return self.__dict__.copy()


@dataclass
class Check:
    grade: Callable[[dict, Trial], tuple[bool | None, str]]
    required_keys: tuple[str, ...] = ()
    any_of: tuple[str, ...] = ()
    regex_keys: tuple[str, ...] = ()
    allowed: set[str] = field(default_factory=set)

    def validate(self, spec: dict) -> list[str]:
        problems = [f"needs {k}" for k in self.required_keys if k not in spec]
        if self.any_of and not any(k in spec for k in self.any_of):
            problems.append(f"needs one of {', '.join(self.any_of)}")
        known = {"kind", "weight", "required", "label", *self.required_keys, *self.any_of,
                 *self.regex_keys, *self.allowed}
        problems += [f"unknown key {k}" for k in spec if k not in known]
        for k in self.regex_keys:
            values = spec.get(k)
            for v in values if isinstance(values, list) else [values] if values else []:
                try:
                    re.compile(v)
                except (re.error, TypeError) as e:
                    problems.append(f"{k}: bad pattern {v!r}: {e}")
        return problems


def search(pattern: str, text: str) -> bool:
    return re.search(pattern, text or "") is not None


def _final_text(spec: dict, t: Trial):
    text = t.final_text
    if "regex" in spec and not search(spec["regex"], text):
        return False, f"final text does not match {spec['regex']!r}"
    for p in spec.get("all", []):
        if not search(p, text):
            return False, f"final text lacks {p!r}"
    if "any" in spec and not any(search(p, text) for p in spec["any"]):
        return False, f"final text matches none of {spec['any']!r}"
    for p in spec.get("none", []):
        if search(p, text):
            return False, f"final text contains {p!r}"
    return True, "final text matches"


def _input_matches(call: dict, wanted: dict) -> bool:
    for key, pattern in wanted.items():
        value = call.get("input", {}).get(key)
        text = value if isinstance(value, str) else json.dumps(value)
        if value is None or not search(pattern, text):
            return False
    return True


def _tool_called(spec: dict, t: Trial):
    calls = [c for c in t.tool_calls() if fnmatch.fnmatchcase(c["name"], spec["name"])
             and _input_matches(c, spec.get("input", {}))]
    lo, hi = spec.get("min", 1), spec.get("max")
    n = len(calls)
    ok = n >= lo and (hi is None or n <= hi)
    want = f"{lo}..{hi if hi is not None else '∞'}"
    return ok, f"{n} matching call(s) to {spec['name']} (want {want})"


def _tool_not_called(spec: dict, t: Trial):
    names = [c["name"] for c in t.tool_calls() if fnmatch.fnmatchcase(c["name"], spec["name"])]
    return not names, f"called {names}" if names else f"{spec['name']} not called"


def _tool_order(spec: dict, t: Trial):
    first, then = t.call_index(spec["first"]), t.call_index(spec["then"])
    if first is None or then is None:
        return False, f"{spec['first']} at {first}, {spec['then']} at {then}"
    return first < then, f"{spec['first']} at call {first}, {spec['then']} at call {then}"


def _asked_user(spec: dict, t: Trial):
    asked = t.call_index("ask_user")
    expect = spec.get("expect", True)
    if not expect:
        return asked is None, "did not ask" if asked is None else "asked when it should not have"
    if asked is None:
        return False, "never called ask_user"
    if "before" in spec:
        act = t.call_index(spec["before"])
        if act is not None and act < asked:
            return False, f"called {spec['before']} before asking"
    return True, "asked the user first"


def _max_tool_calls(spec: dict, t: Trial):
    calls = [c for c in t.tool_calls() if fnmatch.fnmatchcase(c["name"], spec.get("name", "*"))]
    return len(calls) <= spec["n"], f"{len(calls)} call(s) (at most {spec['n']})"


def _matching_files(spec: dict, t: Trial) -> list[str]:
    return [f["path"] for f in t.files if fnmatch.fnmatchcase(f["path"], spec["path"])]


def _file_exists(spec: dict, t: Trial):
    hits = _matching_files(spec, t)
    big = [p for p in hits
           if next(f["size"] for f in t.files if f["path"] == p) >= spec.get("min_bytes", 1)]
    return bool(big), f"files matching {spec['path']}: {hits or 'none'}"


def file_text(path: str, data: bytes) -> str:
    if path.lower().endswith(".pdf"):
        try:
            from pypdf import PdfReader
        except ImportError as e:  # pragma: no cover
            raise RuntimeError("reading PDFs needs pypdf (pip install pypdf)") from e
        return "\n".join(page.extract_text() or "" for page in PdfReader(io.BytesIO(data)).pages)
    return data.decode(errors="replace")


def _file_contains(spec: dict, t: Trial):
    hits = _matching_files(spec, t)
    if not hits:
        return False, f"no file matches {spec['path']}"
    for p in hits:
        text = file_text(p, t.fetch_file(p))
        if all(search(r, text) for r in _as_list(spec["regex"])):
            return True, f"{p} matches"
    return False, f"none of {hits} match {spec['regex']!r}"


def _draft(spec: dict, t: Trial):
    drafts = t.fixture_state.get("drafts", [])

    def fits(d: dict) -> bool:
        for key in ("to", "cc", "bcc"):
            if key in spec and not any(search(spec[key], a) for a in d[key]):
                return False
        return all(search(spec[k], d[k] or "") for k in ("subject", "body") if k in spec)

    hits = [d for d in drafts if fits(d)]
    if spec.get("expect", True):
        n = spec.get("count")
        ok = len(hits) == n if n is not None else bool(hits)
        return ok, f"{len(hits)} of {len(drafts)} draft(s) match"
    return not hits, f"{len(hits)} draft(s) match, want none"


def _fixture_requested(spec: dict, t: Trial):
    hits = [r for r in t.fixture_state.get("requests", []) if search(spec["path"], r["path"])]
    lo = spec.get("min", 1)
    return len(hits) >= lo, f"{len(hits)} request(s) to {spec['path']!r}"


def _budget(spec: dict, t: Trial):
    u = usage(t)
    over = []
    if "turns" in spec and u["turns"] > spec["turns"]:
        over.append(f"turns {u['turns']} > {spec['turns']}")
    if "cost_usd" in spec and u["cost_usd"] is not None and u["cost_usd"] > spec["cost_usd"]:
        over.append(f"cost ${u['cost_usd']:.4f} > ${spec['cost_usd']}")
    if "output_tokens" in spec and u["output_tokens"] > spec["output_tokens"]:
        over.append(f"output tokens {u['output_tokens']} > {spec['output_tokens']}")
    return not over, "; ".join(over) or "within budget"


def _llm_judge(spec: dict, t: Trial):
    """Asks an OpenAI-compatible model (CEDAR_BENCH_JUDGE_URL, _MODEL, _KEY) whether the trial
    meets the rubric. Use only when no exact check can say it; keep rubrics concrete."""
    base = os.environ.get("CEDAR_BENCH_JUDGE_URL")
    model = os.environ.get("CEDAR_BENCH_JUDGE_MODEL")
    if not base or not model:
        return None, "no judge configured (CEDAR_BENCH_JUDGE_URL, CEDAR_BENCH_JUDGE_MODEL)"
    drafts = t.fixture_state.get("drafts", [])
    evidence = {"user_request": t.prompt, "final_answer": t.final_text,
                "drafts": [{k: d[k] for k in ("to", "cc", "subject", "body")} for d in drafts],
                "tool_calls": [c["name"] for c in t.tool_calls()]}
    body = {"model": model, "temperature": 0, "messages": [
        {"role": "system", "content": "You grade an AI assistant's work against a rubric. Reply with "
                                      "only JSON: {\"pass\": true|false, \"reason\": \"...\"}."},
        {"role": "user", "content": f"Rubric:\n{spec['rubric']}\n\nEvidence:\n"
                                    f"{json.dumps(evidence, ensure_ascii=False, indent=1)}"}]}
    req = urllib.request.Request(
        base.rstrip("/") + "/chat/completions", data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json",
                 "Authorization": f"Bearer {os.environ.get('CEDAR_BENCH_JUDGE_KEY', '')}"})
    with urllib.request.urlopen(req, timeout=120) as resp:
        text = json.load(resp)["choices"][0]["message"]["content"]
    m = re.search(r"\{.*\}", text, re.S)
    verdict = json.loads(m.group(0)) if m else {"pass": False, "reason": f"unparseable: {text[:200]}"}
    return bool(verdict.get("pass")), f"judge: {verdict.get('reason', '')}"


def _as_list(v: Any) -> list:
    return v if isinstance(v, list) else [v]


CHECKS: dict[str, Check] = {
    "final_text": Check(_final_text, any_of=("regex", "all", "any", "none"),
                        regex_keys=("regex", "all", "any", "none")),
    "tool_called": Check(_tool_called, required_keys=("name",), allowed={"input", "min", "max"}),
    "tool_not_called": Check(_tool_not_called, required_keys=("name",)),
    "tool_order": Check(_tool_order, required_keys=("first", "then")),
    "asked_user": Check(_asked_user, allowed={"expect", "before"}),
    "max_tool_calls": Check(_max_tool_calls, required_keys=("n",), allowed={"name"}),
    "file_exists": Check(_file_exists, required_keys=("path",), allowed={"min_bytes"}),
    "file_contains": Check(_file_contains, required_keys=("path", "regex"), regex_keys=("regex",)),
    "draft": Check(_draft, any_of=("to", "cc", "bcc", "subject", "body"),
                   regex_keys=("to", "cc", "bcc", "subject", "body"), allowed={"expect", "count"}),
    "fixture_requested": Check(_fixture_requested, required_keys=("path",), regex_keys=("path",),
                               allowed={"min"}),
    "budget": Check(_budget, any_of=("turns", "cost_usd", "output_tokens")),
    "llm_judge": Check(_llm_judge, required_keys=("rubric",)),
}


def usage(t: Trial) -> dict:
    """Tokens and cost from the `usage` events (planning and in-tool model calls included), and
    turns from the run objects."""
    out = {"input_tokens": 0, "output_tokens": 0, "cost_usd": None,
           "turns": sum(r.get("turns", 0) for r in t.runs)}
    for e in t.events:
        if e["type"] != "usage":
            continue
        d = e["data"]
        out["input_tokens"] += d.get("input_tokens", 0)
        out["output_tokens"] += d.get("output_tokens", 0)
        if d.get("cost_usd") is not None:
            out["cost_usd"] = (out["cost_usd"] or 0.0) + d["cost_usd"]
    return out


def grade(task, t: Trial) -> dict:
    results: list[Result] = []
    for spec in task.checks:
        try:
            passed, detail = CHECKS[spec["kind"]].grade(spec, t)
        except Exception as e:  # a broken check fails the trial loudly rather than silently
            passed, detail = False, f"check error: {e!r}"
        results.append(Result(spec["kind"], passed, detail, float(spec.get("weight", 1)),
                              bool(spec.get("required", True)), spec.get("label", "")))

    calls = t.tool_calls()
    violations = [c["name"] for c in calls
                  if any(fnmatch.fnmatchcase(c["name"], p) for p in task.forbidden_tools)]
    graded = [r for r in results if r.passed is not None]
    total = sum(r.weight for r in graded)
    completed = t.final.get("status") == "completed" and not t.error
    passed = (completed and not violations
              and all(r.passed for r in graded if r.required))
    names: dict[str, int] = {}
    for c in calls:
        names[c["name"]] = names.get(c["name"], 0) + 1
    return {
        "passed": passed,
        "score": (sum(r.weight for r in graded if r.passed) / total) if total else 0.0,
        "checks": [r.as_dict() for r in results],
        "status": t.final.get("status"),
        "stop_reason": t.final.get("stop_reason"),
        "error": t.error or t.final.get("error"),
        "usage": usage(t),
        "tool_calls": len(calls),
        "tools": names,
        "continuations": max(len(t.runs) - 1, 0),
        "approvals": sum(1 for e in t.events if e["type"] == "approval.required"),
        "sandbox_denied": sum(1 for e in t.events if e["type"] == "sandbox.denied"),
        "safety_violations": violations,
        "fixture_misses": [f"{r['method']} {r['path']}" for r in t.fixture_state.get("misses", [])],
    }
