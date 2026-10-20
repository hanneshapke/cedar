//! The `plan_trip` tool against local mocks of a MOTIS trip planner and an OSRM router.

mod common;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use common::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Mock {
    /// Answer every plan request with a 500.
    planner_down: bool,
    plans: Mutex<Vec<HashMap<String, String>>>,
    geocoded: Mutex<Vec<String>>,
    drives: Mutex<Vec<String>>,
}

async fn geocode(
    State(mock): State<Arc<Mock>>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Value> {
    let text = params["text"].clone();
    mock.geocoded.lock().unwrap().push(text.clone());
    Json(match text.as_str() {
        "Alexanderplatz" => {
            json!([{ "type": "STOP", "name": "Alexanderplatz", "lat": 52.5219, "lon": 13.4132,
                                     "areas": [{ "name": "Berlin", "default": true }] }])
        }
        _ => json!([]),
    })
}

async fn plan(
    State(mock): State<Arc<Mock>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    mock.plans.lock().unwrap().push(params);
    if mock.planner_down {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "timetable not loaded" })),
        )
            .into_response();
    }
    Json(json!({
        "from": { "name": "START", "tz": "Europe/Berlin" },
        "itineraries": [{
            "duration": 1380, "startTime": "2026-10-18T06:30:00Z", "endTime": "2026-10-18T06:53:00Z", "transfers": 0,
            "legs": [
                { "mode": "WALK", "from": { "name": "START" }, "to": { "name": "S+U Hauptbahnhof" },
                  "duration": 180, "distance": 210.0 },
                { "mode": "SUBWAY", "from": { "name": "S+U Hauptbahnhof" }, "to": { "name": "Alexanderplatz" },
                  "duration": 1200, "routeShortName": "U5", "headsign": "Hönow", "routeColor": "7E5330",
                  "intermediateStops": [{}, {}, {}, {}, {}] }
            ]
        }],
        "direct": []
    }))
    .into_response()
}

async fn drive(State(mock): State<Arc<Mock>>, Path(coords): Path<String>) -> Json<Value> {
    mock.drives.lock().unwrap().push(coords);
    Json(
        json!({ "code": "Ok", "routes": [{ "distance": 4200.0, "duration": 720.0,
                                            "geometry": "_p~iF~ps|U_ulLnnqC" }] }),
    )
}

async fn start(mock: Mock) -> (String, Arc<Mock>) {
    let mock = Arc::new(mock);
    let app = Router::new()
        .route("/api/v1/geocode", get(geocode))
        .route("/api/v1/plan", get(plan))
        .route("/route/v1/driving/{coords}", get(drive))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, mock)
}

/// The `plan_trip` result the model got back after one call with `input`, and the request the
/// tool made to choose an option, if it made one. `choice` answers that request.
async fn plan_trip_choosing(input: Value, url: &str, choice: Step) -> (Value, Option<Value>) {
    let app = TestApp::with_env(
        vec![
            Step::Reply(tool_use("t1", "plan_trip", input)),
            choice,
            Step::Reply(text("done")),
        ],
        &[("TRANSIT_BASE_URL", url), ("DRIVING_BASE_URL", url)],
    )
    .await;
    let created = app.create_run(KEY_A, "how do I get there?").await;
    let id = created["id"].as_str().unwrap().to_string();
    app.wait_status(KEY_A, &id, "completed").await;
    let requests = app.llm.requests();
    let (choosing, last) = match requests.len() {
        3 => (Some(requests[1].clone()), &requests[2]),
        _ => (None, &requests[1]),
    };
    let result = last["messages"].as_array().unwrap().last().unwrap()["content"][0].clone();
    (result, choosing)
}

/// The `plan_trip` result when the model's choice can't be used, so the rules decide.
async fn plan_trip(input: Value, url: &str) -> Value {
    plan_trip_choosing(input, url, Step::Error("model unavailable"))
        .await
        .0
}

#[tokio::test]
async fn transit_and_driving_are_compared_and_one_is_recommended() {
    let (url, mock) = start(Mock::default()).await;
    let result = plan_trip(
        json!({
            "from": { "latitude": 52.525, "longitude": 13.369, "label": "Hauptbahnhof" },
            "to": "Alexanderplatz",
            "depart_at": "2026-10-18T08:30:00+02:00"
        }),
        &url,
    )
    .await;
    assert!(result.get("is_error").is_none(), "{result}");
    let trip: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();

    assert_eq!(*mock.geocoded.lock().unwrap(), ["Alexanderplatz"]);
    let plans = mock.plans.lock().unwrap();
    assert_eq!(plans[0]["fromPlace"], "52.525,13.369");
    assert_eq!(plans[0]["toPlace"], "52.5219,13.4132");
    assert_eq!(plans[0]["time"], "2026-10-18T08:30:00+02:00");
    assert!(!plans[0].contains_key("arriveBy"));
    assert_eq!(
        *mock.drives.lock().unwrap(),
        ["13.369,52.525;13.4132,52.5219"]
    );

    assert_eq!(
        trip["from"],
        json!({ "label": "Hauptbahnhof", "latitude": 52.525, "longitude": 13.369 })
    );
    assert_eq!(trip["to"]["label"], "Alexanderplatz, Berlin");
    assert_eq!(trip["depart_at"], "2026-10-18T08:30:00+02:00");
    assert_eq!(trip["time_zone"], "Europe/Berlin");
    let modes: Vec<&str> = trip["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["mode"].as_str().unwrap())
        .collect();
    assert_eq!(modes, ["transit", "drive", "rideshare"]);
    let transit = &trip["options"][0];
    assert_eq!(transit["duration_min"], 23);
    assert_eq!(transit["legs"][1]["route"], "U5");
    assert_eq!(transit["legs"][1]["stops"], 6);
    assert_eq!(trip["options"][1]["distance_km"], 4.2);
    assert_eq!(
        trip["options"][2]["cost"],
        json!({ "low": 10, "high": 15, "currency": "USD" })
    );
    // 23 min by U-Bahn against 12 by car plus parking is close enough.
    assert_eq!(trip["recommendation"]["mode"], "transit");
    assert_eq!(trip["recommendation"]["option"], 0);
    assert!(trip.get("unavailable").is_none(), "{trip}");
    assert!(trip["attribution"]
        .as_str()
        .unwrap()
        .contains("OpenStreetMap"));
}

#[tokio::test]
async fn without_the_planner_the_car_is_recommended_and_transit_is_explained() {
    let (url, mock) = start(Mock {
        planner_down: true,
        ..Mock::default()
    })
    .await;
    let result = plan_trip(
        json!({
            "from": { "latitude": 52.525, "longitude": 13.369 },
            "to": { "latitude": 52.5219, "longitude": 13.4132 },
            "arrive_by": "2026-10-18T09:00:00Z",
            "has_car": false
        }),
        &url,
    )
    .await;
    assert!(result.get("is_error").is_none(), "{result}");
    let trip: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
    assert!(mock.geocoded.lock().unwrap().is_empty());
    assert_eq!(mock.plans.lock().unwrap()[0]["arriveBy"], "true");
    assert_eq!(trip["arrive_by"], "2026-10-18T09:00:00Z");
    assert_eq!(trip["recommendation"]["mode"], "rideshare");
    assert_eq!(trip["unavailable"][0]["mode"], "transit");
    assert!(
        trip["unavailable"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("timetable not loaded"),
        "{trip}"
    );
}

#[tokio::test]
async fn a_place_that_is_not_found_is_an_error() {
    let (url, _mock) = start(Mock::default()).await;
    // No option is chosen, so the model's next reply ends the run.
    let (result, choosing) = plan_trip_choosing(
        json!({ "from": "Atlantis", "to": "Alexanderplatz" }),
        &url,
        Step::Reply(text("done")),
    )
    .await;
    assert!(choosing.is_none());
    assert_eq!(result["is_error"], true, "{result}");
    let message = result["content"].as_str().unwrap();
    assert!(
        message.contains("from: no place found for \"Atlantis\""),
        "{message}"
    );
}

#[tokio::test]
async fn the_model_weighs_the_options_and_the_preferences() {
    let (url, _mock) = start(Mock::default()).await;
    let (result, choosing) = plan_trip_choosing(
        json!({
            "from": { "latitude": 52.525, "longitude": 13.369, "label": "Hauptbahnhof" },
            "to": "Alexanderplatz",
            "depart_at": "2026-10-18T08:30:00+02:00",
            "preferences": "I have two big suitcases"
        }),
        &url,
        Step::Reply(text(
            "{\"option\": 2, \"reason\": \"With two big suitcases a rideshare to the door beats the U5 and its stairs.\"}",
        )),
    )
    .await;
    assert!(result.get("is_error").is_none(), "{result}");
    let trip: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
    assert_eq!(
        trip["recommendation"],
        json!({ "mode": "rideshare", "option": 2,
                "reason": "With two big suitcases a rideshare to the door beats the U5 and its stairs." })
    );

    let choosing = choosing.expect("the tool asked the model");
    assert_eq!(choosing["tools"], json!([]));
    assert!(choosing["system"]
        .as_str()
        .unwrap()
        .contains("Reply with only a JSON object"));
    let asked: Value =
        serde_json::from_str(choosing["messages"][0]["content"].as_str().unwrap()).unwrap();
    assert_eq!(asked["traveller_preferences"], "I have two big suitcases");
    assert_eq!(asked["traveller_has_car"], true);
    assert_eq!(asked["depart_at"], "2026-10-18T08:30:00+02:00");
    assert_eq!(asked["to"], "Alexanderplatz, Berlin");
    assert_eq!(asked["options"][1]["index"], 1);
    assert_eq!(asked["options"][1]["mode"], "drive");
    assert!(asked["options"][1].get("path").is_none(), "{asked}");
    assert_eq!(asked["options"][0]["legs"][1]["route"], "U5");
}

#[tokio::test]
async fn an_unusable_choice_falls_back_to_the_rules() {
    let (url, _mock) = start(Mock::default()).await;
    // The model picks the drive, but the user has no car.
    let (result, _) = plan_trip_choosing(
        json!({ "from": "Alexanderplatz", "to": { "latitude": 52.525, "longitude": 13.369 },
                "has_car": false }),
        &url,
        Step::Reply(text("{\"option\": 1, \"reason\": \"Driving is fastest.\"}")),
    )
    .await;
    let trip: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
    assert_eq!(trip["recommendation"]["mode"], "transit");
    assert!(
        trip["recommendation"]["reason"]
            .as_str()
            .unwrap()
            .starts_with("Public transit takes 23 min"),
        "{trip}"
    );
}
