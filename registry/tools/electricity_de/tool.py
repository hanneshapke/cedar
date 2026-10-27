#!/usr/bin/env python3
"""electricity_de: a community cedar tool. Reads its input as JSON on stdin and prints the result.

Electricity tariffs and providers for a German postal code (Postleitzahl). Germany has no open
API for retail offers (the comparison portals keep theirs closed), so the tool builds each
tariff type's price from its public parts, the way a Preisblatt does:

- the day-ahead exchange price for Germany (Energy-Charts by Fraunhofer ISE, aWATTar as a
  fallback), which a dynamic tariff passes on hour by hour,
- the state's average grid fee (Netzentgelt) and the municipality's concession fee
  (Konzessionsabgabe, by population, from Open-Meteo's geocoding),
- the national levies, electricity tax and VAT, which are the same everywhere,

plus the supply share each tariff type typically adds. Those rates live in rates.json, dated, so
they are easy to update. The report names the providers that offer each type, the local default
supplier where it is known, and ends with `[chart] {...}` lines (annual cost per option, and the
dynamic price by hour), which the cedar chat draws as charts.

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
from pathlib import Path

GEOCODE = "https://geocoding-api.open-meteo.com/v1/search"
ENERGY_CHARTS = "https://api.energy-charts.info/price"
AWATTAR = "https://api.awattar.de/v1/marketdata"
USER_AGENT = "cedar-electricity_de/1.0 (+https://github.com/hanneshapke/cedar/tree/main/registry/tools/electricity_de)"
RATES = json.loads((Path(__file__).resolve().parent / "rates.json").read_text(encoding="utf-8"))
# Text the user sees in the charts, by CEDAR_LANGUAGE. The report itself stays English for the model.
STRINGS = {
    "en": {"cost_title": "Estimated annual cost, {place}", "per_year": "€/year", "per_year_axis": "€ per year",
           "dynamic": "Dynamic tariff", "fixed": "Fixed-price offer", "hourly_title": "Dynamic price by hour (gross)",
           "hour": "{a} {t}"},
    "de": {"cost_title": "Geschätzte Jahreskosten, {place}", "per_year": "€/Jahr", "per_year_axis": "€ pro Jahr",
           "dynamic": "Dynamischer Tarif", "fixed": "Festpreisangebot", "hourly_title": "Dynamischer Preis pro Stunde (brutto)",
           "hour": "{a} {t}"},
    "fr": {"cost_title": "Coût annuel estimé, {place}", "per_year": "€/an", "per_year_axis": "€ par an",
           "dynamic": "Tarif dynamique", "fixed": "Offre à prix fixe", "hourly_title": "Prix dynamique par heure (TTC)",
           "hour": "{a} {t}"},
}
# The options' chart labels; the English ones are rates.json's.
OPTION_LABELS = {
    "de": {"fixed": "Günstigstes Festpreisangebot", "green": "Zertifizierter Ökostrom",
           "dynamic": "Dynamischer Stundentarif", "default": "Grundversorgung"},
    "fr": {"fixed": "Offre à prix fixe la moins chère", "green": "Électricité verte certifiée",
           "dynamic": "Tarif horaire dynamique", "default": "Fourniture par défaut (Grundversorgung)"},
}
WEEKDAYS = {"en": ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
            "de": ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"],
            "fr": ["lun.", "mar.", "mer.", "jeu.", "ven.", "sam.", "dim."]}


def language():
    """The user's language from CEDAR_LANGUAGE (older servers don't set it: English)."""
    lang = (os.environ.get("CEDAR_LANGUAGE") or "").strip().lower().split("-")[0]
    return lang if lang in STRINGS else "en"


def swiss(text):
    """Swiss German (de-CH) writes ss for ß."""
    if language() == "de" and (os.environ.get("CEDAR_LOCALE") or "").replace("_", "-").lower().startswith("de-ch"):
        return text.replace("ß", "ss")
    return text


def tr(key, **values):
    """A chart string in the user's language."""
    return swiss(STRINGS[language()].get(key, STRINGS["en"][key]).format(**values))


def option_label(option):
    return swiss(OPTION_LABELS.get(language(), {}).get(option["key"], option["label"]))


def hour_label(t):
    return tr("hour", a=WEEKDAYS[language()][t.weekday()], t=f"{t:%H:%M}")

# The state of each two-digit postal region (Leitregion). A few regions straddle a border; the
# geocoded state, when there is one, wins.
PLZ_STATE = {
    **dict.fromkeys(["01", "02", "04", "08", "09"], "Sachsen"),
    **dict.fromkeys(["03", "14", "15", "16"], "Brandenburg"),
    "06": "Sachsen-Anhalt", "39": "Sachsen-Anhalt",
    **dict.fromkeys(["07", "98", "99"], "Thüringen"),
    **dict.fromkeys(["10", "12", "13"], "Berlin"),
    **dict.fromkeys(["17", "18", "19"], "Mecklenburg-Vorpommern"),
    "20": "Hamburg", "22": "Hamburg",
    **dict.fromkeys(["23", "24", "25"], "Schleswig-Holstein"),
    "28": "Bremen",
    **dict.fromkeys(["21", "26", "27", "29", "30", "31", "37", "38", "49"], "Niedersachsen"),
    **dict.fromkeys(["32", "33", "40", "41", "42", "44", "45", "46", "47", "48", "50", "51", "52",
                     "53", "57", "58", "59"], "Nordrhein-Westfalen"),
    **dict.fromkeys(["34", "35", "36", "60", "61", "63", "64", "65"], "Hessen"),
    **dict.fromkeys(["54", "55", "56", "67"], "Rheinland-Pfalz"),
    "66": "Saarland",
    **dict.fromkeys(["68", "69", "70", "71", "72", "73", "74", "75", "76", "77", "78", "79", "88",
                     "89"], "Baden-Württemberg"),
    **dict.fromkeys(["80", "81", "82", "83", "84", "85", "86", "87", "90", "91", "92", "93", "94",
                     "95", "96", "97"], "Bayern"),
}
# Open-Meteo names states in English when asked to; map those back.
STATE_ALIASES = {
    "bavaria": "Bayern", "lower saxony": "Niedersachsen", "north rhine-westphalia": "Nordrhein-Westfalen",
    "hesse": "Hessen", "rhineland-palatinate": "Rheinland-Pfalz", "saxony": "Sachsen",
    "saxony-anhalt": "Sachsen-Anhalt", "thuringia": "Thüringen", "mecklenburg-western pomerania":
    "Mecklenburg-Vorpommern", "mecklenburg-vorpommern": "Mecklenburg-Vorpommern",
    "land berlin": "Berlin", "free and hanseatic city of hamburg": "Hamburg",
    "free hanseatic city of bremen": "Bremen", "baden-wurttemberg": "Baden-Württemberg",
    "baden-württemberg": "Baden-Württemberg", "thüringen": "Thüringen",
}
# A standard household load profile (BDEW H0, averaged over the year): the share of a day's use
# in each hour. Weights a dynamic tariff's hourly price the way a household without a battery,
# heat pump or car actually draws power.
H0 = [0.62, 0.52, 0.47, 0.45, 0.45, 0.50, 0.66, 0.88, 0.98, 1.00, 1.01, 1.06, 1.15, 1.10,
      1.00, 0.95, 0.99, 1.13, 1.33, 1.44, 1.36, 1.21, 1.02, 0.82]
# Typical yearly use by household size, kWh (Stromspiegel, flat with electric water heating off).
HOUSEHOLD_KWH = {1: 1500, 2: 2500, 3: 3200, 4: 4000, 5: 4800, 6: 5500}
HEAT_PUMP_KWH, EV_KWH = 4500, 2200  # a typical heat pump in an existing house; ~13,000 km a year


def fail(message):
    sys.exit(message)


def get(url, params, source):
    full = f"{url}?{urllib.parse.urlencode(params)}"
    req = urllib.request.Request(full, headers={"User-Agent": USER_AGENT, "Accept": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            return json.load(resp)
    except urllib.error.HTTPError as e:
        raise LookupError(f"{source} answered {e.code}: {e.reason}") from None
    except (urllib.error.URLError, TimeoutError, ValueError, OSError) as e:
        raise LookupError(f"couldn't reach {source} ({e})") from None


def number(args, key, low, high):
    value = args.get(key)
    if value is None or value == "":
        return None
    try:
        value = float(value)
    except (TypeError, ValueError):
        fail(f"{key} must be a number")
    if not low <= value <= high:
        fail(f"{key} must be between {low:g} and {high:g}")
    return value


def euro(value):
    return f"{value:,.0f} €".replace(",", ".")


def kwh_text(value):
    return f"{value:,} kWh".replace(",", ".")


# --- time in Germany ---------------------------------------------------------------------------

def last_sunday(year, month):
    d = date(year, month + 1, 1) - timedelta(days=1) if month < 12 else date(year, 12, 31)
    return d - timedelta(days=(d.weekday() + 1) % 7)


def berlin(ts):
    """A UTC timestamp (seconds) as a naive datetime in German time. EU rule: summer time from
    01:00 UTC on the last Sunday of March to 01:00 UTC on the last Sunday of October."""
    utc = datetime.fromtimestamp(ts, timezone.utc).replace(tzinfo=None)
    start = datetime.combine(last_sunday(utc.year, 3), datetime.min.time()) + timedelta(hours=1)
    end = datetime.combine(last_sunday(utc.year, 10), datetime.min.time()) + timedelta(hours=1)
    return utc + timedelta(hours=2 if start <= utc < end else 1)


# --- the place ---------------------------------------------------------------------------------

def normalize_state(name):
    if not name:
        return None
    if name in RATES["grid_fee_ct"]:
        return name
    return STATE_ALIASES.get(name.strip().lower())


def find_place(postal_code, location):
    """(label, state, population, postal_code) for the user's place. The postal code decides the
    state when geocoding fails; a place name alone must be geocoded."""
    query = postal_code or location
    hits, note = [], None
    try:
        hits = get(GEOCODE, {"name": query, "count": 10, "language": "de", "format": "json",
                             "countryCode": "DE"}, "Open-Meteo geocoding").get("results") or []
    except LookupError as e:
        note = str(e)
    hits = [h for h in hits if (h.get("country_code") or "DE").upper() == "DE"]
    if postal_code:
        # Prefer hits that list the code, then the most populous (the town, not a district).
        hits.sort(key=lambda h: (postal_code not in (h.get("postcodes") or []), -(h.get("population") or 0)))
    elif location and "," in location:
        wanted = [w.strip().lower() for w in location.split(",")[1:] if w.strip()]
        hits.sort(key=lambda h: -sum(any(w in (h.get(k) or "").lower() for k in ("admin1", "admin2", "admin3"))
                                     for w in wanted))
    hit = hits[0] if hits else None
    if not hit and not postal_code:
        fail(f"couldn't find {location!r} in Germany" + (f" ({note})" if note else "")
             + ". Ask the user for their 5-digit postal code (Postleitzahl) with ask_user, then call again.")
    state = normalize_state(hit.get("admin1")) if hit else None
    if not state and postal_code:
        state = PLZ_STATE.get(postal_code[:2])
    if not state:
        fail(f"couldn't tell which state {query!r} is in; check the postal code")
    label = None
    if hit:
        parts = [hit.get("name")]
        if hit.get("admin1") and hit.get("admin1") != hit.get("name"):
            parts.append(state)
        label = ", ".join(p for p in parts if p)
    code = postal_code or (next(iter(hit.get("postcodes") or []), None) if hit else None)
    return label or f"PLZ {postal_code}, {state}", state, (hit or {}).get("population"), code


def concession_fee(population):
    """Konzessionsabgabe for household customers, by the municipality's population (KAV §2)."""
    if not population:
        return RATES["concession_ct"]["unknown"], "population unknown, middle tier assumed"
    for tier in RATES["concession_ct"]["tiers"]:
        if population <= tier["up_to"]:
            break
    return tier["ct"], f"town of about {population:,} people".replace(",", ".")


# --- the exchange price ------------------------------------------------------------------------

def hourly(points):
    """[(unix_seconds, EUR/MWh)] at any resolution (day-ahead is quarter-hourly since October
    2025) as [(German local datetime of the hour, ct/kWh)]."""
    hours = {}
    for ts, price in points:
        if price is None:
            continue
        key = int(ts) // 3600 * 3600
        hours.setdefault(key, []).append(float(price))
    return [(berlin(k), sum(v) / len(v) / 10) for k, v in sorted(hours.items())]


def spot_prices(days=7):
    """The day-ahead price for the last `days` days and tomorrow, when published."""
    today = datetime.now(timezone.utc).date()
    start, end = today - timedelta(days=days), today + timedelta(days=2)
    try:
        data = get(ENERGY_CHARTS, {"bzn": "DE-LU", "start": start.isoformat(), "end": end.isoformat()},
                   "Energy-Charts")
        points = list(zip(data.get("unix_seconds") or [], data.get("price") or []))
        if points:
            return hourly(points), "Energy-Charts (Fraunhofer ISE), EPEX day-ahead DE-LU"
    except LookupError:
        pass
    try:
        to_ms = lambda d: int(datetime.combine(d, datetime.min.time(), timezone.utc).timestamp() * 1000)
        data = get(AWATTAR, {"start": to_ms(start), "end": to_ms(end)}, "aWATTar")
        points = [(row["start_timestamp"] / 1000, row.get("marketprice")) for row in data.get("data") or []]
        if points:
            return hourly(points), "aWATTar, EPEX day-ahead DE-LU"
    except (LookupError, KeyError, TypeError):
        pass
    return [], None


def weighted_spot(hours):
    """The exchange price a standard household pays on average, weighted by the H0 profile."""
    if not hours:
        return None
    total = sum(H0[t.hour] for t, _ in hours)
    return sum(H0[t.hour] * p for t, p in hours) / total


def cheapest_window(hours, length):
    """The cheapest `length` consecutive hours from now on: (start, end, average ct/kWh)."""
    now = berlin(datetime.now(timezone.utc).timestamp()).replace(minute=0, second=0, microsecond=0)
    future = [(t, p) for t, p in hours if t >= now]
    best = None
    for i in range(len(future) - length + 1):
        window = future[i:i + length]
        if (window[-1][0] - window[0][0]).total_seconds() != 3600 * (length - 1):
            continue
        avg = sum(p for _, p in window) / length
        if best is None or avg < best[2]:
            best = (window[0][0], window[-1][0] + timedelta(hours=1), avg)
    return best


# --- the tariffs -------------------------------------------------------------------------------

def gross(net_ct):
    return net_ct * (1 + RATES["vat"])


def build_options(kwh, grid_ct, concession_ct, spot_ct, extras):
    """Each tariff type's price for this place: (key, label, ct/kWh gross, €/month base gross,
    €/year gross)."""
    fixed = grid_ct + concession_ct + sum(RATES["levies_ct"].values()) + RATES["electricity_tax_ct"]
    options = []
    for key, t in RATES["tariff_types"].items():
        if key == "dynamic":
            if spot_ct is None:
                energy, source = RATES["fallback_spot_ct"], "fallback"
            else:
                energy, source = spot_ct, "live"
            energy += t["markup_ct"]
        else:
            energy, source = t["energy_ct"], None
        work = gross(energy + fixed)
        base = t["base_eur_month"] + extras.get(key, 0)
        options.append({"key": key, "label": t["label"], "ct": work, "base": base,
                        "year": work * kwh / 100 + base * 12, "energy_net": energy, "source": source,
                        "note": t.get("note", "")})
    return options


def heat_pump_option(kwh_hp, grid_ct, concession_ct):
    """A separate heat-pump meter under §14a EnWG module 2: 60 % off the grid work price, a
    lower concession fee, and a heat-pump tariff's supply share."""
    t = RATES["heat_pump"]
    fixed = (grid_ct * (1 - t["grid_discount"]) + RATES["concession_ct"]["off_peak"]
             + sum(RATES["levies_ct"].values()) + RATES["electricity_tax_ct"])
    work = gross(t["energy_ct"] + fixed)
    return work, work * kwh_hp / 100 + t["base_eur_month"] * 12


# --- the report --------------------------------------------------------------------------------

def default_supplier(place):
    city = (place or "").split(",")[0].strip()
    return RATES["default_suppliers"].get(city)


def report(args, place, state, code, population, kwh, kwh_note, spot, spot_source):
    grid_override = number(args, "grid_fee_ct", 0, 40)
    grid_ct = grid_override if grid_override is not None else RATES["grid_fee_ct"][state]
    concession_ct, concession_note = concession_fee(population)
    spot_ct = weighted_spot(spot)
    green_only = bool(args.get("green_only"))
    options = build_options(kwh, grid_ct, concession_ct, spot_ct,
                            {"dynamic": RATES["smart_meter_eur_month"]})
    if green_only:
        options = [o for o in options if o["key"] in ("green", "dynamic")]
    options.sort(key=lambda o: o["year"])

    where = place + (f" (PLZ {code})" if code and code not in place else "")
    out = [f"Electricity in {where} for {kwh_text(kwh)} a year{kwh_note}"
           + f". Estimates as of {RATES['as_of']}, gross with 19 % VAT, cheapest first:"]
    out.append("")
    for o in options:
        line = (f"- {o['label']}: about {o['ct']:.1f} ct/kWh + {o['base']:.2f} €/month "
                f"= {euro(o['year'])}/year ({euro(o['year'] / 12)}/month)")
        if o["key"] == "dynamic":
            line += (f"; exchange price averaged {o['energy_net'] - RATES['tariff_types']['dynamic']['markup_ct']:.1f} ct/kWh net over the last days"
                     if o["source"] == "live" else "; live exchange prices unavailable, a typical average was used")
        out.append(line + (f". {o['note']}" if o["note"] else "."))

    current_ct = number(args, "current_price_ct", 5, 150)
    current_base = number(args, "current_base_eur_month", 0, 100) or 0
    if current_ct is not None:
        current_year = current_ct * kwh / 100 + current_base * 12
        best = options[0]
        who = str(args.get("current_provider") or "").strip()
        out.append("")
        out.append(f"Your current tariff{f' ({who})' if who else ''}: {current_ct:.1f} ct/kWh + "
                   f"{current_base:.2f} €/month = {euro(current_year)}/year.")
        diff = current_year - best["year"]
        if diff > 30:
            out.append(f"Switching to the {best['label'].lower()} could save about {euro(diff)}/year.")
        else:
            out.append("That is already about as cheap as the options here; switching saves little.")

    if args.get("heat_pump"):
        kwh_hp = int(number(args, "heat_pump_kwh", 500, 50000) or HEAT_PUMP_KWH)
        hp_ct, hp_year = heat_pump_option(kwh_hp, grid_ct, concession_ct)
        out.append("")
        out.append(f"Heat pump on its own meter (§14a EnWG, grid fee module 2): about {hp_ct:.1f} ct/kWh, "
                   f"{euro(hp_year)}/year for {kwh_text(kwh_hp)}"
                   + ". The grid operator may then throttle it briefly at peak times (never below 4.2 kW). "
                   "Without a second meter, module 1 gives a flat grid-fee discount instead (typically "
                   "110-190 €/year). A dynamic tariff suits a heat pump well if a smart meter is installed.")
    if args.get("ev"):
        out.append("")
        out.append("Electric car: a wallbox also qualifies for the §14a grid-fee discount. Charging in the "
                   "cheapest night or midday hours on a dynamic tariff often costs well under the fixed price; "
                   "some providers (Octopus Intelligent Go, Tibber, Rabot) offer car tariffs with set cheap hours.")

    if spot:
        out.append("")
        window = cheapest_window(spot, 3)
        if window:
            s, e, p = window
            out.append(f"Cheapest 3 hours ahead on the exchange: {s:%a %H:%M}-{e:%H:%M}, "
                       f"{gross(p + RATES['tariff_types']['dynamic']['markup_ct'] + grid_ct + concession_ct + sum(RATES['levies_ct'].values()) + RATES['electricity_tax_ct']):.1f} ct/kWh "
                       f"on a dynamic tariff (exchange {p:.1f} ct net). Good for the washing machine, dishwasher or car.")
        negative = sum(1 for _, p in spot if p < 0)
        if negative:
            out.append(f"The exchange price was negative in {negative} hours of the last days.")

    out.append("")
    grid_note = "your value" if grid_override is not None else \
        f"average for {state}; your grid operator can differ by 2-3 ct either way"
    out.append(f"How the price is built here: grid fee {grid_ct:.2f} ct/kWh net ({grid_note}), "
               f"concession fee {concession_ct:.2f} ct ({concession_note})"
               f", levies {sum(RATES['levies_ct'].values()):.3f} ct, electricity tax {RATES['electricity_tax_ct']:.2f} ct, plus each tariff's supply share, all plus 19 % VAT.")

    supplier = default_supplier(place)
    out.append("")
    out.append("Providers to compare:")
    for key in [o["key"] for o in options]:
        names = RATES["providers"].get(key)
        if names:
            out.append(f"- {RATES['tariff_types'][key]['label']}: " + "; ".join(
                f"{p['name']} ({p['url']})" + (f" - {p['note']}" if p.get("note") else "") for p in names))
    out.append(f"- Local default supplier (Grundversorger): {supplier}" if supplier else
               "- Local default supplier (Grundversorger): named on the user's bill or by the local Stadtwerke; "
               "often the most expensive option, but with a two-week notice period.")
    out.append("Live offers for this postal code, with exact prices: "
               + ", ".join(f"{p['name']} ({p['url']})" for p in RATES["comparison_sites"])
               + ". Compare them without new-customer bonuses, and check the second year's price.")
    out.append("")
    out.append("What to look for: a price guarantee (Preisgarantie) of 12 months or more, a contract term of at "
               "most 12 months, at most one month's notice, monthly payments rather than prepayment, and for "
               "green power a label (ok-power, Grüner Strom) rather than plain certificates. Switching is free; the "
               "new provider cancels the old contract. Grid fees fall in 2026 thanks to a federal subsidy that "
               "shrinks in 2027, so a guarantee that runs into 2027 is worth more.")
    out.append(f"Sources: {spot_source or 'no live exchange prices'}; grid fees from BNetzA and portal "
               f"averages; levies from netztransparenz.de ({RATES['as_of']}). Estimates, not offers.")
    return "\n".join(out), options, grid_ct, concession_ct


def charts(options, spot, grid_ct, concession_ct, place):
    out = [{"kind": "barh", "title": tr("cost_title", place=place.split(",")[0]),
            "labels": [option_label(o) for o in options],
            "series": [{"name": tr("per_year"), "values": [round(o["year"]) for o in options]}],
            "x_label": tr("per_year_axis")}]
    if spot:
        fixed = grid_ct + concession_ct + sum(RATES["levies_ct"].values()) + RATES["electricity_tax_ct"]
        markup = RATES["tariff_types"]["dynamic"]["markup_ct"]
        recent = spot[-72:]
        fixed_price = next((o["ct"] for o in options if o["key"] == "fixed"), None)
        series = [{"name": tr("dynamic"), "values": [round(gross(p + markup + fixed), 1) for _, p in recent]}]
        if fixed_price:
            series.append({"name": tr("fixed"), "values": [round(fixed_price, 1)] * len(recent)})
        out.append({"kind": "line", "title": tr("hourly_title"),
                    "labels": [hour_label(t) for t, _ in recent], "series": series, "y_label": "ct/kWh"})
    return ["[chart] " + json.dumps(c, separators=(",", ":"), ensure_ascii=False) for c in out]


def consumption(args):
    kwh = number(args, "annual_kwh", 100, 100000)
    if kwh is not None:
        return int(kwh), ""
    size = int(number(args, "household_size", 1, 12) or 2)
    kwh = HOUSEHOLD_KWH[min(size, 6)]
    parts = [f"{size}-person household"]
    if args.get("ev"):
        kwh += EV_KWH
        parts.append(f"+{kwh_text(EV_KWH)} for an electric car")
    return kwh, " (estimated: " + ", ".join(parts) + "; give annual_kwh from the bill for a better figure)"


def main():
    args = json.load(sys.stdin) or {}
    postal_code = re.sub(r"\s", "", str(args.get("postal_code") or ""))
    location = str(args.get("location") or "").strip()
    if not postal_code and re.fullmatch(r"\d{5}", location):
        postal_code, location = location, ""
    if postal_code and not re.fullmatch(r"\d{5}", postal_code):
        fail(f"{postal_code!r} is not a German postal code (5 digits, like 10115)")
    if postal_code and postal_code[:2] not in PLZ_STATE:
        fail(f"{postal_code} is not a postal code in use in Germany; ask the user to check it")
    if not postal_code and not location:
        fail("no location: ask the user for their 5-digit postal code (Postleitzahl) with ask_user "
             "(optionally also their yearly kWh or household size), then call again")

    place, state, population, code = find_place(postal_code, location)
    kwh, kwh_note = consumption(args)
    spot, spot_source = spot_prices()
    text, options, grid_ct, concession_ct = report(args, place, state, code, population, kwh, kwh_note,
                                                   spot, spot_source)
    print(text)
    print("\n" + "\n".join(charts(options, spot, grid_ct, concession_ct, place)))


if __name__ == "__main__":
    main()
