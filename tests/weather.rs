//! The `weather_forecast` tool against a local mock of the Open-Meteo geocoding and forecast APIs.

mod common;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use common::*;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

/// One endpoint of the mock: canned responses and the query parameters of every request.
struct Route {
    /// (status, body); the last entry repeats once the queue is down to one.
    responses: Mutex<VecDeque<(u16, Value)>>,
    seen: Mutex<Vec<HashMap<String, String>>>,
}

impl Route {
    fn new(responses: Vec<(u16, Value)>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            seen: Mutex::new(vec![]),
        }
    }

    fn respond(&self, params: HashMap<String, String>) -> impl IntoResponse {
        self.seen.lock().unwrap().push(params);
        let mut queue = self.responses.lock().unwrap();
        let (status, body) = if queue.len() > 1 {
            queue.pop_front().unwrap()
        } else {
            queue.front().cloned().unwrap()
        };
        (StatusCode::from_u16(status).unwrap(), Json(body))
    }
}

struct Mock {
    geocoding: Route,
    forecast: Route,
}

async fn geocoding(
    State(mock): State<Arc<Mock>>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    mock.geocoding.respond(params)
}

async fn forecast(
    State(mock): State<Arc<Mock>>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    mock.forecast.respond(params)
}

async fn start(
    geocoding_responses: Vec<(u16, Value)>,
    forecast_responses: Vec<(u16, Value)>,
) -> (String, Arc<Mock>) {
    let mock = Arc::new(Mock {
        geocoding: Route::new(geocoding_responses),
        forecast: Route::new(forecast_responses),
    });
    let app = Router::new()
        .route("/geo", get(geocoding))
        .route("/forecast", get(forecast))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, mock)
}

fn berlin() -> Value {
    json!({ "results": [{
        "name": "Berlin", "latitude": 52.52437, "longitude": 13.41053,
        "country": "Germany", "admin1": "State of Berlin", "timezone": "Europe/Berlin"
    }] })
}

fn forecast_body() -> Value {
    json!({
        "timezone": "Europe/Berlin",
        "current_units": { "temperature_2m": "°C", "weather_code": "wmo code" },
        "current": { "time": "2026-10-12T21:00", "temperature_2m": 10.2, "weather_code": 0 },
        "daily_units": { "temperature_2m_max": "°C", "temperature_2m_min": "°C" },
        "daily": { "time": ["2026-10-12", "2026-10-13"], "weather_code": [3, 61],
                   "temperature_2m_max": [13.7, 15.1], "temperature_2m_min": [6.8, 5.1] }
    })
}

async fn app_with_weather(script: Vec<Step>, base: &str) -> TestApp {
    TestApp::with_env(
        script,
        &[
            ("WEATHER_BASE_URL", &format!("{base}/forecast")),
            ("WEATHER_GEOCODING_BASE_URL", &format!("{base}/geo")),
        ],
    )
    .await
}

fn ask_weather(input: Value) -> Vec<Step> {
    vec![
        Step::Reply(tool_use("t1", "weather_forecast", input)),
        Step::Reply(text("done")),
    ]
}

/// The tool result at the end of the messages sent in the model's `request`-th call.
fn tool_result(app: &TestApp, request: usize) -> Value {
    app.llm.requests()[request]["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["content"][0]
        .clone()
}

async fn run_to_completion(app: &TestApp) {
    let created = app.create_run(KEY_A, "what's the weather?").await;
    let id = created["id"].as_str().unwrap().to_string();
    app.wait_status(KEY_A, &id, "completed").await;
}

fn tool_names(request: &Value) -> Vec<&str> {
    request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn a_place_name_is_geocoded_and_the_forecast_reaches_the_model() {
    let (base, mock) = start(vec![(200, berlin())], vec![(200, forecast_body())]).await;
    let app = app_with_weather(
        ask_weather(json!({ "location": "Berlin", "days": 2 })),
        &base,
    )
    .await;
    run_to_completion(&app).await;

    let requests = app.llm.requests();
    assert!(tool_names(&requests[0]).contains(&"weather_forecast"));

    let geo = mock.geocoding.seen.lock().unwrap();
    assert_eq!(geo.len(), 1);
    assert_eq!(geo[0]["name"], "Berlin");
    assert_eq!(geo[0]["count"], "1");
    assert_eq!(geo[0]["language"], "en");

    let seen = mock.forecast.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0]["latitude"], "52.52437");
    assert_eq!(seen[0]["longitude"], "13.41053");
    assert_eq!(seen[0]["timezone"], "auto");
    assert_eq!(seen[0]["forecast_days"], "2");
    assert!(seen[0]["current"].contains("temperature_2m"));
    assert!(seen[0]["daily"].contains("sunrise"));
    assert!(!seen[0].contains_key("temperature_unit"));

    let result = tool_result(&app, 1);
    assert_eq!(result["tool_use_id"], "t1");
    assert!(result.get("is_error").is_none(), "{result}");
    assert_eq!(
        result["content"],
        "Berlin, State of Berlin, Germany (52.52, 13.41), timezone Europe/Berlin\n\n\
         Now (2026-10-12 21:00): clear sky, 10.2°C\n\n\
         Mon 2026-10-12: overcast, low 6.8°C, high 13.7°C\n\
         Tue 2026-10-13: slight rain, low 5.1°C, high 15.1°C\n\n\
         Weather data by Open-Meteo.com (CC BY 4.0)"
    );
}

#[tokio::test]
async fn coordinates_skip_geocoding_and_imperial_units_are_requested() {
    let (base, mock) = start(vec![(200, berlin())], vec![(200, forecast_body())]).await;
    let app = app_with_weather(
        ask_weather(json!({
            "location": "Home", "latitude": 40.7128, "longitude": -74.006, "units": "imperial"
        })),
        &base,
    )
    .await;
    run_to_completion(&app).await;

    assert!(mock.geocoding.seen.lock().unwrap().is_empty());
    let seen = mock.forecast.seen.lock().unwrap();
    assert_eq!(seen[0]["latitude"], "40.7128");
    assert_eq!(seen[0]["longitude"], "-74.006");
    assert_eq!(seen[0]["forecast_days"], "3");
    assert_eq!(seen[0]["temperature_unit"], "fahrenheit");
    assert_eq!(seen[0]["wind_speed_unit"], "mph");
    assert_eq!(seen[0]["precipitation_unit"], "inch");

    let result = tool_result(&app, 1);
    assert!(result.get("is_error").is_none(), "{result}");
    assert!(result["content"]
        .as_str()
        .unwrap()
        .starts_with("Home (40.71, -74.01), timezone Europe/Berlin"));
}

#[tokio::test]
async fn an_unknown_place_is_an_error_and_no_forecast_is_requested() {
    // Open-Meteo answers 200 with no `results` when nothing matches.
    let (base, mock) = start(
        vec![(200, json!({ "generationtime_ms": 0.5 }))],
        vec![(200, forecast_body())],
    )
    .await;
    let app = app_with_weather(ask_weather(json!({ "location": "Zzzqqxx" })), &base).await;
    run_to_completion(&app).await;

    assert!(mock.forecast.seen.lock().unwrap().is_empty());
    let result = tool_result(&app, 1);
    assert_eq!(result["is_error"], true);
    let content = result["content"].as_str().unwrap();
    assert!(
        content.contains("no place found for \"Zzzqqxx\""),
        "{content}"
    );
    assert!(content.contains("latitude and longitude"), "{content}");
}

#[tokio::test]
async fn a_rate_limited_request_is_retried_once() {
    let (base, mock) = start(vec![], vec![(429, json!({})), (200, forecast_body())]).await;
    let app = app_with_weather(
        ask_weather(json!({ "latitude": 52.5, "longitude": 13.4 })),
        &base,
    )
    .await;
    run_to_completion(&app).await;

    assert_eq!(mock.forecast.seen.lock().unwrap().len(), 2);
    let result = tool_result(&app, 1);
    assert!(result.get("is_error").is_none(), "{result}");
    assert!(result["content"].as_str().unwrap().contains("clear sky"));
}

#[tokio::test]
async fn persistent_rate_limiting_is_reported_to_the_model() {
    let (base, mock) = start(vec![], vec![(429, json!({}))]).await;
    let app = app_with_weather(
        ask_weather(json!({ "latitude": 52.5, "longitude": 13.4 })),
        &base,
    )
    .await;
    run_to_completion(&app).await;

    assert_eq!(
        mock.forecast.seen.lock().unwrap().len(),
        2,
        "one try plus one retry"
    );
    let result = tool_result(&app, 1);
    assert_eq!(result["is_error"], true);
    assert!(result["content"].as_str().unwrap().contains("rate limited"));
}

#[tokio::test]
async fn api_errors_carry_open_meteo_s_reason() {
    let (base, _mock) = start(
        vec![],
        vec![(
            400,
            json!({ "error": true, "reason": "Cannot initialize WeatherVariable from invalid String value" }),
        )],
    )
    .await;
    let app = app_with_weather(
        ask_weather(json!({ "latitude": 52.5, "longitude": 13.4 })),
        &base,
    )
    .await;
    run_to_completion(&app).await;

    let result = tool_result(&app, 1);
    assert_eq!(result["is_error"], true);
    let content = result["content"].as_str().unwrap();
    assert!(
        content.contains("400") && content.contains("Cannot initialize WeatherVariable"),
        "{content}"
    );
}

#[tokio::test]
async fn bad_input_never_calls_open_meteo() {
    let (base, mock) = start(vec![(200, berlin())], vec![(200, forecast_body())]).await;
    let app = app_with_weather(ask_weather(json!({ "location": "  " })), &base).await;
    run_to_completion(&app).await;

    assert!(mock.geocoding.seen.lock().unwrap().is_empty());
    assert!(mock.forecast.seen.lock().unwrap().is_empty());
    let result = tool_result(&app, 1);
    assert_eq!(result["is_error"], true);
    assert_eq!(
        result["content"],
        "give a location, or both latitude and longitude"
    );
}

#[tokio::test]
async fn with_weather_off_the_tool_is_not_offered() {
    let app = TestApp::with_env(vec![Step::Reply(text("hi"))], &[("WEATHER", "off")]).await;
    run_to_completion(&app).await;

    assert!(!tool_names(&app.llm.requests()[0]).contains(&"weather_forecast"));
}
