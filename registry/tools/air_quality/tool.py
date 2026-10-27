#!/usr/bin/env python3
"""air_quality: a community cedar tool. Reads its input as JSON on stdin and prints the result.

Current air quality and an hourly forecast for a place, from Open-Meteo's air-quality API (open
data from the Copernicus Atmosphere Monitoring Service, no API key): the US or European air
quality index with its main pollutant, particulate matter, ozone, nitrogen and sulphur dioxide,
carbon monoxide, dust, UV, and pollen where it is modelled (Europe). The report ends with
`[chart] {...}` lines, which the cedar chat draws as charts (the same spec as the generate_chart
tool's input).

Exit non-zero with a message on stderr to report an error; the model sees it as the tool's
error and can try again differently.
"""
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime

GEOCODE = "https://geocoding-api.open-meteo.com/v1/search"
AIR = "https://air-quality-api.open-meteo.com/v1/air-quality"
USER_AGENT = "cedar-air_quality/1.0 (+https://github.com/hanneshapke/cedar/tree/main/registry/tools/air_quality)"
POLLUTANTS = {  # Open-Meteo variable: (label, unit)
    "pm2_5": ("PM2.5", "µg/m³"), "pm10": ("PM10", "µg/m³"), "ozone": ("ozone", "µg/m³"),
    "nitrogen_dioxide": ("NO₂", "µg/m³"), "sulphur_dioxide": ("SO₂", "µg/m³"),
    "carbon_monoxide": ("CO", "µg/m³"), "dust": ("dust", "µg/m³"),
}
SUB_INDEX = {"us": ["pm2_5", "pm10", "ozone", "nitrogen_dioxide", "sulphur_dioxide", "carbon_monoxide"],
             "european": ["pm2_5", "pm10", "ozone", "nitrogen_dioxide", "sulphur_dioxide"]}
# The upper bound of each band, and its name.
BANDS = {
    "us": [(50, "good"), (100, "moderate"), (150, "unhealthy for sensitive groups"),
           (200, "unhealthy"), (300, "very unhealthy"), (float("inf"), "hazardous")],
    "european": [(20, "good"), (40, "fair"), (60, "moderate"), (80, "poor"),
                 (100, "very poor"), (float("inf"), "extremely poor")],
}
ADVICE = {
    "us": ["Air is fine for everyone outdoors.",
           "Unusually sensitive people should consider shorter or lighter outdoor exertion.",
           "People with asthma, heart or lung disease, children and older adults should cut back long or heavy outdoor exertion.",
           "Everyone should cut back long or heavy outdoor exertion; sensitive groups should avoid it.",
           "Everyone should avoid long or heavy outdoor exertion; sensitive groups should stay indoors.",
           "Everyone should avoid outdoor activity."],
    "european": ["Air is fine for everyone outdoors.",
                 "Air is fine for outdoor activity.",
                 "Sensitive people should consider cutting back intense outdoor activity.",
                 "Consider cutting back intense outdoor activity; sensitive people should avoid it.",
                 "Cut back intense outdoor activity; sensitive people should avoid outdoor activity.",
                 "Avoid outdoor activity."],
}
# Pollen levels in grains/m³, after the US National Allergy Bureau's scales.
POLLEN = {  # variable: (label, upper bounds of low, moderate, high)
    "alder_pollen": ("alder", (14, 89, 1499)), "birch_pollen": ("birch", (14, 89, 1499)),
    "olive_pollen": ("olive", (14, 89, 1499)), "grass_pollen": ("grass", (4, 19, 199)),
    "mugwort_pollen": ("mugwort", (9, 49, 499)), "ragweed_pollen": ("ragweed", (9, 49, 499)),
}
POLLEN_LEVELS = ["low", "moderate", "high", "very high"]
UV = [(2, "low"), (5, "moderate"), (7, "high"), (10, "very high"), (float("inf"), "extreme")]
# Text the user sees in the charts, by CEDAR_LANGUAGE. The report itself stays English for the model.
STRINGS = {
    "en": {"name_us": "US AQI", "name_european": "European AQI", "forecast": "{name} forecast",
           "good": "good", "pollen_title": "Daily peak pollen", "pollen_unit": "grains/m³",
           "alder": "alder", "birch": "birch", "olive": "olive", "grass": "grass", "mugwort": "mugwort",
           "ragweed": "ragweed", "day": "{a} {d:02d} {b}", "hour": "{a} {t}"},
    "de": {"name_us": "US-AQI", "name_european": "Europäischer AQI", "forecast": "{name}: Prognose",
           "good": "gut", "pollen_title": "Pollen: Tageshöchstwerte", "pollen_unit": "Pollenkörner/m³",
           "alder": "Erle", "birch": "Birke", "olive": "Olive", "grass": "Gräser", "mugwort": "Beifuß",
           "ragweed": "Ambrosia", "day": "{a}, {d}. {b}", "hour": "{a} {t}"},
    "fr": {"name_us": "IQA américain", "name_european": "IQA européen", "forecast": "Prévision de l'{name}",
           "good": "bon", "pollen_title": "Pic quotidien de pollen", "pollen_unit": "grains/m³",
           "alder": "aulne", "birch": "bouleau", "olive": "olivier", "grass": "graminées", "mugwort": "armoise",
           "ragweed": "ambroisie", "day": "{a} {d} {b}", "hour": "{a} {t}"},
}
WEEKDAYS = {"en": ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
            "de": ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"],
            "fr": ["lun.", "mar.", "mer.", "jeu.", "ven.", "sam.", "dim."]}
MONTHS = {"en": ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
          "de": ["Jan", "Feb", "Mär", "Apr", "Mai", "Jun", "Jul", "Aug", "Sep", "Okt", "Nov", "Dez"],
          "fr": ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."]}


def language():
    """The user's language from CEDAR_LANGUAGE (older servers don't set it: English)."""
    lang = (os.environ.get("CEDAR_LANGUAGE") or "").strip().lower().split("-")[0]
    return lang if lang in STRINGS else "en"


def tr(key, **values):
    """A chart string in the user's language; Swiss German (de-CH) writes ss for ß."""
    lang = language()
    text = STRINGS[lang].get(key, STRINGS["en"][key])
    if lang == "de" and (os.environ.get("CEDAR_LOCALE") or "").replace("_", "-").lower().startswith("de-ch"):
        text = text.replace("ß", "ss")
    return text.format(**values)


def day_label(d):
    lang = language()
    return tr("day", a=WEEKDAYS[lang][d.weekday()], d=d.day, b=MONTHS[lang][d.month - 1])


def hour_label(t):
    return tr("hour", a=WEEKDAYS[language()][t.weekday()], t=f"{t:%H:%M}")


def whole(value, default, low, high):
    try:
        return max(low, min(high, int(value)))
    except (TypeError, ValueError):
        return default


def get(url, params):
    full = f"{url}?{urllib.parse.urlencode(params)}"
    req = urllib.request.Request(full, headers={"User-Agent": USER_AGENT, "Accept": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            return json.load(resp)
    except urllib.error.HTTPError as e:
        try:
            reason = json.load(e).get("reason") or e.reason
        except ValueError:
            reason = e.reason
        sys.exit(f"Open-Meteo answered {e.code}: {reason}")
    except (urllib.error.URLError, TimeoutError, ValueError) as e:
        sys.exit(f"couldn't reach Open-Meteo ({e}); try again in a little while")


# --- the place -------------------------------------------------------------------------------

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


def place_label(hit):
    parts = []
    for key in ("name", "admin1", "country"):
        value = (hit.get(key) or "").strip()
        if value and value not in parts:
            parts.append(value)
    return ", ".join(parts)


def geocode(location):
    """The best match for a place name or postal code. "Springfield, Illinois" searches for
    Springfield and prefers the match whose region or country fits the rest."""
    name, *rest = [p.strip() for p in location.split(",") if p.strip()]
    hits = get(GEOCODE, {"name": name, "count": 10 if rest else 1, "language": "en",
                         "format": "json"}).get("results") or []
    if not hits:
        sys.exit(f"couldn't find {location!r}; try a nearby city, add the country, or give "
                 "latitude and longitude")
    if rest:
        wanted = [r.lower() for r in rest]

        def fits(hit):
            fields = [(hit.get(k) or "").lower() for k in ("admin1", "admin2", "country", "country_code")]
            return sum(any(w == f or (len(w) > 2 and w in f) for f in fields) for w in wanted)
        hits.sort(key=fits, reverse=True)
    hit = hits[0]
    return hit["latitude"], hit["longitude"], place_label(hit)


# --- reading the forecast ----------------------------------------------------------------------

def band(standard, aqi):
    for i, (top, name) in enumerate(BANDS[standard]):
        if aqi <= top:
            return i, name
    return len(BANDS[standard]) - 1, BANDS[standard][-1][1]


def pollen_level(var, value):
    for i, top in enumerate(POLLEN[var][1]):
        if value <= top:
            return POLLEN_LEVELS[i]
    return POLLEN_LEVELS[-1]


def uv_level(value):
    return next(name for top, name in UV if value <= top)


def main_pollutant(standard, values):
    """The pollutant whose sub-index is highest, from a {variable: value} mapping."""
    subs = {p: values.get(f"{standard}_aqi_{p}") for p in SUB_INDEX[standard]}
    subs = {p: v for p, v in subs.items() if v is not None}
    return POLLUTANTS[max(subs, key=subs.get)][0] if subs else None


def fmt(value):
    return f"{value:.0f}" if value >= 10 else f"{value:.1f}"


def hourly_rows(data):
    """The hourly forecast as one dict per hour, keyed by variable, plus 'time' (a datetime)."""
    hourly = data.get("hourly", {})
    times = hourly.get("time", [])
    rows = []
    for i, t in enumerate(times):
        row = {k: v[i] for k, v in hourly.items() if k != "time" and i < len(v)}
        row["time"] = datetime.fromisoformat(t)
        rows.append(row)
    return rows


def report(place, lat, lon, data, standard, rows):
    tz = data.get("timezone_abbreviation") or data.get("timezone") or "UTC"
    current = data.get("current", {})
    key = f"{standard}_aqi"
    name = "US AQI" if standard == "us" else "European AQI"
    out = []
    now = datetime.fromisoformat(current["time"]) if current.get("time") else None
    when = f", {now:%a %d %b %H:%M} {tz}" if now else ""
    where = f"{place} ({lat:.2f}, {lon:.2f})" if place else f"{lat:.2f}, {lon:.2f}"
    out.append(f"Air quality for {where}{when}, from Open-Meteo (CAMS):")

    aqi = current.get(key)
    if aqi is None:
        out.append(f"No current {name} for this place.")
    else:
        level, label = band(standard, aqi)
        main = main_pollutant(standard, current)
        out.append(f"{name} {aqi:.0f} ({label})" + (f", mostly from {main}." if main and aqi > 0 else ".")
                   + f" {ADVICE[standard][level]}")
    levels = [f"{POLLUTANTS[p][0]} {fmt(current[p])} {POLLUTANTS[p][1]}"
              for p in POLLUTANTS if current.get(p) is not None]
    if levels:
        out.append("Now: " + ", ".join(levels) + ".")
    if current.get("uv_index") is not None:
        out.append(f"UV index {current['uv_index']:.1f} ({uv_level(current['uv_index'])}).")
    pollen_now = [(POLLEN[v][0], current[v], pollen_level(v, current[v]))
                  for v in POLLEN if current.get(v)]
    if pollen_now:
        out.append("Pollen now: " + ", ".join(f"{n} {fmt(x)} grains/m³ ({lvl})" for n, x, lvl in pollen_now) + ".")

    days = {}
    for r in rows:
        days.setdefault(r["time"].date(), []).append(r)
    if days:
        out.append("")
        out.append("Outlook by day:")
    for d, hours in days.items():
        values = [(h[key], h) for h in hours if h.get(key) is not None]
        if not values:
            continue
        low = min(v for v, _ in values)
        worst, worst_hour = max(values, key=lambda x: x[0])
        lo_band, hi_band = band(standard, low)[1], band(standard, worst)[1]
        bands = hi_band if lo_band == hi_band else f"{lo_band} to {hi_band}"
        main = main_pollutant(standard, worst_hour)
        line = (f"- {d:%a %d %b}: {name} {low:.0f}-{worst:.0f} ({bands}), worst around "
                f"{worst_hour['time']:%H:%M}" + (f" from {main}" if main and worst > 0 else ""))
        uv = [h["uv_index"] for h in hours if h.get("uv_index") is not None]
        if uv:
            line += f"; UV up to {max(uv):.0f} ({uv_level(max(uv))})"
        pollen = []
        for v in POLLEN:
            peak = max((h[v] for h in hours if h.get(v) is not None), default=None)
            if peak and pollen_level(v, peak) != "low":
                pollen.append(f"{POLLEN[v][0]} {pollen_level(v, peak)}")
        if pollen:
            line += "; pollen: " + ", ".join(pollen)
        out.append(line + ".")

    risky = 2 if standard == "us" else 3  # unhealthy for sensitive groups, or poor
    bad = [r for r in rows if r.get(key) is not None and band(standard, r[key])[0] >= risky
           and (not now or r["time"] >= now)]
    if bad:
        out.append("")
        out.append(f"Hours to plan around ({BANDS[standard][risky][1]} or worse): "
                   + spans([r["time"] for r in bad]) + ".")
    if not any(r.get(v) for r in rows for v in POLLEN):
        out.append("")
        out.append("No pollen forecast here: Open-Meteo models pollen for Europe only.")
    return "\n".join(out)


def spans(times):
    """Consecutive hours as ranges: 'Tue 14:00-18:00, Wed 15:00'."""
    out, start, prev = [], None, None
    for t in times + [None]:
        if start is not None and (t is None or (t - prev).total_seconds() != 3600):
            end = "" if prev == start else f"-{prev:%H:%M}" if prev.date() == start.date() else f"-{prev:%a %H:%M}"
            out.append(f"{start:%a %H:%M}{end}")
            start = None
        if t is not None and start is None:
            start = t
        prev = t
    return ", ".join(out[:8]) + (f", and {len(out) - 8} more" if len(out) > 8 else "")


def charts(rows, standard, place):
    out = []
    key = f"{standard}_aqi"
    name = tr(f"name_{standard}")
    points = [r for r in rows if r.get(key) is not None]
    if points:
        step = 1 if len(points) <= 96 else 2
        points = points[::step]
        good = BANDS[standard][0][0]
        out.append({"kind": "line", "title": tr("forecast", name=name) + (f", {place}" if place else ""),
                    "labels": [hour_label(r["time"]) for r in points],
                    "series": [{"name": name, "values": [round(r[key]) for r in points]}],
                    "y_label": name, "band": {"from": 0, "to": good, "label": tr("good")}})
    days = {}
    for r in rows:
        days.setdefault(r["time"].date(), []).append(r)
    series = []
    for v in POLLEN:
        peaks = [max((h[v] for h in hours if h.get(v) is not None), default=None) for hours in days.values()]
        if any(peaks):
            series.append({"name": tr(POLLEN[v][0]), "values": [None if p is None else round(p) for p in peaks]})
    if series:
        out.append({"kind": "bar", "title": tr("pollen_title"), "labels": [day_label(d) for d in days],
                    "series": series, "y_label": tr("pollen_unit")})
    return ["[chart] " + json.dumps(c, separators=(",", ":"), ensure_ascii=False) for c in out]


def main():
    args = json.load(sys.stdin) or {}
    lat, lon = coordinate(args, "latitude", 90), coordinate(args, "longitude", 180)
    location = str(args.get("location") or "").strip()
    if (lat is None) != (lon is None):
        sys.exit("give latitude and longitude together")
    days = whole(args.get("days"), 3, 1, 7)
    standard = {"eu": "european", "europe": "european", "usa": "us"}.get(
        str(args.get("standard") or "auto").lower(), str(args.get("standard") or "auto").lower())
    if standard not in ("auto", "us", "european"):
        sys.exit(f"standard is auto, us or european, not {standard!r}")
    if lat is not None:
        place = location or None
    elif location:
        if len(location) < 2:
            sys.exit("the location is too short; give a place name or postal code")
        lat, lon, place = geocode(location)
    else:
        sys.exit("give a location (a place name or postal code) or latitude and longitude")

    variables = (["us_aqi", "european_aqi", "uv_index", *POLLUTANTS, *POLLEN]
                 + [f"{s}_aqi_{p}" for s, ps in SUB_INDEX.items() for p in ps])
    data = get(AIR, {"latitude": lat, "longitude": lon, "timezone": "auto", "forecast_days": days,
                     "current": ",".join(variables), "hourly": ",".join(variables)})
    if standard == "auto":
        # Europe uses the European index; everywhere else the US one is the more familiar.
        standard = "european" if str(data.get("timezone", "")).startswith(("Europe/", "Atlantic/Reykjavik",
                                                                           "Atlantic/Canary", "Atlantic/Madeira",
                                                                           "Atlantic/Faroe")) else "us"
    rows = hourly_rows(data)
    print(report(place, data.get("latitude", lat), data.get("longitude", lon), data, standard, rows))
    lines = charts(rows, standard, place.split(",")[0] if place else None)
    if lines:
        print("\n" + "\n".join(lines))


if __name__ == "__main__":
    main()
