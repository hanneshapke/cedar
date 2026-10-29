use super::{Tool, ToolContext};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use reqwest::StatusCode;
use serde_json::{json, Map, Value};
use std::time::Duration;

const MAX_AIRPORTS: usize = 7;
const MAX_PASSENGERS: u64 = 9;
const DEFAULT_RESULTS: usize = 8;
const MAX_RESULTS: usize = 20;
const MAX_BOOKING_OPTIONS: usize = 12;
const MAX_HISTORY: usize = 90;
/// Airlines sell about eleven months ahead.
const MAX_DAYS_AHEAD: i64 = 330;
const MAX_TEXT_CHARS: usize = 200;
const MAX_TOKEN_CHARS: usize = 2000;
const CABINS: &[&str] = &["economy", "premium_economy", "business", "first"];
const SORTS: &[&str] = &[
    "best",
    "price",
    "departure",
    "arrival",
    "duration",
    "emissions",
];
/// What SerpApi answers when Google Flights has nothing for a search; that is a result, not an error.
const NO_RESULTS: &str = "hasn't returned any results";
const NOTE: &str = "Prices are Google Flights' as of now, for all passengers together, before \
                    bags and seat fees unless noted; a round trip's price covers both ways. They \
                    change quickly and no seat is held: the user books on the airline's or \
                    agency's site.";
const USER_AGENT: &str = concat!(
    "cedar/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/hanneshapke/cedar)"
);

/// Searches flights on Google Flights through SerpApi's Google Flights API, and shows them to the
/// user as a card in the chat. It needs a SerpApi key (set in the environment or saved by an
/// admin) and runs in the cedar process, since the sandbox has no network.
///
/// A search's result is JSON: `{"kind": "flights", "leg": "outbound" | "return" | "one_way",
/// "from", "to", "date", "return_date"?, "passengers", "cabin", "currency", "options": [{"id",
/// "top_pick", "price", "airlines", "logo"?, "depart", "arrive", "duration_min", "stops",
/// "segments", "layovers", "co2_kg"?, "co2_vs_typical_pct"?, "departure_token"?,
/// "booking_token"?}], "more", "insights"?, "google_flights_url"?, "as_of", "note"}`. With a
/// `booking_token` it is `{"kind": "booking", ..., "flights": [the chosen option], "options":
/// [{"seller", "price", "fare"?, "bags", "marketed_as", "part"}]}`.
pub struct SearchFlights {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl SearchFlights {
    /// `base_url` is SerpApi's root: searches go to `/search.json`, the key check to
    /// `/account.json` (see `DEFAULT_FLIGHTS_API_URL`).
    pub fn new(api_key: String, base_url: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .user_agent(USER_AGENT)
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            http,
            api_key,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    /// Check a key against SerpApi's account endpoint, which doesn't use up a search.
    pub async fn check_key(&self, timeout: Duration) -> Result<()> {
        let resp = self
            .http
            .get(format!("{}/account.json", self.base_url))
            .header("Accept", "application/json")
            .query(&[("api_key", self.api_key.as_str())])
            .timeout(timeout)
            .send()
            .await
            .context("asking SerpApi about the key")?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        match status {
            s if s.is_success() && body["error"].is_null() => Ok(()),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                bail!("SerpApi rejected the API key ({status})")
            }
            _ => bail!(
                "SerpApi error {status}: {}",
                body["error"].as_str().unwrap_or("no details")
            ),
        }
    }

    async fn search(&self, params: &[(&str, String)], timeout: Duration) -> Result<Value> {
        let resp = self
            .http
            .get(format!("{}/search.json", self.base_url))
            .header("Accept", "application/json")
            .query(&[("engine", "google_flights"), ("api_key", &self.api_key)])
            .query(params)
            .timeout(timeout)
            .send()
            .await
            .context("sending the flight search")?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        let error = body["error"].as_str().map(|e| clip(e, MAX_TEXT_CHARS));
        if error.as_deref().is_some_and(|e| e.contains(NO_RESULTS)) {
            return Ok(json!({ "no_results": true }));
        }
        match (status, error) {
            (StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN, _) => {
                bail!("SerpApi rejected the API key ({status}); an admin should check it")
            }
            (StatusCode::TOO_MANY_REQUESTS, e) => bail!(
                "SerpApi's search limit is used up ({}); try again later",
                e.unwrap_or_else(|| "too many requests".into())
            ),
            (_, Some(e)) => bail!("SerpApi: {e}"),
            (s, None) if s.is_success() => Ok(body),
            (s, None) => bail!("SerpApi error {s}"),
        }
    }
}

#[async_trait]
impl Tool for SearchFlights {
    fn name(&self) -> &str {
        "search_flights"
    }

    fn description(&self) -> &str {
        "Search flights on Google Flights, shown to the user as a card in the chat: one-way or \
         round trips between airports on given dates, for up to 9 passengers, in economy, premium \
         economy, business or first, optionally limited by stops, price, airlines and carry-on \
         bags, sorted by Google's top picks, price, times, duration or emissions. The card shows \
         each option's price, airlines, times, stops and layovers, duration, flights, aircraft \
         and CO2, Google's verdict on whether prices are low or high for the route with their \
         recent history, and a link to Google Flights. Give airports as IATA codes; for a city \
         with several airports list them all (London: LHR, LGW, STN, LTN, LCY; New York: JFK, \
         EWR, LGA). If the user hasn't said where from, when, or how many travel, ask first \
         rather than guessing. For a round trip the first call lists outbound options priced for \
         the whole trip; to show the return flights for one, call again with the same search \
         plus its departure_token. To show where to book an option, call with the same search \
         plus its booking_token. Afterwards say in a few sentences which options you would pick \
         and why (price, times, stops, duration), and mention the price verdict; don't repeat \
         what the card already lists. Prices change quickly and no seat is held: never say a \
         flight is booked."
    }

    fn schema(&self) -> Value {
        let airports = |what: &str| {
            json!({ "type": "array", "minItems": 1, "maxItems": MAX_AIRPORTS,
                    "items": { "type": "string" },
                    "description": format!("{what}: IATA airport codes, e.g. [\"SFO\"] or [\"JFK\", \"EWR\", \"LGA\"].") })
        };
        let count = |what: &str, default: u64| {
            json!({ "type": "integer", "minimum": 0, "maximum": MAX_PASSENGERS, "default": default,
                    "description": what })
        };
        json!({ "type": "object",
                "properties": {
                    "from": airports("Where from"),
                    "to": airports("Where to"),
                    "date": { "type": "string", "format": "date",
                              "description": "The day of the outbound flight, YYYY-MM-DD." },
                    "return_date": { "type": "string", "format": "date",
                                     "description": "The day of the return flight for a round trip; leave out for one way." },
                    "adults": count("Adults (12 and older).", 1),
                    "children": count("Children (2 to 11).", 0),
                    "infants_in_seat": count("Infants under 2 with their own seat.", 0),
                    "infants_on_lap": count("Infants under 2 on an adult's lap, at most one per adult.", 0),
                    "cabin": { "type": "string", "enum": CABINS, "default": "economy" },
                    "max_stops": { "type": "integer", "minimum": 0, "maximum": 2,
                                   "description": "At most this many stops each way; 0 for nonstop only. Leave out for any." },
                    "max_price": { "type": "integer", "minimum": 1,
                                   "description": "Only options up to this price, in the search's currency, for all passengers." },
                    "airlines": { "type": "array", "items": { "type": "string" }, "maxItems": MAX_AIRPORTS,
                                  "description": "Only these airlines, as two-character IATA codes, e.g. [\"LH\", \"UA\"]." },
                    "exclude_airlines": { "type": "array", "items": { "type": "string" }, "maxItems": MAX_AIRPORTS,
                                          "description": "Not these airlines, as two-character IATA codes." },
                    "carry_on_bags": { "type": "integer", "minimum": 0, "maximum": 2,
                                       "description": "Carry-on bags per passenger to include in the price." },
                    "sort": { "type": "string", "enum": SORTS, "default": "best",
                              "description": "best (Google's top picks first), price, departure or arrival time, duration, or emissions." },
                    "currency": { "type": "string",
                                  "description": "Three-letter currency code, e.g. USD or EUR; defaults to the user's region's." },
                    "max_results": { "type": "integer", "minimum": 1, "maximum": MAX_RESULTS, "default": DEFAULT_RESULTS,
                                     "description": "How many options to list." },
                    "departure_token": { "type": "string",
                                         "description": "From a round trip's outbound option: lists the return flights that go with it. Repeat the rest of the search unchanged." },
                    "booking_token": { "type": "string",
                                       "description": "From an option: lists where to book it and for how much. Repeat the rest of the search unchanged." }
                },
                "required": ["from", "to", "date"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let req = parse_input(input, &ctx.locale, Utc::now().date_naive())?;
        let mut params = req.params();
        let result = if let Some(token) = &req.booking_token {
            params.push(("booking_token", token.clone()));
            booking(&req, &self.search(&params, ctx.timeout).await?)
        } else {
            if let Some(token) = &req.departure_token {
                params.push(("departure_token", token.clone()));
            }
            flights(&req, &self.search(&params, ctx.timeout).await?, Utc::now())
        };
        Ok(result.to_string())
    }
}

// -- input ------------------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
struct Request {
    from: Vec<String>,
    to: Vec<String>,
    date: NaiveDate,
    return_date: Option<NaiveDate>,
    adults: u64,
    children: u64,
    infants_in_seat: u64,
    infants_on_lap: u64,
    cabin: &'static str,
    max_stops: Option<u64>,
    max_price: Option<u64>,
    airlines: Vec<String>,
    exclude_airlines: Vec<String>,
    carry_on_bags: Option<u64>,
    sort: &'static str,
    currency: String,
    /// The region of the user's locale, for Google's country (`gl`).
    country: Option<String>,
    max_results: usize,
    departure_token: Option<String>,
    booking_token: Option<String>,
}

impl Request {
    /// The search as SerpApi's Google Flights parameters.
    fn params(&self) -> Vec<(&'static str, String)> {
        let index =
            |list: &[&str], v: &str| (list.iter().position(|x| *x == v).unwrap() + 1).to_string();
        let mut p = vec![
            ("departure_id", self.from.join(",")),
            ("arrival_id", self.to.join(",")),
            ("outbound_date", self.date.to_string()),
            (
                "type",
                if self.return_date.is_some() { "1" } else { "2" }.into(),
            ),
            ("travel_class", index(CABINS, self.cabin)),
            ("adults", self.adults.to_string()),
            ("children", self.children.to_string()),
            ("infants_in_seat", self.infants_in_seat.to_string()),
            ("infants_on_lap", self.infants_on_lap.to_string()),
            ("sort_by", index(SORTS, self.sort)),
            ("currency", self.currency.clone()),
            ("hl", "en".into()),
        ];
        if let Some(d) = self.return_date {
            p.push(("return_date", d.to_string()));
        }
        if let Some(c) = &self.country {
            p.push(("gl", c.to_ascii_lowercase()));
        }
        // Google's stops filter: 1 nonstop only, 2 one stop or fewer, 3 two stops or fewer.
        if let Some(s) = self.max_stops {
            p.push(("stops", (s + 1).to_string()));
        }
        if let Some(m) = self.max_price {
            p.push(("max_price", m.to_string()));
        }
        if let Some(b) = self.carry_on_bags {
            p.push(("bags", b.to_string()));
        }
        if !self.airlines.is_empty() {
            p.push(("include_airlines", self.airlines.join(",")));
        }
        if !self.exclude_airlines.is_empty() {
            p.push(("exclude_airlines", self.exclude_airlines.join(",")));
        }
        p
    }
}

/// A list of codes of `len` letters (airports) or letters and digits (airlines), upper-cased.
/// Models sometimes give one string, or a comma-separated one.
fn codes(input: &Value, key: &str, airline: bool) -> Result<Vec<String>> {
    let raw: Vec<String> = match &input[key] {
        Value::Null => vec![],
        Value::String(s) => s.split(',').map(String::from).collect(),
        Value::Array(items) => items
            .iter()
            .map(|v| {
                v.as_str()
                    .map(String::from)
                    .ok_or_else(|| anyhow!("{key} are strings"))
            })
            .collect::<Result<_>>()?,
        _ => bail!("give {key} as a list of codes"),
    };
    let mut out = Vec::new();
    for r in raw
        .iter()
        .map(|r| r.trim().to_ascii_uppercase())
        .filter(|r| !r.is_empty())
    {
        let ok = if airline {
            r.len() == 2 && r.chars().all(|c| c.is_ascii_alphanumeric())
        } else {
            r.len() == 3 && r.chars().all(|c| c.is_ascii_uppercase())
        };
        if !ok {
            if airline {
                bail!("{key}: {r:?} is not an airline code; use two-character IATA codes such as LH or UA");
            }
            bail!("{key}: {r:?} is not an airport code; use three-letter IATA codes such as JFK, and list a city's airports (JFK, EWR, LGA)");
        }
        if !out.contains(&r) {
            out.push(r);
        }
    }
    if out.len() > MAX_AIRPORTS {
        bail!("{key}: at most {MAX_AIRPORTS} codes");
    }
    Ok(out)
}

fn date(input: &Value, key: &str, today: NaiveDate) -> Result<Option<NaiveDate>> {
    let Some(raw) = input[key].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let d = NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .map_err(|_| anyhow!("{key} must be a date like 2026-12-01, not {raw:?}"))?;
    // A day's grace for time zones ahead of UTC.
    if d < today.pred_opt().unwrap_or(today) {
        bail!("{key} {d} is in the past; today is {today}");
    }
    if (d - today).num_days() > MAX_DAYS_AHEAD {
        bail!("{key} {d} is too far ahead: airlines sell about 11 months out");
    }
    Ok(Some(d))
}

fn count(input: &Value, key: &str, default: u64, max: u64) -> Result<u64> {
    match &input[key] {
        Value::Null => Ok(default),
        v => match v.as_u64() {
            Some(n) if n <= max => Ok(n),
            _ => bail!("{key} is a whole number from 0 to {max}"),
        },
    }
}

fn optional(input: &Value, key: &str, min: u64, max: u64) -> Result<Option<u64>> {
    match &input[key] {
        Value::Null => Ok(None),
        v => match v.as_u64() {
            Some(n) if (min..=max).contains(&n) => Ok(Some(n)),
            _ => bail!("{key} is a whole number from {min} to {max}"),
        },
    }
}

fn choice(
    input: &Value,
    key: &str,
    list: &[&'static str],
    default: &'static str,
) -> Result<&'static str> {
    match input[key]
        .as_str()
        .map(|s| s.trim().to_ascii_lowercase().replace([' ', '-'], "_"))
    {
        None => Ok(default),
        Some(s) if s.is_empty() => Ok(default),
        Some(s) => list
            .iter()
            .find(|c| **c == s)
            .copied()
            .ok_or_else(|| anyhow!("{key} is one of {}, not {s:?}", list.join(", "))),
    }
}

fn token(input: &Value, key: &str) -> Result<Option<String>> {
    match input[key].as_str().map(str::trim) {
        None | Some("") => Ok(None),
        Some(t) if t.len() <= MAX_TOKEN_CHARS && !t.chars().any(char::is_control) => {
            Ok(Some(t.to_string()))
        }
        Some(_) => bail!("{key} is not a token this tool returned"),
    }
}

/// The region of a locale: `CH` for `de-CH`, `419` for `es-419`.
fn region(locale: &str) -> Option<&str> {
    locale.split('-').nth(1).filter(|r| !r.is_empty())
}

/// The currency a region usually pays in, USD where unknown.
fn currency_of(region: Option<&str>) -> &'static str {
    match region.unwrap_or("") {
        "AT" | "BE" | "DE" | "ES" | "FI" | "FR" | "GR" | "IE" | "IT" | "LU" | "NL" | "PT" => "EUR",
        "GB" => "GBP",
        "CH" => "CHF",
        "CA" => "CAD",
        "AU" => "AUD",
        "NZ" => "NZD",
        "MX" => "MXN",
        "BR" => "BRL",
        "AR" => "ARS",
        "CL" => "CLP",
        "CO" => "COP",
        "PE" => "PEN",
        "JP" => "JPY",
        "IN" => "INR",
        "SE" => "SEK",
        "NO" => "NOK",
        "DK" => "DKK",
        "PL" => "PLN",
        "CZ" => "CZK",
        _ => "USD",
    }
}

fn parse_input(input: &Value, locale: &str, today: NaiveDate) -> Result<Request> {
    let from = codes(input, "from", false)?;
    let to = codes(input, "to", false)?;
    if from.is_empty() || to.is_empty() {
        bail!("give from and to as IATA airport codes, e.g. [\"SFO\"] and [\"MUC\"]");
    }
    if from.iter().any(|a| to.contains(a)) {
        bail!("from and to share an airport");
    }
    let date_ = date(input, "date", today)?.ok_or_else(|| anyhow!("give date as YYYY-MM-DD"))?;
    let return_date = date(input, "return_date", today)?;
    if return_date.is_some_and(|r| r < date_) {
        bail!("return_date is before date");
    }
    let adults = count(input, "adults", 1, MAX_PASSENGERS)?;
    let children = count(input, "children", 0, MAX_PASSENGERS)?;
    let infants_in_seat = count(input, "infants_in_seat", 0, MAX_PASSENGERS)?;
    let infants_on_lap = count(input, "infants_on_lap", 0, MAX_PASSENGERS)?;
    if adults + children + infants_in_seat + infants_on_lap > MAX_PASSENGERS {
        bail!("at most {MAX_PASSENGERS} passengers in one search");
    }
    if adults + children == 0 {
        bail!("at least one adult or child has to travel");
    }
    if infants_on_lap > adults {
        bail!("each infant on a lap needs an adult");
    }
    let airlines = codes(input, "airlines", true)?;
    let exclude_airlines = codes(input, "exclude_airlines", true)?;
    if !airlines.is_empty() && !exclude_airlines.is_empty() {
        bail!("give airlines or exclude_airlines, not both");
    }
    let country = region(locale).filter(|r| r.len() == 2).map(String::from);
    let currency = match input["currency"]
        .as_str()
        .map(|c| c.trim().to_ascii_uppercase())
    {
        None => currency_of(region(locale)).to_string(),
        Some(c) if c.is_empty() => currency_of(region(locale)).to_string(),
        Some(c) if c.len() == 3 && c.chars().all(|x| x.is_ascii_uppercase()) => c,
        Some(c) => bail!("currency is a three-letter code such as USD or EUR, not {c:?}"),
    };
    let departure_token = token(input, "departure_token")?;
    let booking_token = token(input, "booking_token")?;
    if departure_token.is_some() && return_date.is_none() {
        bail!("departure_token is for round trips: repeat the first search, return_date included");
    }
    Ok(Request {
        from,
        to,
        date: date_,
        return_date,
        adults,
        children,
        infants_in_seat,
        infants_on_lap,
        cabin: choice(input, "cabin", CABINS, "economy")?,
        max_stops: optional(input, "max_stops", 0, 2)?,
        max_price: optional(input, "max_price", 1, 10_000_000)?,
        airlines,
        exclude_airlines,
        carry_on_bags: optional(input, "carry_on_bags", 0, 2)?,
        sort: choice(input, "sort", SORTS, "best")?,
        currency,
        country,
        max_results: optional(input, "max_results", 1, MAX_RESULTS as u64)?
            .map_or(DEFAULT_RESULTS, |n| n as usize),
        departure_token,
        booking_token,
    })
}

// -- the result -------------------------------------------------------------------------------

fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

fn text(v: &Value) -> Option<String> {
    v.as_str()
        .map(|s| clip(s, MAX_TEXT_CHARS))
        .filter(|s| !s.is_empty())
}

fn https(v: &Value) -> Option<String> {
    v.as_str()
        .filter(|u| u.starts_with("https://") && u.len() <= 2000)
        .map(String::from)
}

/// Google's local time, `2026-12-01 07:05`, as `2026-12-01T07:05`.
fn local_time(v: &Value) -> Option<String> {
    let t = NaiveDateTime::parse_from_str(v.as_str()?, "%Y-%m-%d %H:%M").ok()?;
    Some(t.format("%Y-%m-%dT%H:%M").to_string())
}

fn number(v: &Value) -> Option<f64> {
    v.as_f64().filter(|x| x.is_finite() && *x >= 0.0)
}

fn minutes(v: &Value) -> Option<u64> {
    v.as_u64()
}

fn put(m: &mut Map<String, Value>, key: &str, v: Option<impl Into<Value>>) {
    if let Some(v) = v {
        m.insert(key.into(), v.into());
    }
}

/// One airport of a segment: `{"airport", "name"?, "time"?}`.
fn airport(v: &Value) -> Value {
    let mut m = Map::new();
    m.insert("airport".into(), json!(text(&v["id"]).unwrap_or_default()));
    put(&mut m, "name", text(&v["name"]));
    put(&mut m, "time", local_time(&v["time"]));
    Value::Object(m)
}

/// One option of Google's answer as the card shows it, or `None` without flights.
fn option(id: usize, o: &Value, top_pick: bool) -> Option<Value> {
    let legs = o["flights"].as_array().filter(|l| !l.is_empty())?;
    let mut airlines: Vec<String> = Vec::new();
    let segments: Vec<Value> = legs
        .iter()
        .map(|leg| {
            if let Some(a) = text(&leg["airline"]) {
                if !airlines.contains(&a) {
                    airlines.push(a);
                }
            }
            let mut s = Map::new();
            put(&mut s, "flight", text(&leg["flight_number"]));
            put(&mut s, "airline", text(&leg["airline"]));
            put(&mut s, "logo", https(&leg["airline_logo"]));
            s.insert("from".into(), airport(&leg["departure_airport"]));
            s.insert("to".into(), airport(&leg["arrival_airport"]));
            put(&mut s, "duration_min", minutes(&leg["duration"]));
            put(&mut s, "aircraft", text(&leg["airplane"]));
            put(&mut s, "cabin", text(&leg["travel_class"]));
            put(&mut s, "legroom", text(&leg["legroom"]));
            s.insert(
                "overnight".into(),
                json!(leg["overnight"].as_bool().unwrap_or(false)),
            );
            s.insert(
                "often_delayed".into(),
                json!(leg["often_delayed_by_over_30_min"]
                    .as_bool()
                    .unwrap_or(false)),
            );
            Value::Object(s)
        })
        .collect();
    let layovers: Vec<Value> = o["layovers"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|l| {
            let mut m = Map::new();
            m.insert("airport".into(), json!(text(&l["id"]).unwrap_or_default()));
            put(&mut m, "name", text(&l["name"]));
            put(&mut m, "duration_min", minutes(&l["duration"]));
            m.insert(
                "overnight".into(),
                json!(l["overnight"].as_bool().unwrap_or(false)),
            );
            Value::Object(m)
        })
        .collect();
    let mut m = Map::new();
    m.insert("id".into(), json!(id));
    m.insert("top_pick".into(), json!(top_pick));
    m.insert("price".into(), json!(number(&o["price"])));
    m.insert("airlines".into(), json!(airlines));
    put(&mut m, "logo", https(&o["airline_logo"]));
    m.insert("depart".into(), segments[0]["from"].clone());
    m.insert("arrive".into(), segments[segments.len() - 1]["to"].clone());
    put(&mut m, "duration_min", minutes(&o["total_duration"]));
    m.insert("stops".into(), json!(segments.len() - 1));
    m.insert("segments".into(), Value::Array(segments));
    m.insert("layovers".into(), Value::Array(layovers));
    let co2 = &o["carbon_emissions"];
    put(
        &mut m,
        "co2_kg",
        number(&co2["this_flight"]).map(|g| (g / 1000.0).round()),
    );
    put(
        &mut m,
        "co2_vs_typical_pct",
        co2["difference_percent"].as_i64(),
    );
    for key in ["departure_token", "booking_token"] {
        put(
            &mut m,
            key,
            o[key].as_str().filter(|t| t.len() <= MAX_TOKEN_CHARS),
        );
    }
    Some(Value::Object(m))
}

/// Google's price insights: the lowest price, its verdict, the typical range and the history.
fn insights(v: &Value) -> Option<Value> {
    let lowest = number(&v["lowest_price"])?;
    let mut m = Map::new();
    m.insert("lowest".into(), json!(lowest));
    put(&mut m, "level", text(&v["price_level"]));
    if let Some([lo, hi]) = v["typical_price_range"].as_array().map(Vec::as_slice) {
        if let (Some(lo), Some(hi)) = (number(lo), number(hi)) {
            m.insert("typical".into(), json!([lo, hi]));
        }
    }
    let history: Vec<Value> = v["price_history"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let t = p[0].as_i64()?;
            // Seconds, or milliseconds from some answers.
            let t = if t > 100_000_000_000 { t / 1000 } else { t };
            let day = DateTime::from_timestamp(t, 0)?.date_naive();
            Some(json!({ "date": day.to_string(), "price": number(&p[1])? }))
        })
        .collect();
    let skip = history.len().saturating_sub(MAX_HISTORY);
    if history.len() >= 2 {
        m.insert("history".into(), Value::Array(history[skip..].to_vec()));
    }
    Some(Value::Object(m))
}

/// What both kinds of result say about the search.
fn header(req: &Request, kind: &str) -> Map<String, Value> {
    let returning = req.departure_token.is_some() && kind == "flights";
    let (from, to) = if returning {
        (&req.to, &req.from)
    } else {
        (&req.from, &req.to)
    };
    let mut m = Map::new();
    m.insert("kind".into(), json!(kind));
    m.insert(
        "leg".into(),
        json!(match (req.return_date, returning) {
            (None, _) => "one_way",
            (Some(_), false) => "outbound",
            (Some(_), true) => "return",
        }),
    );
    m.insert("from".into(), json!(from));
    m.insert("to".into(), json!(to));
    m.insert(
        "date".into(),
        json!(if returning {
            req.return_date.unwrap()
        } else {
            req.date
        }
        .to_string()),
    );
    if let Some(d) = req.return_date {
        m.insert("return_date".into(), json!(d.to_string()));
    }
    m.insert(
        "passengers".into(),
        json!({ "adults": req.adults, "children": req.children,
                "infants_in_seat": req.infants_in_seat, "infants_on_lap": req.infants_on_lap }),
    );
    m.insert("cabin".into(), json!(req.cabin));
    m.insert("currency".into(), json!(req.currency));
    m
}

fn flights(req: &Request, body: &Value, now: DateTime<Utc>) -> Value {
    let mut m = header(req, "flights");
    let best = body["best_flights"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    let other = body["other_flights"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    // Google's top picks only mean something under its own order.
    let picks = if req.sort == "best" { best.len() } else { 0 };
    let all: Vec<Value> = best
        .iter()
        .chain(other)
        .enumerate()
        .filter_map(|(i, o)| option(i + 1, o, i < picks))
        .collect();
    m.insert("sort".into(), json!(req.sort));
    m.insert(
        "more".into(),
        json!(all.len().saturating_sub(req.max_results)),
    );
    m.insert(
        "options".into(),
        Value::Array(all.into_iter().take(req.max_results).collect()),
    );
    if body["no_results"] == true {
        m.insert(
            "no_results".into(),
            json!(
                "Google Flights found no flights for this search. Try nearby dates, more \
                   airports, more stops, or without the price cap or airline filter."
            ),
        );
    }
    if req.departure_token.is_none() {
        put(&mut m, "insights", insights(&body["price_insights"]));
    }
    put(
        &mut m,
        "google_flights_url",
        https(&body["search_metadata"]["google_flights_url"]),
    );
    m.insert(
        "as_of".into(),
        json!(now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
    );
    m.insert("note".into(), json!(NOTE));
    Value::Object(m)
}

fn booking(req: &Request, body: &Value) -> Value {
    let mut m = header(req, "booking");
    let chosen: Vec<Value> = body["selected_flights"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(i, o)| option(i + 1, o, false))
        .map(|mut o| {
            // A chosen flight can't be chosen again.
            if let Value::Object(m) = &mut o {
                m.remove("departure_token");
                m.remove("booking_token");
            }
            o
        })
        .collect();
    m.insert("flights".into(), Value::Array(chosen));
    // Each option sells the whole trip ("together") or one way of it on a ticket of its own.
    let options: Vec<Value> = body["booking_options"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|o| {
            ["together", "departing", "returning"]
                .into_iter()
                .filter(|part| o[part].is_object())
                .map(|part| (part, &o[part]))
                .collect::<Vec<_>>()
        })
        .filter_map(|(part, o)| {
            let mut b = Map::new();
            b.insert("seller".into(), json!(text(&o["book_with"])?));
            b.insert("part".into(), json!(part));
            b.insert("price".into(), json!(number(&o["price"])));
            put(&mut b, "fare", text(&o["option_title"]));
            let list = |key: &str| -> Vec<String> {
                o[key]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(text)
                    .take(6)
                    .collect()
            };
            b.insert("bags".into(), json!(list("baggage_prices")));
            b.insert("marketed_as".into(), json!(list("marketed_as")));
            Some(Value::Object(b))
        })
        .take(MAX_BOOKING_OPTIONS)
        .collect();
    if options.is_empty() {
        m.insert(
            "no_results".into(),
            json!("Google Flights lists no booking options for this choice any more; search again for fresh prices."),
        );
    }
    m.insert("options".into(), Value::Array(options));
    put(
        &mut m,
        "google_flights_url",
        https(&body["search_metadata"]["google_flights_url"]),
    );
    m.insert("note".into(), json!(NOTE));
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 29).unwrap()
    }

    fn parse(input: Value) -> Result<Request> {
        parse_input(&input, "de-CH", today())
    }

    #[test]
    fn input_is_normalized_into_serpapi_parameters() {
        let req = parse(
            json!({ "from": " sfo, oak ", "to": ["MUC", "muc"], "date": "2026-12-01",
                                "return_date": "2026-12-10", "adults": 2, "max_stops": 1,
                                "cabin": "Premium Economy", "airlines": ["lh", "UA"] }),
        )
        .unwrap();
        assert_eq!(req.from, ["SFO", "OAK"]);
        assert_eq!(req.to, ["MUC"]);
        assert_eq!(req.currency, "CHF");
        assert_eq!(req.cabin, "premium_economy");
        let p: std::collections::HashMap<_, _> = req.params().into_iter().collect();
        assert_eq!(p["departure_id"], "SFO,OAK");
        assert_eq!(p["type"], "1");
        assert_eq!(p["travel_class"], "2");
        assert_eq!(p["stops"], "2");
        assert_eq!(p["sort_by"], "1");
        assert_eq!(p["gl"], "ch");
        assert_eq!(p["include_airlines"], "LH,UA");
        assert!(!p.contains_key("max_price"));

        let one_way = parse_input(
            &json!({ "from": "JFK", "to": "LHR", "date": "2026-11-02" }),
            "en",
            today(),
        )
        .unwrap();
        let p: std::collections::HashMap<_, _> = one_way.params().into_iter().collect();
        assert_eq!(p["type"], "2");
        assert_eq!(p["currency"], "USD");
        assert!(!p.contains_key("gl") && !p.contains_key("return_date"));
    }

    #[test]
    fn bad_input_is_rejected() {
        let err = |input: Value| format!("{:#}", parse(input).unwrap_err());
        let base = |extra: Value| {
            let mut v = json!({ "from": ["SFO"], "to": ["MUC"], "date": "2026-12-01" });
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            v
        };
        assert!(err(base(json!({ "from": ["San Francisco"] }))).contains("not an airport code"));
        assert!(err(base(json!({ "to": ["SFO"] }))).contains("share an airport"));
        assert!(err(base(json!({ "date": "2025-01-01" }))).contains("in the past"));
        assert!(err(base(json!({ "date": "2027-12-01" }))).contains("too far ahead"));
        assert!(err(base(json!({ "date": "1.12.2026" }))).contains("a date like"));
        assert!(err(base(json!({ "return_date": "2026-11-30" }))).contains("before date"));
        assert!(err(base(json!({ "infants_on_lap": 2 }))).contains("needs an adult"));
        assert!(err(base(json!({ "adults": 5, "children": 5 }))).contains("at most 9"));
        assert!(err(base(json!({ "adults": 0 }))).contains("at least one"));
        assert!(err(base(
            json!({ "airlines": ["LH"], "exclude_airlines": ["UA"] })
        ))
        .contains("not both"));
        assert!(err(base(json!({ "airlines": ["Lufthansa"] }))).contains("not an airline code"));
        assert!(err(base(json!({ "cabin": "coach" }))).contains("cabin is one of"));
        assert!(err(base(json!({ "currency": "dollars" }))).contains("three-letter"));
        assert!(err(base(json!({ "departure_token": "x" }))).contains("for round trips"));
        assert!(err(json!({ "to": ["MUC"], "date": "2026-12-01" })).contains("give from and to"));
    }

    fn leg(flight: &str, from: &str, to: &str, departs: &str, arrives: &str) -> Value {
        json!({ "departure_airport": { "name": format!("{from} Airport"), "id": from, "time": departs },
                "arrival_airport": { "name": format!("{to} Airport"), "id": to, "time": arrives },
                "duration": 500, "airplane": "Airbus A350", "airline": "Lufthansa",
                "airline_logo": "https://www.gstatic.com/flights/airline_logos/70px/LH.png",
                "travel_class": "Economy", "flight_number": flight, "legroom": "31 in",
                "often_delayed_by_over_30_min": true })
    }

    #[test]
    fn google_answers_become_the_card() {
        let req = parse(
            json!({ "from": ["SFO"], "to": ["MUC"], "date": "2026-12-01",
                                "return_date": "2026-12-10", "max_results": 1 }),
        )
        .unwrap();
        let body = json!({
            "search_metadata": { "google_flights_url": "https://www.google.com/travel/flights?tfs=x" },
            "best_flights": [{ "flights": [leg("LH 455", "SFO", "MUC", "2026-12-01 15:40", "2026-12-02 11:30")],
                               "total_duration": 650, "price": 1234, "departure_token": "DEP1",
                               "carbon_emissions": { "this_flight": 512_400, "difference_percent": -7 } }],
            "other_flights": [{ "flights": [leg("UA 58", "SFO", "EWR", "2026-12-01 07:00", "2026-12-01 15:30"),
                                            leg("UA 30", "EWR", "MUC", "2026-12-01 17:30", "2026-12-02 07:45")],
                                "layovers": [{ "duration": 120, "name": "Newark", "id": "EWR" }],
                                "total_duration": 1005, "price": 999 }],
            "price_insights": { "lowest_price": 999, "price_level": "low", "typical_price_range": [1100, 1600],
                                "price_history": [[1_792_000_000, 1300], [1_792_086_400_000_i64, 1250]] }
        });
        let now = DateTime::from_timestamp(1_792_800_000, 0).unwrap();
        let r = flights(&req, &body, now);
        assert_eq!(r["kind"], "flights");
        assert_eq!(r["leg"], "outbound");
        assert_eq!(r["currency"], "CHF");
        assert_eq!(r["more"], 1);
        let o = &r["options"][0];
        assert_eq!(o["top_pick"], true);
        assert_eq!(o["price"], 1234.0);
        assert_eq!(o["airlines"], json!(["Lufthansa"]));
        assert_eq!(
            o["depart"],
            json!({ "airport": "SFO", "name": "SFO Airport", "time": "2026-12-01T15:40" })
        );
        assert_eq!(o["arrive"]["time"], "2026-12-02T11:30");
        assert_eq!(o["stops"], 0);
        assert_eq!(o["co2_kg"], 512.0);
        assert_eq!(o["co2_vs_typical_pct"], -7);
        assert_eq!(o["segments"][0]["often_delayed"], true);
        assert_eq!(o["departure_token"], "DEP1");
        assert_eq!(r["insights"]["typical"], json!([1100.0, 1600.0]));
        assert_eq!(r["insights"]["history"][1]["date"], "2026-10-15");
        assert_eq!(
            r["google_flights_url"],
            "https://www.google.com/travel/flights?tfs=x"
        );

        let all = flights(
            &Request {
                max_results: 5,
                sort: "price",
                ..req
            },
            &body,
            now,
        );
        let second = &all["options"][1];
        assert_eq!(
            second["top_pick"], false,
            "no top picks when sorted by price"
        );
        assert_eq!(second["stops"], 1);
        assert_eq!(
            second["layovers"][0],
            json!({ "airport": "EWR", "name": "Newark", "duration_min": 120, "overnight": false })
        );
    }

    #[test]
    fn the_return_leg_and_booking_options() {
        let mut req = parse(
            json!({ "from": ["SFO"], "to": ["MUC"], "date": "2026-12-01",
                                    "return_date": "2026-12-10", "departure_token": "DEP1" }),
        )
        .unwrap();
        let r = flights(&req, &json!({ "no_results": true }), Utc::now());
        assert_eq!(r["leg"], "return");
        assert_eq!(r["from"], json!(["MUC"]));
        assert_eq!(r["date"], "2026-12-10");
        assert_eq!(r["options"], json!([]));
        assert!(r["no_results"].as_str().unwrap().contains("no flights"));

        req.departure_token = None;
        req.booking_token = Some("B".into());
        let body = json!({
            "selected_flights": [{ "flights": [leg("LH 455", "SFO", "MUC", "2026-12-01 15:40", "2026-12-02 11:30")],
                                   "price": 1234, "booking_token": "B" }],
            "booking_options": [
                { "together": { "book_with": "Lufthansa", "price": 1234, "option_title": "Economy Light",
                                "baggage_prices": ["1 free carry-on", "1st checked bag: 60 CHF"],
                                "booking_request": { "url": "https://www.google.com/travel/clk/f", "post_data": "u=x" } } },
                { "departing": { "book_with": "Expedia", "price": 600 }, "returning": { "book_with": "Expedia", "price": 640 } }
            ]
        });
        let b = booking(&req, &body);
        assert_eq!(b["kind"], "booking");
        assert_eq!(b["leg"], "outbound");
        assert!(b["flights"][0].get("booking_token").is_none());
        let options = b["options"].as_array().unwrap();
        assert_eq!(options.len(), 3);
        assert_eq!(options[0]["seller"], "Lufthansa");
        assert_eq!(options[0]["fare"], "Economy Light");
        assert_eq!(options[0]["bags"][1], "1st checked bag: 60 CHF");
        assert_eq!(options[2]["part"], "returning");
        assert!(b.get("no_results").is_none());
    }
}
