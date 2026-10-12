use super::open_meteo::{coordinates, OpenMeteo};
use super::{Tool, ToolContext};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Map, Value};

const MAX_MARKERS: usize = 25;
const MAX_TEXT_CHARS: usize = 200;

/// Shows the user a map with pins, drawn by the chat front end from this tool's result.
///
/// The tool itself only resolves place names to coordinates, through the same Open-Meteo geocoding
/// API `weather_forecast` uses, so it runs in the cedar process. Its result is a JSON object the
/// chat page turns into a map: `{"title"?, "markers": [{"label", "latitude", "longitude"}],
/// "not_found"?: [names]}`.
pub struct ShowMap {
    api: OpenMeteo,
    geocoding_url: String,
}

impl ShowMap {
    /// `geocoding_url` is the full endpoint (see `DEFAULT_WEATHER_GEOCODING_BASE_URL`).
    pub fn new(geocoding_url: String) -> Result<Self> {
        Ok(Self {
            api: OpenMeteo::new()?,
            geocoding_url,
        })
    }
}

#[async_trait]
impl Tool for ShowMap {
    fn name(&self) -> &str {
        "show_map"
    }

    fn description(&self) -> &str {
        "Show the user a map with a pin for each of one or more places; the map appears in the chat. \
         Give each pin a place name (a city, town, or region, e.g. \"Berlin\" or \"Paris, France\"; \
         street addresses and businesses are not found) or exact latitude and longitude, with an \
         optional label. Returns the coordinates the pins use, and lists any names that were not \
         found. Use it when seeing where places are would help the user."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "A short caption shown with the map." },
                    "markers": {
                        "type": "array", "minItems": 1, "maxItems": MAX_MARKERS,
                        "description": "The places to pin on the map.",
                        "items": { "type": "object",
                                   "properties": {
                                       "location": { "type": "string",
                                                     "description": "A place name or postal code, e.g. \"Berlin\", \"Paris, France\". Use this or latitude and longitude." },
                                       "latitude": { "type": "number", "minimum": -90, "maximum": 90,
                                                     "description": "Latitude in degrees; use with longitude instead of a place name." },
                                       "longitude": { "type": "number", "minimum": -180, "maximum": 180,
                                                      "description": "Longitude in degrees; use with latitude instead of a place name." },
                                       "label": { "type": "string",
                                                  "description": "Text for the pin. Defaults to the place name." }
                                   } }
                    }
                },
                "required": ["markers"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let request = parse_input(input)?;

        let mut markers = Vec::new();
        let mut not_found = Vec::new();
        for marker in request.markers {
            let (latitude, longitude, found_label) = match marker.spot {
                Spot::Point {
                    latitude,
                    longitude,
                } => (latitude, longitude, None),
                Spot::Name(name) => {
                    match self
                        .api
                        .geocode(&self.geocoding_url, name, ctx.timeout)
                        .await?
                    {
                        Some(place) => (place.latitude, place.longitude, place.label),
                        None => {
                            not_found.push(name.to_string());
                            continue;
                        }
                    }
                }
            };
            let label = marker
                .label
                .or(found_label)
                .unwrap_or_else(|| format!("{latitude:.4}, {longitude:.4}"));
            markers.push(json!({ "label": label, "latitude": latitude, "longitude": longitude }));
        }
        if markers.is_empty() {
            let names: Vec<String> = not_found.iter().map(|n| format!("{n:?}")).collect();
            bail!(
                "no place found for {}; try larger nearby cities or pass latitude and longitude",
                names.join(", ")
            );
        }
        Ok(map_json(request.title, markers, &not_found).to_string())
    }
}

/// Where a pin goes: a name to look up, or exact coordinates.
#[derive(Debug, PartialEq)]
enum Spot<'a> {
    Name(&'a str),
    Point { latitude: f64, longitude: f64 },
}

#[derive(Debug, PartialEq)]
struct Marker<'a> {
    spot: Spot<'a>,
    /// What the model asked the pin to say; otherwise the place's own name is used.
    label: Option<String>,
}

#[derive(Debug)]
struct MapRequest<'a> {
    title: Option<String>,
    markers: Vec<Marker<'a>>,
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

fn parse_input(input: &Value) -> Result<MapRequest<'_>> {
    let list = input["markers"]
        .as_array()
        .filter(|m| !m.is_empty())
        .ok_or_else(|| anyhow!("give markers as a list with at least one place"))?;
    if list.len() > MAX_MARKERS {
        bail!(
            "a map holds at most {MAX_MARKERS} markers, got {}",
            list.len()
        );
    }
    let markers = list
        .iter()
        .enumerate()
        .map(|(i, m)| parse_marker(m).with_context(|| format!("marker {}", i + 1)))
        .collect::<Result<Vec<_>>>()?;
    Ok(MapRequest {
        title: text(&input["title"]),
        markers,
    })
}

fn parse_marker(marker: &Value) -> Result<Marker<'_>> {
    if !marker.is_object() {
        bail!("must be an object with a location, or latitude and longitude");
    }
    let name = marker["location"]
        .as_str()
        .map(str::trim)
        .filter(|l| !l.is_empty());
    let label = text(&marker["label"]);
    Ok(match coordinates(marker)? {
        // Exact coordinates win; a name given with them labels the pin.
        Some((latitude, longitude)) => Marker {
            spot: Spot::Point {
                latitude,
                longitude,
            },
            label: label.or_else(|| name.map(clip)),
        },
        None => Marker {
            spot: Spot::Name(
                name.ok_or_else(|| anyhow!("give a location, or both latitude and longitude"))?,
            ),
            label,
        },
    })
}

/// The tool result: the pins, the caption, and the names that could not be placed.
fn map_json(title: Option<String>, markers: Vec<Value>, not_found: &[String]) -> Value {
    let mut map = Map::new();
    if let Some(title) = title {
        map.insert("title".into(), json!(title));
    }
    map.insert("markers".into(), Value::Array(markers));
    if !not_found.is_empty() {
        map.insert("not_found".into(), json!(not_found));
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_points_are_parsed_with_labels() {
        let input = json!({
            "title": "  Trip  ",
            "markers": [
                { "location": " Berlin " },
                { "location": "Paris, France", "label": "Home" },
                { "latitude": 40.7, "longitude": -74.0, "label": "NYC" },
                { "location": "Rome", "latitude": 41.9, "longitude": 12.5 },
            ]
        });
        let r = parse_input(&input).unwrap();
        assert_eq!(r.title.as_deref(), Some("Trip"));
        assert_eq!(
            r.markers,
            [
                Marker {
                    spot: Spot::Name("Berlin"),
                    label: None
                },
                Marker {
                    spot: Spot::Name("Paris, France"),
                    label: Some("Home".into())
                },
                Marker {
                    spot: Spot::Point {
                        latitude: 40.7,
                        longitude: -74.0
                    },
                    label: Some("NYC".into())
                },
                Marker {
                    spot: Spot::Point {
                        latitude: 41.9,
                        longitude: 12.5
                    },
                    label: Some("Rome".into())
                },
            ]
        );
    }

    #[test]
    fn blank_titles_and_labels_are_dropped_and_long_ones_cut() {
        let long = "x".repeat(500);
        let input = json!({
            "title": "   ",
            "markers": [{ "location": "A", "label": long }, { "location": "B", "label": " " }]
        });
        let r = parse_input(&input).unwrap();
        assert_eq!(r.title, None);
        assert_eq!(r.markers[0].label.as_ref().unwrap().chars().count(), 200);
        assert_eq!(r.markers[1].label, None);
    }

    #[test]
    fn bad_input_is_rejected_with_the_marker_number() {
        let err = |input: Value| format!("{:#}", parse_input(&input).unwrap_err());
        assert!(err(json!({})).contains("at least one place"));
        assert!(err(json!({ "markers": [] })).contains("at least one place"));
        assert!(err(json!({ "markers": "Berlin" })).contains("at least one place"));
        assert_eq!(
            err(json!({ "markers": [{ "location": "A" }, {}] })),
            "marker 2: give a location, or both latitude and longitude"
        );
        assert_eq!(
            err(json!({ "markers": ["Berlin"] })),
            "marker 1: must be an object with a location, or latitude and longitude"
        );
        assert!(err(json!({ "markers": [{ "latitude": 1.0 }] }))
            .contains("marker 1: latitude and longitude must be given together"));
        assert!(
            err(json!({ "markers": [{ "latitude": 95.0, "longitude": 0 }] }))
                .contains("marker 1: latitude must be between")
        );
        let many: Vec<Value> = (0..26).map(|_| json!({ "location": "A" })).collect();
        assert!(err(json!({ "markers": many })).contains("at most 25 markers, got 26"));
    }

    #[test]
    fn the_result_lists_pins_and_what_was_not_found() {
        let pins = vec![json!({ "label": "Berlin", "latitude": 52.5, "longitude": 13.4 })];
        assert_eq!(
            map_json(None, pins.clone(), &[]),
            json!({ "markers": [{ "label": "Berlin", "latitude": 52.5, "longitude": 13.4 }] })
        );
        assert_eq!(
            map_json(Some("Trip".into()), pins, &["Atlantis".to_string()]),
            json!({
                "title": "Trip",
                "markers": [{ "label": "Berlin", "latitude": 52.5, "longitude": 13.4 }],
                "not_found": ["Atlantis"]
            })
        );
    }
}
