//! What `weather_forecast` and `show_map` share: an HTTP client for the Open-Meteo APIs, place name
//! lookup through its geocoding API, and the parsing of coordinates from tool input.

use super::truncate_output;
use anyhow::{bail, Context, Result};
use reqwest::StatusCode;
use serde_json::Value;
use std::time::Duration;

/// Open-Meteo answers 429 when a client is over its free-tier limit; one short pause is enough for
/// the per-minute window to move on.
const RATE_LIMIT_PAUSE: Duration = Duration::from_secs(1);

/// A resolved location.
#[derive(Debug)]
pub(super) struct Place {
    pub label: Option<String>,
    pub latitude: f64,
    pub longitude: f64,
}

pub(super) struct OpenMeteo {
    http: reqwest::Client,
}

impl OpenMeteo {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .context("building HTTP client")?;
        Ok(Self { http })
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
    pub async fn get_json(
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

    /// The best match for a place name, or `None` if there is none. Open-Meteo answers 200 without
    /// `results` when nothing matches.
    pub async fn geocode(
        &self,
        geocoding_url: &str,
        name: &str,
        timeout: Duration,
    ) -> Result<Option<Place>> {
        let params = [
            ("name", name.to_string()),
            ("count", "1".to_string()),
            ("language", "en".to_string()),
            ("format", "json".to_string()),
        ];
        let body = self.get_json(geocoding_url, &params, timeout).await?;
        let hit = &body["results"][0];
        Ok(
            match (hit["latitude"].as_f64(), hit["longitude"].as_f64()) {
                (Some(latitude), Some(longitude)) => Some(Place {
                    label: Some(place_label(hit)),
                    latitude,
                    longitude,
                }),
                _ => None,
            },
        )
    }
}

/// The number at `input[key]` if there is one: `None` when absent, an error when it isn't a number
/// or lies outside ±`limit`.
pub(super) fn coordinate(input: &Value, key: &str, limit: f64) -> Result<Option<f64>> {
    match &input[key] {
        Value::Null => Ok(None),
        v => match v.as_f64() {
            Some(x) if x.abs() <= limit => Ok(Some(x)),
            Some(x) => bail!("{key} must be between -{limit} and {limit}, got {x}"),
            None => bail!("{key} must be a number"),
        },
    }
}

/// The latitude and longitude in `input`, which must be given together: `None` when neither is.
pub(super) fn coordinates(input: &Value) -> Result<Option<(f64, f64)>> {
    match (
        coordinate(input, "latitude", 90.0)?,
        coordinate(input, "longitude", 180.0)?,
    ) {
        (Some(latitude), Some(longitude)) => Ok(Some((latitude, longitude))),
        (None, None) => Ok(None),
        _ => bail!("latitude and longitude must be given together"),
    }
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
    use serde_json::json;

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
    fn error_reason_is_extracted() {
        let body =
            r#"{"error":true,"reason":"Latitude must be in range of -90 to 90°. Given: 999.0."}"#;
        assert_eq!(
            error_message(body),
            "Latitude must be in range of -90 to 90°. Given: 999.0."
        );
        assert_eq!(error_message("  upstream exploded "), "upstream exploded");
    }

    #[test]
    fn coordinates_come_in_pairs_within_range() {
        let pair = |input: Value| coordinates(&input);
        assert_eq!(pair(json!({})).unwrap(), None);
        assert_eq!(
            pair(json!({ "latitude": 52.5, "longitude": -13.4 })).unwrap(),
            Some((52.5, -13.4))
        );
        let err = |input: Value| pair(input).unwrap_err().to_string();
        assert!(err(json!({ "latitude": 1.0 })).contains("together"));
        assert!(err(json!({ "longitude": 1.0 })).contains("together"));
        assert!(err(json!({ "latitude": 91.0, "longitude": 0 })).contains("latitude"));
        assert!(err(json!({ "latitude": 0, "longitude": -180.5 })).contains("longitude"));
        assert!(err(json!({ "latitude": "north", "longitude": 0 })).contains("must be a number"));
    }
}
