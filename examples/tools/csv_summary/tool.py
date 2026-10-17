#!/usr/bin/env python3
"""csv_summary: a personal cedar tool. Reads its input as JSON on stdin and prints the result.

Exit non-zero with a message on stderr to report an error; the model sees it as the tool's
error and can try again differently.
"""
import csv
import json
import sys
from collections import Counter

args = json.load(sys.stdin)
path = args["path"]
max_rows = int(args.get("max_rows", 100_000))
top = int(args.get("top", 3))

try:
    with open(path, newline="", encoding="utf-8", errors="replace") as f:
        reader = csv.reader(f)
        header = next(reader, None)
        if header is None:
            sys.exit(f"{path} is empty")
        rows = []
        for row in reader:
            rows.append(row)
            if len(rows) >= max_rows:
                break
except FileNotFoundError:
    sys.exit(f"no such file: {path}")

print(f"{path}: {len(rows)} rows, {len(header)} columns" + (" (truncated)" if len(rows) >= max_rows else ""))
for i, name in enumerate(header):
    values = [r[i] for r in rows if i < len(r) and r[i] != ""]
    numbers = []
    for v in values:
        try:
            numbers.append(float(v))
        except ValueError:
            break
    line = f"- {name}: {len(values)}/{len(rows)} filled, {len(set(values))} distinct"
    if values and len(numbers) == len(values):
        line += f", min {min(numbers):g}, max {max(numbers):g}, mean {sum(numbers) / len(numbers):.4g}"
    elif values:
        common = ", ".join(f"{v!r} ({n})" for v, n in Counter(values).most_common(top))
        line += f", most common: {common}"
    print(line)
