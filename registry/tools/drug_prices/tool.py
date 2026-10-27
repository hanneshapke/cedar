#!/usr/bin/env python3
"""drug_prices: a community cedar tool. Reads its input as JSON on stdin and prints the result.

Finds the cheapest way to buy a prescription drug in the US, from open data that needs no key:
the National Library of Medicine's RxNorm (names, strengths, brand and generic versions, and their
NDC package codes), CMS's NADAC (the national average price pharmacies pay, updated weekly), and
Mark Cuban Cost Plus Drugs' public price API (a real cash price, by mail). Prices the user already
has (an insurance copay, a pharmacy's quote, a coupon) are ranked alongside. The report ends with
`[chart] {...}` lines, which the cedar chat draws as charts (the same spec as the generate_chart
tool's input).

Exit non-zero with a message on stderr to report an error; the model sees it as the tool's
error and can try again differently.
"""
import json
import os
import re
import statistics
import sys
import urllib.error
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from datetime import date, datetime

RXNAV = "https://rxnav.nlm.nih.gov/REST"
MEDICAID = "https://data.medicaid.gov/api/1"
COST_PLUS = "https://us-central1-costplusdrugs-publicapi.cloudfunctions.net/main"
USER_AGENT = "cedar-drug_prices/1.0 (+https://github.com/hanneshapke/cedar/tree/main/registry/tools/drug_prices)"
PRODUCT_TTYS = ("SCD", "SBD", "GPCK", "BPCK")  # clinical (generic) and branded drugs and packs
GENERIC_TTYS = ("SCD", "GPCK")
NADAC_TITLE = re.compile(r"^NADAC \(National Average Drug Acquisition Cost\)\s+(\d{4})$", re.I)
MAX_NDCS = 3000  # per drug; a much-repackaged generic like metformin has hundreds per strength
NDC_CHUNK = 60
NADAC_WORKERS = 8
# Cost Plus: their cost plus 15%, a $5 pharmacy fee per drug, and $5 shipping per order.
COST_PLUS_MARKUP, COST_PLUS_FEE, COST_PLUS_SHIPPING = 1.15, 5.0, 5.0
UNITS = {"mg": "MG", "mcg": "MCG", "µg": "MCG", "ug": "MCG", "g": "G", "ml": "ML", "unit": "UNT",
         "units": "UNT", "unt": "UNT", "iu": "UNT", "meq": "MEQ", "%": "%"}
STRENGTH_IN_QUERY = re.compile(r"(\d+(?:\.\d+)?)\s*(mg|mcg|µg|ug|g|ml|units?|unt|iu|meq|%)(?=\W|$)", re.I)
STRENGTH_IN_NAME = re.compile(r"(\d+(?:\.\d+)?)\s(MG|MCG|G|ML|UNT|MEQ|MMOL|%)(?:/(?:ML|ACTUAT|HR|G|MG|L))?")
FORM_WORDS = {  # what people say: the words RxNorm uses in dose forms
    "er": ["extended release"], "xr": ["extended release"], "xl": ["extended release"],
    "sr": ["extended release"], "extended": ["extended release"], "dr": ["delayed release"],
    "ec": ["delayed release"], "tablet": ["tablet"], "tablets": ["tablet"], "tab": ["tablet"],
    "capsule": ["capsule"], "capsules": ["capsule"], "cap": ["capsule"],
    "liquid": ["solution", "suspension"], "solution": ["solution"], "suspension": ["suspension"],
    "cream": ["cream"], "ointment": ["ointment"], "patch": ["transdermal"], "inhaler": ["inhal"],
    "injection": ["injecti", "prefilled syringe", "auto-injector", "pen injector"], "pen": ["pen injector"],
    "drops": ["drops", "ophthalmic"], "chewable": ["chewable"], "odt": ["disintegrating"],
}
# Text the user sees in the chart, by CEDAR_LANGUAGE. The report itself stays English for the model.
STRINGS = {
    "en": {"title": "{base}, {quantity} units", "x_label": "USD (* what pharmacies pay, not a price you can pay)",
           "nadac": "Pharmacy cost, {kind} (NADAC)", "generic": "generic", "brand": "brand",
           "estimate": "Cost Plus Drugs (estimate)", "copay": "Your insurance copay", "quote": "a quote"},
    "de": {"title": "{base}, {quantity} Einheiten",
           "x_label": "USD (* was Apotheken zahlen, kein Preis, den du zahlen kannst)",
           "nadac": "Apothekeneinkauf, {kind} (NADAC)", "generic": "Generikum", "brand": "Markenpräparat",
           "estimate": "Cost Plus Drugs (Schätzung)", "copay": "Deine Zuzahlung (Versicherung)",
           "quote": "ein Angebot"},
    "fr": {"title": "{base}, {quantity} unités",
           "x_label": "USD (* ce que paient les pharmacies, pas un prix que vous pouvez payer)",
           "nadac": "Coût pour la pharmacie, {kind} (NADAC)", "generic": "générique", "brand": "princeps",
           "estimate": "Cost Plus Drugs (estimation)", "copay": "Votre quote-part d'assurance",
           "quote": "un devis"},
}
# The English names this tool gives its own price sources, for the chart's labels.
OWN_SOURCES = {"Pharmacy cost, generic (NADAC)": ("nadac", "generic"),
               "Pharmacy cost, brand (NADAC)": ("nadac", "brand"),
               "Cost Plus Drugs (estimate)": ("estimate", None),
               "Your insurance copay": ("copay", None), "a quote": ("quote", None)}


def tr(key, **values):
    """A chart string in CEDAR_LANGUAGE (English when unset); Swiss German (de-CH) writes ss for ß."""
    lang = (os.environ.get("CEDAR_LANGUAGE") or "").strip().lower().split("-")[0]
    lang = lang if lang in STRINGS else "en"
    text = STRINGS[lang].get(key, STRINGS["en"][key])
    if lang == "de" and (os.environ.get("CEDAR_LOCALE") or "").replace("_", "-").lower().startswith("de-ch"):
        text = text.replace("ß", "ss")
    return text.format(**values)


def source_label(source):
    """A price source's name for the chart: this tool's own in the user's language, the user's as given."""
    key, kind = OWN_SOURCES.get(source, (None, None))
    if key is None:
        return source
    return tr(key, kind=tr(kind)) if kind else tr(key)


class SourceError(Exception):
    """A price source couldn't be reached or answered something unusable; the others still run."""


def whole(value, default, low, high):
    try:
        return max(low, min(high, int(value)))
    except (TypeError, ValueError):
        return default


def money(value):
    if value is None or value == "":
        return None
    try:
        return round(float(str(value).replace("$", "").replace(",", "").strip()), 2)
    except ValueError:
        return None


def fetch(url, params=None, body=None, timeout=30):
    full = f"{url}?{urllib.parse.urlencode(params, doseq=True)}" if params else url
    headers = {"User-Agent": USER_AGENT, "Accept": "application/json"}
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(full, data=data, headers=headers)
    host = urllib.parse.urlsplit(url).hostname
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return json.load(resp)
    except urllib.error.HTTPError as e:
        raise SourceError(f"{host} answered {e.code} {e.reason}") from e
    except (urllib.error.URLError, TimeoutError, ValueError) as e:
        raise SourceError(f"couldn't reach {host} ({e})") from e


# --- RxNorm: which drug, which versions ---------------------------------------------------------

def strength_pairs(text):
    """The (number, unit) pairs of a strength: '5 MG / 20 MG' -> {('5', 'MG'), ('20', 'MG')}."""
    return {(n.rstrip("0").rstrip(".") if "." in n else n, u.upper()) for n, u in STRENGTH_IN_NAME.findall(text)}


def describe(name):
    """An RxNorm drug name split up: 'atorvastatin 20 MG Oral Tablet [Lipitor]' ->
    ('atorvastatin 20 MG Oral Tablet', 'Lipitor', '20 MG', 'Oral Tablet')."""
    brand = None
    m = re.search(r"\s*\[([^\]]+)\]\s*$", name)
    if m:
        brand, name = m.group(1), name[:m.start()]
    hits = list(STRENGTH_IN_NAME.finditer(name))
    strength = " / ".join(h.group(0) for h in hits)
    form = name[hits[-1].end():].strip(" }") if hits else ""
    form = re.sub(r"^\)\s*\}?\s*", "", form)
    return name.strip(), brand, strength, form


def product(concept):
    base, brand, strength, form = describe(concept["name"])
    return {"rxcui": concept["rxcui"], "name": concept["name"], "tty": concept["tty"], "base": base,
            "brand": brand, "strength": strength, "form": form,
            "generic": concept["tty"] in GENERIC_TTYS, "synonym": concept.get("synonym") or ""}


def drug_products(name):
    data = fetch(f"{RXNAV}/drugs.json", {"name": name})
    out = []
    for group in (data.get("drugGroup") or {}).get("conceptGroup") or []:
        if group.get("tty") in PRODUCT_TTYS:
            out.extend(product(c) for c in group.get("conceptProperties") or [])
    return out


def ingredients(term):
    """The ingredient names behind what the user typed: a brand, a generic, a misspelling, or a
    full product name. A combination drug gives its multi-ingredient name ('amlodipine /
    atorvastatin'), so its products aren't mixed with the single ones."""
    ids = (fetch(f"{RXNAV}/rxcui.json", {"name": term, "search": 2}).get("idGroup") or {}).get("rxnormId") or []
    if not ids:
        candidates = (fetch(f"{RXNAV}/approximateTerm.json", {"term": term, "maxEntries": 4})
                      .get("approximateGroup") or {}).get("candidate") or []
        ids = list(dict.fromkeys(c["rxcui"] for c in candidates if c.get("rxcui")))
    for rxcui in ids[:3]:
        props = fetch(f"{RXNAV}/rxcui/{rxcui}/properties.json").get("properties") or {}
        if props.get("tty") in ("IN", "MIN"):
            return [props["name"]]
        related = fetch(f"{RXNAV}/rxcui/{rxcui}/related.json", {"tty": "IN MIN"})
        names = {"IN": [], "MIN": []}
        for group in (related.get("relatedGroup") or {}).get("conceptGroup") or []:
            if group.get("tty") in names:
                names[group["tty"]].extend(c["name"] for c in group.get("conceptProperties") or [])
        if names["MIN"] or names["IN"]:
            return names["MIN"][:1] or names["IN"]
    return []


def find_products(term):
    found, names = [], ingredients(term)
    for name in names:
        single = "/" not in name
        found.extend(p for p in drug_products(name) if not (single and " / " in p["base"]))
    if not found:
        found = drug_products(term)
    unique = {p["rxcui"]: p for p in found}
    return names, sorted(unique.values(), key=lambda p: (not p["generic"], p["base"].lower(), p["brand"] or ""))


def query_strength(text):
    """A strength the user typed, as RxNorm pairs, and the text without it."""
    pairs = {(n.rstrip("0").rstrip(".") if "." in n else n, UNITS[u.lower()])
             for n, u in STRENGTH_IN_QUERY.findall(text)}
    return pairs, STRENGTH_IN_QUERY.sub(" ", text)


def form_words(text):
    """The dose-form words a request names, and the request without them."""
    wanted, rest = [], []
    for word in re.findall(r"[\w%./-]+", text):
        if word.lower() in FORM_WORDS:
            wanted.append(FORM_WORDS[word.lower()])
        else:
            rest.append(word)
    return wanted, " ".join(rest)


def matches(p, pairs, forms):
    if pairs and not pairs <= strength_pairs(p["strength"]):
        return False
    form = p["form"].lower()
    return all(any(w in form for w in alternatives) for alternatives in forms)


# --- NADAC: what pharmacies pay -------------------------------------------------------------------

def nadac_datasets():
    """NADAC's yearly datasets on data.medicaid.gov, newest first, as (year, id)."""
    found = {}
    try:
        hits = fetch(f"{MEDICAID}/search", {"fulltext": "National Average Drug Acquisition Cost", "page-size": 50})
        results = hits.get("results") or {}
        items = results.values() if isinstance(results, dict) else results
    except SourceError:
        items = fetch(f"{MEDICAID}/metastore/schemas/dataset/items", {"show-reference-ids": "false"}, timeout=60)
    for item in items:
        m = NADAC_TITLE.match(str(item.get("title") or "").strip())
        if m and item.get("identifier"):
            found[int(m.group(1))] = item["identifier"]
    if not found:
        raise SourceError("couldn't find the NADAC dataset on data.medicaid.gov")
    return sorted(found.items(), reverse=True)


def parse_day(text):
    text = str(text or "")[:10]
    for pattern in ("%Y-%m-%d", "%m/%d/%Y"):
        try:
            return datetime.strptime(text, pattern).date()
        except ValueError:
            pass
    return date.min


def nadac_chunk(dataset, ndcs):
    query = {"conditions": [{"property": "ndc", "value": ndcs, "operator": "in"}],
             "limit": 500, "keys": True, "schema": False, "count": False,
             "sorts": [{"property": "as_of_date", "order": "desc"}]}
    try:
        data = fetch(f"{MEDICAID}/datastore/query/{dataset}/0", body=query, timeout=60)
    except SourceError:
        query.pop("sorts")  # the weekly rows of a year still fit, unsorted, for a few NDCs
        data = fetch(f"{MEDICAID}/datastore/query/{dataset}/0", body=query, timeout=60)
    return data.get("results") or []


def nadac_rows(dataset, ndcs):
    """NADAC's rows for every NDC asked for. Most of a common generic's NDCs are repackagers that
    NADAC doesn't price, so none can be skipped; the chunks run side by side."""
    chunks = [ndcs[i:i + NDC_CHUNK] for i in range(0, len(ndcs), NDC_CHUNK)]
    with ThreadPoolExecutor(NADAC_WORKERS) as pool:
        return [row for rows in pool.map(lambda c: nadac_chunk(dataset, c), chunks) for row in rows]


def nadac_prices(ndcs):
    """The current NADAC per unit of each NDC that has one, from the newest yearly dataset that
    lists any of them (a new year's dataset starts out empty)."""
    ndcs = ndcs[:MAX_NDCS]
    for _, dataset in nadac_datasets()[:2]:
        latest = {}
        for row in nadac_rows(dataset, ndcs):
            per_unit = money(row.get("nadac_per_unit"))
            if not per_unit or not row.get("ndc"):
                continue
            row["_day"] = max(parse_day(row.get("as_of_date")), parse_day(row.get("effective_date")))
            row["_per_unit"] = per_unit
            if row["ndc"] not in latest or row["_day"] > latest[row["ndc"]]["_day"]:
                latest[row["ndc"]] = row
        if latest:
            return list(latest.values())
    return []


def ndcs_of(rxcui):
    data = fetch(f"{RXNAV}/rxcui/{rxcui}/ndcs.json")
    return ((data.get("ndcGroup") or {}).get("ndcList") or {}).get("ndc") or []


# --- Cost Plus Drugs: a cash price by mail ---------------------------------------------------------

def same_form(theirs, ours):
    theirs, ours = theirs.lower(), ours.lower()
    if not theirs:
        return True
    release = lambda t: bool(re.search(r"\b(er|xr|xl|sr|extended|24 ?hr?)\b", t))
    kind = lambda t: next((k for k in ("tablet", "capsule", "solution", "suspension", "cream",
                                       "ointment", "patch", "inhal", "inject") if k in t), None)
    return release(theirs) == release(ours) and kind(theirs) in (None, kind(ours))


def cost_plus(ingredient, p, quantity):
    data = fetch(COST_PLUS, {"medication_name": ingredient, "quantity_units": quantity})
    rows = data if isinstance(data, list) else data.get("results") or data.get("data") or []
    wanted = {n for n, _ in strength_pairs(p["strength"])}
    out = []
    for row in rows:
        if not isinstance(row, dict):
            continue
        numbers = set(re.findall(r"\d+(?:\.\d+)?", str(row.get("strength") or "")))
        if wanted and numbers and not wanted <= {n.rstrip("0").rstrip(".") if "." in n else n for n in numbers}:
            continue
        if not same_form(str(row.get("form") or row.get("dosage_form") or ""), p["form"]):
            continue
        unit = money(row.get("unit_price") or row.get("price_per_unit"))
        price = money(row.get("requested_quote") or row.get("price") or row.get("total_price"))
        if price is None and unit is not None:
            price = round(unit * quantity + COST_PLUS_FEE, 2)
        if price is None:
            continue
        branded = str(row.get("brand_name") or "").strip()
        generic = not branded or branded.lower() in str(row.get("medication_name") or "").lower()
        out.append({"price": price, "unit": unit, "generic": generic, "url": row.get("url"),
                    "name": " ".join(str(row.get(k) or "") for k in ("medication_name", "strength", "form")).strip()})
    return sorted(out, key=lambda o: o["price"])


# --- putting it together -----------------------------------------------------------------------

def usd(value):
    return f"${value:,.2f}"


def slug(text):
    return re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")


def options(names, products, term):
    """When the request fits more than one strength or form: the choices, for the user."""
    bases = {}
    for p in products:
        entry = bases.setdefault(p["base"], set())
        if p["brand"]:
            entry.add(p["brand"])
    label = " / ".join(names) or term
    out = [f"{label} comes in {len(bases)} strengths and forms. Ask the user which one they take "
           "(it's on the bottle or prescription), then call again with strength and, if needed, form:"]
    for base, brands in list(bases.items())[:40]:
        out.append(f"- {base}" + (f" (brand: {', '.join(sorted(brands))})" if brands else ""))
    if len(bases) > 40:
        out.append(f"- and {len(bases) - 40} more; narrow it with strength or form")
    return "\n".join(out)


def build(args):
    raw = str(args.get("drug") or "").strip()
    if len(raw) < 3:
        sys.exit("give the drug's name (brand or generic), such as \"atorvastatin\" or \"Lipitor 20 mg\"")
    quantity = whole(args.get("quantity"), 30, 1, 1000)
    pairs, rest = query_strength(f"{raw} {args.get('strength') or ''}")
    forms, term = form_words(rest)
    more_forms, _ = form_words(str(args.get("form") or ""))
    forms += more_forms
    term = term.strip() or raw
    try:
        names, products = find_products(term)
    except SourceError as e:
        sys.exit(f"RxNorm lookup failed: {e}; try again in a little while")
    if not products:
        sys.exit(f"couldn't find {term!r} in RxNorm; check the spelling or try the generic name")
    fitting = [p for p in products if matches(p, pairs, forms)]
    if not fitting:
        return options(names, products, term) + "\n\nNone of them matches the strength or form asked for."
    bases = list(dict.fromkeys(p["base"] for p in fitting))
    if len(bases) > 1:
        return options(names, fitting, term)

    base = bases[0]
    versions = [p for p in fitting if p["base"] == base]
    generic = next((p for p in versions if p["generic"]), None)
    brands = [p for p in versions if not p["generic"]]
    asked_brand = next((p["brand"] for p in brands if p["brand"] and p["brand"].lower() in raw.lower()), None)
    ingredient = names[0] if names else term
    offers, failed = [], []

    # What pharmacies pay, generic and brand apart.
    nadac, kinds = {"generic": [], "brand": []}, {}
    try:
        for p in versions:
            for ndc in ndcs_of(p["rxcui"]):
                kinds.setdefault(ndc, "generic" if p["generic"] else "brand")
        for row in nadac_prices(list(kinds)):
            nadac[kinds.get(row["ndc"], "brand")].append(row)
    except SourceError as e:
        failed.append(f"NADAC ({e})")
    for kind in ("generic", "brand"):
        rows = sorted(nadac[kind], key=lambda r: r["_per_unit"])
        if not rows:
            continue
        low, typical = rows[0]["_per_unit"], statistics.median(r["_per_unit"] for r in rows)
        unit = (rows[0].get("pricing_unit") or "unit").upper()
        offers.append({"source": f"Pharmacy cost, {kind} (NADAC)", "price": round(low * quantity, 2),
                       "buyable": False, "generic": kind == "generic",
                       "detail": f"{usd(low)} per {unit} for the cheapest of {len(rows)} package"
                                 f"{'s' if len(rows) > 1 else ''} (median {usd(typical)}), as of "
                                 f"{max(r['_day'] for r in rows):%b %d, %Y}"})

    # Cost Plus Drugs, live, or its published formula on NADAC when the API has nothing. Their
    # brand_name can name the reference brand on a generic, so a brand-looking quote only gives
    # way to a generic-looking one; it is never dropped for having nothing to give way to.
    try:
        quotes = cost_plus(ingredient, versions[0], quantity)
        quotes = [q for q in quotes if q["generic"] or not generic] or quotes
    except SourceError as e:
        quotes = []
        failed.append(f"Cost Plus Drugs ({e})")
    if quotes:
        q = quotes[0]
        detail = f"{q['name']}, by mail; {usd(COST_PLUS_SHIPPING)} shipping per order on top"
        if q["unit"] and quantity <= 31:
            ninety = q["unit"] * quantity * 3 + COST_PLUS_FEE
            detail += f"; a 3-fill supply would be about {usd(ninety)} ({usd(ninety / 3)} per {quantity})"
        offers.append({"source": "Cost Plus Drugs", "price": q["price"], "buyable": True,
                       "generic": q["generic"], "detail": detail, "url": q["url"]})
    elif nadac["generic"] or nadac["brand"]:
        low = min(r["_per_unit"] for r in nadac["generic"] or nadac["brand"])
        offers.append({"source": "Cost Plus Drugs (estimate)", "price": round(low * quantity * COST_PLUS_MARKUP + COST_PLUS_FEE, 2),
                       "buyable": True, "generic": bool(nadac["generic"]), "estimate": True,
                       "detail": "their formula (cost + 15% + $5 fee) on NADAC; check costplusdrugs.com, "
                                 "they may not carry it; plus $5 shipping"})

    copay = money(args.get("copay"))
    if copay is not None:
        offers.append({"source": "Your insurance copay", "price": copay, "buyable": True, "generic": None,
                       "detail": "what the user said their plan charges"})
    for known in args.get("known_prices") or []:
        if not isinstance(known, dict) or money(known.get("price")) is None:
            continue
        their_qty = whole(known.get("quantity"), quantity, 1, 1000)
        price = money(known["price"])
        offer = {"source": str(known.get("source") or "a quote")[:60], "price": round(price * quantity / their_qty, 2),
                 "buyable": True, "generic": None, "detail": "a price the user gave"}
        if their_qty != quantity:
            offer["detail"] += f": {usd(price)} for {their_qty}, scaled to {quantity}"
        offers.append(offer)

    if not offers and failed:
        sys.exit("couldn't get any prices: " + "; ".join(failed) + ". Try again in a little while.")
    if not offers:
        return unlisted(base, ingredient, quantity, len(kinds))
    offers.sort(key=lambda o: o["price"])
    return report(base, ingredient, generic, asked_brand, quantity, offers, failed)


def coupon_links(ingredient):
    name = slug(ingredient.replace(" / ", "-"))
    return f"https://www.goodrx.com/{name} and https://www.singlecare.com/prescription/{name}"


def unlisted(base, ingredient, quantity, ndcs):
    """Every source answered, and none lists this product: an answer, not an error to retry."""
    return "\n".join([
        f"No open price source lists {base}: NADAC has no pharmacy cost for any of the {ndcs} "
        "package codes RxNorm gives for it, and Cost Plus Drugs had no matching quote. That is "
        "usual for specialty drugs, drugs given at a clinic, and new brands; for a common generic "
        "it is unexpected and worth reporting. Asking again won't change it.",
        f"Compare coupon prices for {quantity} at {coupon_links(ingredient)}, or ask the pharmacy "
        "for its cash price. For a brand, the maker's savings card or patient assistance program "
        "is often the cheapest way.",
        "Informational only: don't change the drug, strength or dose to save money without the "
        "prescriber or pharmacist.",
    ])


def report(base, ingredient, generic, asked_brand, quantity, offers, failed):
    out = [f"Prices for {quantity} of {base} (US, cash):"]
    if asked_brand and generic:
        out.append(f"{asked_brand} is a brand of {ingredient}; the generic is the same drug and "
                   "pharmacists can usually swap it in unless the prescription says otherwise.")
    elif not generic:
        out.append("There is no generic version of this exact product in RxNorm.")
    buyable = [o for o in offers if o["buyable"]]
    if buyable:
        best = buyable[0]
        out.append(f"Cheapest: {best['source']} at {usd(best['price'])}.")
    out.append("")
    for o in offers:
        tag = "" if o["buyable"] else " [benchmark, not a price you can pay]"
        out.append(f"- {o['source']}: {usd(o['price'])}{tag}. {o['detail']}." + (f" {o['url']}" if o.get("url") else ""))

    pharmacy = [o for o in offers if not o["buyable"] and o["generic"]]
    if pharmacy:
        fair = pharmacy[0]["price"] + 12
        out.append("")
        out.append(f"A fair cash price at a local pharmacy is near what it pays plus a dispensing fee "
                   f"(about $10-15): around {usd(fair)}. Quotes far above that are worth beating with a coupon.")
        dear = [o for o in offers if not o["buyable"] and o["generic"] is False]
        if dear and dear[0]["price"] > pharmacy[0]["price"] * 1.5:
            out.append(f"The brand costs pharmacies {dear[0]['price'] / pharmacy[0]['price']:.0f}x the generic.")
    out.append("")
    out.append("Coupon prices at local pharmacies change daily and were not fetched; compare at "
               f"{coupon_links(ingredient)} before going. "
               "Cash prices don't count toward an insurance deductible, and a 90-day supply is usually cheaper per pill.")
    if failed:
        out.append("Not available this time: " + "; ".join(failed) + ".")
    out.append("Informational only: prices are not guaranteed. Don't change the drug, strength or "
               "dose to save money without the prescriber or pharmacist.")

    lines = []
    if len(offers) > 1:
        lines.append({"kind": "barh", "title": tr("title", base=base, quantity=quantity),
                      "labels": [source_label(o["source"]) + (" *" if not o["buyable"] else "") for o in offers],
                      "series": [{"name": "USD", "values": [o["price"] for o in offers]}],
                      "x_label": tr("x_label")})
    for c in lines:
        out.append("")
        out.append("[chart] " + json.dumps(c, separators=(",", ":"), ensure_ascii=False))
    return "\n".join(out)


def main():
    args = json.load(sys.stdin) or {}
    print(build(args))


if __name__ == "__main__":
    main()
