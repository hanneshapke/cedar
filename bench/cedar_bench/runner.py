"""Runs tasks against a cedar server: for each model x reasoning level x task x repeat, reset the
fixtures, submit the run, follow its events, play the scripted user, collect what it left behind,
and grade it. Each trial is appended to `results.jsonl` as soon as it is graded, so an interrupted
sweep resumes where it stopped (`--resume`).

The run config (see bench/models.example.yaml):

    cedar: {url: http://127.0.0.1:8080, key_env: CEDAR_BENCH_KEY, admin_key_env: CEDAR_BENCH_ADMIN_KEY}
    # or several servers: cedar looks up the calendar time zone once per process, so personas in
    # different time zones need different cedar processes (all pointed at the same fixtures).
    # A server with a `timezone` takes only personas in it; one without takes the first it gets.
    # cedar:
    #   - {url: http://127.0.0.1:8080, key_env: CEDAR_BENCH_KEY, timezone: America/Los_Angeles}
    #   - {url: http://127.0.0.1:8081, key_env: CEDAR_BENCH_KEY, timezone: Europe/Berlin}
    fixtures_url: http://127.0.0.1:8765
    repeats: 3
    trial_timeout_secs: 900
    models:
      - id: deepseek                      # the registry id runs name
        reasoning: [none, high]           # levels to sweep; omit for the model's default
        register:                         # optional: PUT /v1/admin/models/{id} before the sweep
          base_url: http://localhost:8000/v1
          model: deepseek-ai/DeepSeek-V4-Flash
          api_key_env: DEEPSEEK_API_KEY   # read from the environment, never stored in the config
          reasoning_levels: [none, low, high, max]
          price: {input_usd_per_m: 0.27, output_usd_per_m: 1.10}
"""

from __future__ import annotations

import base64
import dataclasses
import datetime as dt
import json
import os
import re
import time
import urllib.request
from pathlib import Path
from typing import Any

import yaml

from . import __version__, grading, timeutil
from .client import Cedar
from .pdfgen import make_pdf
from .schema import Task
from .user import ScriptedUser

TERMINAL = {"run.completed", "run.failed", "run.cancelled"}


def _fixtures(url: str, path: str, body: dict | None = None) -> dict:
    req = urllib.request.Request(
        url.rstrip("/") + path, method="POST" if body is not None else "GET",
        data=json.dumps(body).encode() if body is not None else None,
        headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=30) as resp:
        return json.load(resp)


def resolved(task: Task, anchor: dt.date, base: str) -> Task:
    """The task with its date tokens and `{base}` (the fixture server's address) filled in."""
    def fill(value):
        return json.loads(json.dumps(timeutil.resolve(value, anchor)).replace("{base}", base))

    return dataclasses.replace(task, run=fill(task.run), user=fill(task.user),
                               checks=fill(task.checks))


def attachments(task: Task, fixtures_dir: Path) -> list[dict]:
    out = []
    for a in task.run.get("attachments") or []:
        if "text_pdf" in a:
            data, media = make_pdf(a["text_pdf"], title=a["name"]), "application/pdf"
        else:
            path = fixtures_dir / a["path"]
            data = path.read_bytes()
            media = a.get("media_type") or {
                ".pdf": "application/pdf", ".png": "image/png", ".jpg": "image/jpeg",
                ".jpeg": "image/jpeg", ".gif": "image/gif", ".webp": "image/webp",
            }[path.suffix.lower()]
        out.append({"name": a["name"], "media_type": media, "data": base64.b64encode(data).decode()})
    return out


def run_body(task: Task, model: str, reasoning: str | None, fixtures_dir: Path) -> dict:
    r = task.run
    body: dict[str, Any] = {"prompt": r["prompt"].strip(), "model": model,
                            "plan": str(r.get("plan", "off")),
                            "interactive": bool(r.get("interactive", False))}
    for key in ("tools", "max_turns", "locale", "system"):
        if key in r:
            body[key] = r[key]
    if reasoning:
        body["reasoning"] = reasoning
    files = attachments(task, fixtures_dir)
    if files:
        body["attachments"] = files
    return body


def run_trial(cedar: Cedar, fixtures_url: str, fixtures_dir: Path, task: Task, model: str,
              reasoning: str | None, timeout: float) -> tuple[dict, grading.Trial]:
    state = _fixtures(fixtures_url, "/_admin/reset", {"persona": task.persona, "anchor": task.anchor})
    anchor = dt.date.fromisoformat(state["anchor"])
    task = resolved(task, anchor, state["base"])
    user = ScriptedUser(task.user)
    deadline = time.monotonic() + timeout
    runs: list[dict] = []
    events: list[dict] = []
    error = None
    body = run_body(task, model, reasoning, fixtures_dir)
    started = time.monotonic()
    try:
        while True:
            run = cedar.create_run(body)
            questions: list[dict] = []
            try:
                for ev in cedar.events(run["id"], deadline):
                    ev["run_id"] = run["id"]
                    events.append(ev)
                    data = ev.get("data") or {}
                    if ev["type"] == "approval.required":
                        cedar.answer_approval(run["id"], data["id"], user.decide(data))
                    elif ev["type"] == "plan" and data.get("awaiting"):
                        cedar.answer_plan(run["id"], user.plan(data["steps"]))
                    elif ev["type"] == "tool.call" and data.get("name") == "ask_user":
                        questions = (data.get("input") or {}).get("questions") or []
            except TimeoutError:
                cedar.cancel(run["id"])
                error = f"trial timed out after {timeout:.0f}s"
            runs.append(cedar.run(run["id"]))
            if error or runs[-1].get("stop_reason") != "ask_user":
                break
            reply = user.answer(questions)
            if reply is None:
                break
            # A continuation keeps its parent's tools, plan mode and callback unless it names them.
            body = {"prompt": reply, "parent_id": run["id"], "model": model,
                    "interactive": body["interactive"]}
            if reasoning:
                body["reasoning"] = reasoning
    except Exception as e:  # a harness failure: record it, grade what there is
        error = f"harness: {e!r}"

    last = runs[-1]["id"] if runs else None
    listing = cedar.files(last) if last else {"files": []}
    trial = grading.Trial(
        prompt=task.run["prompt"], runs=runs, events=events, files=listing.get("files", []),
        fetch_file=(lambda p: cedar.file(last, p)) if last else (lambda p: b""),
        fixture_state=_fixtures(fixtures_url, "/_admin/state"), error=error,
    )
    result = grading.grade(task, trial)
    wrong = wrong_zone(events, state.get("timezone"))
    if wrong:
        result.update(passed=False, error=f"harness: cedar's calendar answered in {wrong}, the "
                      f"persona is in {state['timezone']} (one cedar process per time zone)")
    result.update({"anchor": anchor.isoformat(), "run_ids": [r["id"] for r in runs],
                   "duration_secs": round(time.monotonic() - started, 1)})
    return result, trial


def wrong_zone(events: list[dict], zone: str | None) -> str | None:
    """The time zone a calendar tool answered in, if it isn't the persona's."""
    if not zone:
        return None
    names = {e["data"]["id"]: e["data"]["name"] for e in events if e["type"] == "tool.call"}
    for e in events:
        if e["type"] == "tool.result" and names.get(e["data"].get("id"), "").startswith("calendar_"):
            m = re.search(r'"timeZone":\s*"([^"]+)"', str(e["data"].get("output", "")))
            if m and m.group(1) != zone:
                return m.group(1)
    return None


class Server:
    """One cedar process, and the time zone its calendar tools are (or will be) fixed to."""

    def __init__(self, spec: dict):
        self.url = spec["url"]
        self.client = Cedar(spec["url"], os.environ[spec.get("key_env", "CEDAR_BENCH_KEY")],
                            os.environ.get(spec.get("admin_key_env", "CEDAR_BENCH_ADMIN_KEY")))
        self.zone: str | None = spec.get("timezone")


def pick(servers: list[Server], zone: str | None) -> Server | None:
    if zone is None:
        return servers[0]
    for s in servers:
        if s.zone == zone:
            return s
    for s in servers:
        if s.zone is None:
            s.zone = zone
            return s
    return None


def persona_zones(personas_dir: Path, tasks: list[Task]) -> dict[str, str]:
    return {t.persona: yaml.safe_load((personas_dir / t.persona / "persona.yaml").read_text())["timezone"]
            for t in tasks if t.persona}


def load_config(path: Path) -> dict:
    cfg = yaml.safe_load(path.read_text())
    cfg.setdefault("repeats", 1)
    cfg.setdefault("trial_timeout_secs", 900)
    return cfg


def register_models(cedar: Cedar, cfg: dict) -> None:
    for m in cfg["models"]:
        entry = dict(m.get("register") or {})
        if not entry:
            continue
        key_env = entry.pop("api_key_env", None)
        if key_env:
            entry["api_key"] = os.environ.get(key_env, "")
        cedar.register_model(m["id"], entry)
        print(f"registered {m['id']} ({entry.get('model')})", flush=True)


def _key(r: dict) -> tuple:
    return (r["task"], r["task_version"], r["model"], r["reasoning"], r["repeat"])


def sweep(cfg: dict, tasks: list[Task], out_dir: Path, fixtures_dir: Path, personas_dir: Path,
          resume: bool, register: bool, keep_trace: bool) -> Path:
    specs = cfg["cedar"] if isinstance(cfg["cedar"], list) else [cfg["cedar"]]
    servers = [Server(s) for s in specs]
    if register:
        for server in servers:
            register_models(server.client, cfg)
    zones = persona_zones(personas_dir, tasks)
    # Tasks of one time zone together, so a server without a fixed zone isn't claimed by a stray one.
    tasks = sorted(tasks, key=lambda t: (zones.get(t.persona, ""), t.id))
    out_dir.mkdir(parents=True, exist_ok=True)
    results_path = out_dir / "results.jsonl"
    done = set()
    if resume and results_path.exists():
        done = {_key(json.loads(line)) for line in results_path.read_text().splitlines() if line}
    (out_dir / "meta.json").write_text(json.dumps({
        "bench_version": __version__, "started_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "cedar_urls": {s.url: s.zone for s in servers}, "tasks": len(tasks), "repeats": cfg["repeats"],
        "models": [{k: v for k, v in m.items() if k != "register"} | {
            "model": (m.get("register") or {}).get("model"),
            "price": (m.get("register") or {}).get("price")} for m in cfg["models"]],
    }, indent=2))

    with results_path.open("a") as out:
        for m in cfg["models"]:
            for reasoning in m.get("reasoning") or [None]:
                for task in tasks:
                    for repeat in range(cfg["repeats"]):
                        meta = {"task": task.id, "task_version": task.version, "model": m["id"],
                                "reasoning": reasoning, "repeat": repeat,
                                "category": task.category, "tags": task.tags, "split": task.split}
                        if _key(meta) in done:
                            continue
                        server = pick(servers, zones.get(task.persona))
                        if server is None:
                            print(f"skip {task.id}: no cedar server for {zones[task.persona]} "
                                  "(add one to the config's cedar list)", flush=True)
                            continue
                        result, trial = run_trial(server.client, cfg["fixtures_url"], fixtures_dir,
                                                  task, m["id"], reasoning, cfg["trial_timeout_secs"])
                        result = meta | result | {"cedar_url": server.url}
                        if keep_trace:
                            result["events"] = trial.events
                        out.write(json.dumps(result, ensure_ascii=False) + "\n")
                        out.flush()
                        mark = "PASS" if result["passed"] else "fail"
                        cost = result["usage"]["cost_usd"]
                        print(f"{mark} {task.id} [{m['id']}/{reasoning or '-'} #{repeat}] "
                              f"score={result['score']:.2f} turns={result['usage']['turns']} "
                              f"cost={'-' if cost is None else f'${cost:.4f}'} "
                              f"{result['error'] or ''}", flush=True)
    return results_path
