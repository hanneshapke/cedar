"""The Gmail and Calendar REST APIs (what cedar calls with `GOOGLE_BACKEND=api`) over a persona.

Only what cedar's `gmail_*` and `calendar_*` tools use is served (see src/tools/gmail.rs and
src/tools/calendar.rs). Drafts the agent creates are kept, with their MIME parsed, so a task can
check who a draft went to and what it said.
"""

from __future__ import annotations

import base64
import datetime as dt
import email
import email.policy
import email.utils
import re
import shlex
from typing import Any
from zoneinfo import ZoneInfo

from . import timeutil
from .personas import Event, Message, Persona


def b64url(text: str) -> str:
    return base64.urlsafe_b64encode(text.encode()).decode().rstrip("=")


class NotFound(Exception):
    pass


class GoogleMock:
    def __init__(self, persona: Persona):
        self.persona = persona
        self.drafts: list[dict] = []

    # -- Gmail ---------------------------------------------------------------------------------

    def gmail_get(self, parts: list[str], query: dict[str, str]) -> dict:
        match parts:
            case ["threads"]:
                return self._list_threads(query)
            case ["threads", tid]:
                msgs = [m for m in self.persona.messages if m.thread_id == tid]
                if not msgs:
                    raise NotFound(tid)
                return {"id": tid, "messages": [self._message(m) for m in msgs]}
            case ["messages", mid]:
                for m in self.persona.messages:
                    if m.id == mid:
                        return self._message(m)
                raise NotFound(mid)
            case ["labels"]:
                system = ["INBOX", "SENT", "DRAFT", "UNREAD", "STARRED", "IMPORTANT", "TRASH",
                          "SPAM", "CATEGORY_PROMOTIONS", "CATEGORY_UPDATES"]
                return {"labels": [{"id": s, "name": s, "type": "system"} for s in system]
                        + [{"id": f"Label_{i}", "name": n, "type": "user"}
                           for i, n in enumerate(self.persona.labels, 1)]}
            case ["drafts"]:
                return {"drafts": [{"id": d["id"], "message": {"id": d["message_id"]}}
                                   for d in reversed(self.drafts)],
                        "resultSizeEstimate": len(self.drafts)}
            case ["drafts", did]:
                for d in self.drafts:
                    if d["id"] == did:
                        return {"id": did, "message": self._draft_message(d)}
                raise NotFound(did)
        raise NotFound("/".join(parts))

    def gmail_create_draft(self, body: dict) -> dict:
        msg = body.get("message", {})
        raw = msg.get("raw", "")
        parsed = email.message_from_bytes(
            base64.urlsafe_b64decode(raw + "=" * (-len(raw) % 4)), policy=email.policy.default
        )
        text = parsed.get_body(preferencelist=("plain", "html"))
        n = len(self.drafts) + 1
        draft = {
            "id": f"r-{n}",
            "message_id": f"draft-msg-{n}",
            "thread_id": msg.get("threadId") or f"draft-thread-{n}",
            "to": _addresses(parsed.get_all("To", [])),
            "cc": _addresses(parsed.get_all("Cc", [])),
            "bcc": _addresses(parsed.get_all("Bcc", [])),
            "subject": str(parsed.get("Subject", "")),
            "in_reply_to": str(parsed.get("In-Reply-To", "")) or None,
            "body": text.get_content() if text else "",
        }
        self.drafts.append(draft)
        return {"id": draft["id"],
                "message": {"id": draft["message_id"], "threadId": draft["thread_id"]}}

    def _list_threads(self, query: dict[str, str]) -> dict:
        q = query.get("q", "")
        include_trash = query.get("includeSpamTrash") == "true"
        hits = [m for m in self.persona.messages
                if (include_trash or not {"TRASH", "SPAM"} & set(m.labels)) and matches(m, q, self)]
        latest: dict[str, dt.datetime] = {}
        for m in hits:
            latest[m.thread_id] = max(latest.get(m.thread_id, m.date), m.date)
        order = sorted(latest, key=latest.get, reverse=True)
        size = int(query.get("maxResults", 10))
        start = int(query.get("pageToken") or 0)
        page = order[start:start + size]
        out: dict[str, Any] = {"threads": [{"id": t} for t in page],
                               "resultSizeEstimate": len(order)}
        if start + size < len(order):
            out["nextPageToken"] = str(start + size)
        return out

    def _message(self, m: Message) -> dict:
        headers = {"From": m.sender, "To": ", ".join(m.to), "Subject": m.subject,
                   "Date": email.utils.format_datetime(m.date),
                   "Message-ID": f"<{m.id}@mail.cedar-bench.example>", **m.headers}
        if m.cc:
            headers["Cc"] = ", ".join(m.cc)
        text_part = {"mimeType": "text/plain", "body": {"data": b64url(m.body), "size": len(m.body)}}
        if m.attachments:
            payload = {"mimeType": "multipart/mixed", "parts": [text_part] + [
                {"mimeType": a.get("mime_type", "application/octet-stream"),
                 "filename": a["filename"], "body": {"size": a.get("size", 1024),
                                                     "attachmentId": f"{m.id}-a{i}"}}
                for i, a in enumerate(m.attachments)]}
        else:
            payload = dict(text_part)
        payload["headers"] = [{"name": k, "value": v} for k, v in headers.items()]
        return {"id": m.id, "threadId": m.thread_id, "labelIds": m.labels,
                "snippet": " ".join(m.body.split())[:140],
                "internalDate": str(int(m.date.timestamp() * 1000)), "payload": payload}

    def _draft_message(self, d: dict) -> dict:
        headers = [{"name": "To", "value": ", ".join(d["to"])},
                   {"name": "Subject", "value": d["subject"]}]
        if d["cc"]:
            headers.append({"name": "Cc", "value": ", ".join(d["cc"])})
        return {"id": d["message_id"], "threadId": d["thread_id"], "labelIds": ["DRAFT"],
                "snippet": d["body"][:140],
                "payload": {"mimeType": "text/plain", "headers": headers,
                            "body": {"data": b64url(d["body"])}}}

    # -- Calendar ------------------------------------------------------------------------------

    def calendar_get(self, parts: list[str], query: dict[str, str]) -> dict:
        p = self.persona
        match parts:
            case ["users", "me", "settings", "timezone"]:
                return {"value": p.timezone}
            case ["users", "me", "calendarList"]:
                return {"items": [{"id": p.email, "summary": p.email, "primary": True,
                                   "accessRole": "owner", "timeZone": p.timezone}]}
            case ["calendars", cal, "events"]:
                return self._list_events(cal, query)
            case ["calendars", cal, "events", eid]:
                for e in p.events:
                    if e.id == eid and self._on(e, cal):
                        return self._event(e)
                raise NotFound(eid)
        raise NotFound("/".join(parts))

    def free_busy(self, body: dict) -> dict:
        lo = timeutil.parse_rfc3339(body["timeMin"])
        hi = timeutil.parse_rfc3339(body["timeMax"])
        out = {}
        for item in body.get("items", []):
            cid = item["id"]
            if cid in ("primary", self.persona.email):
                spans = [self._span(e) for e in self.persona.events if self._on(e, "primary")]
            elif cid in self.persona.busy:
                spans = self.persona.busy[cid]
            else:
                out[cid] = {"errors": [{"domain": "global", "reason": "notFound"}]}
                continue
            out[cid] = {"busy": [{"start": _utc(s), "end": _utc(e)}
                                 for s, e in sorted(spans) if s < hi and e > lo]}
        return {"kind": "calendar#freeBusy", "timeMin": body["timeMin"],
                "timeMax": body["timeMax"], "calendars": out}

    def _on(self, e: Event, cal: str) -> bool:
        return e.calendar == "primary" if cal in ("primary", self.persona.email) else e.calendar == cal

    def _span(self, e: Event) -> tuple[dt.datetime, dt.datetime]:
        zone = ZoneInfo(self.persona.timezone)

        def aware(v):
            if isinstance(v, dt.datetime):
                return v
            return dt.datetime.combine(v, dt.time(), zone)

        return aware(e.start), aware(e.end)

    def _list_events(self, cal: str, query: dict[str, str]) -> dict:
        lo = timeutil.parse_rfc3339(query["timeMin"]) if "timeMin" in query else None
        hi = timeutil.parse_rfc3339(query["timeMax"]) if "timeMax" in query else None
        words = query.get("q", "").lower().split()
        hits = []
        for e in self.persona.events:
            if not self._on(e, cal):
                continue
            s, en = self._span(e)
            if (lo and en <= lo) or (hi and s >= hi):
                continue
            text = " ".join([e.summary, e.description or "", e.location or "",
                             *[a["email"] for a in e.attendees],
                             *[a.get("displayName", "") for a in e.attendees]]).lower()
            if all(w in text for w in words):
                hits.append(e)
        size = int(query.get("maxResults", 250))
        start = int(query.get("pageToken") or 0)
        out: dict[str, Any] = {"timeZone": self.persona.timezone,
                               "items": [self._event(e) for e in hits[start:start + size]]}
        if start + size < len(hits):
            out["nextPageToken"] = str(start + size)
        return out

    def _event(self, e: Event) -> dict:
        def when(v):
            if isinstance(v, dt.datetime):
                return {"dateTime": v.isoformat(), "timeZone": self.persona.timezone}
            return {"date": v.isoformat()}

        out = {"id": e.id, "status": "confirmed", "summary": e.summary, "start": when(e.start),
               "end": when(e.end), "organizer": {"email": e.organizer},
               "attendees": e.attendees,
               "htmlLink": f"https://calendar.cedar-bench.example/event?eid={e.id}"}
        if e.location:
            out["location"] = e.location
        if e.description:
            out["description"] = e.description
        return out


def _utc(v: dt.datetime) -> str:
    return v.astimezone(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _addresses(values: list) -> list[str]:
    return [addr.lower() for _, addr in email.utils.getaddresses([str(v) for v in values]) if addr]


# -- Gmail search -----------------------------------------------------------------------------

OPERATOR = re.compile(r"^(-?)(from|to|cc|subject|is|in|label|has|newer_than|older_than|after|before):(.*)$")


def matches(m: Message, q: str, mock: GoogleMock) -> bool:
    """A useful subset of Gmail's search syntax: from/to/cc/subject/label/is/in/has, newer_than
    and older_than (d, m, y), after/before (YYYY/MM/DD), quoted phrases, and plain words (in the
    subject, body or sender). Every term must match; OR and grouping are not supported."""
    try:
        terms = shlex.split(q)
    except ValueError:
        terms = q.split()
    labels = {label.upper() for label in m.labels}
    now = dt.datetime.now(m.date.tzinfo)
    for term in terms:
        op = OPERATOR.match(term)
        if not op:
            if term.upper() == "OR":
                continue
            negate = term.startswith("-")
            word = term.lstrip("-").lower()
            hit = word in f"{m.subject}\n{m.body}\n{m.sender}".lower()
            if hit == negate:
                return False
            continue
        negate, key, value = op.group(1) == "-", op.group(2), op.group(3).strip("()\"").lower()
        if key == "from":
            hit = value in m.sender.lower()
        elif key in ("to", "cc"):
            hit = any(value in a.lower() for a in (m.to if key == "to" else m.cc))
        elif key == "subject":
            hit = value in m.subject.lower()
        elif key == "is":
            hit = {"unread": "UNREAD" in labels, "read": "UNREAD" not in labels,
                   "starred": "STARRED" in labels, "important": "IMPORTANT" in labels}.get(value, False)
        elif key in ("in", "label"):
            hit = value.upper() in labels or value.upper().replace("-", "_") in labels or (
                value == "anywhere")
        elif key == "has":
            hit = value == "attachment" and bool(m.attachments)
        elif key in ("newer_than", "older_than"):
            age = _age(value)
            hit = age is not None and ((m.date > now - age) == (key == "newer_than"))
        else:  # after / before
            try:
                day = dt.datetime.strptime(value.replace("-", "/"), "%Y/%m/%d").date()
            except ValueError:
                return False
            hit = m.date.date() >= day if key == "after" else m.date.date() < day
        if hit == negate:
            return False
    return True


def _age(value: str) -> dt.timedelta | None:
    m = re.fullmatch(r"(\d+)([dmy])", value)
    if not m:
        return None
    n = int(m.group(1))
    return dt.timedelta(days=n * {"d": 1, "m": 30, "y": 365}[m.group(2)])
