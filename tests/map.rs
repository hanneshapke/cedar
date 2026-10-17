//! The `show_map` tool against a local mock of the Open-Meteo geocoding API.

mod common;

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use common::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

struct Mock {
    /// Geocoding answers by the `name` asked for; anything else matches nothing.
    places: HashMap<&'static str, Value>,
    seen: Mutex<Vec<HashMap<String, String>>>,
}

async fn geocoding(
    State(mock): State<Arc<Mock>>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Value> {
    let answer = mock
        .places
        .get(params["name"].as_str())
        .cloned()
        // Open-Meteo answers 200 without `results` when nothing matches.
        .unwrap_or_else(|| json!({ "generationtime_ms": 0.5 }));
    mock.seen.lock().unwrap().push(params);
    Json(answer)
}

async fn start() -> (String, Arc<Mock>) {
    let hit = |name: &str, admin1: &str, country: &str, latitude: f64, longitude: f64| {
        json!({ "results": [{
            "name": name, "admin1": admin1, "country": country,
            "latitude": latitude, "longitude": longitude
        }] })
    };
    let mock = Arc::new(Mock {
        places: HashMap::from([
            (
                "Berlin",
                hit("Berlin", "State of Berlin", "Germany", 52.52437, 13.41053),
            ),
            (
                "Paris, France",
                hit("Paris", "Île-de-France Region", "France", 48.85341, 2.3488),
            ),
        ]),
        seen: Mutex::new(vec![]),
    });
    let app = Router::new()
        .route("/geo", get(geocoding))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/geo", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, mock)
}

async fn app_with_maps(input: Value, geocoding_url: &str) -> TestApp {
    TestApp::with_env(
        vec![
            Step::Reply(tool_use("t1", "show_map", input)),
            Step::Reply(text("done")),
        ],
        &[("WEATHER_GEOCODING_BASE_URL", geocoding_url)],
    )
    .await
}

/// The tool result at the end of the messages sent in the model's second call.
fn tool_result(app: &TestApp) -> Value {
    app.llm.requests()[1]["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["content"][0]
        .clone()
}

async fn run_to_completion(app: &TestApp) {
    let created = app.create_run(KEY_A, "where are these?").await;
    let id = created["id"].as_str().unwrap().to_string();
    app.wait_status(KEY_A, &id, "completed").await;
}

#[tokio::test]
async fn names_are_geocoded_and_points_pass_through() {
    let (url, mock) = start().await;
    let app = app_with_maps(
        json!({
            "title": "Where we've been",
            "markers": [
                { "location": "Berlin" },
                { "location": "Paris, France", "label": "Home" },
                { "latitude": 40.7128, "longitude": -74.006, "label": "New York" }
            ]
        }),
        &url,
    )
    .await;
    run_to_completion(&app).await;

    let seen = mock.seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "the coordinates need no lookup");
    assert_eq!(seen[0]["name"], "Berlin");
    assert_eq!(seen[0]["count"], "1");
    assert_eq!(seen[1]["name"], "Paris, France");

    let result = tool_result(&app);
    assert_eq!(result["tool_use_id"], "t1");
    assert!(result.get("is_error").is_none(), "{result}");
    let map: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
    assert_eq!(
        map,
        json!({
            "title": "Where we've been",
            "markers": [
                { "label": "Berlin, State of Berlin, Germany", "latitude": 52.52437, "longitude": 13.41053 },
                { "label": "Home", "latitude": 48.85341, "longitude": 2.3488 },
                { "label": "New York", "latitude": 40.7128, "longitude": -74.006 }
            ]
        })
    );
}

#[tokio::test]
async fn a_name_that_is_not_found_is_listed_beside_the_pins_that_were() {
    let (url, _mock) = start().await;
    let app = app_with_maps(
        json!({ "markers": [{ "location": "Atlantis" }, { "location": "Berlin" }, { "latitude": 1.5, "longitude": 2.5 }] }),
        &url,
    )
    .await;
    run_to_completion(&app).await;

    let result = tool_result(&app);
    assert!(result.get("is_error").is_none(), "{result}");
    let map: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
    assert_eq!(map["not_found"], json!(["Atlantis"]));
    assert!(map.get("title").is_none());
    let labels: Vec<_> = map["markers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["label"].as_str().unwrap())
        .collect();
    assert_eq!(
        labels,
        ["Berlin, State of Berlin, Germany", "1.5000, 2.5000"]
    );
}

#[tokio::test]
async fn nothing_found_is_an_error_the_model_can_act_on() {
    let (url, _mock) = start().await;
    let app = app_with_maps(
        json!({ "markers": [{ "location": "Atlantis" }, { "location": "El Dorado" }] }),
        &url,
    )
    .await;
    run_to_completion(&app).await;

    let result = tool_result(&app);
    assert_eq!(result["is_error"], true);
    let content = result["content"].as_str().unwrap();
    assert!(
        content.contains("\"Atlantis\", \"El Dorado\"")
            && content.contains("latitude and longitude"),
        "{content}"
    );
}

#[tokio::test]
async fn bad_input_names_the_marker_and_never_calls_the_geocoder() {
    let (url, mock) = start().await;
    let app = app_with_maps(
        json!({ "markers": [{ "location": "Berlin" }, { "label": "nowhere" }] }),
        &url,
    )
    .await;
    run_to_completion(&app).await;

    assert!(mock.seen.lock().unwrap().is_empty());
    let result = tool_result(&app);
    assert_eq!(result["is_error"], true);
    assert_eq!(
        result["content"],
        "marker 2: give a location, or both latitude and longitude"
    );
}

#[tokio::test]
async fn with_maps_off_the_tool_is_not_offered() {
    let app = TestApp::with_env(vec![Step::Reply(text("hi"))], &[("MAPS", "off")]).await;
    run_to_completion(&app).await;

    let names: Vec<_> = app.llm.requests()[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert!(!names.contains(&"show_map".to_string()), "{names:?}");
    assert!(names.contains(&"weather_forecast".to_string()), "{names:?}");
}

#[tokio::test]
async fn by_default_the_tool_is_offered_after_the_weather_tool() {
    let app = TestApp::new(vec![Step::Reply(text("hi"))]).await;
    run_to_completion(&app).await;

    let requests = app.llm.requests();
    let names: Vec<_> = requests[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        &names[names.len() - 5..],
        [
            "weather_forecast",
            "show_map",
            "show_recipe",
            "plan_trip",
            "ask_user"
        ]
    );
}
