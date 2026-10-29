#!/usr/bin/env python3
"""flight_search: a community cedar tool. Reads its input as JSON on stdin and prints the result.

Searches flights on Google Flights through SerpApi's Google Flights API (a free SerpApi plan
covers a limited number of searches a month): one-way and round trips between airports, with
passengers, cabin, stops, airlines, a price cap and a sort order. It lists the options with times,
stops and layovers, duration, aircraft, emissions and price, says whether today's prices are low
or high for the route, and links the search on Google Flights. A round trip's outbound options
carry a token that lists the matching return flights; any option's booking token lists where to
book it. The report ends with a `[chart] {...}` line of the route's recent price history, which
the cedar chat draws as a chart (the same spec as the generate_chart tool's input).

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
from datetime import date, datetime, timezone

SERPAPI = "https://serpapi.com/search.json"
USER_AGENT = "cedar-flight_search/1.0 (+https://github.com/hanneshapke/cedar/tree/main/registry/tools/flight_search)"
AIRPORT = re.compile(r"^[A-Z]{3}$")
AIRLINE = re.compile(r"^[A-Z0-9]{2}$")
CABINS = {"economy": 1, "premium_economy": 2, "business": 3, "first": 4}
SORTS = {"best": 1, "price": 2, "departure": 3, "arrival": 4, "duration": 5, "emissions": 6}
# Google Flights' stops filter: 0 any, 1 nonstop only, 2 one stop or fewer, 3 two stops or fewer.
STOPS = {0: 1, 1: 2, 2: 3}
# The currency a region usually pays in, when the call names none.
CURRENCIES = {
    "US": "USD", "GB": "GBP", "CA": "CAD", "AU": "AUD", "NZ": "NZD", "CH": "CHF", "MX": "MXN",
    "BR": "BRL", "AR": "ARS", "CL": "CLP", "CO": "COP", "PE": "PEN", "JP": "JPY", "IN": "INR",
    "SE": "SEK", "NO": "NOK", "DK": "DKK", "PL": "PLN", "CZ": "CZK", "419": "USD",
}
EURO = {"DE", "AT", "FR", "BE", "LU", "ES", "IT", "NL", "IE", "PT", "FI", "GR"}
# Text the user sees in the chart, by CEDAR_LANGUAGE. The report itself stays English for the model.
STRINGS = {
    "en": {"title": "Price history, {route}", "series": "Lowest price", "typical": "typical", "day": "{d:02d} {b}"},
    "de": {"title": "Preisverlauf, {route}", "series": "Niedrigster Preis", "typical": "üblich", "day": "{d}. {b}"},
    "fr": {"title": "Historique des prix, {route}", "series": "Prix le plus bas", "typical": "habituel",
           "day": "{d} {b}"},
    "es": {"title": "Historial de precios, {route}", "series": "Precio más bajo", "typical": "habitual",
           "day": "{d} {b}"},
}
MONTHS = {"en": ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
          "de": ["Jan", "Feb", "Mär", "Apr", "Mai", "Jun", "Jul", "Aug", "Sep", "Okt", "Nov", "Dez"],
          "fr": ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."],
          "es": ["ene.", "feb.", "mar.", "abr.", "may.", "jun.", "jul.", "ago.", "sept.", "oct.", "nov.", "dic."]}


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


def region():
    """The region of CEDAR_LOCALE (de-CH -> CH), or None."""
    parts = (os.environ.get("CEDAR_LOCALE") or "").replace("_", "-").split("-")
    return parts[1].upper() if len(parts) > 1 and parts[1] else None


def default_currency():
    r = region()
    return "EUR" if r in EURO else CURRENCIES.get(r, "USD")


def whole(value, default, low, high):
    try:
        return max(low, min(high, int(value)))
    except (TypeError, ValueError):
        return default


def get(params):
    key = (os.environ.get("SERPAPI_API_KEY") or "").strip()
    if not key:
        sys.exit("SERPAPI_API_KEY is not set: an admin saves a SerpApi key in cedar's settings "
                 "(Integrations → Community tools → flight_search); a free key is at serpapi.com")
    full = f"{SERPAPI}?{urllib.parse.urlencode({**params, 'engine': 'google_flights', 'api_key': key})}"
    req = urllib.request.Request(full, headers={"User-Agent": USER_AGENT, "Accept": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            data = json.load(resp)
    except urllib.error.HTTPError as e:
        try:
            reason = json.load(e).get("error") or e.reason
        except ValueError:
            reason = e.reason
        if e.code == 401:
            sys.exit(f"SerpApi refused the key ({reason}); an admin should check SERPAPI_API_KEY")
        if e.code == 429:
            sys.exit(f"SerpApi's limit is used up ({reason}); try again later or with a bigger plan")
        sys.exit(f"SerpApi answered {e.code}: {reason}")
    except (urllib.error.URLError, TimeoutError, ValueError) as e:
        sys.exit(f"couldn't reach SerpApi ({e}); try again in a little while")
    error = data.get("error")
    if error and "hasn't returned any results" in error:
        return {"no_results": True, **data}
    if error:
        sys.exit(f"SerpApi: {error}")
    return data


# --- the request -----------------------------------------------------------------------------

def codes(args, key, pattern, what, required=False):
    """A comma-separated list of codes (airports or airlines), checked and upper-cased."""
    raw = args.get(key)
    if isinstance(raw, list):
        raw = ",".join(str(x) for x in raw)
    items = [c.strip().upper() for c in str(raw or "").split(",") if c.strip()]
    if required and not items:
        sys.exit(f"give {key}: one or more {what}, comma-separated")
    bad = [c for c in items if not pattern.match(c)]
    if bad:
        sys.exit(f"{key}: {', '.join(bad)} {'is' if len(bad) == 1 else 'are'} not {what}; "
                 + ("use three-letter IATA airport codes such as JFK, or list a city's airports (JFK,EWR,LGA)"
                    if pattern is AIRPORT else "use two-character IATA airline codes such as LH or UA"))
    if len(items) > 7:
        sys.exit(f"{key}: at most 7 codes")
    return ",".join(items)


def day(args, key, required=False):
    value = str(args.get(key) or "").strip()
    if not value:
        if required:
            sys.exit(f"give {key} as YYYY-MM-DD")
        return None
    try:
        d = date.fromisoformat(value)
    except ValueError:
        sys.exit(f"{key} must be a date like 2026-12-01, not {value!r}")
    if d < date.today():
        sys.exit(f"{key} {d} is in the past; today is {date.today()}")
    if (d - date.today()).days > 330:
        sys.exit(f"{key} {d} is too far ahead: airlines sell about 11 months out")
    return d


def search_params(args):
    origin = codes(args, "origin", AIRPORT, "airport codes", required=True)
    destination = codes(args, "destination", AIRPORT, "airport codes", required=True)
    if set(origin.split(",")) & set(destination.split(",")):
        sys.exit("origin and destination share an airport")
    out_date = day(args, "departure_date", required=True)
    back_date = day(args, "return_date")
    if back_date and back_date < out_date:
        sys.exit("return_date is before departure_date")
    adults = whole(args.get("adults"), 1, 0, 9)
    children = whole(args.get("children"), 0, 0, 9)
    in_seat = whole(args.get("infants_in_seat"), 0, 0, 9)
    on_lap = whole(args.get("infants_on_lap"), 0, 0, 9)
    if adults + children + in_seat + on_lap > 9:
        sys.exit("at most 9 passengers in one search")
    if adults == 0 and children == 0:
        sys.exit("at least one adult or child has to travel")
    if on_lap > adults:
        sys.exit("each infant on a lap needs an adult")
    cabin = str(args.get("cabin") or "economy").lower().replace(" ", "_").replace("-", "_")
    if cabin not in CABINS:
        sys.exit(f"cabin is one of {', '.join(CABINS)}, not {cabin!r}")
    sort = str(args.get("sort") or "best").lower()
    if sort not in SORTS:
        sys.exit(f"sort is one of {', '.join(SORTS)}, not {sort!r}")
    currency = str(args.get("currency") or default_currency()).strip().upper()
    if not re.match(r"^[A-Z]{3}$", currency):
        sys.exit(f"currency must be a three-letter code such as USD or EUR, not {currency!r}")
    params = {
        "departure_id": origin, "arrival_id": destination, "outbound_date": out_date.isoformat(),
        "type": 1 if back_date else 2, "travel_class": CABINS[cabin], "adults": adults,
        "children": children, "infants_in_seat": in_seat, "infants_on_lap": on_lap,
        "sort_by": SORTS[sort], "currency": currency, "hl": "en",
    }
    if back_date:
        params["return_date"] = back_date.isoformat()
    if region() and region() != "419":
        params["gl"] = region().lower()
    if args.get("max_stops") is not None:
        params["stops"] = STOPS[whole(args.get("max_stops"), 2, 0, 2)]
    if args.get("max_price") is not None:
        params["max_price"] = whole(args.get("max_price"), 0, 1, 1_000_000)
    if args.get("carry_on_bags") is not None:
        params["bags"] = whole(args.get("carry_on_bags"), 0, 0, 2)
    include = codes(args, "include_airlines", AIRLINE, "airline codes")
    exclude = codes(args, "exclude_airlines", AIRLINE, "airline codes")
    if include and exclude:
        sys.exit("give include_airlines or exclude_airlines, not both")
    if include:
        params["include_airlines"] = include
    if exclude:
        params["exclude_airlines"] = exclude
    return params


# --- the report ------------------------------------------------------------------------------

def hm(minutes):
    if minutes is None:
        return "?"
    h, m = divmod(int(minutes), 60)
    return f"{h}h {m:02d}m" if h else f"{m}m"


def clock(text):
    """'2026-12-01 08:15' -> ('Tue 01 Dec', '08:15'); anything else as it is."""
    try:
        t = datetime.strptime(text, "%Y-%m-%d %H:%M")
    except (TypeError, ValueError):
        return None, text or "?"
    return f"{t:%a %d %b}", f"{t:%H:%M}"


def option_lines(n, option, currency, round_trip):
    legs = option.get("flights") or []
    if not legs:
        return []
    first, last = legs[0], legs[-1]
    dep_day, dep = clock(first.get("departure_airport", {}).get("time"))
    arr_day, arr = clock(last.get("arrival_airport", {}).get("time"))
    later = f" ({arr_day})" if arr_day and arr_day != dep_day else ""
    stops = len(legs) - 1
    airlines = []
    for leg in legs:
        if leg.get("airline") and leg["airline"] not in airlines:
            airlines.append(leg["airline"])
    price = option.get("price")
    price = f"{price:,} {currency}" if isinstance(price, (int, float)) else "no price shown"
    if round_trip and option.get("departure_token"):
        price += " round trip"
    head = (f"{n}. {price}: {' + '.join(airlines) or 'unknown airline'}, "
            f"{first.get('departure_airport', {}).get('id', '?')} {dep_day or ''} {dep} -> "
            f"{last.get('arrival_airport', {}).get('id', '?')} {arr}{later}, "
            f"{'nonstop' if stops == 0 else f'{stops} stop' + ('s' if stops > 1 else '')}, "
            f"{hm(option.get('total_duration'))}")
    out = [head]
    for leg in legs:
        bits = [f"{leg.get('flight_number', '?')} {leg.get('departure_airport', {}).get('id', '?')}"
                f"-{leg.get('arrival_airport', {}).get('id', '?')} {clock(leg.get('departure_airport', {}).get('time'))[1]}"
                f"-{clock(leg.get('arrival_airport', {}).get('time'))[1]}"]
        bits += [x for x in (leg.get("airplane"), leg.get("travel_class"), leg.get("legroom")) if x]
        if leg.get("often_delayed_by_over_30_min"):
            bits.append("often delayed 30+ min")
        if leg.get("overnight"):
            bits.append("overnight")
        out.append("   - " + ", ".join(bits))
    for layover in option.get("layovers") or []:
        note = f"   - layover {hm(layover.get('duration'))} at {layover.get('name') or layover.get('id', '?')}"
        if layover.get("overnight"):
            note += " (overnight)"
        out.append(note)
    co2 = option.get("carbon_emissions") or {}
    if co2.get("this_flight"):
        diff = co2.get("difference_percent")
        rel = f", {abs(diff)}% {'above' if diff > 0 else 'below'} typical" if diff else ""
        out.append(f"   - CO2 about {co2['this_flight'] / 1000:.0f} kg per passenger{rel}")
    for key, label in (("departure_token", "return flights"), ("booking_token", "booking options")):
        if option.get(key) and (key != "departure_token" or round_trip):
            out.append(f"   - {label}: {key}={option[key]}")
    return out


def report(params, data, max_results, returns):
    currency = params["currency"]
    round_trip = params["type"] == 1 and not returns
    route = f"{params['departure_id']} -> {params['arrival_id']}"
    if returns:
        route = f"{params['arrival_id']} -> {params['departure_id']}"
    when = params["return_date"] if returns else params["outbound_date"]
    kind = "Return flights" if returns else ("Round trip, outbound flights" if params["type"] == 1 else "One-way flights")
    party = [f"{params[k]} {one if params[k] == 1 else many}"
             for k, one, many in (("adults", "adult", "adults"), ("children", "child", "children"),
                                  ("infants_in_seat", "infant in seat", "infants in seat"),
                                  ("infants_on_lap", "infant on lap", "infants on lap")) if params[k]]
    cabin = next(c for c, v in CABINS.items() if v == params["travel_class"]).replace("_", " ")
    out = [f"{kind} {route} on {when}"
           + (f" (back {params['return_date']})" if round_trip else "")
           + f", {', '.join(party)}, {cabin}, from Google Flights via SerpApi"
           + f" (fetched {datetime.now(timezone.utc):%Y-%m-%d %H:%M} UTC). Prices in {currency}"
           + (" for the whole round trip" if params["type"] == 1 else "")
           + ", for all passengers together, before bags and seat fees unless noted."]
    if data.get("no_results"):
        out.append("Google Flights found no flights for this search. Try nearby dates, more airports, "
                   "more stops, or without the price cap or airline filter.")
        return out
    best = data.get("best_flights") or []
    other = data.get("other_flights") or []
    options = best + other
    if not options:
        out.append("No flights in the answer.")
    shown = options[:max_results]
    if best and params["sort_by"] == 1:
        out.append(f"Google's top picks first ({min(len(best), len(shown))}), then the others.")
    for i, option in enumerate(shown, 1):
        out.append("")
        out.extend(option_lines(i, option, currency, round_trip))
    if len(options) > len(shown):
        out.append("")
        out.append(f"{len(options) - len(shown)} more options not shown (raise max_results to see them).")

    insights = data.get("price_insights") or {}
    if insights.get("lowest_price") is not None:
        line = f"Price insight: lowest {insights['lowest_price']:,} {currency}"
        if insights.get("price_level"):
            line += f", which Google rates {insights['price_level']} for this route"
        rng = insights.get("typical_price_range")
        if rng and len(rng) == 2:
            line += f" (typical {rng[0]:,}-{rng[1]:,} {currency})"
        out.append("")
        out.append(line + ".")
    link = (data.get("search_metadata") or {}).get("google_flights_url")
    if link:
        out.append(f"See or book on Google Flights: {link}")
    if round_trip and any(o.get("departure_token") for o in shown):
        out.append("To list the return flights for an outbound option, call again with the same search "
                   "and that option's departure_token.")
    if any(o.get("booking_token") for o in shown):
        out.append("To see where to book an option (airline or agency, with their price), call with its booking_token.")
    return out


def booking_report(data, currency):
    out = []
    options = data.get("booking_options") or []
    if not options:
        return ["Google Flights lists no booking options for this choice any more; search again for fresh prices."]
    out.append(f"Where to book (prices in {currency}, for all passengers):")
    for option in options[:10]:
        parts = [option[k] for k in ("together", "departing", "returning") if option.get(k)]
        for part in parts:
            price = part.get("price")
            line = f"- {part.get('book_with', '?')}: " + (f"{price:,} {currency}" if price is not None else "price on request")
            if part.get("marketed_as"):
                line += f" ({', '.join(part['marketed_as'])})"
            if part.get("baggage_prices"):
                line += f"; bags: {'; '.join(part['baggage_prices'])}"
            if part.get("option_title"):
                line += f"; fare: {part['option_title']}"
            req = part.get("booking_request") or {}
            if req.get("url"):
                line += f"; link: {req['url']}"
            out.append(line)
    link = (data.get("search_metadata") or {}).get("google_flights_url")
    if link:
        out.append(f"On Google Flights: {link}")
    return out


def chart(data, params):
    history = (data.get("price_insights") or {}).get("price_history") or []
    points = [(datetime.fromtimestamp(p[0], timezone.utc).date(), p[1]) for p in history
              if isinstance(p, list) and len(p) == 2 and isinstance(p[1], (int, float))]
    if len(points) < 2:
        return []
    lang = language()
    spec = {"kind": "line", "title": tr("title", route=f"{params['departure_id']} → {params['arrival_id']}"),
            "labels": [tr("day", d=d.day, b=MONTHS[lang][d.month - 1]) for d, _ in points],
            "series": [{"name": tr("series"), "values": [v for _, v in points]}],
            "y_label": params["currency"]}
    rng = (data.get("price_insights") or {}).get("typical_price_range")
    if rng and len(rng) == 2:
        spec["band"] = {"from": rng[0], "to": rng[1], "label": tr("typical")}
    return ["[chart] " + json.dumps(spec, separators=(",", ":"), ensure_ascii=False)]


def main():
    args = json.load(sys.stdin) or {}
    max_results = whole(args.get("max_results"), 8, 1, 30)
    params = search_params(args)
    booking = str(args.get("booking_token") or "").strip()
    departure = str(args.get("departure_token") or "").strip()
    if booking:
        print("\n".join(booking_report(get({**params, "booking_token": booking}), params["currency"])))
        return
    if departure and params["type"] != 1:
        sys.exit("departure_token is for round trips: give the same return_date as the first search")
    data = get({**params, "departure_token": departure} if departure else params)
    print("\n".join(report(params, data, max_results, returns=bool(departure))))
    lines = [] if departure else chart(data, params)
    if lines:
        print("\n" + "\n".join(lines))


if __name__ == "__main__":
    main()
