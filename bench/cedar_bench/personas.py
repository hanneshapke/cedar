"""Synthetic personas: a fictional person's inbox, calendar, contacts and preferences.

A persona file holds the hand-written "plot" (the threads and events tasks are about) and a
`filler` section that `expand` turns into background mail and meetings, deterministically from its
seed, so an inbox looks lived in without anyone writing 40 newsletters. Nothing here is real data,
which is what makes the dataset safe to publish.

    python -m cedar_bench persona new --id theo --name "Theo Marchetti" --seed 11
writes a starting persona to fill in.
"""

from __future__ import annotations

import datetime as dt
import random
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import yaml

from . import timeutil

ME = "me"
# Recurring plot events are expanded over this many days around the anchor.
FILLER_DAYS = (-14, 28)
# Background events only fill the past: the calendar from today on is all plot, so a question like
# "when are we both free next week" has the same answer whatever weekday the bench runs on.
FILLER_EVENT_DAYS = (-21, -1)
WORK_HOURS = (9, 17)


@dataclass
class Message:
    id: str
    thread_id: str
    sender: str  # "Name <email>"
    to: list[str]
    cc: list[str]
    subject: str
    body: str
    date: dt.datetime
    labels: list[str]
    attachments: list[dict] = field(default_factory=list)
    headers: dict[str, str] = field(default_factory=dict)


@dataclass
class Event:
    id: str
    summary: str
    start: dt.datetime | dt.date
    end: dt.datetime | dt.date
    attendees: list[dict]
    organizer: str
    location: str | None = None
    description: str | None = None
    calendar: str = "primary"


@dataclass
class Persona:
    id: str
    name: str
    email: str
    timezone: str
    profile: dict
    contacts: list[dict]
    labels: list[str]
    messages: list[Message]
    events: list[Event]
    # Other people's busy blocks, by email, for freeBusy.
    busy: dict[str, list[tuple[dt.datetime, dt.datetime]]]

    def address(self, who: str) -> str:
        """`Name <email>` for "me", a contact's email or id, or an address as given."""
        if who == ME:
            return f"{self.name} <{self.email}>"
        for c in self.contacts:
            if who in (c["email"], c.get("id")):
                return f"{c['name']} <{c['email']}>"
        return who

    def email_of(self, who: str) -> str:
        if who == ME:
            return self.email
        for c in self.contacts:
            if who in (c["email"], c.get("id")):
                return c["email"]
        return who


def load(path: Path, anchor: dt.date) -> Persona:
    raw = yaml.safe_load(path.read_text())
    return expand(raw, anchor)


def find(personas_dir: Path, persona_id: str) -> Path:
    path = personas_dir / persona_id / "persona.yaml"
    if not path.exists():
        raise FileNotFoundError(f"no persona {persona_id!r} at {path}")
    return path


def expand(raw: dict, anchor: dt.date) -> Persona:
    raw = timeutil.resolve(raw, anchor)
    zone = raw["timezone"]
    contacts = raw.get("contacts", [])
    persona = Persona(
        id=raw["id"],
        name=raw["name"],
        email=raw["email"],
        timezone=zone,
        profile=raw.get("profile", {}),
        contacts=contacts,
        labels=raw.get("labels", []),
        messages=[],
        events=[],
        busy={},
    )
    for c in contacts:
        persona.busy[c["email"]] = [
            (timeutil.local_datetime(b["start"], zone), timeutil.local_datetime(b["end"], zone))
            for b in c.get("busy", [])
        ]

    for t in raw.get("threads", []):
        for i, m in enumerate(t["messages"]):
            persona.messages.append(
                Message(
                    id=m.get("id", f"{t['id']}-{i + 1}"),
                    thread_id=t["id"],
                    sender=persona.address(m.get("from", ME)),
                    to=[persona.address(x) for x in m.get("to", [ME])],
                    cc=[persona.address(x) for x in m.get("cc", [])],
                    subject=m.get("subject") or t.get("subject", ""),
                    body=m.get("body", ""),
                    date=timeutil.local_datetime(m["date"], zone),
                    labels=list(m.get("labels", t.get("labels", ["INBOX"]))),
                    attachments=m.get("attachments", []),
                )
            )

    for e in raw.get("events", []):
        persona.events.extend(_events(persona, e, anchor))

    filler = raw.get("filler")
    if filler:
        _add_filler(persona, filler, anchor)
    persona.messages.sort(key=lambda m: m.date)
    persona.events.sort(key=lambda e: _sort_key(e.start, zone))
    return persona


def _sort_key(value: dt.datetime | dt.date, zone: str) -> dt.datetime:
    if isinstance(value, dt.datetime):
        return value
    return timeutil.local_datetime(value.isoformat() + "T00:00", zone)


def _when(text: str, zone: str) -> dt.datetime | dt.date:
    """A date alone is an all-day event's day; anything with a time is local to the persona."""
    if "T" in text:
        return timeutil.local_datetime(text, zone)
    return dt.date.fromisoformat(text)


def _events(persona: Persona, e: dict, anchor: dt.date) -> list[Event]:
    zone = persona.timezone
    attendees = [
        {"email": persona.email_of(a), "self": a == ME or persona.email_of(a) == persona.email}
        for a in e.get("attendees", [ME])
    ]
    for a in attendees:
        a["responseStatus"] = "accepted"
        name = next((c["name"] for c in persona.contacts if c["email"] == a["email"]), None)
        if name:
            a["displayName"] = name
    start, end = _when(e["start"], zone), _when(e["end"], zone)
    base = dict(
        summary=e["summary"],
        attendees=attendees,
        organizer=persona.email_of(e.get("organizer", ME)),
        location=e.get("location"),
        description=e.get("description"),
        calendar=e.get("calendar", "primary"),
    )
    repeat = e.get("repeat")
    if not repeat:
        return [Event(id=e["id"], start=start, end=end, **base)]
    # Recurring events are expanded into single instances (cedar asks with singleEvents=true).
    out = []
    lo = anchor + dt.timedelta(days=FILLER_DAYS[0])
    hi = anchor + dt.timedelta(days=FILLER_DAYS[1])
    step = dt.timedelta(days=7 if repeat == "weekly" else 1)
    day_shift = dt.timedelta(0)
    while True:
        s, en = start + day_shift, end + day_shift
        day = s.date() if isinstance(s, dt.datetime) else s
        if day > hi:
            break
        if day >= lo and not (repeat == "weekdays" and day.weekday() >= 5):
            out.append(Event(id=f"{e['id']}_{day:%Y%m%d}", start=s, end=en, **base))
        day_shift += step
    return out


# -- filler -----------------------------------------------------------------------------------

FILLER_SENDERS = [
    ("Pinecone Coffee Co.", "hello@pinecone-coffee.example", "Your weekly roast is on its way",
     "Thanks for your order! Your bag of {thing} ships today and should arrive in 2-3 days."),
    ("Ledgerly", "receipts@ledgerly.example", "Receipt for your payment",
     "We received your payment of ${amount}.00 for {thing}. No action is needed."),
    ("Trailhead Weekly", "newsletter@trailhead-weekly.example", "This week: {thing}",
     "Five stories about {thing}, a reader's photo of the week, and our gear picks."),
    ("Civic Library", "notices@civic-library.example", "Your hold is ready",
     "The item you placed on hold, \"{thing}\", is ready for pickup for the next 7 days."),
    ("Orbit Cloud", "no-reply@orbit-cloud.example", "Usage report",
     "Your workspace used {amount} GB of storage this month. You are on the Team plan."),
    ("Harbor Dental", "appointments@harbor-dental.example", "Appointment reminder",
     "This is a reminder of your cleaning. Reply C to confirm or call us to reschedule."),
]
FILLER_THINGS = ["single-origin beans", "trail running", "The Quiet Orchard", "fall hiking",
                 "invoice #4471", "a ceramic pour-over", "urban gardening", "monthly plan"]
FILLER_EVENTS = [("Focus block", 120), ("Gym", 60), ("1:1 sync", 30), ("Lunch", 60),
                 ("Planning", 45), ("Coffee chat", 30), ("Design review", 60)]


def _add_filler(persona: Persona, filler: dict, anchor: dt.date) -> None:
    # Placed by offsets from the anchor, so filler sits the same way around "today" on every run.
    rng = random.Random(f"{filler.get('seed', 0)}:{persona.id}")
    zone = persona.timezone
    for i in range(filler.get("threads", 0)):
        name, addr, subject, body = rng.choice(FILLER_SENDERS)
        thing = rng.choice(FILLER_THINGS)
        amount = rng.randint(12, 240)
        days_ago = rng.randint(0, 30)
        hour, minute = rng.randint(6, 21), rng.choice([0, 7, 15, 22, 34, 41, 50])
        date = timeutil.local_datetime(
            f"{anchor - dt.timedelta(days=days_ago)}T{hour:02d}:{minute:02d}", zone
        )
        labels = ["INBOX", "CATEGORY_PROMOTIONS" if "newsletter" in addr else "CATEGORY_UPDATES"]
        if days_ago <= 2 and rng.random() < 0.5:
            labels.append("UNREAD")
        persona.messages.append(
            Message(
                id=f"f{i:03d}", thread_id=f"filler-{i:03d}",
                sender=f"{name} <{addr}>", to=[persona.address(ME)], cc=[],
                subject=subject.format(thing=thing),
                body=body.format(thing=thing, amount=amount), date=date, labels=labels,
            )
        )
    for i in range(filler.get("events", 0)):
        summary, minutes = rng.choice(FILLER_EVENTS)
        day = anchor + dt.timedelta(days=rng.randint(*FILLER_EVENT_DAYS))
        if day.weekday() >= 5:
            day -= dt.timedelta(days=day.weekday() - 4)
        hour = rng.randint(WORK_HOURS[0], WORK_HOURS[1] - 2)
        start = timeutil.local_datetime(f"{day}T{hour:02d}:{rng.choice([0, 30]):02d}", zone)
        persona.events.append(
            Event(
                id=f"filler{i:03d}", summary=summary, start=start,
                end=start + dt.timedelta(minutes=minutes),
                attendees=[{"email": persona.email, "self": True, "responseStatus": "accepted"}],
                organizer=persona.email,
            )
        )


# -- generator --------------------------------------------------------------------------------

def new_persona(persona_id: str, name: str, seed: int, timezone: str) -> dict:
    """A starting persona: contacts and a sample thread and event to replace with the plot your
    tasks need. Names and domains are fictional (`.example` is reserved and never resolves)."""
    rng = random.Random(seed)
    first = name.split()[0].lower()
    domain = f"{name.split()[-1].lower()}.example"
    pool = ["Dana Whitfield", "Sam Rivera", "Priya Natarajan", "Jonas Berg", "Lena Okafor",
            "Marco Silva", "Aiko Tanaka", "Ruth Abebe", "Felix Moreau", "Grace Kim"]
    contacts = []
    for full in rng.sample(pool, 5):
        cid = full.split()[0].lower()
        contacts.append({"id": cid, "name": full,
                         "email": f"{cid}@{rng.choice(['northwind', 'fabrikam', 'contoso'])}.example",
                         "relation": rng.choice(["colleague", "client", "friend"]), "busy": []})
    return {
        "id": persona_id,
        "name": name,
        "email": f"{first}@{domain}",
        "timezone": timezone,
        "profile": {"role": "TODO", "preferences": ["TODO: preferences a task may rely on"]},
        "labels": ["Clients", "Receipts"],
        "contacts": contacts,
        "threads": [{
            "id": "t-example", "labels": ["INBOX", "UNREAD"],
            "messages": [{"from": contacts[0]["id"], "to": [ME], "date": "{{today-1}}T10:30",
                          "subject": "TODO", "body": "TODO"}],
        }],
        "events": [{"id": "e-example", "summary": "TODO", "start": "{{nextmon}}T10:00",
                    "end": "{{nextmon}}T10:30", "attendees": [ME, contacts[0]["id"]]}],
        "filler": {"seed": seed, "threads": 30, "events": 20},
    }


def write_new(personas_dir: Path, persona_id: str, name: str, seed: int, timezone: str) -> Path:
    path = personas_dir / persona_id / "persona.yaml"
    if path.exists():
        raise FileExistsError(path)
    path.parent.mkdir(parents=True)
    path.write_text(yaml.safe_dump(new_persona(persona_id, name, seed, timezone),
                                   sort_keys=False, allow_unicode=True))
    return path


def summary(persona: Persona) -> dict[str, Any]:
    return {"id": persona.id, "messages": len(persona.messages), "events": len(persona.events),
            "contacts": len(persona.contacts)}
