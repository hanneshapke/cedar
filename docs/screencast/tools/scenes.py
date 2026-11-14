"""What the scripted model says in the Cedar intro screencast.

reply(system, messages, request) returns {"text", "calls": [(tool, input)], "delay"}. Which reply
it is follows from where the conversation stands, not from its words, so it works in every
language of content.py: the first request is the task, the second asks to delete the file, the
third to remember something; within one, the number of tool results so far says which step is
next. CEDAR_DEMO_LANG picks the language (default en).

The numbers in the texts are the ones the run_python step really prints for the demo file (see
make_csv.py); the tools themselves run for real in Cedar's sandbox.
"""
import os

from content import content

C = content(os.environ.get("CEDAR_DEMO_LANG", "en"))

SERIES = [
    [1450] * 10,
    [571, 552, 549, 589, 493, 611, 700, 643, 594, 624],
    [178, 185, 184, 203, 242, 408, 376, 422, 263, 235],
    [190, 171, 185, 180, 195, 179, 176, 194, 179, 185],
    [212, 205, 188, 150, 118, 96, 92, 95, 121, 168],
    [141, 104, 197, 122, 185, 146, 124, 139, 194, 77],
    [113] * 10,
]


def analysis(c):
    """The Python the model "writes": totals by category, then a table by month."""
    date, category, _, amount = c["cols"]
    groceries, dining, energy = c["cats"][1], c["cats"][2], c["cats"][4]
    py = c["py"]
    over = py["over"].replace("{n}", "{len(by_month)}")
    return f'''# {py["comment"]}
import csv
from collections import defaultdict

by_month = defaultdict(lambda: defaultdict(float))
by_cat = defaultdict(float)
with open("attachments/{c["csv"]}") as f:
    for row in csv.DictReader(f):
        amount = float(row["{amount}"])
        by_month[row["{date}"][:7]][row["{category}"]] += amount
        by_cat[row["{category}"]] += amount

total = sum(by_cat.values())
cats = sorted(by_cat, key=by_cat.get, reverse=True)
for c in cats:
    print(f"{{c:14}} {{by_cat[c]:9.2f}} EUR  {{by_cat[c] / total:6.1%}}  {{by_cat[c] / len(by_month):8.2f}} {py["a_month"]}")
print(f"{{'{py["total"]}':14}} {{total:9.2f}} EUR {over}")
print()
print(f"{{'{py["month"]}':9}}{{'{py["sum"]}':>8}}{{'{groceries}':>14}}{{'{dining}':>16}}{{'{energy}':>10}}")
for month in sorted(by_month):
    m = by_month[month]
    print(f"{{month:9}}{{sum(m.values()):8.0f}}{{m['{groceries}']:14.0f}}{{m['{dining}']:16.0f}}{{m['{energy}']:10.0f}}")
'''


def reply(system, messages, req):
    asked = [i for i, m in enumerate(messages) if m["role"] == "user"]
    turn = len(asked)
    n = sum(1 for m in messages[asked[-1] + 1:] if m["role"] == "tool") if asked else 0
    if system.startswith("You plan work"):
        import json
        return {"delay": 2.0, "text": json.dumps({"steps": C["plan"]}, ensure_ascii=False)}
    if system.startswith("You turn a conversation"):
        return {"delay": 2.0, "text": C["agent_prompt"]}
    if not req.get("tools"):
        return {"delay": 0.2, "text": "{}"}  # the notes Cedar takes after a run: nothing to add
    if turn == 1:
        if n == 0:
            return {"delay": 3.0, "text": C["say_read"], "calls": [("run_python", {"code": analysis(C)})]}
        if n == 1:
            return {"delay": 4.0, "text": C["say_chart"],
                    "calls": [("generate_chart", {
                        "path": C["chart_file"], "kind": "bar", "stacked": True,
                        "title": C["chart_title"], "y_label": "EUR", "labels": C["months"],
                        "series": [{"name": k, "values": v} for k, v in zip(C["cats"], SERIES)]})]}
        if n == 2:
            return {"delay": 4.0, "text": C["say_pdf"],
                    "calls": [("generate_pdf", {"path": C["pdf_file"], "content": C["report"],
                                                "title": C["pdf_title"]})]}
        return {"delay": 3.5, "text": C["summary"]}
    if turn == 2:
        if n == 0:
            return {"delay": 1.5, "text": C["say_delete"],
                    "calls": [("bash", {"command": f"rm attachments/{C['csv']} && ls -R"})]}
        return {"delay": 1.2, "text": C["done_delete"]}
    if turn == 3:
        if n == 0:
            return {"delay": 1.2, "calls": [("remember", {"text": C["remember"], "kind": "preference"})]}
        return {"delay": 1.0, "text": C["noted"]}
    return {"delay": 1.0, "text": C["unscripted"]}
