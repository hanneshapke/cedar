"""The scripted user: answers what a run asks, the way the task's `user` section says.

- `ask_user` ends a run with `stop_reason: ask_user`; the user answers each question with the first
  rule whose `match` pattern is found in it (or its header and choices), and the answer starts a
  continuation (`parent_id`). With no rule for a question the user stops answering, and the trial
  is graded as it stands.
- `approval.required` is answered by the first approval rule whose `tool` glob (and `input`
  patterns, if any) fit the call, else by `default` (deny unless the task says otherwise).
- an awaited `plan` is accepted as proposed.
"""

from __future__ import annotations

import fnmatch
import json
import re


class ScriptedUser:
    def __init__(self, spec: dict):
        self.spec = spec or {}
        self.continuations_left = int(self.spec.get("max_continuations", 2))

    def answer(self, questions: list[dict]) -> str | None:
        if self.continuations_left <= 0 or not questions:
            return None
        rules = self.spec.get("answers") or []
        lines = []
        for q in questions:
            text = "\n".join([q.get("header") or "", q.get("question", ""),
                              *[o.get("label", "") for o in q.get("options") or []]])
            rule = next((r for r in rules if re.search(r.get("match", ".*"), text)), None)
            if rule is None:
                return None
            lines.append((q.get("question", ""), str(rule["answer"]).strip()))
        self.continuations_left -= 1
        if len(lines) == 1:
            return lines[0][1]
        return "\n".join(f"{q}\n{a}" for q, a in lines)

    def decide(self, approval: dict) -> str:
        spec = self.spec.get("approvals") or {}
        for rule in spec.get("rules") or []:
            if not fnmatch.fnmatchcase(approval.get("tool", ""), rule.get("tool", "*")):
                continue
            inputs = approval.get("input") or {}
            if all(re.search(p, json.dumps(inputs.get(k, ""))) for k, p in
                   (rule.get("input") or {}).items()):
                return rule["decision"]
        return spec.get("default", "deny")

    def plan(self, steps: list[dict]) -> list[dict]:
        return [{"id": s["id"], "title": s["title"]} for s in steps]
