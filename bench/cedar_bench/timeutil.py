"""Dates in fixtures and tasks are written relative to an anchor day, so "next week" in a task
means the same thing whenever the benchmark runs.

cedar's Calendar tools read the real clock, so the anchor defaults to today; pass a fixed one
(`--anchor 2026-11-02`) to replay a run exactly when the clock doesn't matter (web, documents).

Tokens, written inside `{{...}}`:

    today, today+3, today-2      the anchor day, shifted by days
    nextmon, nextmon+1           the Monday after the anchor (never the anchor itself), shifted
    sat, sun                     the coming Saturday / Sunday (the anchor itself if it is one)

A token becomes an ISO date (`2026-11-03`); add the time after it: `{{nextmon+1}}T14:00`.
"""

from __future__ import annotations

import datetime as dt
import re
from typing import Any
from zoneinfo import ZoneInfo

TOKEN = re.compile(r"\{\{\s*(today|nextmon|sat|sun)\s*(?:([+-])\s*(\d+))?\s*\}\}")


def parse_anchor(value: str | None) -> dt.date:
    return dt.date.fromisoformat(value) if value else dt.date.today()


def _base(name: str, anchor: dt.date) -> dt.date:
    wd = anchor.weekday()  # Monday is 0
    if name == "today":
        return anchor
    if name == "nextmon":
        return anchor + dt.timedelta(days=7 - wd)
    if name == "sat":
        return anchor + dt.timedelta(days=(5 - wd) % 7)
    return anchor + dt.timedelta(days=(6 - wd) % 7)


def resolve_str(text: str, anchor: dt.date) -> str:
    def sub(m: re.Match) -> str:
        day = _base(m.group(1), anchor)
        if m.group(3):
            n = int(m.group(3))
            day += dt.timedelta(days=n if m.group(2) == "+" else -n)
        return day.isoformat()

    return TOKEN.sub(sub, text)


def resolve(value: Any, anchor: dt.date) -> Any:
    """Resolves every token in a YAML value: strings, and the strings in lists and mappings."""
    if isinstance(value, str):
        return resolve_str(value, anchor)
    if isinstance(value, list):
        return [resolve(v, anchor) for v in value]
    if isinstance(value, dict):
        return {k: resolve(v, anchor) for k, v in value.items()}
    return value


def local_datetime(text: str, zone: str) -> dt.datetime:
    """`2026-11-03T14:00` (already resolved) in the persona's time zone, as an aware datetime."""
    value = dt.datetime.fromisoformat(text)
    if value.tzinfo is None:
        value = value.replace(tzinfo=ZoneInfo(zone))
    return value


def parse_rfc3339(text: str) -> dt.datetime:
    return dt.datetime.fromisoformat(text.replace("Z", "+00:00"))
