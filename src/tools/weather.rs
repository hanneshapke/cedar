use super::{truncate_output, Tool, ToolContext};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use chrono::NaiveDate;
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::time::Duration;

const DEFAULT_DAYS: u64 = 3;
const MAX_DAYS: u64 = 16;
/// Open-Meteo answers 429 when a client is over its free-tier limit; one short pause is enough for
/// the per-minute window to move on.
const RATE_LIMIT_PAUSE: Duration = Duration::from_secs(1);
const ATTRIBUTION: &str = "Weather data by Open-Meteo.com (CC BY 4.0)";
const CURRENT_VARIABLES: &str = "temperature_2m,apparent_temperature,relative_humidity_2m,\
    precipitation,weather_code,wind_speed_10m,wind_direction_10m,wind_gusts_10m";
const DAILY_VARIABLES: &str = "weather_code,temperature_2m_max,temperature_2m_min,\
    precipitation_sum,precipitation_probability_max,wind_speed_10m_max,wind_gusts_10m_max,\
    uv_index_max,sunrise,sunset";

/// Weather forecasts from [Open-Meteo](https://open-meteo.com), an open-source weather API with
/// open data and no API key.
///
/// Like `web_search` this runs in the cedar process, because the sandbox has no network. The model
/// only supplies a place name or coordinates; the hosts are fixed by configuration, so there is no
/// URL for it to point somewhere else.
pub struct WeatherForecast {
    http: reqwest::Client,
    forecast_url: String,
    geocoding_url: String,
}

impl WeatherForecast {
    /// `forecast_url` and `geocoding_url` are the full endpoints (see `DEFAULT_WEATHER_BASE_URL`
    /// and `DEFAULT_WEATHER_GEOCODING_BASE_URL`).
    pub fn new(forecast_url: String, geocoding_url: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            http,
            forecast_url,
            geocoding_url,
        })
    }

    async fn send(
        &self,
        url: &str,
        params: &[(&str, String)],
        timeout: Duration,
    ) -> Result<reqwest::Response> {
        self.http
            .get(url)
            .header("Accept", "application/json")
            .query(params)
            .timeout(timeout)
            .send()
            .await
            .context("sending Open-Meteo request")
    }

    /// GET `url` and parse the JSON body, retrying once if rate limited.
    async fn get_json(
        &self,
        url: &str,
        params: &[(&str, String)],
        timeout: Duration,
    ) -> Result<Value> {
        let mut resp = self.send(url, params, timeout).await?;
        if resp.status() == StatusCode::TOO_MANY_REQUESTS {
            tokio::time::sleep(RATE_LIMIT_PAUSE).await;
            resp = self.send(url, params, timeout).await?;
        }
        let status = resp.status();
        let text = resp.text().await.context("reading Open-Meteo response")?;
        match status {
            s if s.is_success() => {}
            StatusCode::TOO_MANY_REQUESTS => {
                bail!("Open-Meteo is rate limited; wait a moment before asking again")
            }
            _ => bail!("Open-Meteo error {status}: {}", error_message(&text)),
        }
        serde_json::from_str(&text).context("parsing Open-Meteo response")
    }

    /// The best match for a place name. Open-Meteo answers 200 without `results` when nothing matches.
    async fn geocode(&self, name: &str, timeout: Duration) -> Result<Place> {
        let params = [
            ("name", name.to_string()),
            ("count", "1".to_string()),
            ("language", "en".to_string()),
            ("format", "json".to_string()),
        ];
        let body = self.get_json(&self.geocoding_url, &params, timeout).await?;
        let hit = &body["results"][0];
        match (hit["latitude"].as_f64(), hit["longitude"].as_f64()) {
            (Some(latitude), Some(longitude)) => Ok(Place {
                label: Some(place_label(hit)),
                latitude,
                longitude,
            }),
            _ => bail!(
                "no place found for {name:?}; try a larger nearby city or pass latitude and longitude"
            ),
        }
    }
}

#[async_trait]
impl Tool for WeatherForecast {
    fn name(&self) -> &str {
        "weather_forecast"
    }

    fn description(&self) -> &str {
        "Get the current weather and a daily forecast (up to 16 days) for a place, from Open-Meteo. \
         Pass a place name such as \"Berlin\" or \"Paris, France\", or latitude and longitude. \
         Returns temperatures, conditions, precipitation, wind, UV index, and sunrise and sunset. \
         Use it for any weather question instead of searching the web."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": {
                    "location": { "type": "string",
                                  "description": "A place name or postal code, e.g. \"Berlin\", \"Paris, France\", \"Portland, Oregon\"." },
                    "latitude": { "type": "number", "minimum": -90, "maximum": 90,
                                  "description": "Latitude in degrees; use with longitude instead of a place name." },
                    "longitude": { "type": "number", "minimum": -180, "maximum": 180,
                                   "description": "Longitude in degrees; use with latitude instead of a place name." },
                    "days": { "type": "integer", "minimum": 1, "maximum": MAX_DAYS,
                              "description": "How many days to forecast, starting today (default 3)." },
                    "units": { "type": "string", "enum": ["metric", "imperial"],
                               "description": "metric (°C, km/h, mm; the default) or imperial (°F, mph, inch)." }
                } })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let request = parse_input(input)?;

        let place = match request.target {
            Target::Coordinates {
                latitude,
                longitude,
                label,
            } => Place {
                label: label.map(String::from),
                latitude,
                longitude,
            },
            Target::Name(name) => self.geocode(name, ctx.timeout).await?,
        };

        let mut params = vec![
            ("latitude", place.latitude.to_string()),
            ("longitude", place.longitude.to_string()),
            ("current", CURRENT_VARIABLES.to_string()),
            ("daily", DAILY_VARIABLES.to_string()),
            ("timezone", "auto".to_string()),
            ("forecast_days", request.days.to_string()),
        ];
        if request.imperial {
            params.push(("temperature_unit", "fahrenheit".to_string()));
            params.push(("wind_speed_unit", "mph".to_string()));
            params.push(("precipitation_unit", "inch".to_string()));
        }
        let body = self
            .get_json(&self.forecast_url, &params, ctx.timeout)
            .await?;
        Ok(truncate_output(
            &format_forecast(&place, &body),
            ctx.max_output,
        ))
    }
}

/// Where to forecast: a name to geocode, or coordinates (optionally with the name to show for them).
#[derive(Debug, PartialEq)]
enum Target<'a> {
    Name(&'a str),
    Coordinates {
        latitude: f64,
        longitude: f64,
        label: Option<&'a str>,
    },
}

#[derive(Debug)]
struct Request<'a> {
    target: Target<'a>,
    days: u64,
    imperial: bool,
}

/// A resolved location.
#[derive(Debug)]
struct Place {
    label: Option<String>,
    latitude: f64,
    longitude: f64,
}

fn parse_input(input: &Value) -> Result<Request<'_>> {
    let name = input["location"]
        .as_str()
        .map(str::trim)
        .filter(|l| !l.is_empty());
    let coordinate = |key: &str, limit: f64| -> Result<Option<f64>> {
        match &input[key] {
            Value::Null => Ok(None),
            v => match v.as_f64() {
                Some(x) if x.abs() <= limit => Ok(Some(x)),
                Some(x) => bail!("{key} must be between -{limit} and {limit}, got {x}"),
                None => bail!("{key} must be a number"),
            },
        }
    };
    let target = match (
        coordinate("latitude", 90.0)?,
        coordinate("longitude", 180.0)?,
    ) {
        (Some(latitude), Some(longitude)) => Target::Coordinates {
            latitude,
            longitude,
            label: name,
        },
        (None, None) => Target::Name(
            name.ok_or_else(|| anyhow!("give a location, or both latitude and longitude"))?,
        ),
        _ => bail!("latitude and longitude must be given together"),
    };
    let days = input["days"]
        .as_u64()
        .unwrap_or(DEFAULT_DAYS)
        .clamp(1, MAX_DAYS);
    let imperial = match input["units"].as_str() {
        None | Some("metric") => false,
        Some("imperial") => true,
        Some(u) => bail!("invalid units {u:?}; use metric or imperial"),
    };
    Ok(Request {
        target,
        days,
        imperial,
    })
}

/// "Berlin, State of Berlin, Germany" from a geocoding hit, without repeating a name.
fn place_label(hit: &Value) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for key in ["name", "admin1", "country"] {
        if let Some(s) = hit[key].as_str().filter(|s| !s.is_empty()) {
            if parts.last() != Some(&s) {
                parts.push(s);
            }
        }
    }
    parts.join(", ")
}

/// The report the model sees: a header, the current conditions, one line per day, and the
/// attribution Open-Meteo's license asks for. Fields the API left out are skipped.
fn format_forecast(place: &Place, body: &Value) -> String {
    let coordinates = format!("{:.2}, {:.2}", place.latitude, place.longitude);
    let mut header = match &place.label {
        Some(label) => format!("{label} ({coordinates})"),
        None => coordinates,
    };
    if let Some(zone) = body["timezone"].as_str() {
        header.push_str(&format!(", timezone {zone}"));
    }

    let mut sections = vec![header];
    sections.extend(current_line(body));
    let days = day_lines(body);
    if !days.is_empty() {
        sections.push(days.join("\n"));
    }
    sections.push(ATTRIBUTION.to_string());
    sections.join("\n\n")
}

fn current_line(body: &Value) -> Option<String> {
    let current = &body["current"];
    let units = &body["current_units"];
    let amount = |key: &str| {
        current[key]
            .as_f64()
            .map(|v| quantity(v, units[key].as_str().unwrap_or_default()))
    };

    let mut parts = Vec::new();
    if let Some(code) = current["weather_code"].as_f64() {
        parts.push(describe(code as i64));
    }
    if let Some(temperature) = amount("temperature_2m") {
        parts.push(match amount("apparent_temperature") {
            Some(feels) => format!("{temperature} (feels {feels})"),
            None => temperature,
        });
    }
    if let Some(humidity) = amount("relative_humidity_2m") {
        parts.push(format!("humidity {humidity}"));
    }
    if let Some(speed) = amount("wind_speed_10m") {
        let from = current["wind_direction_10m"]
            .as_f64()
            .map(|d| format!(" from {}", compass(d)))
            .unwrap_or_default();
        parts.push(format!("wind {speed}{from}"));
    }
    if let Some(gusts) = amount("wind_gusts_10m") {
        parts.push(format!("gusts {gusts}"));
    }
    if let Some(precipitation) = amount("precipitation") {
        parts.push(format!("precipitation {precipitation}"));
    }
    if parts.is_empty() {
        return None;
    }
    let when = match current["time"].as_str() {
        Some(t) => format!("Now ({})", t.replace('T', " ")),
        None => "Now".to_string(),
    };
    Some(format!("{when}: {}", parts.join(", ")))
}

/// One line per day. Open-Meteo returns each daily variable as a column, indexed like `daily.time`.
fn day_lines(body: &Value) -> Vec<String> {
    let daily = &body["daily"];
    let units = &body["daily_units"];
    let dates = daily["time"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    dates
        .iter()
        .enumerate()
        .filter_map(|(i, date)| Some((i, date.as_str()?)))
        .map(|(i, date)| {
            let amount = |key: &str| {
                daily[key][i]
                    .as_f64()
                    .map(|v| quantity(v, units[key].as_str().unwrap_or_default()))
            };
            let clock = |key: &str| daily[key][i].as_str().and_then(|t| t.split('T').nth(1));

            let mut parts = Vec::new();
            if let Some(code) = daily["weather_code"][i].as_f64() {
                parts.push(describe(code as i64));
            }
            if let Some(low) = amount("temperature_2m_min") {
                parts.push(format!("low {low}"));
            }
            if let Some(high) = amount("temperature_2m_max") {
                parts.push(format!("high {high}"));
            }
            if let Some(sum) = amount("precipitation_sum") {
                parts.push(match amount("precipitation_probability_max") {
                    Some(chance) => format!("precipitation {sum} ({chance} chance)"),
                    None => format!("precipitation {sum}"),
                });
            }
            if let Some(wind) = amount("wind_speed_10m_max") {
                parts.push(match amount("wind_gusts_10m_max") {
                    Some(gusts) => format!("wind up to {wind} (gusts {gusts})"),
                    None => format!("wind up to {wind}"),
                });
            }
            if let Some(uv) = amount("uv_index_max") {
                parts.push(format!("UV {uv}"));
            }
            if let Some(rise) = clock("sunrise") {
                parts.push(format!("sunrise {rise}"));
            }
            if let Some(set) = clock("sunset") {
                parts.push(format!("sunset {set}"));
            }
            let details = if parts.is_empty() {
                "no data".to_string()
            } else {
                parts.join(", ")
            };
            format!("{}: {details}", weekday_and_date(date))
        })
        .collect()
}

/// "Mon 2026-10-12" for "2026-10-12"; anything that isn't a date is passed through.
fn weekday_and_date(date: &str) -> String {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map(|d| d.format("%a %Y-%m-%d").to_string())
        .unwrap_or_else(|_| date.to_string())
}

/// A number with its unit: no space before `°C`, `°F`, and `%`, a space before words, nothing for
/// unitless values (Open-Meteo gives the UV index an empty unit).
fn quantity(value: f64, unit: &str) -> String {
    // Adding zero turns -0.0 into 0.0, so a rounded-down "-0.0" prints as 0.
    let value = value + 0.0;
    // For `wind_speed_unit=mph` Open-Meteo labels the values "mp/h".
    let unit = if unit == "mp/h" { "mph" } else { unit };
    match unit {
        "" => value.to_string(),
        u if u.starts_with('°') || u == "%" => format!("{value}{u}"),
        u => format!("{value} {u}"),
    }
}

/// The WMO weather interpretation code Open-Meteo reports, in words.
fn describe(code: i64) -> String {
    match code {
        0 => "clear sky",
        1 => "mainly clear",
        2 => "partly cloudy",
        3 => "overcast",
        45 => "fog",
        48 => "freezing fog",
        51 => "light drizzle",
        53 => "moderate drizzle",
        55 => "dense drizzle",
        56 => "light freezing drizzle",
        57 => "dense freezing drizzle",
        61 => "slight rain",
        63 => "moderate rain",
        65 => "heavy rain",
        66 => "light freezing rain",
        67 => "heavy freezing rain",
        71 => "slight snow",
        73 => "moderate snow",
        75 => "heavy snow",
        77 => "snow grains",
        80 => "slight rain showers",
        81 => "moderate rain showers",
        82 => "violent rain showers",
        85 => "slight snow showers",
        86 => "heavy snow showers",
        95 => "thunderstorm",
        96 => "thunderstorm with slight hail",
        99 => "thunderstorm with heavy hail",
        _ => return format!("weather code {code}"),
    }
    .to_string()
}

/// The 16-point compass name for a bearing in degrees.
fn compass(degrees: f64) -> &'static str {
    const POINTS: [&str; 16] = [
        "N", "NNE", "NE", "ENE", "E", "ESE", "SE", "SSE", "S", "SSW", "SW", "WSW", "W", "WNW",
        "NW", "NNW",
    ];
    POINTS[(degrees.rem_euclid(360.0) / 22.5).round() as usize % 16]
}

/// The `reason` from Open-Meteo's `{"error": true, "reason": ...}` body, or a prefix of the raw body.
fn error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["reason"].as_str().map(String::from))
        .unwrap_or_else(|| truncate_output(body.trim(), 300))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn berlin() -> Place {
        Place {
            label: Some("Berlin, State of Berlin, Germany".to_string()),
            latitude: 52.52437,
            longitude: 13.41053,
        }
    }

    /// The shape Open-Meteo returned for Berlin on 2026-10-12, cut to two days.
    fn response() -> Value {
        json!({
            "timezone": "Europe/Berlin",
            "current_units": { "time": "iso8601", "temperature_2m": "°C", "apparent_temperature": "°C",
                               "relative_humidity_2m": "%", "precipitation": "mm", "weather_code": "wmo code",
                               "wind_speed_10m": "km/h", "wind_direction_10m": "°", "wind_gusts_10m": "km/h" },
            "current": { "time": "2026-10-12T21:00", "temperature_2m": 10.2, "apparent_temperature": 8.3,
                         "relative_humidity_2m": 70, "precipitation": 0.0, "weather_code": 0,
                         "wind_speed_10m": 4.6, "wind_direction_10m": 315, "wind_gusts_10m": 9.4 },
            "daily_units": { "time": "iso8601", "weather_code": "wmo code", "temperature_2m_max": "°C",
                             "temperature_2m_min": "°C", "precipitation_sum": "mm",
                             "precipitation_probability_max": "%", "wind_speed_10m_max": "km/h",
                             "wind_gusts_10m_max": "km/h", "uv_index_max": "", "sunrise": "iso8601",
                             "sunset": "iso8601" },
            "daily": { "time": ["2026-10-12", "2026-10-13"], "weather_code": [3, 61],
                       "temperature_2m_max": [13.7, 15.1], "temperature_2m_min": [6.8, 5.1],
                       "precipitation_sum": [0.0, 0.2], "precipitation_probability_max": [32, 19],
                       "wind_speed_10m_max": [14.1, 13.4], "wind_gusts_10m_max": [31.0, 29.5],
                       "uv_index_max": [2.9, 2.8],
                       "sunrise": ["2026-10-12T07:26", "2026-10-13T07:28"],
                       "sunset": ["2026-10-12T18:18", "2026-10-13T18:16"] }
        })
    }

    #[test]
    fn forecast_lists_place_current_conditions_and_each_day() {
        assert_eq!(
            format_forecast(&berlin(), &response()),
            "Berlin, State of Berlin, Germany (52.52, 13.41), timezone Europe/Berlin\n\n\
             Now (2026-10-12 21:00): clear sky, 10.2°C (feels 8.3°C), humidity 70%, \
             wind 4.6 km/h from NW, gusts 9.4 km/h, precipitation 0 mm\n\n\
             Mon 2026-10-12: overcast, low 6.8°C, high 13.7°C, precipitation 0 mm (32% chance), \
             wind up to 14.1 km/h (gusts 31 km/h), UV 2.9, sunrise 07:26, sunset 18:18\n\
             Tue 2026-10-13: slight rain, low 5.1°C, high 15.1°C, precipitation 0.2 mm (19% chance), \
             wind up to 13.4 km/h (gusts 29.5 km/h), UV 2.8, sunrise 07:28, sunset 18:16\n\n\
             Weather data by Open-Meteo.com (CC BY 4.0)"
        );
    }

    #[test]
    fn units_come_from_the_response() {
        let mut body = response();
        body["current_units"]["temperature_2m"] = json!("°F");
        body["current_units"]["wind_speed_10m"] = json!("mph");
        body["current"]["temperature_2m"] = json!(50.4);
        body["current"]["wind_speed_10m"] = json!(2.9);
        let report = format_forecast(&berlin(), &body);
        assert!(report.contains("50.4°F (feels 8.3°C)"), "{report}");
        assert!(report.contains("wind 2.9 mph from NW"), "{report}");
    }

    #[test]
    fn missing_and_null_fields_are_skipped() {
        let body = json!({
            "current": { "time": "2026-10-12T21:00", "temperature_2m": 10.2, "wind_speed_10m": null },
            "current_units": { "temperature_2m": "°C" },
            "daily": { "time": ["2026-10-12", "2026-10-13"], "weather_code": [null, 2],
                       "temperature_2m_max": [null, 15.1] },
            "daily_units": { "temperature_2m_max": "°C" }
        });
        let place = Place {
            label: None,
            latitude: -33.8688,
            longitude: 151.2093,
        };
        assert_eq!(
            format_forecast(&place, &body),
            "-33.87, 151.21\n\n\
             Now (2026-10-12 21:00): 10.2°C\n\n\
             Mon 2026-10-12: no data\n\
             Tue 2026-10-13: partly cloudy, high 15.1°C\n\n\
             Weather data by Open-Meteo.com (CC BY 4.0)"
        );
    }

    #[test]
    fn a_response_without_data_still_names_the_place_and_source() {
        let report = format_forecast(&berlin(), &json!({}));
        assert_eq!(
            report,
            "Berlin, State of Berlin, Germany (52.52, 13.41)\n\nWeather data by Open-Meteo.com (CC BY 4.0)"
        );
    }

    #[test]
    fn negative_zero_prints_as_zero() {
        assert_eq!(quantity(-0.0, "°C"), "0°C");
        assert_eq!(quantity(-3.5, "°C"), "-3.5°C");
        assert_eq!(quantity(2.0, "mm"), "2 mm");
        assert_eq!(quantity(2.9, ""), "2.9");
        assert_eq!(quantity(40.0, "%"), "40%");
        assert_eq!(quantity(4.9, "mp/h"), "4.9 mph");
    }

    #[test]
    fn weather_codes_are_described() {
        assert_eq!(describe(0), "clear sky");
        assert_eq!(describe(63), "moderate rain");
        assert_eq!(describe(99), "thunderstorm with heavy hail");
        assert_eq!(describe(4), "weather code 4");
    }

    #[test]
    fn bearings_map_to_compass_points() {
        assert_eq!(compass(0.0), "N");
        assert_eq!(compass(90.0), "E");
        assert_eq!(compass(225.0), "SW");
        assert_eq!(compass(315.0), "NW");
        assert_eq!(compass(349.0), "N");
        assert_eq!(compass(360.0), "N");
        assert_eq!(compass(-90.0), "W");
    }

    #[test]
    fn place_labels_skip_repeats_and_blanks() {
        let hit = json!({ "name": "Berlin", "admin1": "State of Berlin", "country": "Germany" });
        assert_eq!(place_label(&hit), "Berlin, State of Berlin, Germany");
        let hit = json!({ "name": "Singapore", "admin1": "Singapore", "country": "Singapore" });
        assert_eq!(place_label(&hit), "Singapore");
        let hit = json!({ "name": "Nowhere", "country": "" });
        assert_eq!(place_label(&hit), "Nowhere");
    }

    #[test]
    fn dates_get_a_weekday() {
        assert_eq!(weekday_and_date("2026-10-12"), "Mon 2026-10-12");
        assert_eq!(weekday_and_date("soon"), "soon");
    }

    #[test]
    fn a_place_name_is_geocoded_with_defaults() {
        let input = json!({ "location": "  Berlin  " });
        let r = parse_input(&input).unwrap();
        assert_eq!(r.target, Target::Name("Berlin"));
        assert_eq!((r.days, r.imperial), (3, false));
    }

    #[test]
    fn coordinates_skip_geocoding_and_keep_the_name_as_a_label() {
        let input = json!({ "latitude": 52.5, "longitude": 13.4 });
        assert_eq!(
            parse_input(&input).unwrap().target,
            Target::Coordinates {
                latitude: 52.5,
                longitude: 13.4,
                label: None
            }
        );
        let input = json!({ "location": "Home", "latitude": 52.5, "longitude": 13.4 });
        assert_eq!(
            parse_input(&input).unwrap().target,
            Target::Coordinates {
                latitude: 52.5,
                longitude: 13.4,
                label: Some("Home")
            }
        );
    }

    #[test]
    fn days_are_clamped_and_units_chosen() {
        let input = json!({ "location": "x", "days": 99, "units": "imperial" });
        let r = parse_input(&input).unwrap();
        assert_eq!((r.days, r.imperial), (16, true));
        let input = json!({ "location": "x", "days": 0, "units": "metric" });
        let r = parse_input(&input).unwrap();
        assert_eq!((r.days, r.imperial), (1, false));
    }

    #[test]
    fn bad_input_is_rejected() {
        let err = |input: Value| parse_input(&input).unwrap_err().to_string();
        assert!(err(json!({})).contains("give a location"));
        assert!(err(json!({ "location": "   " })).contains("give a location"));
        assert!(err(json!({ "location": 7 })).contains("give a location"));
        assert!(err(json!({ "latitude": 1.0 })).contains("together"));
        assert!(err(json!({ "location": "x", "longitude": 1.0 })).contains("together"));
        assert!(err(json!({ "latitude": 91.0, "longitude": 0 })).contains("latitude"));
        assert!(err(json!({ "latitude": 0, "longitude": -180.5 })).contains("longitude"));
        assert!(err(json!({ "latitude": "north", "longitude": 0 })).contains("must be a number"));
        assert!(err(json!({ "location": "x", "units": "kelvin" })).contains("units"));
    }

    #[test]
    fn error_reason_is_extracted() {
        let body =
            r#"{"error":true,"reason":"Latitude must be in range of -90 to 90°. Given: 999.0."}"#;
        assert_eq!(
            error_message(body),
            "Latitude must be in range of -90 to 90°. Given: 999.0."
        );
        assert_eq!(error_message("  upstream exploded "), "upstream exploded");
    }
}
