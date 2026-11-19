#!/usr/bin/env python3
"""ticketmaster_events: a community cedar tool. Reads its input as JSON on stdin and prints the result.

Searches live events (concerts, sports, theatre, comedy, family shows, ...) through Ticketmaster's
Discovery API v2, or reads one event's details. Needs a free Consumer Key from
developer.ticketmaster.com in TICKETMASTER_API_KEY. The Discovery API is read-only: it can't buy
tickets or see seats, so every event carries its Ticketmaster link.

Exit non-zero with a message on stderr to report an error; the model sees it as the tool's
error and can try again differently.
"""
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from datetime import date, datetime, timedelta, timezone

API = "https://app.ticketmaster.com/discovery/v2"
USER_AGENT = ("cedar-ticketmaster_events/1.0 "
              "(+https://github.com/hanneshapke/cedar/tree/main/registry/tools/ticketmaster_events)")
# Ticketmaster's segment ids (the top of its classification tree).
SEGMENTS = {
    "music": "KZFzniwnSyZfZ7v7nJ",
    "sports": "KZFzniwnSyZfZ7v7nE",
    "arts_theatre": "KZFzniwnSyZfZ7v7na",
    "film": "KZFzniwnSyZfZ7v7nn",
    "miscellaneous": "KZFzniwnSyZfZ7v7n1",
}
SORTS = {"date": "date,asc", "relevance": "relevance,desc", "distance": "distance,asc"}
# Countries that give distances in miles.
MILES = {"US", "GB", "LR", "MM"}
# The API pages no deeper than this many results.
MAX_DEPTH = 1000
MAX_RESULTS = 50
MAX_PERFORMERS = 6
MAX_TEXT = 600
EVENT_ID = re.compile(r"^[A-Za-z0-9_-]{1,64}$")
GEOHASH = "0123456789bcdefghjkmnpqrstuvwxyz"


def text(value, limit=MAX_TEXT):
    """A field from the API as one trimmed line, or None."""
    if not isinstance(value, str):
        return None
    value = " ".join(value.split())
    if len(value) > limit:
        value = value[:limit].rsplit(" ", 1)[0] + " …"
    return value or None


def whole(value, default, low, high):
    if value is None:
        return default
    try:
        return max(low, min(high, int(value)))
    except (TypeError, ValueError):
        sys.exit(f"expected a whole number, not {value!r}")


def coordinate(args, key, limit):
    value = args.get(key)
    if value is None:
        return None
    try:
        value = float(value)
    except (TypeError, ValueError):
        sys.exit(f"{key} must be a number")
    if not -limit <= value <= limit:
        sys.exit(f"{key} must be between -{limit} and {limit}")
    return value


def day(args, key):
    value = args.get(key)
    if value in (None, ""):
        return None
    try:
        return date.fromisoformat(str(value).strip()[:10])
    except ValueError:
        sys.exit(f"{key} must be a date as YYYY-MM-DD, not {value!r}")


def code(args, key, length):
    value = (args.get(key) or "").strip().upper()
    if value and not re.fullmatch(r"[A-Z]{%d}" % length, value):
        sys.exit(f"{key} must be a {length}-letter code, not {args.get(key)!r}")
    return value or None


def geohash(lat, lon, precision=9):
    """The geohash Ticketmaster's geoPoint takes (latlong is deprecated)."""
    lat_range, lon_range = [-90.0, 90.0], [-180.0, 180.0]
    out, bits, ch, even = [], 0, 0, True
    while len(out) < precision:
        rng, value = (lon_range, lon) if even else (lat_range, lat)
        mid = (rng[0] + rng[1]) / 2
        ch <<= 1
        if value >= mid:
            ch |= 1
            rng[0] = mid
        else:
            rng[1] = mid
        even = not even
        bits += 1
        if bits == 5:
            out.append(GEOHASH[ch])
            bits, ch = 0, 0
    return "".join(out)


def default_unit(country):
    """Miles for the US and the UK, km elsewhere; with no country, the run's locale decides."""
    if not country:
        locale = (os.environ.get("CEDAR_LOCALE") or "").replace("_", "-").split("-")
        country = locale[1].upper() if len(locale) > 1 else "US"
    return "miles" if country in MILES else "km"


# --- the API -----------------------------------------------------------------------------------

def api_error(e):
    try:
        body = json.load(e)
    except (ValueError, OSError):
        body = {}
    detail = ((body.get("fault") or {}).get("faultstring")
              or next((x.get("detail") for x in body.get("errors") or [] if isinstance(x, dict)), None)
              or e.reason)
    if e.code in (401, 403):
        return (f"Ticketmaster refused TICKETMASTER_API_KEY ({detail}): an admin checks the key "
                "(the Consumer Key of an app on developer.ticketmaster.com) in this tool's settings")
    if e.code == 429:
        reset = e.headers.get("Rate-Limit-Reset") if e.headers else None
        when = ""
        if reset and reset.isdigit():
            at = datetime.fromtimestamp(int(reset) / 1000, timezone.utc)
            when = f" until {at:%Y-%m-%d %H:%M} UTC"
        return f"Ticketmaster's rate limit for this key is used up{when} (5 calls a second, 5,000 a day)"
    return f"Ticketmaster answered {e.code}: {detail}"


def get(path, params, key):
    query = urllib.parse.urlencode({**params, "apikey": key, "locale": "*"})
    req = urllib.request.Request(f"{API}/{path}?{query}",
                                 headers={"User-Agent": USER_AGENT, "Accept": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            return json.load(resp)
    except urllib.error.HTTPError as e:
        if e.code == 404 and path.startswith("events/"):
            sys.exit("Ticketmaster has no event with that id; search again for the event's current id")
        sys.exit(api_error(e))
    except (urllib.error.URLError, TimeoutError) as e:
        sys.exit(f"couldn't reach Ticketmaster ({getattr(e, 'reason', e)}); try again in a little while")
    except ValueError:
        sys.exit("Ticketmaster sent an answer that isn't JSON; try again in a little while")


# --- reading events ----------------------------------------------------------------------------

def named(value):
    name = (value or {}).get("name") if isinstance(value, dict) else None
    return name if name and name.lower() != "undefined" else None


def image(images):
    """A 16:9 picture about 640 pixels wide, else the widest one."""
    images = [i for i in images or [] if isinstance(i, dict) and i.get("url")]
    if not images:
        return None
    wide = [i for i in images if i.get("ratio") == "16_9"] or images
    return min(wide, key=lambda i: abs((i.get("width") or 0) - 640))["url"]


def price(ranges):
    out = []
    for r in ranges or []:
        lo, hi, cur = r.get("min"), r.get("max"), r.get("currency") or ""
        if lo is None and hi is None:
            continue
        span = f"{lo:g}" if hi in (None, lo) else f"{lo:g}–{hi:g}" if lo is not None else f"up to {hi:g}"
        kind = r.get("type")
        out.append(f"{span} {cur}".strip() + (f" ({kind})" if kind and kind != "standard" else ""))
    return out


def venue(ev, details):
    venues = (ev.get("_embedded") or {}).get("venues") or []
    if not venues:
        place = ev.get("place") or {}
        return {"name": text(place.get("name"))} if place.get("name") else None
    v = venues[0]
    out = {
        "name": text(v.get("name")),
        "city": named(v.get("city")),
        "state": (v.get("state") or {}).get("stateCode"),
        "country": (v.get("country") or {}).get("countryCode"),
    }
    if details:
        addr = v.get("address") or {}
        out["address"] = ", ".join(x for x in (addr.get("line1"), addr.get("line2"), named(v.get("city")),
                                               v.get("postalCode")) if x) or None
        loc = v.get("location") or {}
        if loc.get("latitude") and loc.get("longitude"):
            out["latitude"], out["longitude"] = float(loc["latitude"]), float(loc["longitude"])
        out["parking"] = text(v.get("parkingDetail"), 300)
        out["accessibility"] = text((v.get("accessibleSeatingDetail")), 300)
        out["url"] = v.get("url")
    return {k: x for k, x in out.items() if x is not None}


def event(ev, details=False):
    dates = ev.get("dates") or {}
    start = dates.get("start") or {}
    cls = next((c for c in ev.get("classifications") or [] if c.get("primary")), None) \
        or next(iter(ev.get("classifications") or []), {})
    sales = ev.get("sales") or {}
    public = sales.get("public") or {}
    when = start.get("localTime")
    out = {
        "id": ev.get("id"),
        "name": text(ev.get("name"), 200),
        "date": start.get("localDate"),
        "time": when[:5] if when and not start.get("noSpecificTime") else None,
        "date_to_be_announced": True if start.get("dateTBA") or start.get("dateTBD") else None,
        "time_to_be_announced": True if start.get("timeTBA") else None,
        "timezone": dates.get("timezone"),
        "status": (dates.get("status") or {}).get("code"),
        "segment": named(cls.get("segment")),
        "genre": named(cls.get("genre")),
        "subgenre": named(cls.get("subGenre")),
        "venue": venue(ev, details),
        "distance": f"{ev['distance']:g} {ev.get('units', '').lower()}".strip()
        if isinstance(ev.get("distance"), (int, float)) else None,
        "performers": [n for n in (named(a) for a in ((ev.get("_embedded") or {}).get("attractions") or []))
                       if n][:MAX_PERFORMERS] or None,
        "price": price(ev.get("priceRanges")) or None,
        "on_sale_from": None if public.get("startTBD") else public.get("startDateTime"),
        "on_sale_until": public.get("endDateTime") if details else None,
        "image": image(ev.get("images")),
        "url": ev.get("url"),
    }
    if out["status"] == "onsale" and out["on_sale_from"]:
        try:  # already on sale: when it started doesn't matter
            if datetime.fromisoformat(out["on_sale_from"].replace("Z", "+00:00")) <= datetime.now(timezone.utc):
                out["on_sale_from"] = None
        except ValueError:
            pass
    if details:
        out["presales"] = [{"name": text(p.get("name"), 120), "from": p.get("startDateTime"),
                            "until": p.get("endDateTime")}
                           for p in sales.get("presales") or []] or None
        out["info"] = text(ev.get("info"))
        out["please_note"] = text(ev.get("pleaseNote"))
        out["ticket_limit"] = text((ev.get("ticketLimit") or {}).get("info"), 300)
        out["age_restriction"] = text((ev.get("ageRestrictions") or {}).get("ageRuleDescription"), 200) \
            or (True if (ev.get("ageRestrictions") or {}).get("legalAgeEnforced") else None)
        out["promoter"] = named(ev.get("promoter"))
        out["seatmap"] = (ev.get("seatmap") or {}).get("staticUrl")
        doors = (ev.get("doorsTimes") or {}).get("localTime")
        out["doors"] = doors[:5] if doors else None
        if not out["price"]:
            out["price_note"] = "Ticketmaster lists no price for this event; the link shows current prices"
    return {k: v for k, v in out.items() if v is not None}


def dedupe(events):
    """The same show is often listed more than once (packages, platinum, resale): keep the first."""
    seen, out = {}, []
    for e in events:
        key = ((e.get("name") or "").lower(), e.get("date"), e.get("time"), (e.get("venue") or {}).get("name"))
        if key in seen:
            seen[key]["other_listings"] = seen[key].get("other_listings", 0) + 1
            continue
        seen[key] = e
        out.append(e)
    return out


# --- the two calls -----------------------------------------------------------------------------

def details(event_id, key):
    if not EVENT_ID.fullmatch(event_id):
        sys.exit("event_id is the id of an event from an earlier search, such as vvG1fZ9pVAnaWE")
    ev = get(f"events/{event_id}.json", {}, key)
    return {"source": "Ticketmaster Discovery API", "event": event(ev, details=True)}


def search(args, key):
    params, notes = {}, []
    keyword = text(args.get("keyword"), 200)
    if keyword:
        params["keyword"] = keyword
    city = text(args.get("city"), 100)
    if city:
        params["city"] = city
    state, country = code(args, "state_code", 2), code(args, "country_code", 2)
    if state:
        params["stateCode"] = state
    if country:
        params["countryCode"] = country
    postal = text(args.get("postal_code"), 20)
    if postal:
        params["postalCode"] = postal

    lat, lon = coordinate(args, "latitude", 90), coordinate(args, "longitude", 180)
    if (lat is None) != (lon is None):
        sys.exit("latitude and longitude go together")
    if lat is not None:
        params["geoPoint"] = geohash(lat, lon)
    if lat is not None or postal:
        params["radius"] = whole(args.get("radius"), 25, 1, 500)
        params["unit"] = args.get("unit") if args.get("unit") in ("miles", "km") else default_unit(country)
    elif args.get("radius") is not None:
        notes.append("radius only applies around latitude/longitude or a postal code, so it was left out")

    start, end = day(args, "start_date"), day(args, "end_date")
    if end and not start:
        start = date.today()
    if start and end and end < start:
        sys.exit("end_date is before start_date")
    if start:
        end = end or start + timedelta(days=365)
        params["localStartDateTime"] = f"{start}T00:00:00,{end}T23:59:59"

    segment = args.get("segment")
    if segment in SEGMENTS:
        params["segmentId"] = SEGMENTS[segment]
    elif segment not in (None, "", "family"):
        sys.exit(f"segment is one of {', '.join([*SEGMENTS, 'family'])}")
    genre = text(args.get("genre"), 100)
    if genre:
        params["classificationName"] = genre
    if segment == "family":
        params["includeFamily"] = "only"

    sort = args.get("sort") or "date"
    if sort not in SORTS:
        sys.exit("sort is date, relevance or distance")
    if sort == "distance" and lat is None:
        notes.append("sorting by distance needs latitude and longitude, so the events are sorted by date")
        sort = "date"
    if sort == "relevance" and not keyword:
        sort = "date"
    params["sort"] = SORTS[sort]

    size = whole(args.get("max_results"), 15, 1, MAX_RESULTS)
    page = whole(args.get("page"), 0, 0, MAX_DEPTH)
    if (page + 1) * size > MAX_DEPTH:
        sys.exit(f"Ticketmaster shows only the first {MAX_DEPTH} results of a search; narrow it instead of paging further")
    params["size"] = size
    params["page"] = page

    if not (keyword or city or state or country or postal or lat is not None or genre or segment):
        notes.append("no place or keyword was given, so these are events anywhere; pass a city, "
                     "postal_code or latitude/longitude for events near the user")

    data = get("events.json", params, key)
    events = dedupe([event(e) for e in (data.get("_embedded") or {}).get("events") or []])
    info = data.get("page") or {}
    total = info.get("totalElements", len(events))
    out = {
        "source": "Ticketmaster Discovery API",
        "search": {k: v for k, v in params.items() if k not in ("size", "page")},
        "total": total,
        "page": page,
        "events": events,
    }
    if info.get("totalPages", 1) > page + 1 and (page + 2) * size <= MAX_DEPTH:
        out["more"] = f"{total} events match; page {page + 1} shows more"
    if not events:
        notes.append("no events matched; try other dates, a wider radius, a nearby bigger city, "
                     "or fewer filters (a keyword must match an event, performer or venue name)")
    if notes:
        out["notes"] = notes
    return out


def main():
    args = json.load(sys.stdin) or {}
    key = (os.environ.get("TICKETMASTER_API_KEY") or "").strip()
    if not key:
        sys.exit("TICKETMASTER_API_KEY isn't set: an admin saves the Consumer Key of an app on "
                 "developer.ticketmaster.com in this tool's settings")
    event_id = (args.get("event_id") or "").strip()
    result = details(event_id, key) if event_id else search(args, key)
    print(json.dumps(result, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    main()
