"""What the scripted model says in the Cedar intro screencast.

reply(system, prompt, results, request) returns {"text", "calls": [(tool, input)], "reasoning",
"delay"}: `prompt` is the latest user message and `results` the tool results since it. The numbers
in the texts below are the ones the run_python step really prints for household-spending-2026.csv
(see make_csv.py); the tools themselves run for real in Cedar's sandbox.
"""

MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct"]
SERIES = {
    "Rent": [1450] * 10,
    "Groceries": [571, 552, 549, 589, 493, 611, 700, 643, 594, 624],
    "Dining out": [178, 185, 184, 203, 242, 408, 376, 422, 263, 235],
    "Transport": [190, 171, 185, 180, 195, 179, 176, 194, 179, 185],
    "Energy": [212, 205, 188, 150, 118, 96, 92, 95, 121, 168],
    "Other": [141, 104, 197, 122, 185, 146, 124, 139, 194, 77],
    "Subscriptions": [113] * 10,
}

ANALYSIS = '''# Total the spending by month and category
import csv
from collections import defaultdict

by_month = defaultdict(lambda: defaultdict(float))
by_cat = defaultdict(float)
with open("attachments/household-spending-2026.csv") as f:
    for row in csv.DictReader(f):
        amount = float(row["amount_eur"])
        by_month[row["date"][:7]][row["category"]] += amount
        by_cat[row["category"]] += amount

total = sum(by_cat.values())
cats = sorted(by_cat, key=by_cat.get, reverse=True)
for c in cats:
    print(f"{c:14} {by_cat[c]:9.2f} EUR  {by_cat[c] / total:6.1%}  {by_cat[c] / len(by_month):8.2f} a month")
print(f"{'Total':14} {total:9.2f} EUR over {len(by_month)} months")
print()
print(f"{'month':9}{'total':>8}{'groceries':>11}{'dining out':>12}{'energy':>8}")
for month in sorted(by_month):
    m = by_month[month]
    print(f"{month:9}{sum(m.values()):8.0f}{m['Groceries']:11.0f}{m['Dining out']:12.0f}{m['Energy']:8.0f}")
'''

REPORT = '''# Household spending, January to October 2026

**Total: €28,959 over ten months, about €2,896 a month.**

![Spending by month and category](spending-by-month.png)

| Category | Total | Share | Per month |
| --- | ---: | ---: | ---: |
| Rent | €14,500 | 50.1% | €1,450 |
| Groceries | €5,928 | 20.5% | €593 |
| Dining out | €2,695 | 9.3% | €270 |
| Transport | €1,834 | 6.3% | €183 |
| Energy | €1,445 | 5.0% | €145 |
| Other | €1,428 | 4.9% | €143 |
| Subscriptions | €1,129 | 3.9% | €113 |

## Three ways to save

1. **Dining out in summer.** It runs near €180 a month in winter and over €400 from June to August. Holding summer to €250 saves about €450 a year.
2. **Subscriptions.** Six of them cost €113 a month, two of them video streaming services. Dropping one streaming service and the news subscription saves €347 a year.
3. **Groceries since July.** The weekly shop rose from about €560 to €640 a month. Getting halfway back saves about €480 a year.

Together that is roughly **€1,280 a year**, without touching rent.
'''

SUMMARY = '''Here is where the money goes. From January to October you spent **€28,959**, about **€2,896 a month**.

- **Rent** is half of it: €1,450 a month.
- **Groceries** are a fifth, €593 a month, and about €80 higher since July.
- **Dining out** averages €270, but it more than doubles in summer: over €400 a month from June to August.
- **Subscriptions** are six fixed payments, €113 a month.

Three ways to save, about **€1,280 a year** together:

1. Hold summer dining out to €250 a month: about €450.
2. Drop one of the two streaming services and the news subscription: €347.
3. Bring the weekly shop halfway back to its spring level: about €480.

The one-page summary is in `spending-summary.pdf`, with the chart in it.'''


AGENT_PROMPT = ("From the household_spending_2026 records, total last month's spending by category. "
    "Compare it with the month before and with the target of 2,800 euros a month. Draw one stacked bar chart of the "
    "year so far, and write a one-page PDF summary that says plainly where spending went up and names three ways to save.")


def reply(system, prompt, results, req):
    p = prompt.lower()
    n = len(results)
    if system.startswith("You plan work"):
        return {"delay": 2.0, "text": '{"steps": ["Read the spending file and check its columns", '
                '"Total the spending by month and category", "Draw a chart of spending by month", '
                '"Find three realistic ways to save", "Write the one-page PDF summary"]}'}
    if system.startswith("You turn a conversation"):
        return {"delay": 2.0, "text": AGENT_PROMPT}
    if not req.get("tools"):
        return {"delay": 0.2, "text": "{}"}  # the notes Cedar takes after a run: nothing to add
    if "household spending" in p:
        if n == 0:
            return {"delay": 3.0, "text": "I'll read the file and total it by month and category.",
                    "calls": [("run_python", {"code": ANALYSIS})]}
        if n == 1:
            return {"delay": 4.0, "text": "Rent is half of the spending. Dining out stands out in summer. Here is the chart.",
                    "calls": [("generate_chart", {
                        "path": "spending-by-month.png", "kind": "bar", "stacked": True,
                        "title": "Household spending by month, 2026", "y_label": "EUR",
                        "labels": MONTHS,
                        "series": [{"name": k, "values": v} for k, v in SERIES.items()]})]}
        if n == 2:
            return {"delay": 4.0, "text": "Now the one-page summary.",
                    "calls": [("generate_pdf", {"path": "spending-summary.pdf", "content": REPORT,
                                                "title": "Household spending 2026"})]}
        return {"delay": 3.5, "text": SUMMARY}
    if "delete" in p:
        if n == 0:
            return {"delay": 1.5, "text": "I'll remove the original file from the workspace.",
                    "calls": [("bash", {"command": "rm attachments/household-spending-2026.csv && ls -R"})]}
        return {"delay": 1.2, "text": "Done. The original CSV is gone from the workspace. The summary PDF and the chart are still there."}
    if "remember" in p:
        if n == 0:
            return {"delay": 1.2, "calls": [("remember", {
                "text": "Wants to keep household spending under 2,800 euros a month", "kind": "preference"})]}
        return {"delay": 1.0, "text": "Noted. I'll measure future months against **€2,800**. This year you were over it in eight months out of ten."}
    return {"delay": 1.0, "text": "I'm the scripted model of this demo, and this prompt is not in my script."}
