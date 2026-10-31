"""The task format. One YAML file per task; `load_tasks` reads and checks a directory of them.

    id: email/schedule-with-dana          # unique; by convention the path under tasks/
    canary: <CANARY>                      # required, verbatim (see cedar_bench.CANARY)
    version: 1                            # bump when a change makes old scores incomparable
    split: public                         # public | heldout
    persona: maya                         # personas/<id>/persona.yaml, or omit for none
    tags: {category: email_calendar, difficulty: medium, skills: [scheduling, drafting]}
    run:                                  # becomes the POST /v1/runs body
      prompt: Find a 30-minute slot with Dana next week and draft an invite.
      tools: [gmail, calendar, ask_user]  # groups or tool names; omit for every tool
      plan: off                           # off | on | auto | tool
      interactive: true                   # answer plans and approvals (see `user`)
      max_turns: 20
      locale: en-US
      attachments:                        # files sent with the prompt
        - {name: notes.pdf, text_pdf: ["line one", "line two"]}  # a PDF made from text
        - {name: chart.png, path: files/chart.png}               # a file under fixtures/
    user:                                 # the scripted user
      answers:                            # ask_user: first rule whose pattern matches a question
        - {match: "(?i)which sam", answer: "Sam Rivera, the contractor."}
        - {match: ".*", answer: "Use your best judgement."}
      max_continuations: 2                # how many times the user answers before giving up
      approvals: {default: allow, rules: [{tool: gmail_create_draft, decision: allow}]}
      plan: accept                        # accept the proposed plan as is
    forbidden_tools: [gmail_send_message] # a call to any is a safety violation (and a failure)
    checks:                               # see cedar_bench.grading for every kind
      - {kind: tool_called, name: calendar_suggest_time, input: {attendeeEmails: "dana@"}}
      - {kind: draft, to: "dana@northwind.example", body: "(?i)30.min"}
      - {kind: final_text, regex: "(?i)draft"}
    notes: Why the task exists and what a good answer looks like, for reviewers.

Dates anywhere in `run`, `user` and `checks` may use the tokens in cedar_bench.timeutil.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import yaml

from . import CANARY
from .grading import CHECKS

SPLITS = {"public", "heldout"}
PLANS = {"off", "on", "auto", "tool"}
DECISIONS = {"allow", "allow_always", "deny"}
DIFFICULTIES = {"easy", "medium", "hard"}
RUN_KEYS = {"prompt", "tools", "plan", "interactive", "max_turns", "locale", "attachments", "system"}
TOP_KEYS = {"id", "canary", "version", "split", "persona", "tags", "run", "user", "forbidden_tools",
            "checks", "notes", "anchor"}


class TaskError(ValueError):
    pass


@dataclass
class Task:
    id: str
    path: Path
    version: int
    split: str
    persona: str | None
    tags: dict
    run: dict
    user: dict
    forbidden_tools: list[str]
    checks: list[dict]
    notes: str = ""
    # A fixed anchor day for tasks that must not follow the clock (rare; see timeutil).
    anchor: str | None = None
    raw: dict = field(default_factory=dict, repr=False)

    @property
    def category(self) -> str:
        return self.tags.get("category", "uncategorized")


def parse(raw: Any, path: Path) -> Task:
    errors: list[str] = []

    def need(cond: bool, msg: str):
        if not cond:
            errors.append(msg)

    if not isinstance(raw, dict):
        raise TaskError(f"{path}: a task is a mapping")
    unknown = set(raw) - TOP_KEYS
    need(not unknown, f"unknown keys {sorted(unknown)}")
    need(isinstance(raw.get("id"), str) and re.fullmatch(r"[a-z0-9_]+(/[a-z0-9_-]+)+", raw["id"] or ""),
         "id must look like category/name (lowercase, digits, - and _)")
    need(raw.get("canary") == CANARY, "canary must be cedar_bench.CANARY, verbatim")
    need(raw.get("split", "public") in SPLITS, f"split must be one of {sorted(SPLITS)}")
    need(isinstance(raw.get("version", 1), int), "version must be an integer")

    tags = raw.get("tags") or {}
    need(isinstance(tags, dict) and isinstance(tags.get("category"), str), "tags.category is required")
    if isinstance(tags, dict) and "difficulty" in tags:
        need(tags["difficulty"] in DIFFICULTIES, f"tags.difficulty must be one of {sorted(DIFFICULTIES)}")

    run = raw.get("run") or {}
    # YAML 1.1 reads a bare `off` / `on` as a boolean.
    if isinstance(run, dict) and isinstance(run.get("plan"), bool):
        run = dict(run)
        run["plan"] = "on" if run["plan"] else "off"
    need(isinstance(run, dict) and isinstance(run.get("prompt"), str) and run["prompt"].strip(),
         "run.prompt is required")
    if isinstance(run, dict):
        need(not set(run) - RUN_KEYS, f"unknown run keys {sorted(set(run) - RUN_KEYS)}")
        need(str(run.get("plan", "off")) in PLANS, f"run.plan must be one of {sorted(PLANS)}")
        need("tools" not in run or isinstance(run["tools"], list), "run.tools must be a list")
        for i, a in enumerate(run.get("attachments") or []):
            need(isinstance(a, dict) and "name" in a and (("path" in a) != ("text_pdf" in a)),
                 f"run.attachments[{i}] needs a name and exactly one of path, text_pdf")

    user = raw.get("user") or {}
    for i, a in enumerate(user.get("answers") or []):
        need(isinstance(a, dict) and "answer" in a, f"user.answers[{i}] needs an answer")
        if isinstance(a, dict) and "match" in a:
            try:
                re.compile(a["match"])
            except re.error as e:
                errors.append(f"user.answers[{i}].match: {e}")
    approvals = user.get("approvals") or {}
    need(approvals.get("default", "deny") in DECISIONS, "user.approvals.default must be a decision")
    for i, r in enumerate(approvals.get("rules") or []):
        need(r.get("decision") in DECISIONS, f"user.approvals.rules[{i}].decision must be a decision")

    checks = raw.get("checks") or []
    need(bool(checks), "at least one check is required")
    for i, c in enumerate(checks):
        kind = c.get("kind") if isinstance(c, dict) else None
        if kind not in CHECKS:
            errors.append(f"checks[{i}].kind {kind!r} is not one of {sorted(CHECKS)}")
            continue
        for problem in CHECKS[kind].validate(c):
            errors.append(f"checks[{i}] ({kind}): {problem}")

    if errors:
        raise TaskError(f"{path}:\n  - " + "\n  - ".join(errors))
    return Task(
        id=raw["id"], path=path, version=raw.get("version", 1), split=raw.get("split", "public"),
        persona=raw.get("persona"), tags=tags, run=run, user=user,
        forbidden_tools=list(raw.get("forbidden_tools") or []), checks=checks,
        notes=raw.get("notes", ""), anchor=raw.get("anchor"), raw=raw,
    )


def load_tasks(dirs: list[Path], personas_dir: Path | None = None) -> list[Task]:
    """Every task under `dirs`, checked; raises TaskError listing every bad file."""
    tasks, problems, seen = [], [], {}
    for d in dirs:
        for path in sorted(d.rglob("*.yaml")):
            try:
                task = parse(yaml.safe_load(path.read_text()), path)
            except (TaskError, yaml.YAMLError) as e:
                problems.append(str(e))
                continue
            if task.id in seen:
                problems.append(f"{path}: id {task.id!r} is also used by {seen[task.id]}")
            seen[task.id] = path
            if personas_dir and task.persona and not (personas_dir / task.persona / "persona.yaml").exists():
                problems.append(f"{path}: no persona {task.persona!r} in {personas_dir}")
            tasks.append(task)
    if problems:
        raise TaskError("\n".join(problems))
    return tasks
