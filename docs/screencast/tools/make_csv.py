#!/usr/bin/env python3
"""Write the demo file, ten months of invented household spending, once per language of content.py
(household-spending-2026.csv and its translations). The amounts and dates are the same in each."""
import csv, os, random

from content import C, LANGS

random.seed(7)
rows = []
energy = [212, 205, 188, 150, 118, 96, 92, 95, 121, 168]
for m in range(1, 11):
    d = lambda day: f"2026-{m:02d}-{day:02d}"
    rows.append((d(1), "Rent", "Rent and service charge", 1450.00))
    rows.append((d(5), "Energy", "Electricity and gas", float(energy[m - 1])))
    for name, amt in [("Streaming video", 17.99), ("Music family plan", 16.99), ("Cloud storage", 9.99),
                      ("News subscription", 14.90), ("Gym membership", 39.00), ("Second streaming service", 13.99)]:
        rows.append((d(3 + len(name) % 20), "Subscriptions", name, amt))
    for day in (2, 6, 9, 13, 16, 20, 23, 27):
        rows.append((d(day), "Groceries", random.choice(["Supermarket", "Market stall", "Organic shop"]),
                     round(random.uniform(48, 92) * (1.08 if m >= 7 else 1.0), 2)))
    rows.append((d(1), "Transport", "Monthly transit pass", 58.00))
    for day in (11, 24):
        rows.append((d(day), "Transport", "Fuel", round(random.uniform(52, 74), 2)))
    n = 5 + (3 if m in (6, 7, 8) else 0) + (m > 4)
    for i in range(n):
        rows.append((d(3 + i * 3), "Dining out", random.choice(["Restaurant", "Cafe", "Takeaway", "Lunch"]),
                     round(random.uniform(18, 64), 2)))
    for i in range(3):
        rows.append((d(8 + i * 7), "Other", random.choice(["Pharmacy", "Household goods", "Clothing", "Gift"]),
                     round(random.uniform(15, 95), 2)))
rows.sort()
HERE = os.path.dirname(os.path.abspath(__file__))
EN_CATS = C["en"]["cats"]
for lang in LANGS:
    c = C[lang]
    cat = dict(zip(EN_CATS, c["cats"]))
    with open(os.path.join(HERE, c["csv"]), "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(c["cols"])
        w.writerows((date, cat[category], c["descs"].get(desc, desc), amount) for date, category, desc, amount in rows)
    print(c["csv"], len(rows), "rows")
