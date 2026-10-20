use super::open_meteo::coordinates;
use super::{truncate_output, Tool, ToolContext, ToolModel};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use reqwest::StatusCode;
use serde_json::{json, Map, Value};
use std::time::Duration;

const MAX_TEXT_CHARS: usize = 200;
/// Longest preferences note passed to the model, and longest reason kept from its answer.
const MAX_NOTE_CHARS: usize = 500;
/// Transit itineraries kept from the planner's answer.
const MAX_ITINERARIES: usize = 3;
/// Points kept per drawn line after simplification; enough for a city-scale route.
const MAX_PATH_POINTS: usize = 250;
/// Simplification tolerance in degrees (about 10 m).
const PATH_TOLERANCE: f64 = 0.0001;
/// A walk this short is the obvious answer (for the rules used when the model can't choose).
const SHORT_WALK_MIN: i64 = 20;
/// Minutes added to a drive for parking and walking from the car, when comparing it to transit.
const PARKING_MIN: i64 = 5;
/// Minutes to wait for a rideshare pickup.
const PICKUP_MIN: i64 = 5;
/// Rough US rideshare pricing: base fare, per km, per minute, minimum fare, and how much demand
/// and city can raise it. Only an estimate; there is no rideshare API.
const RIDE_BASE: f64 = 2.5;
const RIDE_PER_KM: f64 = 1.0;
const RIDE_PER_MIN: f64 = 0.3;
const RIDE_MIN_FARE: f64 = 8.0;
const RIDE_SURGE: f64 = 1.5;
const ATTRIBUTION: &str = "Transit: Transitous (transitous.org, MOTIS). Roads: OSRM, map data \
                           © OpenStreetMap contributors.";

/// Compares the ways to make a trip — public transit, walking, driving, and a rideshare — and
/// says which one to take. The chat front end draws the result as a route map with the options.
///
/// Transit itineraries and walking come from a MOTIS trip planner (by default the public
/// Transitous instance), which also geocodes addresses and stops; driving comes from an OSRM
/// router. Both run in the cedar process. The rideshare fare is an estimate from the driving
/// distance and time. The run's model weighs the options against what the user said matters and
/// picks one (see [`choose`]); if it can't, fixed rules do (see [`recommend`]). The result is JSON: `{"from", "to", "options": [...], "recommendation",
/// "unavailable"?, "time_zone"?, ...}`; each option's `path` (and each transit leg's) is an encoded polyline
/// (precision 5) for the map.
pub struct PlanTrip {
    http: reqwest::Client,
    transit_url: String,
    driving_url: String,
}

impl PlanTrip {
    /// `transit_url` is the MOTIS server's root (the `/api/v1/...` routes go under it), and
    /// `driving_url` the OSRM server's (the `/route/v1/driving/...` route goes under it).
    pub fn new(transit_url: String, driving_url: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .user_agent(concat!("cedar/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            http,
            transit_url: transit_url.trim_end_matches('/').to_string(),
            driving_url: driving_url.trim_end_matches('/').to_string(),
        })
    }

    async fn get_json(
        &self,
        service: &str,
        url: &str,
        params: &[(&str, String)],
        timeout: Duration,
    ) -> Result<Value> {
        let resp = self
            .http
            .get(url)
            .header("Accept", "application/json")
            .query(params)
            .timeout(timeout)
            .send()
            .await
            .with_context(|| format!("sending {service} request"))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .with_context(|| format!("reading {service} response"))?;
        match status {
            s if s.is_success() => {}
            StatusCode::TOO_MANY_REQUESTS => {
                bail!("{service} is rate limited; wait a moment before asking again")
            }
            _ => bail!("{service} error {status}: {}", error_message(&text)),
        }
        serde_json::from_str(&text).with_context(|| format!("parsing {service} response"))
    }

    /// The coordinates of an endpoint, looking its name up if it has none.
    async fn resolve(&self, end: End<'_>, timeout: Duration) -> Result<Point> {
        let (name, label) = match end {
            End::Point(p) => return Ok(p),
            End::Name { name, label } => (name, label),
        };
        let url = format!("{}/api/v1/geocode", self.transit_url);
        let body = self
            .get_json("geocoding", &url, &[("text", name.to_string())], timeout)
            .await?;
        let hit = body
            .as_array()
            .and_then(|hits| hits.first())
            .filter(|h| h["lat"].is_f64() && h["lon"].is_f64())
            .ok_or_else(|| {
                anyhow!("no place found for {name:?}; try a fuller address or pass latitude and longitude")
            })?;
        Ok(Point {
            label: label.unwrap_or_else(|| match_label(hit)),
            latitude: hit["lat"].as_f64().unwrap_or_default(),
            longitude: hit["lon"].as_f64().unwrap_or_default(),
        })
    }

    async fn transit(
        &self,
        from: &Point,
        to: &Point,
        when: &When,
        timeout: Duration,
    ) -> Result<Value> {
        let mut params = vec![
            ("fromPlace", from.place()),
            ("toPlace", to.place()),
            ("detailedTransfers", "true".to_string()),
            ("directModes", "WALK".to_string()),
        ];
        match when {
            When::Now => {}
            When::Depart(t) => params.push(("time", t.clone())),
            When::Arrive(t) => {
                params.push(("time", t.clone()));
                params.push(("arriveBy", "true".to_string()));
            }
        }
        let url = format!("{}/api/v1/plan", self.transit_url);
        self.get_json("transit planner", &url, &params, timeout)
            .await
    }

    async fn drive(&self, from: &Point, to: &Point, timeout: Duration) -> Result<Value> {
        let url = format!(
            "{}/route/v1/driving/{},{};{},{}",
            self.driving_url, from.longitude, from.latitude, to.longitude, to.latitude
        );
        let params = [
            ("overview", "full".to_string()),
            ("geometries", "polyline".to_string()),
        ];
        let body = self
            .get_json("driving router", &url, &params, timeout)
            .await?;
        if body["code"].as_str() != Some("Ok") {
            let why = body["message"]
                .as_str()
                .or(body["code"].as_str())
                .unwrap_or("no route");
            bail!("no driving route: {}", clip(why));
        }
        Ok(body)
    }
}

#[async_trait]
impl Tool for PlanTrip {
    fn name(&self) -> &str {
        "plan_trip"
    }

    fn description(&self) -> &str {
        "Compare the ways to get from one place to another — public transit (bus, tram, subway, \
         train, ferry), walking, driving, and a rideshare (taxi, Uber, Lyft) — and get a \
         recommendation. Use it for questions like \"is there a bus to ...\", \"should I drive or \
         take the train\", or \"how do I get to ...\". Give `from` and `to` as an address, stop, or \
         place name, or as latitude and longitude. Returns each option's duration, departure and \
         arrival times, transfers and route names for transit, driving distance, and an estimated \
         rideshare fare range, and which option to take and why; the chat shows the routes on a \
         map with the options. Pass `preferences` with anything the user said matters for this \
         trip (cost, fewest transfers, less walking, luggage, accessibility, being on time), so \
         the recommendation weighs it. Driving times assume no traffic, and rideshare fares are \
         rough US-rate estimates, so say so."
    }

    fn schema(&self) -> Value {
        let end = |what: &str| {
            json!({ "type": "object",
                    "description": format!("Where the trip {what}."),
                    "properties": {
                        "location": { "type": "string",
                                      "description": "An address, stop, station, or place, e.g. \"Alexanderplatz, Berlin\" or \"1 Market St, San Francisco\". Use this or latitude and longitude." },
                        "latitude": { "type": "number", "minimum": -90, "maximum": 90 },
                        "longitude": { "type": "number", "minimum": -180, "maximum": 180 },
                        "label": { "type": "string", "description": "A short name to show, e.g. \"Home\"." }
                    } })
        };
        json!({ "type": "object",
                "properties": {
                    "from": end("starts"),
                    "to": end("ends"),
                    "depart_at": { "type": "string",
                                   "description": "Leave at this time: ISO 8601 with a UTC offset, e.g. \"2026-10-18T08:30:00-07:00\". Defaults to now." },
                    "arrive_by": { "type": "string",
                                   "description": "Arrive by this time instead, in the same format. Give at most one of depart_at and arrive_by." },
                    "has_car": { "type": "boolean",
                                 "description": "Whether the user has a car to drive. Defaults to true; when false, driving is still compared but a rideshare is recommended instead." },
                    "preferences": { "type": "string",
                                     "description": "What the user said matters for this trip, in their words, e.g. \"cheapest\", \"as few transfers as possible\", \"I have a big suitcase\", \"step-free\", or \"must be there by 9\"." },
                    "title": { "type": "string", "description": "A short caption for the trip." }
                },
                "required": ["from", "to"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let request = parse_input(input)?;
        let from = self
            .resolve(request.from, ctx.timeout)
            .await
            .context("from")?;
        let to = self.resolve(request.to, ctx.timeout).await.context("to")?;

        let (transit, drive) = tokio::join!(
            self.transit(&from, &to, &request.when, ctx.timeout),
            self.drive(&from, &to, ctx.timeout)
        );

        let mut options = Vec::new();
        let mut unavailable = Vec::new();
        let mut time_zone = None;
        match transit {
            Ok(plan) => {
                let found = transit_options(&plan);
                if found.transit.is_empty() {
                    unavailable.push(json!({ "mode": "transit", "reason": "No public transit route was found for this trip and time." }));
                }
                options.extend(found.transit);
                options.extend(found.walk);
                time_zone = found.time_zone;
            }
            Err(e) => unavailable.push(json!({ "mode": "transit", "reason": format!("{e:#}") })),
        }
        match drive.and_then(|body| drive_options(&body)) {
            Ok(found) => options.extend(found),
            Err(e) => {
                let reason = format!("{e:#}");
                unavailable.push(json!({ "mode": "drive", "reason": reason }));
                unavailable.push(json!({ "mode": "rideshare", "reason": reason }));
            }
        }
        if options.is_empty() {
            let reasons: Vec<String> = unavailable
                .iter()
                .filter_map(|u| u["reason"].as_str().map(String::from))
                .collect();
            bail!("no way to make this trip was found: {}", reasons.join("; "));
        }
        let recommendation = match &ctx.model {
            // With a single option there is nothing to weigh.
            Some(model) if options.len() > 1 => {
                let trip = Trip {
                    from: &from,
                    to: &to,
                    when: &request.when,
                    has_car: request.has_car,
                    preferences: request.preferences.as_deref(),
                };
                match choose(model, &trip, &options, &unavailable).await {
                    Ok(choice) => choice,
                    Err(e) => {
                        tracing::warn!(error = %format!("{e:#}"), "the model could not choose a trip option; using the rules");
                        recommend(&options, request.has_car)
                    }
                }
            }
            _ => recommend(&options, request.has_car),
        };
        Ok(trip_json(
            request.title,
            &request.when,
            time_zone,
            &from,
            &to,
            options,
            unavailable,
            recommendation,
        )
        .to_string())
    }
}

/// A resolved endpoint.
#[derive(Debug, Clone, PartialEq)]
struct Point {
    label: String,
    latitude: f64,
    longitude: f64,
}

impl Point {
    /// MOTIS takes a place as "lat,lon".
    fn place(&self) -> String {
        format!("{},{}", self.latitude, self.longitude)
    }

    fn json(&self) -> Value {
        json!({ "label": self.label, "latitude": self.latitude, "longitude": self.longitude })
    }
}

/// An endpoint as asked for: exact coordinates, or a name to look up.
#[derive(Debug, PartialEq)]
enum End<'a> {
    Point(Point),
    Name {
        name: &'a str,
        label: Option<String>,
    },
}

#[derive(Debug, PartialEq)]
enum When {
    Now,
    Depart(String),
    Arrive(String),
}

#[derive(Debug)]
struct TripRequest<'a> {
    title: Option<String>,
    from: End<'a>,
    to: End<'a>,
    when: When,
    has_car: bool,
    /// What the user said matters for this trip.
    preferences: Option<String>,
}

fn clip(s: &str) -> String {
    s.chars().take(MAX_TEXT_CHARS).collect()
}

/// `value` as trimmed text, cut to `MAX_TEXT_CHARS`, or `None` if it is missing or blank.
fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(clip)
}

fn parse_input(input: &Value) -> Result<TripRequest<'_>> {
    let from = parse_end(&input["from"]).context("from")?;
    let to = parse_end(&input["to"]).context("to")?;
    let time = |key: &str| -> Result<Option<String>> {
        match text(&input[key]) {
            None => Ok(None),
            Some(t) => chrono::DateTime::parse_from_rfc3339(&t)
                .map(|_| Some(t.clone()))
                .map_err(|_| {
                    anyhow!("{key} must be ISO 8601 with a UTC offset, e.g. 2026-10-18T08:30:00-07:00; got {t:?}")
                }),
        }
    };
    let when = match (time("depart_at")?, time("arrive_by")?) {
        (Some(_), Some(_)) => bail!("give at most one of depart_at and arrive_by"),
        (Some(t), None) => When::Depart(t),
        (None, Some(t)) => When::Arrive(t),
        (None, None) => When::Now,
    };
    let has_car = match &input["has_car"] {
        Value::Null => true,
        Value::Bool(b) => *b,
        _ => bail!("has_car must be true or false"),
    };
    let preferences = input["preferences"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(MAX_NOTE_CHARS).collect());
    Ok(TripRequest {
        title: text(&input["title"]),
        from,
        to,
        when,
        has_car,
        preferences,
    })
}

/// An endpoint: an object with a location or coordinates, or a bare string naming the place.
fn parse_end(end: &Value) -> Result<End<'_>> {
    if let Some(name) = end.as_str().map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(End::Name { name, label: None });
    }
    if !end.is_object() {
        bail!("give a location, or latitude and longitude");
    }
    let name = end["location"]
        .as_str()
        .map(str::trim)
        .filter(|l| !l.is_empty());
    let label = text(&end["label"]);
    Ok(match coordinates(end)? {
        Some((latitude, longitude)) => End::Point(Point {
            label: label
                .or_else(|| name.map(clip))
                .unwrap_or_else(|| format!("{latitude:.4}, {longitude:.4}")),
            latitude,
            longitude,
        }),
        None => End::Name {
            name: name.ok_or_else(|| anyhow!("give a location, or both latitude and longitude"))?,
            label,
        },
    })
}

/// "Alexanderplatz, Berlin" from a MOTIS geocoding match: its name and its default area.
fn match_label(hit: &Value) -> String {
    let name = hit["name"].as_str().unwrap_or("").trim();
    let areas = hit["areas"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let area = areas
        .iter()
        .find(|a| a["default"].as_bool() == Some(true))
        .or_else(|| areas.first())
        .and_then(|a| a["name"].as_str())
        .map(str::trim)
        .filter(|a| !a.is_empty() && *a != name);
    let label = match area {
        Some(area) if !name.is_empty() => format!("{name}, {area}"),
        _ => name.to_string(),
    };
    clip(&label)
}

/// The text of an error body: a MOTIS or OSRM `message`/`error`, or a prefix of the raw body.
fn error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v["message"]
                .as_str()
                .or(v["error"].as_str())
                .map(String::from)
        })
        .unwrap_or_else(|| truncate_output(body.trim(), 300))
}

/// Whole minutes, rounded up so a 30-second hop isn't "0 min".
fn minutes(seconds: f64) -> i64 {
    (seconds / 60.0).ceil().max(0.0) as i64
}

struct TransitFound {
    transit: Vec<Value>,
    walk: Vec<Value>,
    /// The IANA time zone of the start, when the planner names a valid one, for showing times.
    time_zone: Option<String>,
}

/// The transit itineraries (up to `MAX_ITINERARIES`) and the direct walk from a MOTIS plan.
fn transit_options(plan: &Value) -> TransitFound {
    let list = |key: &str| plan[key].as_array().cloned().unwrap_or_default();
    let mut transit: Vec<Value> = list("itineraries")
        .iter()
        .filter_map(itinerary)
        .filter(|o| {
            o["legs"]
                .as_array()
                .is_some_and(|l| l.iter().any(|l| l["mode"] != "walk"))
        })
        .collect();
    transit.sort_by_key(|o| o["duration_min"].as_i64().unwrap_or(i64::MAX));
    transit.truncate(MAX_ITINERARIES);
    let walk = list("direct")
        .iter()
        .filter_map(itinerary)
        .filter(|o| {
            o["legs"]
                .as_array()
                .is_some_and(|l| l.iter().all(|l| l["mode"] == "walk"))
        })
        .take(1)
        .map(|o| {
            let leg = &o["legs"][0];
            let mut walk = Map::new();
            walk.insert("mode".into(), json!("walk"));
            walk.insert("duration_min".into(), o["duration_min"].clone());
            walk.insert("depart".into(), o["depart"].clone());
            walk.insert("arrive".into(), o["arrive"].clone());
            if let Some(d) = leg.get("distance_m") {
                walk.insert("distance_m".into(), d.clone());
            }
            if let Some(p) = leg.get("path") {
                walk.insert("path".into(), p.clone());
            }
            Value::Object(walk)
        })
        .collect();
    let time_zone = plan["from"]["tz"]
        .as_str()
        .filter(|tz| tz.parse::<chrono_tz::Tz>().is_ok())
        .map(String::from);
    TransitFound {
        transit,
        walk,
        time_zone,
    }
}

/// One MOTIS itinerary as an option, or `None` if it has no usable legs.
fn itinerary(it: &Value) -> Option<Value> {
    let legs: Vec<Value> = it["legs"].as_array()?.iter().map(leg).collect();
    if legs.is_empty() {
        return None;
    }
    let duration = it["duration"].as_f64()?;
    let walk_min: i64 = legs
        .iter()
        .filter(|l| l["mode"] == "walk")
        .filter_map(|l| l["duration_min"].as_i64())
        .sum();
    let mut o = Map::new();
    o.insert("mode".into(), json!("transit"));
    o.insert("duration_min".into(), json!(minutes(duration)));
    o.insert("depart".into(), it["startTime"].clone());
    o.insert("arrive".into(), it["endTime"].clone());
    o.insert(
        "transfers".into(),
        json!(it["transfers"].as_i64().unwrap_or(0)),
    );
    o.insert("walk_min".into(), json!(walk_min));
    o.insert("legs".into(), Value::Array(legs));
    Some(Value::Object(o))
}

/// One leg: its mode, stops, times, the line it rides, and its path.
fn leg(l: &Value) -> Value {
    let mode = l["mode"].as_str().unwrap_or("other").to_ascii_lowercase();
    let mut out = Map::new();
    out.insert("mode".into(), json!(clip(&mode)));
    let name = |place: &Value| {
        place["name"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(clip)
    };
    if let Some(from) = name(&l["from"]) {
        out.insert("from".into(), json!(from));
    }
    if let Some(to) = name(&l["to"]) {
        out.insert("to".into(), json!(to));
    }
    for (key, src) in [("depart", "startTime"), ("arrive", "endTime")] {
        if let Some(t) = text(&l[src]) {
            out.insert(key.into(), json!(t));
        }
    }
    let duration = l["duration"].as_f64().unwrap_or(0.0);
    out.insert("duration_min".into(), json!(minutes(duration)));
    let route = text(&l["routeShortName"]).or_else(|| text(&l["displayName"]));
    for (key, value) in [
        ("route", route),
        ("headsign", text(&l["headsign"])),
        ("agency", text(&l["agencyName"])),
        ("color", l["routeColor"].as_str().and_then(hex_color)),
    ] {
        if let Some(v) = value {
            out.insert(key.into(), json!(v));
        }
    }
    if let Some(stops) = l["intermediateStops"].as_array() {
        out.insert("stops".into(), json!(stops.len() + 1));
    }
    if let Some(d) = l["distance"].as_f64() {
        out.insert("distance_m".into(), json!(d.round() as i64));
    }
    let geometry = &l["legGeometry"];
    if let Some(points) = geometry["points"].as_str() {
        // MOTIS's /api/v1 encodes with precision 7 unless it says otherwise.
        let precision = geometry["precision"].as_u64().unwrap_or(7).min(10) as u32;
        let path = simplify(&decode_polyline(points, precision));
        if path.len() >= 2 {
            out.insert("path".into(), json!(encode_polyline(&path)));
        }
    }
    Value::Object(out)
}

/// The drive and the rideshare built from an OSRM route.
fn drive_options(body: &Value) -> Result<Vec<Value>> {
    let route = &body["routes"][0];
    let (Some(distance), Some(duration)) = (route["distance"].as_f64(), route["duration"].as_f64())
    else {
        bail!("no driving route was found");
    };
    let km = (distance / 100.0).round() / 10.0;
    let drive_min = minutes(duration);
    let mut drive = json!({
        "mode": "drive",
        "duration_min": drive_min,
        "distance_km": km,
        "note": "Time without traffic or parking."
    });
    if let Some(points) = route["geometry"].as_str() {
        let path = simplify(&decode_polyline(points, 5));
        if path.len() >= 2 {
            drive["path"] = json!(encode_polyline(&path));
        }
    }
    let (low, high) = rideshare_fare(km, drive_min);
    let ride = json!({
        "mode": "rideshare",
        "duration_min": drive_min + PICKUP_MIN,
        "distance_km": km,
        "cost": { "low": low, "high": high, "currency": "USD" },
        "note": format!("Includes about {PICKUP_MIN} min waiting for pickup. The fare is an estimate at typical US rates; prices vary by city, service, and demand.")
    });
    Ok(vec![drive, ride])
}

/// A rough fare range in whole dollars for a ride of `km` taking `min` minutes.
fn rideshare_fare(km: f64, min: i64) -> (i64, i64) {
    let fare = (RIDE_BASE + RIDE_PER_KM * km + RIDE_PER_MIN * min as f64).max(RIDE_MIN_FARE);
    (fare.round() as i64, (fare * RIDE_SURGE).round() as i64)
}

/// What [`choose`] tells the model about the trip besides the options.
struct Trip<'a> {
    from: &'a Point,
    to: &'a Point,
    when: &'a When,
    has_car: bool,
    preferences: Option<&'a str>,
}

const CHOOSE_SYSTEM: &str = "You help someone decide how to make a trip. You get the options \
a trip planner found (public transit itineraries, a walk, a drive, and a rideshare), indexed from \
0, and what the traveller said matters to them. Pick the one option they should take.

Weigh, in this order unless the traveller says otherwise: getting there on time and in reasonable \
time; effort and hassle (transfers, long walks to and between stops, parking); and cost (walking \
is free and transit is cheap; driving costs fuel and parking; rideshare fares are given). Driving \
and rideshare times assume no traffic, so in a city at rush hour they are optimistic. A walk of \
up to about 20 minutes usually beats waiting for a vehicle. Whatever the traveller said matters \
(price, fewer transfers, luggage, accessibility, a deadline) outweighs these defaults. Never pick \
the drive when the traveller has no car.

Reply with only a JSON object, no other text: {\"option\": <index>, \"reason\": \"<one or two \
plain sentences to the traveller, citing the minutes, transfers, or cost that decided it and how \
it compares to the runner-up>\"}";

/// The model's pick among `options`, in the shape [`recommend`] returns. Fails if the model's
/// answer isn't a valid choice.
async fn choose(
    model: &ToolModel,
    request: &Trip<'_>,
    options: &[Value],
    unavailable: &[Value],
) -> Result<Value> {
    let brief: Vec<Value> = options
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let mut o = without_paths(o);
            o["index"] = json!(i);
            o
        })
        .collect();
    let mut trip = json!({
        "from": request.from.label,
        "to": request.to.label,
        "traveller_has_car": request.has_car,
        "options": brief,
    });
    match request.when {
        When::Now => trip["leaving"] = json!("now"),
        When::Depart(t) => trip["depart_at"] = json!(t),
        When::Arrive(t) => trip["arrive_by"] = json!(t),
    }
    if let Some(p) = request.preferences {
        trip["traveller_preferences"] = json!(p);
    }
    if !unavailable.is_empty() {
        trip["unavailable"] = json!(unavailable);
    }
    let messages = [json!({ "role": "user", "content": trip.to_string() })];
    let reply = model.ask(CHOOSE_SYSTEM, &messages).await?;
    parse_choice(&reply, options, request.has_car)
}

/// An option with its drawn paths (and its legs') left out: the model doesn't need them.
fn without_paths(option: &Value) -> Value {
    let mut o = option.clone();
    if let Some(map) = o.as_object_mut() {
        map.remove("path");
    }
    if let Some(legs) = o["legs"].as_array_mut() {
        for leg in legs.iter_mut().filter_map(Value::as_object_mut) {
            leg.remove("path");
        }
    }
    o
}

/// The recommendation in the model's `reply`: the JSON object in it, naming an option that exists
/// (and isn't the drive when the user has no car) with a reason.
fn parse_choice(reply: &str, options: &[Value], has_car: bool) -> Result<Value> {
    let (Some(start), Some(end)) = (reply.find('{'), reply.rfind('}')) else {
        bail!("no JSON object in the model's answer: {}", clip(reply));
    };
    let choice: Value = serde_json::from_str(reply.get(start..=end).unwrap_or_default())
        .with_context(|| format!("parsing the model's answer: {}", clip(reply)))?;
    let i = choice["option"]
        .as_u64()
        .map(|i| i as usize)
        .filter(|i| *i < options.len())
        .ok_or_else(|| anyhow!("the model chose no valid option: {choice}"))?;
    let mode = &options[i]["mode"];
    if mode == "drive" && !has_car {
        bail!("the model chose to drive, but the user has no car");
    }
    let reason: String = choice["reason"]
        .as_str()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .ok_or_else(|| anyhow!("the model gave no reason: {choice}"))?
        .chars()
        .take(MAX_NOTE_CHARS)
        .collect();
    Ok(json!({ "mode": mode, "option": i, "reason": reason }))
}

/// Which option to take by fixed rules, as its index in `options` and the reason. Used when the
/// model can't choose.
fn recommend(options: &[Value], has_car: bool) -> Value {
    let find = |mode: &str| {
        options
            .iter()
            .enumerate()
            .filter(|(_, o)| o["mode"] == mode)
            .min_by_key(|(_, o)| o["duration_min"].as_i64().unwrap_or(i64::MAX))
            .map(|(i, o)| (i, o["duration_min"].as_i64().unwrap_or(i64::MAX)))
    };
    let pick = |(i, _): (usize, i64), reason: String| json!({ "mode": options[i]["mode"], "option": i, "reason": reason });
    let (walk, transit, drive, ride) = (
        find("walk"),
        find("transit"),
        find("drive"),
        find("rideshare"),
    );

    if let Some(w @ (_, min)) = walk.filter(|(_, m)| *m <= SHORT_WALK_MIN) {
        return pick(w, format!("Walking takes only {min} min."));
    }
    let car = if has_car { drive } else { ride };
    let fare = |i: usize| {
        let c = &options[i]["cost"];
        format!("${}–{}", c["low"], c["high"])
    };
    if let Some(t @ (ti, tmin)) = transit {
        let transfers = options[ti]["transfers"].as_i64().unwrap_or(0);
        let competitive = match car {
            Some((_, cmin)) => {
                let cmin = cmin + if has_car { PARKING_MIN } else { 0 };
                tmin <= (cmin * 3 / 2).max(cmin + 15)
            }
            None => true,
        };
        if competitive && transfers <= 2 {
            let vs = match (car, has_car) {
                (Some((_, cmin)), true) => {
                    format!(", against about {cmin} min by car plus parking")
                }
                (Some((ci, cmin)), false) => {
                    format!(", against about {cmin} min and {} by rideshare", fare(ci))
                }
                (None, _) => String::new(),
            };
            let changes = match transfers {
                0 => "with no transfers".to_string(),
                1 => "with one transfer".to_string(),
                n => format!("with {n} transfers"),
            };
            return pick(
                t,
                format!("Public transit takes {tmin} min {changes}{vs}, and costs less."),
            );
        }
        if let Some(c @ (ci, cmin)) = car {
            let why = if competitive {
                format!("the transit route needs {transfers} transfers")
            } else {
                format!("transit takes {tmin} min")
            };
            return pick(
                c,
                if has_car {
                    format!("Driving takes about {cmin} min without traffic; {why}.")
                } else {
                    format!(
                        "A rideshare takes about {cmin} min and costs {}; {why}.",
                        fare(ci)
                    )
                },
            );
        }
    }
    if let Some(c @ (ci, cmin)) = car {
        return pick(
            c,
            if has_car {
                format!("No public transit route was found; driving takes about {cmin} min without traffic.")
            } else {
                format!("No public transit route was found; a rideshare takes about {cmin} min and costs {}.", fare(ci))
            },
        );
    }
    if let Some(t @ (_, min)) = transit {
        return pick(
            t,
            format!("Public transit is the only way found besides a long walk ({min} min)."),
        );
    }
    if let Some(w @ (_, min)) = walk {
        return pick(w, format!("Walking is the only option found ({min} min)."));
    }
    // Only reached with a drive the user can't take and nothing else: offer it anyway.
    pick((0, 0), "This is the only option found.".to_string())
}

/// The tool result.
#[allow(clippy::too_many_arguments)]
fn trip_json(
    title: Option<String>,
    when: &When,
    time_zone: Option<String>,
    from: &Point,
    to: &Point,
    options: Vec<Value>,
    unavailable: Vec<Value>,
    recommendation: Value,
) -> Value {
    let mut out = Map::new();
    if let Some(title) = title {
        out.insert("title".into(), json!(title));
    }
    out.insert("from".into(), from.json());
    out.insert("to".into(), to.json());
    match when {
        When::Now => {}
        When::Depart(t) => {
            out.insert("depart_at".into(), json!(t));
        }
        When::Arrive(t) => {
            out.insert("arrive_by".into(), json!(t));
        }
    }
    if let Some(tz) = time_zone {
        out.insert("time_zone".into(), json!(tz));
    }
    out.insert("recommendation".into(), recommendation);
    out.insert("options".into(), Value::Array(options));
    if !unavailable.is_empty() {
        out.insert("unavailable".into(), Value::Array(unavailable));
    }
    out.insert("attribution".into(), json!(ATTRIBUTION));
    Value::Object(out)
}

/// `#rrggbb` from a GTFS route color ("rrggbb", with or without `#`), or `None` if it isn't one.
fn hex_color(c: &str) -> Option<String> {
    let c = c.trim().trim_start_matches('#');
    (c.len() == 6 && c.chars().all(|ch| ch.is_ascii_hexdigit()))
        .then(|| format!("#{}", c.to_ascii_lowercase()))
}

/// Points from an encoded polyline (Google's format) with `precision` decimal digits. Decoding
/// stops at the first malformed value.
fn decode_polyline(s: &str, precision: u32) -> Vec<(f64, f64)> {
    let factor = 10f64.powi(precision as i32);
    let bytes = s.as_bytes();
    let mut i = 0;
    let next = |i: &mut usize| -> Option<i64> {
        let (mut result, mut shift) = (0i64, 0u32);
        loop {
            let b = (*bytes.get(*i)? as i64) - 63;
            if !(0..64).contains(&b) || shift > 60 {
                return None;
            }
            *i += 1;
            result |= (b & 0x1f) << shift;
            shift += 5;
            if b < 0x20 {
                break;
            }
        }
        Some(if result & 1 != 0 {
            !(result >> 1)
        } else {
            result >> 1
        })
    };
    let (mut lat, mut lon) = (0i64, 0i64);
    let mut points = Vec::new();
    while i < bytes.len() {
        let (Some(dlat), Some(dlon)) = (next(&mut i), next(&mut i)) else {
            break;
        };
        lat += dlat;
        lon += dlon;
        let p = (lat as f64 / factor, lon as f64 / factor);
        if p.0.abs() > 90.0 || p.1.abs() > 180.0 {
            break;
        }
        points.push(p);
    }
    points
}

/// `points` as an encoded polyline with precision 5.
fn encode_polyline(points: &[(f64, f64)]) -> String {
    let mut out = String::new();
    let mut put = |v: i64| {
        let mut v = if v < 0 { !(v << 1) } else { v << 1 };
        while v >= 0x20 {
            out.push((((v & 0x1f) | 0x20) as u8 + 63) as char);
            v >>= 5;
        }
        out.push((v as u8 + 63) as char);
    };
    let (mut plat, mut plon) = (0i64, 0i64);
    for &(lat, lon) in points {
        let (lat, lon) = ((lat * 1e5).round() as i64, (lon * 1e5).round() as i64);
        put(lat - plat);
        put(lon - plon);
        (plat, plon) = (lat, lon);
    }
    out
}

/// `points` simplified (Douglas–Peucker at `PATH_TOLERANCE`), then thinned evenly to at most
/// `MAX_PATH_POINTS`, always keeping both ends.
fn simplify(points: &[(f64, f64)]) -> Vec<(f64, f64)> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0, points.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let (mut far, mut dist) = (a, 0.0);
        for (i, p) in points.iter().enumerate().take(b).skip(a + 1) {
            let d = segment_distance(*p, points[a], points[b]);
            if d > dist {
                (far, dist) = (i, d);
            }
        }
        if dist > PATH_TOLERANCE {
            keep[far] = true;
            stack.push((a, far));
            stack.push((far, b));
        }
    }
    let kept: Vec<(f64, f64)> = points
        .iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(p, _)| *p)
        .collect();
    if kept.len() <= MAX_PATH_POINTS {
        return kept;
    }
    let step = (kept.len() - 1) as f64 / (MAX_PATH_POINTS - 1) as f64;
    (0..MAX_PATH_POINTS)
        .map(|i| kept[((i as f64 * step).round() as usize).min(kept.len() - 1)])
        .collect()
}

/// The distance in degrees from `p` to the segment `a`–`b`, treating degrees as flat; plenty for
/// deciding which points of a route to keep.
fn segment_distance(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = dx * dx + dy * dy;
    let t = if len == 0.0 {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len).clamp(0.0, 1.0)
    };
    let (x, y) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - x).powi(2) + (p.1 - y).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_are_names_or_points() {
        let input = json!({
            "from": " Alexanderplatz, Berlin ",
            "to": { "latitude": 52.5, "longitude": 13.4, "label": "Home" },
            "title": "  Commute ",
        });
        let r = parse_input(&input).unwrap();
        assert_eq!(r.title.as_deref(), Some("Commute"));
        assert_eq!(
            r.from,
            End::Name {
                name: "Alexanderplatz, Berlin",
                label: None
            }
        );
        assert_eq!(
            r.to,
            End::Point(Point {
                label: "Home".into(),
                latitude: 52.5,
                longitude: 13.4
            })
        );
        assert_eq!(r.when, When::Now);
        assert!(r.has_car);
        assert_eq!(r.preferences, None);

        let input = json!({
            "from": { "location": "Work", "label": "Office" },
            "to": { "location": "Zoo", "latitude": 1.0, "longitude": 2.0 },
            "arrive_by": "2026-10-18T09:00:00+02:00",
            "has_car": false,
            "preferences": "  cheapest please ",
        });
        let r = parse_input(&input).unwrap();
        assert_eq!(r.preferences.as_deref(), Some("cheapest please"));
        assert_eq!(
            r.from,
            End::Name {
                name: "Work",
                label: Some("Office".into())
            }
        );
        assert!(matches!(&r.to, End::Point(p) if p.label == "Zoo"));
        assert_eq!(r.when, When::Arrive("2026-10-18T09:00:00+02:00".into()));
        assert!(!r.has_car);
    }

    #[test]
    fn bad_input_is_rejected() {
        let err = |input: Value| format!("{:#}", parse_input(&input).unwrap_err());
        assert_eq!(
            err(json!({ "to": "B" })),
            "from: give a location, or latitude and longitude"
        );
        assert_eq!(
            err(json!({ "from": "A", "to": {} })),
            "to: give a location, or both latitude and longitude"
        );
        assert!(err(json!({ "from": "A", "to": { "latitude": 1.0 } })).contains("together"));
        assert!(
            err(json!({ "from": "A", "to": "B", "depart_at": "tomorrow 9am" }))
                .contains("depart_at must be ISO 8601")
        );
        assert!(
            err(json!({ "from": "A", "to": "B", "depart_at": "2026-10-18T08:30:00" }))
                .contains("UTC offset")
        );
        assert!(err(json!({
            "from": "A", "to": "B",
            "depart_at": "2026-10-18T08:30:00Z", "arrive_by": "2026-10-18T09:30:00Z"
        }))
        .contains("at most one"));
        assert!(err(json!({ "from": "A", "to": "B", "has_car": "yes" })).contains("has_car"));
    }

    #[test]
    fn polylines_round_trip() {
        // Google's documented example.
        let points = decode_polyline("_p~iF~ps|U_ulLnnqC_mqNvxq`@", 5);
        assert_eq!(
            points,
            [(38.5, -120.2), (40.7, -120.95), (43.252, -126.453)]
        );
        assert_eq!(encode_polyline(&points), "_p~iF~ps|U_ulLnnqC_mqNvxq`@");
        // Precision 7, as MOTIS sends it.
        let seven = decode_polyline(&encode7(&[(3.0, -7.0), (3.1234567, -7.5)]), 7);
        assert_eq!(seven.len(), 2);
        assert!(
            (seven[1].0 - 3.1234567).abs() < 1e-9 && (seven[1].1 + 7.5).abs() < 1e-9,
            "{seven:?}"
        );
        // Garbage stops decoding instead of panicking.
        assert!(decode_polyline("\u{1}\u{2}", 5).is_empty());
        assert!(decode_polyline("~~~~~~~~~~~~~~~~", 5).is_empty());
    }

    #[test]
    fn paths_are_simplified_and_capped() {
        let line: Vec<(f64, f64)> = (0..1000).map(|i| (i as f64 * 1e-4, 0.0)).collect();
        assert_eq!(simplify(&line), [(0.0, 0.0), (0.0999, 0.0)]);
        let zigzag: Vec<(f64, f64)> = (0..1000)
            .map(|i| (i as f64 * 1e-3, if i % 2 == 0 { 0.0 } else { 0.01 }))
            .collect();
        let s = simplify(&zigzag);
        assert_eq!(s.len(), MAX_PATH_POINTS);
        assert_eq!(s.first(), zigzag.first());
        assert_eq!(s.last(), zigzag.last());
    }

    #[test]
    fn colors_and_labels_are_cleaned() {
        assert_eq!(hex_color("FF8800").as_deref(), Some("#ff8800"));
        assert_eq!(hex_color("#00aa00").as_deref(), Some("#00aa00"));
        assert_eq!(hex_color("red"), None);
        assert_eq!(hex_color("12345"), None);
        let hit = json!({ "name": "Alexanderplatz", "areas": [
            { "name": "Mitte" }, { "name": "Berlin", "default": true }
        ] });
        assert_eq!(match_label(&hit), "Alexanderplatz, Berlin");
        assert_eq!(
            match_label(&json!({ "name": "Berlin", "areas": [{ "name": "Berlin" }] })),
            "Berlin"
        );
        assert_eq!(match_label(&json!({ "name": "X" })), "X");
    }

    #[test]
    fn fares_have_a_floor_and_a_range() {
        assert_eq!(rideshare_fare(1.0, 3), (8, 12));
        assert_eq!(rideshare_fare(10.0, 20), (19, 28));
    }

    fn option(mode: &str, min: i64, extra: Value) -> Value {
        let mut o = json!({ "mode": mode, "duration_min": min });
        o.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        o
    }

    fn ride(min: i64) -> Value {
        option(
            "rideshare",
            min,
            json!({ "cost": { "low": 15, "high": 22, "currency": "USD" } }),
        )
    }

    #[test]
    fn the_models_choice_is_checked() {
        let opts = [
            option("transit", 30, json!({ "transfers": 0, "path": "abc" })),
            option("drive", 20, json!({})),
            ride(25),
        ];
        let c = parse_choice(
            "Here you go:\n```json\n{\"option\": 0, \"reason\": \" The U5 is direct. \"}\n```",
            &opts,
            true,
        )
        .unwrap();
        assert_eq!(
            c,
            json!({ "mode": "transit", "option": 0, "reason": "The U5 is direct." })
        );
        assert_eq!(
            parse_choice(r#"{"option": 1, "reason": "Fastest."}"#, &opts, true).unwrap()["mode"],
            "drive"
        );
        let err = |reply: &str, has_car: bool| {
            format!("{:#}", parse_choice(reply, &opts, has_car).unwrap_err())
        };
        assert!(err("Take the train.", true).contains("no JSON object"));
        assert!(err(r#"{"option": 3, "reason": "x"}"#, true).contains("no valid option"));
        assert!(err(r#"{"option": "0", "reason": "x"}"#, true).contains("no valid option"));
        assert!(err(r#"{"option": 0}"#, true).contains("no reason"));
        assert!(err(r#"{"option": 1, "reason": "Fastest."}"#, false).contains("no car"));
        assert!(err("{not json}", true).contains("parsing"));
    }

    #[test]
    fn the_model_is_not_sent_paths() {
        let o = json!({ "mode": "transit", "path": "abc",
                        "legs": [{ "mode": "walk", "path": "def" }, { "mode": "bus", "route": "M41" }] });
        assert_eq!(
            without_paths(&o),
            json!({ "mode": "transit", "legs": [{ "mode": "walk" }, { "mode": "bus", "route": "M41" }] })
        );
    }

    #[test]
    fn a_short_walk_wins() {
        let opts = [
            option("transit", 12, json!({ "transfers": 0 })),
            option("walk", 18, json!({})),
            option("drive", 6, json!({})),
        ];
        let r = recommend(&opts, true);
        assert_eq!(r["mode"], "walk");
        assert_eq!(r["option"], 1);
        assert_eq!(r["reason"], "Walking takes only 18 min.");
    }

    #[test]
    fn transit_wins_when_it_is_close_to_driving() {
        let opts = [
            option("transit", 40, json!({ "transfers": 1 })),
            option("transit", 35, json!({ "transfers": 0 })),
            option("drive", 25, json!({})),
            ride(30),
        ];
        let r = recommend(&opts, true);
        assert_eq!(
            (r["mode"].as_str(), r["option"].as_u64()),
            (Some("transit"), Some(1))
        );
        assert!(
            r["reason"]
                .as_str()
                .unwrap()
                .contains("35 min with no transfers"),
            "{r}"
        );
        let r = recommend(&opts, false);
        assert_eq!(r["mode"], "transit");
        assert!(
            r["reason"]
                .as_str()
                .unwrap()
                .contains("$15–22 by rideshare"),
            "{r}"
        );
    }

    #[test]
    fn a_much_faster_car_wins() {
        let opts = [
            option("transit", 95, json!({ "transfers": 2 })),
            option("drive", 30, json!({})),
            ride(35),
        ];
        let r = recommend(&opts, true);
        assert_eq!(r["mode"], "drive");
        assert_eq!(
            r["reason"],
            "Driving takes about 30 min without traffic; transit takes 95 min."
        );
        let r = recommend(&opts, false);
        assert_eq!(r["mode"], "rideshare");
        assert!(
            r["reason"].as_str().unwrap().contains("costs $15–22"),
            "{r}"
        );
    }

    #[test]
    fn many_transfers_tip_it_to_the_car() {
        let opts = [
            option("transit", 30, json!({ "transfers": 3 })),
            option("drive", 25, json!({})),
        ];
        let r = recommend(&opts, true);
        assert_eq!(r["mode"], "drive");
        assert!(
            r["reason"].as_str().unwrap().contains("needs 3 transfers"),
            "{r}"
        );
    }

    #[test]
    fn without_transit_the_car_or_the_walk_is_left() {
        let opts = [option("drive", 25, json!({})), ride(30)];
        assert!(recommend(&opts, true)["reason"]
            .as_str()
            .unwrap()
            .starts_with("No public transit"));
        let opts = [option("walk", 45, json!({}))];
        assert_eq!(recommend(&opts, true)["mode"], "walk");
        let opts = [
            option("transit", 50, json!({ "transfers": 3 })),
            option("walk", 90, json!({})),
        ];
        assert_eq!(recommend(&opts, true)["mode"], "transit");
    }

    #[test]
    fn motis_itineraries_become_options() {
        let plan = json!({
            "from": { "name": "START", "tz": "Europe/Berlin" },
            "itineraries": [
                { "duration": 1500, "startTime": "2026-10-17T08:00:00Z", "endTime": "2026-10-17T08:25:00Z", "transfers": 0,
                  "legs": [
                    { "mode": "WALK", "from": { "name": "START" }, "to": { "name": "Hauptbahnhof" }, "duration": 240,
                      "startTime": "2026-10-17T08:00:00Z", "endTime": "2026-10-17T08:04:00Z", "distance": 310.4,
                      "legGeometry": { "points": encode7(&[(52.525, 13.369)]), "precision": 7, "length": 1 } },
                    { "mode": "SUBWAY", "from": { "name": "Hauptbahnhof" }, "to": { "name": "Alexanderplatz" }, "duration": 1260,
                      "startTime": "2026-10-17T08:04:00Z", "endTime": "2026-10-17T08:25:00Z",
                      "routeShortName": "U5", "headsign": "Hönow", "agencyName": "BVG", "routeColor": "7E5330",
                      "intermediateStops": [{}, {}, {}],
                      "legGeometry": { "points": encode7(&[(52.525, 13.369), (52.521, 13.413)]), "precision": 7, "length": 2 } }
                  ] },
                { "duration": 600, "legs": [{ "mode": "WALK", "duration": 600 }], "transfers": 0 }
            ],
            "direct": [
                { "duration": 3000, "startTime": "2026-10-17T08:00:00Z", "endTime": "2026-10-17T08:50:00Z",
                  "legs": [{ "mode": "WALK", "duration": 3000, "distance": 3900.0,
                             "legGeometry": { "points": encode7(&[(52.525, 13.369), (52.521, 13.413)]), "precision": 7 } }] }
            ]
        });
        let found = transit_options(&plan);
        assert_eq!(
            found.transit.len(),
            1,
            "a walk-only itinerary is not transit"
        );
        let t = &found.transit[0];
        assert_eq!(t["duration_min"], 25);
        assert_eq!(t["walk_min"], 4);
        assert_eq!(t["transfers"], 0);
        let subway = &t["legs"][1];
        assert_eq!(subway["mode"], "subway");
        assert_eq!(subway["route"], "U5");
        assert_eq!(subway["color"], "#7e5330");
        assert_eq!(subway["stops"], 4);
        assert_eq!(subway["headsign"], "Hönow");
        assert_eq!(
            decode_polyline(subway["path"].as_str().unwrap(), 5),
            [(52.525, 13.369), (52.521, 13.413)]
        );
        assert!(
            t["legs"][0].get("path").is_none(),
            "a one-point path is dropped"
        );
        assert_eq!(t["legs"][0]["distance_m"], 310);
        assert_eq!(found.time_zone.as_deref(), Some("Europe/Berlin"));
        assert_eq!(
            transit_options(&json!({ "from": { "tz": "Mars/Olympus" } })).time_zone,
            None
        );
        assert_eq!(found.walk.len(), 1);
        assert_eq!(found.walk[0]["mode"], "walk");
        assert_eq!(found.walk[0]["duration_min"], 50);
        assert_eq!(found.walk[0]["distance_m"], 3900);
        assert!(found.walk[0]["path"].is_string());
    }

    fn encode7(points: &[(f64, f64)]) -> String {
        // Precision 7 is precision 5 with every coordinate scaled by 100.
        let scaled: Vec<(f64, f64)> = points.iter().map(|(a, b)| (a * 100.0, b * 100.0)).collect();
        encode_polyline(&scaled)
    }

    #[test]
    fn an_osrm_route_becomes_a_drive_and_a_ride() {
        let body = json!({ "code": "Ok", "routes": [{ "distance": 12345.0, "duration": 1190.0,
                                                       "geometry": "_p~iF~ps|U_ulLnnqC_mqNvxq`@" }] });
        let opts = drive_options(&body).unwrap();
        assert_eq!(opts[0]["mode"], "drive");
        assert_eq!(opts[0]["duration_min"], 20);
        assert_eq!(opts[0]["distance_km"], 12.3);
        assert_eq!(opts[0]["path"], "_p~iF~ps|U_ulLnnqC_mqNvxq`@");
        assert_eq!(opts[1]["mode"], "rideshare");
        assert_eq!(opts[1]["duration_min"], 25);
        assert_eq!(
            opts[1]["cost"],
            json!({ "low": 21, "high": 31, "currency": "USD" })
        );
        assert!(drive_options(&json!({ "code": "Ok", "routes": [] })).is_err());
    }
}
