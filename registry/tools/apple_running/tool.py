#!/usr/bin/env python3
"""apple_running: a personal cedar tool. Reads its input as JSON on stdin and prints the result.

Reads the running workouts from an Apple Health export (the export.zip the Health app makes under
your profile → Export All Health Data, which holds what Apple Fitness and the Apple Watch record)
and summarizes them: totals, weekly distance, pace, heart rate, cadence, power, and the longest
and fastest runs. Apple offers no web API for Health or Fitness data, so the export is the source.
The report ends with `[chart] {...}` lines, which the cedar chat draws as charts.

The export is found, in this order:
1. `path` in the input: export.zip, export.xml, or the unzipped apple_health_export folder in
   the workspace;
2. export.zip or export.xml next to this script (put it in the tool's folder in TOOLS_DIR);
3. APPLE_HEALTH_EXPORT_URL: a Dropbox or Google Drive share link to export.zip, saved as this
   tool's secret in cedar (or in a `.env` next to this script). It is downloaded into the tool's
   own environment (its home) and fetched again only when the file behind the link changes.

The runs read from an export are cached in the tool's home, so only the first call on a new
export parses the whole file (which can take a minute or two for years of data).

Exit non-zero with a message on stderr to report an error; the model sees it as the tool's
error and can try again differently.
"""
import csv
import hashlib
import json
import os
import re
import shutil
import statistics
import sys
import urllib.error
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET
import zipfile
from datetime import date, datetime, timedelta
from pathlib import Path

HERE = Path(__file__).resolve().parent
WORKSPACE = Path("/workspace")
CACHE = Path.home() / ".cache" / "apple_running"
CACHE_FORMAT = 1  # bump when the cached run fields change
RUNNING = "HKWorkoutActivityTypeRunning"
# The hosts the manifest declares; a share link (and every redirect it takes) must stay on them.
HOSTS = ("www.dropbox.com", "dropbox.com", "drive.google.com", "drive.usercontent.google.com")
HOST_SUFFIXES = (".dropboxusercontent.com",)
KM = {"km": 1.0, "m": 0.001, "mi": 1.609344, "yd": 0.0009144, "ft": 0.0003048}
METERS = {"m": 1.0, "cm": 0.01, "km": 1000.0, "ft": 0.3048, "in": 0.0254}
MINUTES = {"min": 1.0, "s": 1 / 60, "hr": 60.0, "h": 60.0}
KCAL = {"kcal": 1.0, "Cal": 1.0, "kJ": 1 / 4.184, "cal": 0.001}
Q = "HKQuantityTypeIdentifier"


# --- setup -----------------------------------------------------------------------------------

def load_env(path):
    """Read KEY=VALUE lines into os.environ, without overriding what is already set (the secrets
    cedar passes in win). A missing file is fine."""
    if not path.is_file():
        return
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        key, value = key.strip().removeprefix("export ").strip(), value.strip()
        if value[:1] in "\"'" and value[-1:] == value[:1] and len(value) > 1:
            value = value[1:-1]
        elif " #" in value:
            value = value.split(" #", 1)[0].rstrip()
        os.environ.setdefault(key, value)


# --- finding the export ----------------------------------------------------------------------

def from_workspace(raw):
    """A workspace path the input names: export.zip, export.xml, or a folder holding export.xml."""
    p = (WORKSPACE / raw).resolve() if not Path(raw).is_absolute() else Path(raw).resolve()
    if p != WORKSPACE and WORKSPACE not in p.parents:
        sys.exit(f"path must be inside /workspace, not {raw!r}")
    if p.is_dir():
        for candidate in (p / "export.xml", p / "apple_health_export" / "export.xml"):
            if candidate.is_file():
                return candidate
        sys.exit(f"no export.xml in {raw!r}")
    if not p.is_file():
        sys.exit(f"no such file in the workspace: {raw!r}")
    return p


def allowed(url):
    parts = urllib.parse.urlsplit(url)
    host = (parts.hostname or "").lower()
    return parts.scheme == "https" and (host in HOSTS or host.endswith(HOST_SUFFIXES))


def direct_link(url):
    """Turn a share link into one that returns the file itself."""
    parts = urllib.parse.urlsplit(url.strip())
    host = (parts.hostname or "").lower()
    if host.endswith("dropbox.com"):
        query = [(k, v) for k, v in urllib.parse.parse_qsl(parts.query) if k != "dl"] + [("dl", "1")]
        return urllib.parse.urlunsplit(parts._replace(query=urllib.parse.urlencode(query)))
    if host == "drive.google.com":
        found = re.search(r"/d/([\w-]+)", parts.path) or re.search(r"(?:^|&)id=([\w-]+)", parts.query)
        if not found:
            sys.exit("APPLE_HEALTH_EXPORT_URL: can't find the file id in this Google Drive link")
        return ("https://drive.usercontent.google.com/download?"
                + urllib.parse.urlencode({"id": found.group(1), "export": "download", "confirm": "t"}))
    return url.strip()


class StayOnHosts(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if not allowed(newurl):
            raise urllib.error.URLError(f"the link redirected to {urllib.parse.urlsplit(newurl).hostname}, "
                                        "which this tool doesn't reach")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def download(url, refresh):
    """Fetch the share link's export.zip into the cache, unless the file behind it is the one
    already there (same ETag, or same size and Last-Modified)."""
    url = direct_link(url)
    if not allowed(url):
        sys.exit("APPLE_HEALTH_EXPORT_URL must be an https Dropbox or Google Drive share link")
    CACHE.mkdir(parents=True, exist_ok=True)
    target, meta_path = CACHE / "export.zip", CACHE / "download.json"
    try:
        meta = json.loads(meta_path.read_text()) if meta_path.is_file() else {}
    except ValueError:
        meta = {}
    opener = urllib.request.build_opener(StayOnHosts)
    request = urllib.request.Request(url, headers={"User-Agent": "cedar-apple-running/1.0"})
    try:
        with opener.open(request, timeout=60) as response:
            seen = {"url": url, "etag": response.headers.get("ETag"),
                    "length": response.headers.get("Content-Length"),
                    "modified": response.headers.get("Last-Modified")}
            same = meta.get("url") == url and (
                (seen["etag"] and seen["etag"] == meta.get("etag"))
                or (seen["length"] and seen["modified"]
                    and (seen["length"], seen["modified"]) == (meta.get("length"), meta.get("modified"))))
            if same and target.is_file() and not refresh:
                return target
            if "text/html" in (response.headers.get("Content-Type") or ""):
                sys.exit("the share link returned a web page, not the file: make sure anyone with "
                         "the link can view it, and that it points to export.zip")
            partial = target.with_suffix(".part")
            with open(partial, "wb") as f:
                shutil.copyfileobj(response, f, 1 << 20)
    except (urllib.error.URLError, OSError) as e:
        if target.is_file():
            print(f"(couldn't check the share link for a newer export, so this uses the one "
                  f"downloaded before: {e})\n")
            return target
        sys.exit(f"couldn't download the export from APPLE_HEALTH_EXPORT_URL: {e}")
    if not zipfile.is_zipfile(partial):
        partial.unlink()
        sys.exit("the file behind APPLE_HEALTH_EXPORT_URL is not a zip; share the export.zip "
                 "the Health app made")
    partial.replace(target)
    meta_path.write_text(json.dumps(seen))
    return target


def find_export(args):
    if args.get("path"):
        return from_workspace(str(args["path"]))
    for name in ("export.zip", "export.xml"):
        if (HERE / name).is_file():
            return HERE / name
    url = os.environ.get("APPLE_HEALTH_EXPORT_URL", "").strip()
    if url:
        return download(url, bool(args.get("refresh")))
    sys.exit("no Apple Health export: on the iPhone, open Health → your picture → Export All "
             "Health Data, then either save export.zip to Dropbox or Google Drive and save its share "
             "link as APPLE_HEALTH_EXPORT_URL in this tool's settings, or put the file in the "
             "workspace and pass its path")


# --- reading the export ----------------------------------------------------------------------

def open_xml(source):
    """The export.xml inside a zip (not export_cda.xml), or the file itself."""
    if source.suffix.lower() != ".zip":
        return open(source, "rb")
    try:
        archive = zipfile.ZipFile(source)
    except zipfile.BadZipFile:
        sys.exit(f"{source.name} is not a readable zip")
    names = [n for n in archive.namelist() if n == "export.xml" or n.endswith("/export.xml")]
    if not names:
        sys.exit(f"{source.name} has no export.xml; is it the Health app's export?")
    return archive.open(min(names, key=len))


def quantity(text):
    """'1234 cm' or '21.5 degC' → (1234.0, 'cm')."""
    m = re.match(r"\s*(-?[\d.]+)\s*(\S*)", text or "")
    return (float(m.group(1)), m.group(2)) if m else (None, "")


def when(text):
    return datetime.strptime(text, "%Y-%m-%d %H:%M:%S %z")


def number(text):
    try:
        return float(text)
    except (TypeError, ValueError):
        return None


def parse_run(w):
    """One running <Workout> element → a dict of plain values (km, minutes, bpm, ...)."""
    start, end = when(w.get("startDate")), when(w.get("endDate"))
    minutes = number(w.get("duration"))
    minutes = minutes * MINUTES.get(w.get("durationUnit", "min"), 1.0) if minutes else \
        (end - start).total_seconds() / 60
    run = {"start": start.isoformat(), "minutes": round(minutes, 2), "source": w.get("sourceName", ""),
           "km": None, "kcal": None, "avg_hr": None, "max_hr": None, "cadence": None,
           "power_w": None, "elevation_m": None, "indoor": False, "temperature_c": None}
    # Exports before iOS 16 carry totals as attributes; later ones as WorkoutStatistics.
    if number(w.get("totalDistance")):
        run["km"] = number(w.get("totalDistance")) * KM.get(w.get("totalDistanceUnit", "km"), 1.0)
    if number(w.get("totalEnergyBurned")):
        run["kcal"] = number(w.get("totalEnergyBurned")) * KCAL.get(w.get("totalEnergyBurnedUnit", "kcal"), 1.0)
    steps = None
    for s in w.iter("WorkoutStatistics"):
        kind, unit = s.get("type", ""), s.get("unit", "")
        if kind == Q + "DistanceWalkingRunning" and number(s.get("sum")):
            run["km"] = number(s.get("sum")) * KM.get(unit, 1.0)
        elif kind == Q + "ActiveEnergyBurned" and number(s.get("sum")):
            run["kcal"] = number(s.get("sum")) * KCAL.get(unit, 1.0)
        elif kind == Q + "HeartRate":
            run["avg_hr"], run["max_hr"] = number(s.get("average")), number(s.get("maximum"))
        elif kind == Q + "StepCount":
            steps = number(s.get("sum"))
        elif kind == Q + "RunningPower":
            run["power_w"] = number(s.get("average"))
    for m in w.iter("MetadataEntry"):
        key, value = m.get("key", ""), m.get("value", "")
        if key == "HKElevationAscended":
            v, unit = quantity(value)
            run["elevation_m"] = v * METERS.get(unit, 1.0) if v is not None else None
        elif key == "HKIndoorWorkout":
            run["indoor"] = value.strip() == "1"
        elif key == "HKWeatherTemperature":
            v, unit = quantity(value)
            if v is not None:
                run["temperature_c"] = (v - 32) / 1.8 if unit == "degF" else v
    if steps and minutes:
        run["cadence"] = steps / minutes
    for k in ("km", "kcal", "avg_hr", "max_hr", "cadence", "power_w", "elevation_m", "temperature_c"):
        if run[k] is not None:
            run[k] = round(run[k], 3 if k == "km" else 1)
    return run


def read_runs(source):
    """Every running workout in the export, oldest first. Streams the XML and throws away each
    top-level element once read, so a multi-gigabyte export fits in memory."""
    runs, depth, root = [], 0, None
    with open_xml(source) as f:
        try:
            for event, elem in ET.iterparse(f, events=("start", "end")):
                if event == "start":
                    depth += 1
                    if root is None:
                        root = elem
                    continue
                depth -= 1
                if depth == 1:
                    if elem.tag == "Workout" and elem.get("workoutActivityType") == RUNNING:
                        try:
                            runs.append(parse_run(elem))
                        except (TypeError, ValueError):
                            pass  # a workout without readable dates
                    root.clear()
        except ET.ParseError as e:
            sys.exit(f"couldn't read the export's XML ({e}); export it again from the Health app")
    runs.sort(key=lambda r: r["start"])
    return runs


def cached_runs(source):
    """The runs of this export, from the cache when the same file was read before."""
    stat = source.stat()
    key = hashlib.sha256(f"{CACHE_FORMAT}|{source}|{stat.st_size}|{stat.st_mtime_ns}".encode()).hexdigest()
    path = CACHE / "runs.json"
    try:
        cached = json.loads(path.read_text())
        if cached.get("key") == key:
            return cached["runs"]
    except (OSError, ValueError, KeyError):
        pass
    runs = read_runs(source)
    try:
        CACHE.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps({"key": key, "runs": runs}))
    except OSError:
        pass  # no cache this time; the result is still right
    return runs


# --- analysis --------------------------------------------------------------------------------

class Units:
    def __init__(self, miles):
        self.miles = miles
        self.dist = "mi" if miles else "km"
        self.per = 1 / KM["mi"] if miles else 1.0

    def distance(self, km):
        return round(km * self.per, 2)

    def pace(self, minutes, km):
        """Minutes per km (or mile), as a number for charts."""
        return round(minutes / (km * self.per), 2) if km else None

    def pace_text(self, minutes, km):
        p = self.pace(minutes, km)
        if p is None:
            return "–"
        m, s = divmod(round(p * 60), 60)
        return f"{m}:{s:02d}/{self.dist}"


def duration_text(minutes):
    h, m = divmod(round(minutes), 60)
    return f"{h} h {m:02d} min" if h else f"{m} min"


def week_of(d):
    return d - timedelta(days=d.weekday())


def mean(values):
    values = [v for v in values if v is not None]
    return statistics.fmean(values) if values else None


def summarize(runs, u):
    km = sum(r["km"] or 0 for r in runs)
    minutes = sum(r["minutes"] for r in runs if r["km"])
    lines = [f"- {len(runs)} runs, {u.distance(km)} {u.dist}, {duration_text(sum(r['minutes'] for r in runs))}"]
    lines.append(f"- Average pace {u.pace_text(minutes, km)}; average run {u.distance(km / len(runs))} {u.dist}")
    extras = []
    for key, label, fmt in (("avg_hr", "heart rate", "{:.0f} bpm"), ("cadence", "cadence", "{:.0f} spm"),
                            ("power_w", "power", "{:.0f} W")):
        v = mean(r[key] for r in runs)
        if v is not None:
            extras.append(f"{label} {fmt.format(v)}")
    climb = sum(r["elevation_m"] or 0 for r in runs)
    if climb:
        extras.append(f"climb {round(climb * (3.28084 if u.miles else 1))} {'ft' if u.miles else 'm'}")
    if extras:
        lines.append("- Averages: " + ", ".join(extras))
    return lines


def report(runs, period, start_day, today, u, max_runs):
    if not period:
        last = runs[-1]
        return (f"No runs between {start_day} and {today}. The newest run in the export is from "
                f"{last['start'][:10]}; if you ran since, export the data again from the Health app.")
    out = [f"Running from {start_day} to {today} (Apple Health export, newest run {runs[-1]['start'][:10]})", ""]
    out += ["Totals:"] + summarize(period, u)

    # Weekly load: this week so far and last week against the four weeks before them.
    weeks = {}
    for r in period:
        weeks.setdefault(week_of(date.fromisoformat(r["start"][:10])), []).append(r)
    this_week = week_of(today)
    last_week = this_week - timedelta(days=7)
    before = [sum(x["km"] or 0 for x in weeks.get(last_week - timedelta(days=7 * i), [])) for i in range(1, 5)]
    usual = statistics.fmean(before) if (today - start_day).days >= 35 else None
    load = ["", "Weekly distance:"]
    for label, wk in (("This week so far", this_week), ("Last week", last_week)):
        rs = weeks.get(wk, [])
        load.append(f"- {label}: {u.distance(sum(x['km'] or 0 for x in rs))} {u.dist} in {len(rs)} "
                    + ("run" if len(rs) == 1 else "runs"))
    if usual is not None:
        load.append(f"- The four weeks before: {u.distance(usual)} {u.dist} a week on average")
        last_km = sum(x["km"] or 0 for x in weeks.get(last_week, []))
        if usual > 0 and last_km > 1.3 * usual:
            load.append(f"- Note: last week was {round(100 * (last_km / usual - 1))}% above the usual; "
                        "a jump of more than about 10-30% a week is a common injury risk")
    out += load

    best = [""]
    longest = max(period, key=lambda r: r["km"] or 0)
    if longest["km"]:
        best.append(f"Longest run: {u.distance(longest['km'])} {u.dist} on {longest['start'][:10]} "
                    f"({u.pace_text(longest['minutes'], longest['km'])})")
    shown = None
    for floor, label in ((5, "5 km"), (10, "10 km"), (21.0975, "half marathon")):
        eligible = [r for r in period if r["km"] and r["km"] >= floor and not r["indoor"]]
        if eligible:
            r = min(eligible, key=lambda r: r["minutes"] / r["km"])
            if r is shown:
                continue  # the fastest 5 km+ run was also 10 km or longer
            shown = r
            best.append(f"Fastest run of {label} or longer, by average pace: {u.pace_text(r['minutes'], r['km'])} "
                        f"over {u.distance(r['km'])} {u.dist} on {r['start'][:10]}")
    out += best

    out += ["", f"Runs, newest first{f' (the latest {max_runs})' if len(period) > max_runs else ''}:"]
    for r in reversed(period[-max_runs:]):
        bits = [r["start"][:16].replace("T", " "),
                f"{u.distance(r['km'])} {u.dist}" if r["km"] else "no distance",
                duration_text(r["minutes"]), u.pace_text(r["minutes"], r["km"])]
        if r["avg_hr"]:
            bits.append(f"HR {r['avg_hr']:.0f}" + (f"/{r['max_hr']:.0f}" if r["max_hr"] else ""))
        if r["cadence"]:
            bits.append(f"{r['cadence']:.0f} spm")
        if r["power_w"]:
            bits.append(f"{r['power_w']:.0f} W")
        if r["elevation_m"]:
            bits.append(f"+{round(r['elevation_m'] * (3.28084 if u.miles else 1))} {'ft' if u.miles else 'm'}")
        if r["indoor"]:
            bits.append("indoor")
        out.append("- " + ", ".join(bits))
    return "\n".join(out)


def charts(period, start_day, today, u):
    if not period:
        return []
    # Bars by week for up to half a year, by month beyond, from the first run in the period.
    monthly = (today - start_day).days > 182
    bucket = (lambda d: d.replace(day=1)) if monthly else week_of
    totals = {}
    for r in period:
        b = bucket(date.fromisoformat(r["start"][:10]))
        totals[b] = totals.get(b, 0) + (r["km"] or 0)
    labels, values, b = [], [], min(totals)
    while b <= today:
        labels.append(b.strftime("%b %Y" if monthly else "%b %d"))
        values.append(u.distance(totals.get(b, 0)))
        b = (b + timedelta(days=32)).replace(day=1) if monthly else b + timedelta(days=7)
    out = [{"kind": "bar", "title": f"{'Monthly' if monthly else 'Weekly'} running distance ({u.dist})", "labels": labels,
            "series": [{"name": "Distance", "values": values}], "y_label": u.dist}]
    timed = [r for r in period if r["km"] and r["km"] >= 1][-60:]
    if len(timed) >= 2:
        series = [{"name": f"Pace (min/{u.dist})", "values": [u.pace(r["minutes"], r["km"]) for r in timed]}]
        out.append({"kind": "line", "title": f"Pace per run (min/{u.dist}, lower is faster)",
                    "labels": [r["start"][:10] for r in timed], "series": series, "y_label": f"min/{u.dist}"})
        if sum(1 for r in timed if r["avg_hr"]) >= 2:
            out.append({"kind": "line", "title": "Average heart rate per run",
                        "labels": [r["start"][:10] for r in timed],
                        "series": [{"name": "Heart rate (bpm)", "values": [r["avg_hr"] for r in timed]}],
                        "y_label": "bpm"})
    return ["[chart] " + json.dumps(c, separators=(",", ":"), ensure_ascii=False) for c in out]


def export_csv(runs, u):
    out = Path("apple_running")
    out.mkdir(exist_ok=True)
    fields = ["start", "km", "minutes", "pace_min_per_" + u.dist, "avg_hr", "max_hr", "cadence",
              "power_w", "elevation_m", "kcal", "indoor", "temperature_c", "source"]
    with open(out / "runs.csv", "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(fields)
        for r in runs:
            w.writerow([r["start"], r["km"], r["minutes"], u.pace(r["minutes"], r["km"]), r["avg_hr"],
                        r["max_hr"], r["cadence"], r["power_w"], r["elevation_m"], r["kcal"], r["indoor"],
                        r["temperature_c"], r["source"]])


def whole(value, default, lo, hi):
    """An integer input in [lo, hi]; missing, null, or not a number gives the default."""
    try:
        return max(lo, min(hi, int(value)))
    except (TypeError, ValueError):
        return default


def main():
    args = json.load(sys.stdin) or {}
    days = whole(args.get("days"), 90, 7, 3650)
    max_runs = whole(args.get("max_runs"), 20, 1, 200)
    load_env(HERE / ".env")
    unit = str(args.get("unit") or os.environ.get("DISTANCE_UNIT", "km")).strip().lower()
    u = Units(unit.startswith("mi"))

    source = find_export(args)
    runs = cached_runs(source)
    if not runs:
        sys.exit("the export has no running workouts; runs recorded with Apple Fitness, the Workout "
                 "app, or an app that saves workouts to Health show up here")

    today = date.today()
    start_day = today - timedelta(days=days - 1)
    period = [r for r in runs if date.fromisoformat(r["start"][:10]) >= start_day]
    print(report(runs, period, start_day, today, u, max_runs))
    lines = charts(period, start_day, today, u)
    if lines:
        print("\n" + "\n".join(lines))
    if args.get("export"):
        export_csv(period, u)


if __name__ == "__main__":
    main()
