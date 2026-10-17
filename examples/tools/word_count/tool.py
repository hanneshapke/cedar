#!/usr/bin/env python3
import json
import sys

args = json.load(sys.stdin)
try:
    text = open(args["path"], encoding="utf-8", errors="replace").read()
except OSError as e:
    sys.exit(str(e))
print(json.dumps({"lines": text.count("\n"), "words": len(text.split()), "characters": len(text)}))
