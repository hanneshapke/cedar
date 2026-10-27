#!/usr/bin/env python3
"""tandem_glucose: a personal cedar tool. Reads its input as JSON on stdin and prints the result.

Downloads CGM readings, boluses, and basal deliveries from Tandem Source (t:connect) with
tconnectsync, compares the last hours with the days before, and suggests what to try today.
The analysis follows the Diabetes Concierge backend (stats, advisor, and daily summary rules).
The report ends with `[chart] {...}` lines, which the cedar chat draws as charts (the same spec
as the generate_chart tool's input).

Credentials come from the secrets an admin saved for this tool in cedar (they arrive as
environment variables), or else from a `.env` file next to this script (copy `.env.example`).
cedar runs each call in a container of its own with network access to Tandem (the manifest
declares the hosts), and installs tconnectsync into the tool's own environment when the tool is
installed.

Exit non-zero with a message on stderr to report an error; the model sees it as the tool's
error and can try again differently.
"""
import csv
import importlib
import json
import os
import site
import statistics
import subprocess
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
PACKAGES = ["tconnectsync>=3.0", "setuptools>=70,<81", "tzdata"]
# Seconds from the Unix epoch to the Tandem epoch (2008-01-01), the base of egvTimeStamp.
TANDEM_EPOCH = 1199145600
MMOL_PER_MGDL = 0.0555
URGENT_LOW, URGENT_HIGH = 54, 250
BLOCKS = [("overnight", 0, 6), ("morning", 6, 12), ("afternoon", 12, 18), ("evening", 18, 24)]
WHEN = {"overnight": "overnight (midnight to 6 am)", "morning": "in the morning",
        "afternoon": "in the afternoon", "evening": "in the evening"}
CGM_TYPES = {"LidCgmDataGxb": "Dexcom G6", "LidCgmDataG7": "Dexcom G7", "LidCgmDataFsl2": "FreeStyle Libre 2"}
# A sensor reports readings past its range as "HIGH" or "LOW" (status 1 and 2, value 0). They are
# kept, just past each end of the range, so the worst lows and highs still count.
SENSOR_RANGE = {"Dexcom G6": (40, 400), "Dexcom G7": (40, 400), "FreeStyle Libre 2": (40, 500)}
STATUS_HIGH, STATUS_LOW = 1, 2
SLOT_MINUTES = 15  # the chart's resolution
# Text the user sees in the charts, by CEDAR_LANGUAGE. The report itself stays English for the model.
STRINGS = {
    "en": {"last": "Last {h} h", "average": "{d}-day average", "target": "Target",
           "glucose_title": "Glucose: last {h} h vs. your {d}-day average",
           "tir_title": "Time in range ({low}–{high} {unit}) by time of day",
           "tir_label": "% of the time in range", "overnight": "Overnight", "morning": "Morning",
           "afternoon": "Afternoon", "evening": "Evening"},
    "de": {"last": "Letzte {h} h", "average": "{d}-Tage-Schnitt", "target": "Zielbereich",
           "glucose_title": "Glukose: letzte {h} h vs. dein {d}-Tage-Schnitt",
           "tir_title": "Zeit im Zielbereich ({low}–{high} {unit}) nach Tageszeit",
           "tir_label": "% der Zeit im Zielbereich", "overnight": "Nachts", "morning": "Morgens",
           "afternoon": "Nachmittags", "evening": "Abends"},
    "fr": {"last": "Dernières {h} h", "average": "Moyenne sur {d} jours", "target": "Cible",
           "glucose_title": "Glycémie\u00a0: dernières {h} h vs votre moyenne sur {d} jours",
           "tir_title": "Temps dans la cible ({low}–{high} {unit}) par moment de la journée",
           "tir_label": "% du temps dans la cible", "overnight": "Nuit", "morning": "Matin",
           "afternoon": "Après-midi", "evening": "Soir"},
}
WEEKDAYS = {"en": ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
            "de": ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"],
            "fr": ["lun.", "mar.", "mer.", "jeu.", "ven.", "sam.", "dim."]}


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


def decimal(value):
    """A number written into chart text, with the decimal comma German and French use."""
    return str(value) if language() == "en" else str(value).replace(".", ",")


# --- setup -----------------------------------------------------------------------------------

def load_env(path):
    """Read KEY=VALUE lines into os.environ, without overriding what is already set (the secrets
    cedar passes in win). A missing file is fine when those secrets are set."""
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


def ensure_deps():
    # tconnectsync caches login tokens in a pickle file unless told not to; set before importing.
    os.environ.setdefault("CACHE_CREDENTIALS", "false")
    try:
        import tconnectsync  # noqa: F401
        return
    except ImportError:
        pass
    done = subprocess.run([sys.executable, "-m", "pip", "install", "--user", "--quiet", *PACKAGES],
                          capture_output=True, text=True)
    if done.returncode != 0:
        sys.exit("could not install tconnectsync (set it up again from the tool settings, or check "
                 "TOOLS_NETWORK):\n"
                 + (done.stderr or done.stdout)[-2000:])
    # The user site didn't exist when the interpreter started, so it isn't on sys.path yet.
    site.addsitedir(site.getusersitepackages())
    importlib.invalidate_caches()


# --- download --------------------------------------------------------------------------------

def to_utc(naive, tz):
    """Pump timestamps are local wall time without an offset."""
    naive = getattr(naive, "naive", naive)
    if getattr(naive, "tzinfo", None) is not None:
        naive = naive.replace(tzinfo=None)
    return naive.replace(tzinfo=tz).astimezone(timezone.utc)


def cgm_time(event, tz):
    egv = getattr(event, "egvTimeStamp", 0)
    if egv:
        naive = datetime.fromtimestamp(TANDEM_EPOCH + egv, tz=timezone.utc).replace(tzinfo=None)
        return naive.replace(tzinfo=tz).astimezone(timezone.utc)
    return to_utc(event.eventTimestamp, tz)


def parse_events(events, tz):
    """Turn tconnectsync events into (readings, boluses, basal, sensor names, flagged), sorted by
    time; `flagged` maps the time of each out-of-range reading to "LOW" or "HIGH"."""
    by_type = {}
    for e in events:
        by_type.setdefault(type(e).__name__, []).append(e)

    readings, sensors, flagged = {}, set(), {}
    for name, label in CGM_TYPES.items():
        floor, ceiling = SENSOR_RANGE[label]
        for e in by_type.get(name, []):
            status = getattr(e, "glucoseValueStatusRaw", 0)
            t = cgm_time(e, tz)
            if status == STATUS_LOW:
                value, flagged[t] = floor - 1, "LOW"
            elif status == STATUS_HIGH:
                value, flagged[t] = ceiling + 1, "HIGH"
            elif status == 0 and e.currentGlucoseDisplayValue:
                value = int(e.currentGlucoseDisplayValue)
            else:
                continue
            readings[t] = value
            sensors.add(label)

    msg1 = {e.bolusId: e for e in by_type.get("LidBolusRequestedMsg1", [])}
    msg2 = {e.bolusId: e for e in by_type.get("LidBolusRequestedMsg2", [])}
    boluses = {}
    for c in by_type.get("LidBolusCompleted", []):
        m1, m2 = msg1.get(c.bolusId), msg2.get(c.bolusId)
        kind = None
        if m2 is not None and getattr(m2, "optionsRaw", None) is not None:
            kind = getattr(m2, "OptionsMap", {}).get(str(m2.optionsRaw))
        if kind is None and m1 is not None and getattr(m1, "bolusTypeRaw", None) is not None:
            kind = getattr(m1, "BolustypeMap", {}).get(str(m1.bolusTypeRaw))
        t = to_utc(c.eventTimestamp, tz)
        boluses[(t, c.bolusId)] = {
            "t": t,
            "units": round(float(c.insulinDelivered or 0), 3),
            "carbs_g": (m1.carbAmount or 0) if m1 else 0,
            "bg": (m1.bg or None) if m1 else None,
            "type": kind or "",
        }

    basal = {}
    for e in by_type.get("LidBasalDelivery", []):
        t = to_utc(e.eventTimestamp, tz)
        source = getattr(e, "CommandedratesourceMap", {}).get(str(e.commandedRateSourceRaw))
        basal[t] = {"t": t, "rate": e.commandedRate / 1000.0, "source": source or ""}

    return (sorted(readings.items()), [boluses[k] for k in sorted(boluses, key=lambda k: k[0])],
            [basal[t] for t in sorted(basal)], sorted(sensors), flagged)


def download(days, tz):
    from tconnectsync.api import TConnectApi
    from tconnectsync.api.common import ApiLoginException

    region = os.environ.get("TANDEM_REGION", "us").strip().lower()
    if region not in ("us", "eu"):
        sys.exit(f"TANDEM_REGION must be us or eu, not {region!r}")
    try:
        api = TConnectApi(os.environ["TANDEM_EMAIL"], os.environ["TANDEM_PASSWORD"], region=region.upper())
        source = api.tandemsource  # logs in
    except ApiLoginException as e:
        # Don't retry: repeated bad logins can lock the account.
        sys.exit(f"Tandem login failed; check TANDEM_EMAIL, TANDEM_PASSWORD and TANDEM_REGION ({e})")

    pumps = source.get_pumper().get("pumps") or []
    if not pumps:
        sys.exit("no pumps found on this Tandem account")
    serial = os.environ.get("TANDEM_PUMP_SERIAL", "").strip()
    if serial:
        pump = next((p for p in pumps if str(p.get("serialNumber")) == serial), None)
        if pump is None:
            sys.exit(f"no pump with serial {serial}; available: "
                     + ", ".join(str(p.get("serialNumber")) for p in pumps))
    else:
        pump = max(pumps, key=lambda p: p.get("maxDateOfEvents") or "")

    today = datetime.now(tz).date()
    events = source.pump_events(pump["assignmentId"], min_date=today - timedelta(days=days),
                                max_date=today, fetch_all_event_types=True)
    return pump, parse_events(events, tz)


# --- analysis --------------------------------------------------------------------------------

def stats(values, low, high):
    if not values:
        return {"n": 0}
    n = len(values)
    mean = statistics.fmean(values)
    sd = statistics.pstdev(values) if n > 1 else 0.0
    pct = lambda f: 100.0 * sum(1 for v in values if f(v)) / n  # noqa: E731
    return {
        "n": n, "mean": mean, "sd": sd, "cv": 100 * sd / mean if mean else 0.0,
        "gmi": 3.31 + 0.02392 * mean,
        "tir": pct(lambda v: low <= v <= high),
        "low": pct(lambda v: URGENT_LOW <= v < low), "very_low": pct(lambda v: v < URGENT_LOW),
        "high": pct(lambda v: high < v <= URGENT_HIGH), "very_high": pct(lambda v: v > URGENT_HIGH),
        "min": min(values), "max": max(values),
    }


def basal_units(basal, start, end):
    """Insulin from basal deliveries in [start, end): each rate holds until the next event,
    for at most an hour (so a gap in the data doesn't count as hours of delivery)."""
    total = 0.0
    for i, b in enumerate(basal):
        nxt = basal[i + 1]["t"] if i + 1 < len(basal) else end
        a, z = max(b["t"], start), min(nxt, end, b["t"] + timedelta(hours=1))
        if z > a:
            total += b["rate"] * (z - a).total_seconds() / 3600
    return total


def insulin(boluses, basal, start, end):
    bs = [b for b in boluses if start <= b["t"] < end]
    bolus_u = sum(b["units"] for b in bs)
    basal_u = basal_units(basal, start, end)
    total = bolus_u + basal_u
    return {"bolus": bolus_u, "basal": basal_u, "total": total, "count": len(bs),
            "carbs": sum(b["carbs_g"] for b in bs),
            "basal_pct": 100 * basal_u / total if total else None}


def episodes(readings, below=None, above=None, min_readings=1):
    """Runs of consecutive readings below (or above) a threshold: start, end, extreme value."""
    out, run = [], []
    hit = (lambda v: v < below) if below is not None else (lambda v: v > above)
    for t, v in readings + [(None, None)]:
        if t is not None and hit(v) and (not run or t - run[-1][0] <= timedelta(minutes=20)):
            run.append((t, v))
            continue
        if len(run) >= min_readings:
            vals = [x for _, x in run]
            out.append({"start": run[0][0], "end": run[-1][0],
                        "extreme": min(vals) if below is not None else max(vals)})
        run = [(t, v)] if t is not None and hit(v) else []
    return out


def after_bolus(readings, boluses, low):
    """Per bolus: whether a low followed within 4 h, and for meal boluses the rise within 2.5 h."""
    out = []
    for b in boluses:
        win = lambda h: [v for t, v in readings if b["t"] < t <= b["t"] + timedelta(hours=h)]  # noqa: E731
        base = [v for t, v in readings if abs((t - b["t"]).total_seconds()) <= 1800]
        after = win(2.5)
        rise = max(after) - min(base) if b["carbs_g"] and base and after else None
        out.append({**b, "low_after": any(v < low for v in win(4)), "rise": rise})
    return out


def block_of(t, tz):
    h = t.astimezone(tz).hour
    return next(name for name, a, z in BLOCKS if a <= h < z)


def by_block(readings, tz, low, high):
    groups = {name: [] for name, _, _ in BLOCKS}
    for t, v in readings:
        groups[block_of(t, tz)].append(v)
    return {name: stats(vs, low, high) for name, vs in groups.items()}


def dawn_rise(readings, tz):
    early = [v for t, v in readings if 3 <= t.astimezone(tz).hour < 5]
    morning = [v for t, v in readings if 6 <= t.astimezone(tz).hour < 8]
    if len(early) < 20 or len(morning) < 20:
        return None
    return statistics.median(morning) - statistics.median(early)


def analyze(readings, boluses, basal, flagged, now, hours, baseline_days, tz, low, high):
    recent_start = now - timedelta(hours=hours)
    base_start = recent_start - timedelta(days=baseline_days)
    recent = [(t, v) for t, v in readings if recent_start <= t <= now]
    base = [(t, v) for t, v in readings if base_start <= t < recent_start]
    base_days = len(base) / 288  # a CGM reading every 5 minutes
    base_ins = insulin(boluses, basal, base_start, recent_start)
    per_day = {k: (v / baseline_days if k in ("bolus", "basal", "total", "count", "carbs") else v)
               for k, v in base_ins.items()}
    marked = after_bolus(readings, [b for b in boluses if base_start <= b["t"] <= now], low)

    trend = None
    if recent:
        t_last, v_last = recent[-1]
        prior = [(t, v) for t, v in recent if t_last - timedelta(minutes=35) <= t <= t_last - timedelta(minutes=25)]
        if prior:
            trend = (v_last - prior[-1][1]) / ((t_last - prior[-1][0]).total_seconds() / 60)

    return {
        "now": now, "hours": hours, "baseline_days": baseline_days, "base_days_of_data": base_days,
        "recent": stats([v for _, v in recent], low, high), "base": stats([v for _, v in base], low, high),
        "recent_blocks": by_block(recent, tz, low, high), "base_blocks": by_block(base, tz, low, high),
        "recent_insulin": insulin(boluses, basal, recent_start, now + timedelta(seconds=1)),
        "base_insulin_per_day": per_day,
        "lows": episodes(recent, below=low), "highs": episodes(recent, above=URGENT_HIGH, min_readings=3),
        "recent_boluses": [b for b in marked if b["t"] >= recent_start],
        "base_boluses": [b for b in marked if b["t"] < recent_start],
        "dawn_rise": dawn_rise(base + recent, tz),
        "last": recent[-1] if recent else (readings[-1] if readings else None),
        "sensor_low": sum(1 for t, _ in recent if flagged.get(t) == "LOW"),
        "sensor_high": sum(1 for t, _ in recent if flagged.get(t) == "HIGH"),
        "trend": trend,
    }


def recommend(a, tz, low, high, fmt):
    """Suggestions for the rest of today, safety first. Behavioral only: never doses or settings."""
    out = []
    r, b = a["recent"], a["base"]
    last, trend = a["last"], a["trend"]
    if last and (a["now"] - last[0]) <= timedelta(minutes=30):
        if last[1] < low:
            out.append(f"Your latest reading is low ({fmt(last[1])}). Treat it now with fast carbs and recheck in 15 minutes.")
        elif trend is not None and trend <= -2 and last[1] < 120:
            out.append(f"You're at {fmt(last[1])} and falling fast. Have fast carbs at hand and recheck soon.")

    if r["n"]:
        low_blocks = [n for n, s in a["recent_blocks"].items() if s["n"] >= 12 and s["low"] + s["very_low"] >= 8]
        if r["very_low"] >= 1 or r["low"] + r["very_low"] >= 4 or a["lows"]:
            count = len(a["lows"])
            tip = (f"You had {count} low episode{'s' if count != 1 else ''} in the last {a['hours']} h "
                   f"({r['low'] + r['very_low']:.0f}% of the time below {fmt(low)}). Carry fast carbs today "
                   "and check before driving or exercise.")
            if low_blocks:
                tip += f" They clustered {' and '.join(WHEN[n] for n in low_blocks)}, so be extra watchful then today."
            if "overnight" in low_blocks:
                tip += " For tonight, check before bed and consider a small snack if you're trending down; recurring overnight lows are worth raising with your care team (overnight basal)."
            out.append(tip)

        rb = a["recent_boluses"] + a["base_boluses"]
        if len(rb) >= 10:
            rate = 100 * sum(x["low_after"] for x in rb) / len(rb)
            if rate >= 15:
                out.append(f"{rate:.0f}% of your boluses over these days were followed by a low within 4 h. "
                           "Today, check your insulin on board before any correction and avoid correcting again "
                           "within a few hours of a bolus (stacking).")
        spikes = [x for x in rb if x["rise"] is not None]
        big = [x for x in spikes if x["rise"] > 80]
        recent_big = [x for x in a["recent_boluses"] if x["rise"] is not None and x["rise"] > 80]
        if recent_big or (len(spikes) >= 5 and len(big) / len(spikes) >= 0.5):
            meals = ", ".join(x["t"].astimezone(tz).strftime("%H:%M") for x in recent_big) or "several meals"
            out.append(f"Glucose rose more than {fmt(80)} after meals ({meals}). Try bolusing "
                       "10-15 minutes before you eat today, and take a short walk after your biggest meal.")

        high_blocks = [(n, s) for n, s in a["recent_blocks"].items() if s["n"] >= 12 and s["high"] + s["very_high"] >= 40]
        for n, s in high_blocks:
            out.append(f"Glucose ran high {WHEN[n]} ({s['high'] + s['very_high']:.0f}% of the time above "
                       f"{fmt(high)}). Make that part of today your focus: what you eat then, and bolusing ahead of it.")

        if b["n"]:
            diff = r["tir"] - b["tir"]
            if diff <= -3:
                out.append(f"Time in range ({r['tir']:.0f}%) was below your {a['baseline_days']}-day average "
                           f"({b['tir']:.0f}%). Keep today simple: familiar meals, regular timing, and an extra check "
                           "after meals.")
            elif diff >= 3:
                out.append(f"Time in range ({r['tir']:.0f}%) beat your {a['baseline_days']}-day average "
                           f"({b['tir']:.0f}%). Repeat what worked: same meal timing and pre-bolus habits.")
        if r["cv"] > 36:
            out.append(f"Glucose was quite variable (CV {r['cv']:.0f}%, goal at most 36%). Steady meal times "
                       "and sizes today help smooth it out.")

    if a["dawn_rise"] is not None and a["dawn_rise"] >= 30:
        out.append(f"Glucose typically rises about {fmt(a['dawn_rise'])} between 3-5 am and 6-8 am "
                   "(dawn phenomenon). That's a pattern to discuss with your care team.")
    pct = a["base_insulin_per_day"]["basal_pct"]
    if pct is not None and a["base_insulin_per_day"]["total"] >= 10 and (pct < 30 or pct > 70):
        out.append(f"Basal makes up {pct:.0f}% of your daily insulin (many pump users land near 40-60%). "
                   "Worth discussing with your care team.")
    if not out and r["n"]:
        out.append("No problem patterns stood out. Keep doing what you're doing today.")
    return out


# --- output ----------------------------------------------------------------------------------

def formatter(mmol):
    """A glucose value (or difference) in the user's unit; unit=False leaves the unit off."""
    def fmt(v, unit=True):
        if v is None:
            return "-"
        return (f"{v * MMOL_PER_MGDL:.1f}" if mmol else f"{v:.0f}") + ((" mmol/L" if mmol else " mg/dL") if unit else "")
    return fmt


def report(pump, sensors, a, suggestions, tz, low, high, unit):
    mmol = unit == "mmol"
    fmt = formatter(mmol)
    num = lambda v: fmt(v, unit=False)  # noqa: E731
    local = lambda t: t.astimezone(tz).strftime("%a %H:%M")  # noqa: E731
    r, b, h, d = a["recent"], a["base"], a["hours"], a["baseline_days"]

    lines = [f"Tandem pump {pump.get('serialNumber', '?')}, sensor: {', '.join(sensors) or 'none found'}, "
             f"timezone {tz.key}, units {'mmol/L' if mmol else 'mg/dL'}, target {num(low)}-{num(high)}"]
    if a["last"]:
        age = (a["now"] - a["last"][0]).total_seconds() / 60
        trend = a["trend"]
        arrow = "" if trend is None else (" rising" if trend >= 1 else " falling" if trend <= -1 else " steady")
        lines.append(f"Latest reading: {fmt(a['last'][1])}{arrow} at {local(a['last'][0])} ({age:.0f} min ago)")
        if age > 60:
            lines.append("Note: the newest data is over an hour old; the t:connect app may not have uploaded yet.")
    lines.append(f"Readings: {r['n']} in the last {h} h, {b['n']} in the {d} days before "
                 f"(about {a['base_days_of_data']:.1f} days of CGM data)")
    if a["sensor_low"] or a["sensor_high"]:
        lines.append(f"In the last {h} h the sensor read LOW (below its range) {a['sensor_low']} times and "
                     f"HIGH (above it) {a['sensor_high']} times; they count as just past the range.")
    if not r["n"]:
        lines.append("No CGM readings in the recent window, so there is nothing to analyze.")

    def row(label, key, f=lambda v: f"{v:.0f}%"):
        rv = f(r[key]) if r["n"] else "-"
        bv = f(b[key]) if b["n"] else "-"
        return f"  {label:<22}{rv:>12}{bv:>14}"
    lines += ["", f"Glucose: last {h} h vs. previous {d}-day average",
              f"  {'':<22}{'last ' + str(h) + ' h':>12}{str(d) + '-day avg':>14}",
              row("average", "mean", num), row("time in range", "tir"),
              row(f"below {num(low)}", "low"), row(f"below {num(URGENT_LOW)}", "very_low"),
              row(f"above {num(high)}", "high"), row(f"above {num(URGENT_HIGH)}", "very_high"),
              row("variability (CV)", "cv"), row("GMI (est. A1c)", "gmi", lambda v: f"{v:.1f}%"),
              row("lowest", "min", num), row("highest", "max", num)]

    lines += ["", f"By time of day (average, % in range): last {h} h | {d}-day"]
    for name, _, _ in BLOCKS:
        s, t = a["recent_blocks"][name], a["base_blocks"][name]
        cell = lambda x: f"{num(x['mean'])}, {x['tir']:.0f}%" if x["n"] else "-"  # noqa: E731
        lines.append(f"  {name:<10} {cell(s):>14} | {cell(t)}")

    lines += ["", f"Events in the last {h} h"]
    for e in a["lows"]:
        mins = (e["end"] - e["start"]).total_seconds() / 60 + 5
        lines.append(f"  low: {local(e['start'])}, about {mins:.0f} min, lowest {fmt(e['extreme'])}")
    for e in a["highs"]:
        mins = (e["end"] - e["start"]).total_seconds() / 60 + 5
        lines.append(f"  high: {local(e['start'])}, about {mins:.0f} min above {fmt(URGENT_HIGH)}, peak {fmt(e['extreme'])}")
    for x in a["recent_boluses"]:
        what = f"{x['units']:.2f} U" + (f", {x['carbs_g']} g carbs" if x["carbs_g"] else "") + (f" ({x['type']})" if x["type"] else "")
        after = (f", rose {fmt(x['rise'])} within 2.5 h" if x["rise"] is not None else "") + (", low within 4 h" if x["low_after"] else "")
        lines.append(f"  bolus: {local(x['t'])} {what}{after}")
    if not (a["lows"] or a["highs"] or a["recent_boluses"]):
        lines.append("  none")

    ri, bi = a["recent_insulin"], a["base_insulin_per_day"]
    split = lambda x: f"{x['basal_pct']:.0f}%" if x["basal_pct"] is not None else "-"  # noqa: E731
    lines += ["", f"Insulin: last {h} h | {d}-day daily average",
              f"  total {ri['total']:.1f} U | {bi['total']:.1f} U",
              f"  bolus {ri['bolus']:.1f} U in {ri['count']} boluses | {bi['bolus']:.1f} U in {bi['count']:.1f} boluses",
              f"  basal {ri['basal']:.1f} U ({split(ri)} of total) | {bi['basal']:.1f} U ({split(bi)})",
              f"  carbs entered {ri['carbs']:.0f} g | {bi['carbs']:.0f} g"]

    lines += ["", "Suggestions for today"] + [f"  {i}. {s}" for i, s in enumerate(suggestions, 1)]
    lines += ["", "Not medical advice: don't change doses or pump settings without your care team."]
    return "\n".join(lines)


def charts(readings, a, tz, low, high, unit):
    """`[chart]` lines for the cedar chat: the recent window in 15-minute steps against the
    baseline's average at the same time of day, inside the target range; and time in range by
    time of day. Each is the generate_chart tool's input, without a path."""
    if not a["recent"]["n"]:
        return []
    mmol = unit == "mmol"
    conv = lambda v: None if v is None else (round(v * MMOL_PER_MGDL, 1) if mmol else round(v))  # noqa: E731
    h, d = a["hours"], a["baseline_days"]
    step = timedelta(minutes=SLOT_MINUTES)
    start = a["now"] - timedelta(hours=h)
    base_start = start - timedelta(days=d)
    # Slots on the quarter hour (12:00, 12:15, ...), from the one the window starts in.
    grid = start.replace(minute=start.minute - start.minute % SLOT_MINUTES, second=0, microsecond=0)
    count = int((a["now"] - grid) / step) + 1

    def slot_of_day(t):
        local = t.astimezone(tz)
        return (local.hour * 60 + local.minute) // SLOT_MINUTES

    typical = {}
    for t, v in readings:
        if base_start <= t < start:
            typical.setdefault(slot_of_day(t), []).append(v)
    slots = {}
    for t, v in readings:
        if start <= t <= a["now"]:
            slots.setdefault(int((t - grid) / step), []).append(v)

    labels, recent, usual = [], [], []
    for k in range(count):
        t = grid + k * step
        local = t.astimezone(tz)
        labels.append(f"{local:%H:%M}" if h <= 24 else f"{WEEKDAYS[language()][local.weekday()]} {local:%H:%M}")
        recent.append(conv(statistics.fmean(slots[k])) if k in slots else None)
        same = typical.get(slot_of_day(t))
        usual.append(conv(statistics.fmean(same)) if same else None)
    # Nothing after the newest reading: the line stops there rather than running on as a gap.
    while recent and recent[-1] is None:
        labels.pop(), recent.pop(), usual.pop()

    series = [{"name": tr("last", h=h), "values": recent}]
    if any(v is not None for v in usual):
        series.append({"name": tr("average", d=d), "values": usual})
    unit_label = "mmol/L" if mmol else "mg/dL"
    out = [{
        "kind": "line", "title": tr("glucose_title", h=h, d=d),
        "labels": labels, "series": series, "y_label": unit_label,
        "band": {"from": conv(low), "to": conv(high), "label": tr("target")},
    }]

    names = [name for name, _, _ in BLOCKS]
    tir = lambda blocks: [round(blocks[n]["tir"]) if blocks[n]["n"] else None for n in names]  # noqa: E731
    by_time = [{"name": tr("last", h=h), "values": tir(a["recent_blocks"])}]
    if a["base"]["n"]:
        by_time.append({"name": tr("average", d=d), "values": tir(a["base_blocks"])})
    out.append({
        "kind": "bar", "title": tr("tir_title", low=decimal(conv(low)), high=decimal(conv(high)), unit=unit_label),
        "labels": [tr(n) for n in names], "series": by_time, "y_label": tr("tir_label"),
    })
    return ["[chart] " + json.dumps(c, separators=(",", ":"), ensure_ascii=False) for c in out]


def export(readings, boluses, basal, tz):
    out = Path("tandem")
    out.mkdir(exist_ok=True)
    iso = lambda t: t.astimezone(tz).isoformat(timespec="minutes")  # noqa: E731
    with open(out / "glucose.csv", "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["time", "mg_dl"])
        w.writerows((iso(t), v) for t, v in readings)
    with open(out / "boluses.csv", "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["time", "units", "carbs_g", "bg_mg_dl", "type"])
        w.writerows((iso(b["t"]), b["units"], b["carbs_g"], b["bg"] or "", b["type"]) for b in boluses)
    with open(out / "basal.csv", "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["time", "rate_u_hr", "source"])
        w.writerows((iso(b["t"]), b["rate"], b["source"]) for b in basal)


def whole(value, default, lo, hi):
    """An integer input in [lo, hi]; missing, null, or not a number gives the default."""
    try:
        return max(lo, min(hi, int(value)))
    except (TypeError, ValueError):
        return default


def target(name, default):
    """A target-range end in mg/dL from the environment. Blank means the default, and a value
    under 30 is taken as mmol/L (so 3.9 and 10 work as well as 70 and 180)."""
    raw = os.environ.get(name, "").strip()
    if not raw:
        return default
    try:
        v = float(raw)
    except ValueError:
        sys.exit(f"{name} must be a number in mg/dL (or mmol/L), not {raw!r}")
    return round(v / MMOL_PER_MGDL) if v < 30 else round(v)


def main():
    args = json.load(sys.stdin) or {}
    hours = whole(args.get("hours"), 24, 6, 72)
    baseline_days = whole(args.get("baseline_days"), 7, 1, 14)

    load_env(HERE / ".env")
    if not os.environ.get("TANDEM_EMAIL") or not os.environ.get("TANDEM_PASSWORD"):
        sys.exit("no Tandem credentials: save TANDEM_EMAIL and TANDEM_PASSWORD in this tool's "
                 f"settings in cedar, or put them in {HERE.name}/.env (see .env.example)")
    unit = "mmol" if os.environ.get("GLUCOSE_UNIT", "").strip().lower().startswith("mmol") else "mgdl"
    low, high = target("TARGET_LOW", 70), target("TARGET_HIGH", 180)
    if not 40 <= low < high <= 400:
        sys.exit(f"the target range {low}-{high} mg/dL doesn't look right; check TARGET_LOW and TARGET_HIGH")

    ensure_deps()
    from zoneinfo import ZoneInfo, ZoneInfoNotFoundError
    try:
        tz = ZoneInfo(os.environ.get("TANDEM_TIMEZONE", "America/New_York").strip())
    except (ZoneInfoNotFoundError, ValueError) as e:
        sys.exit(f"TANDEM_TIMEZONE is not a known IANA timezone: {e}")

    # A day of slack on each side: pump days are local, and the windows are rolling.
    days = baseline_days + hours // 24 + 2
    pump, (readings, boluses, basal, sensors, flagged) = download(days, tz)
    if not readings and not boluses:
        sys.exit("Tandem returned no CGM or bolus data for this period; has the t:connect app uploaded recently?")

    now = datetime.now(timezone.utc)
    a = analyze(readings, boluses, basal, flagged, now, hours, baseline_days, tz, low, high)
    suggestions = recommend(a, tz, low, high, formatter(unit == "mmol"))
    print(report(pump, sensors, a, suggestions, tz, low, high, unit))
    lines = charts(readings, a, tz, low, high, unit)
    if lines:
        print("\n" + "\n".join(lines))
    if args.get("export"):
        export(readings, boluses, basal, tz)


if __name__ == "__main__":
    main()
