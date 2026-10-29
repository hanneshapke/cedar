//! The `search_flights` tool and its SerpApi key, against a local mock of SerpApi's Google
//! Flights and account APIs.

mod common;

use axum::extract::{Query, State};
use axum::http::{Method, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use cedar::store::Store;
use common::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const GOOD_KEY: &str = "good-serpapi-key-8765";
const SECRET_KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[derive(Default)]
struct Mock {
    searches: Mutex<Vec<HashMap<String, String>>>,
}

fn leg(flight: &str, from: &str, to: &str, departs: &str, arrives: &str) -> Value {
    json!({ "departure_airport": { "name": format!("{from} Airport"), "id": from, "time": departs },
            "arrival_airport": { "name": format!("{to} Airport"), "id": to, "time": arrives },
            "duration": 600, "airplane": "Airbus A350", "airline": "Lufthansa",
            "airline_logo": "https://www.gstatic.com/flights/airline_logos/70px/LH.png",
            "travel_class": "Economy", "flight_number": flight, "legroom": "31 in" })
}

async fn search(
    State(mock): State<Arc<Mock>>,
    Query(params): Query<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    if params.get("api_key").map(String::as_str) != Some(GOOD_KEY) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Invalid API key." })),
        );
    }
    assert_eq!(params["engine"], "google_flights");
    mock.searches.lock().unwrap().push(params.clone());
    if params["arrival_id"] == "NOP" {
        return (
            StatusCode::OK,
            Json(json!({ "error": "Google Flights hasn't returned any results for this query." })),
        );
    }
    if params.contains_key("booking_token") {
        return (
            StatusCode::OK,
            Json(json!({
                "selected_flights": [{ "flights": [leg("LH 455", "SFO", "MUC", "2026-12-01 15:40", "2026-12-02 11:30")],
                                       "price": 1234 }],
                "booking_options": [{ "together": { "book_with": "Lufthansa", "price": 1234,
                                                    "baggage_prices": ["1 free carry-on"] } }],
                "search_metadata": { "google_flights_url": "https://www.google.com/travel/flights?tfs=booking" }
            })),
        );
    }
    (
        StatusCode::OK,
        Json(json!({
            "search_metadata": { "google_flights_url": "https://www.google.com/travel/flights?tfs=x" },
            "best_flights": [{ "flights": [leg("LH 455", "SFO", "MUC", "2026-12-01 15:40", "2026-12-02 11:30")],
                               "total_duration": 650, "price": 1234, "departure_token": "DEP1",
                               "booking_token": "BOOK1" }],
            "other_flights": [{ "flights": [leg("UA 58", "SFO", "EWR", "2026-12-01 07:00", "2026-12-01 15:30"),
                                            leg("UA 30", "EWR", "MUC", "2026-12-01 17:30", "2026-12-02 07:45")],
                                "layovers": [{ "duration": 120, "name": "Newark", "id": "EWR" }],
                                "total_duration": 1005, "price": 999 }],
            "price_insights": { "lowest_price": 999, "price_level": "low", "typical_price_range": [1100, 1600],
                                "price_history": [[1_792_000_000, 1300], [1_792_086_400, 1250]] }
        })),
    )
}

/// The day `n` days from now, as the tool takes dates.
fn day(n: u64) -> String {
    (chrono::Utc::now().date_naive() + chrono::Days::new(n)).to_string()
}

async fn account(Query(params): Query<HashMap<String, String>>) -> (StatusCode, Json<Value>) {
    if params.get("api_key").map(String::as_str) == Some(GOOD_KEY) {
        (
            StatusCode::OK,
            Json(json!({ "plan_name": "Free Plan", "searches_per_month": 250 })),
        )
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Invalid API key." })),
        )
    }
}

async fn start() -> (&'static str, Arc<Mock>) {
    let mock = Arc::new(Mock::default());
    let app = Router::new()
        .route("/search.json", get(search))
        .route("/account.json", get(account))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (Box::leak(base.into_boxed_str()), mock)
}

/// Run one `search_flights` call with `input` in a `de-CH` run and return its tool result.
async fn call(input: Value, base: &str) -> Value {
    let app = TestApp::with_env(
        vec![
            Step::Reply(tool_use("t1", "search_flights", input)),
            Step::Reply(text("Here are your flights.")),
        ],
        &[("SERPAPI_API_KEY", GOOD_KEY), ("FLIGHTS_API_URL", base)],
    )
    .await;
    let created = app
        .create_run_with(
            KEY_A,
            json!({ "prompt": "flights to Munich?", "locale": "de-CH" }),
        )
        .await;
    let id = created["id"].as_str().unwrap().to_string();
    app.wait_status(KEY_A, &id, "completed").await;
    app.llm.requests()[1]["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["content"][0]
        .clone()
}

fn card(result: &Value) -> Value {
    assert!(result.get("is_error").is_none(), "{result}");
    serde_json::from_str(result["content"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn a_round_trip_search_becomes_the_flights_card() {
    let (base, mock) = start().await;
    let r = card(
        &call(
            json!({ "from": ["sfo"], "to": "MUC", "date": day(30), "return_date": day(39),
                    "adults": 2, "max_stops": 1 }),
            base,
        )
        .await,
    );
    let sent = mock.searches.lock().unwrap()[0].clone();
    assert_eq!(sent["departure_id"], "SFO");
    assert_eq!(sent["type"], "1");
    assert_eq!(sent["adults"], "2");
    assert_eq!(sent["stops"], "2");
    assert_eq!(
        sent["currency"], "CHF",
        "the run's locale picks the currency"
    );
    assert_eq!(sent["gl"], "ch");

    assert_eq!(r["kind"], "flights");
    assert_eq!(r["leg"], "outbound");
    assert_eq!(r["passengers"]["adults"], 2);
    let options = r["options"].as_array().unwrap();
    assert_eq!(options.len(), 2);
    assert_eq!(options[0]["top_pick"], true);
    assert_eq!(options[0]["departure_token"], "DEP1");
    assert_eq!(options[1]["top_pick"], false);
    assert_eq!(options[1]["stops"], 1);
    assert_eq!(options[1]["layovers"][0]["airport"], "EWR");
    assert_eq!(r["insights"]["level"], "low");
    assert_eq!(r["insights"]["history"].as_array().unwrap().len(), 2);
    assert_eq!(
        r["google_flights_url"],
        "https://www.google.com/travel/flights?tfs=x"
    );
}

#[tokio::test]
async fn return_flights_booking_options_and_empty_searches() {
    let (base, mock) = start().await;
    let round = json!({ "from": ["SFO"], "to": ["MUC"], "date": day(30), "return_date": day(39) });

    let mut returns = round.clone();
    returns["departure_token"] = json!("DEP1");
    let r = card(&call(returns, base).await);
    assert_eq!(r["leg"], "return");
    assert_eq!(r["from"], json!(["MUC"]));
    assert!(r.get("insights").is_none());
    assert_eq!(mock.searches.lock().unwrap()[0]["departure_token"], "DEP1");

    let mut book = round.clone();
    book["booking_token"] = json!("BOOK1");
    let b = card(&call(book, base).await);
    assert_eq!(b["kind"], "booking");
    assert_eq!(b["options"][0]["seller"], "Lufthansa");
    assert_eq!(b["flights"][0]["segments"][0]["flight"], "LH 455");

    let nothing = card(
        &call(
            json!({ "from": ["SFO"], "to": ["NOP"], "date": day(30) }),
            base,
        )
        .await,
    );
    assert_eq!(nothing["leg"], "one_way");
    assert_eq!(nothing["options"], json!([]));
    assert!(nothing["no_results"]
        .as_str()
        .unwrap()
        .contains("no flights"));
}

#[tokio::test]
async fn bad_input_is_an_error_for_the_model() {
    let (base, mock) = start().await;
    let result = call(
        json!({ "from": ["San Francisco"], "to": ["MUC"], "date": day(30) }),
        base,
    )
    .await;
    assert_eq!(result["is_error"], true);
    assert!(result["content"]
        .as_str()
        .unwrap()
        .contains("not an airport code"));
    assert!(mock.searches.lock().unwrap().is_empty());
}

fn admin_env(extra: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
    let mut env = vec![
        ("CEDAR_ADMIN_KEYS", KEY_B),
        ("CEDAR_SECRET_KEY", SECRET_KEY),
    ];
    env.extend_from_slice(extra);
    env
}

fn flights_group(catalog: &Value) -> Value {
    catalog["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == "flights")
        .unwrap()
        .clone()
}

#[tokio::test]
async fn a_serpapi_key_saved_by_an_admin_turns_flight_search_on_without_a_restart() {
    let (base, _) = start().await;
    let store = Store::connect_memory().await.unwrap();
    let app = TestApp::build(
        store.clone(),
        ScriptedLlm::new(vec![Step::Repeat(text("ok"))]),
        FakeFactory::new(),
        &admin_env(&[("FLIGHTS_API_URL", base)]),
    )
    .await;

    let (_, status) = app.get("/v1/admin/integrations", KEY_B).await;
    assert_eq!(
        status["serpapi"],
        json!({ "configured": false, "source": null, "enabled": true })
    );
    let (_, catalog) = app.get("/v1/tools", KEY_A).await;
    assert_eq!(flights_group(&catalog)["available"], false);

    // A key SerpApi rejects is not saved.
    let (code, body) = app
        .json(
            Method::PUT,
            "/v1/admin/integrations/serpapi",
            Some(KEY_B),
            Some(json!({ "api_key": "wrong" })),
        )
        .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("SerpApi rejected the API key"),
        "{body}"
    );

    let (code, body) = app
        .json(
            Method::PUT,
            "/v1/admin/integrations/serpapi",
            Some(KEY_B),
            Some(json!({ "api_key": format!(" {GOOD_KEY} ") })),
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(
        body["serpapi"],
        json!({ "configured": true, "source": "ui", "last4": "8765", "enabled": true })
    );
    let (_, _, raw) = app
        .call(Method::GET, "/v1/admin/integrations", Some(KEY_B), None)
        .await;
    assert!(!raw.contains(GOOD_KEY));

    let run = app.create_run(KEY_A, "flights to Munich?").await;
    app.wait_status(KEY_A, run["id"].as_str().unwrap(), "completed")
        .await;
    let request = &app.llm.requests()[0];
    let names: Vec<&str> = request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"search_flights"), "{names:?}");
    assert!(request["system"]
        .as_str()
        .unwrap()
        .contains("use search_flights"));

    // A restarted server picks the saved key up again.
    let restarted = TestApp::build(
        store,
        ScriptedLlm::new(vec![]),
        FakeFactory::new(),
        &admin_env(&[("FLIGHTS_API_URL", base)]),
    )
    .await;
    let (_, catalog) = restarted.get("/v1/tools", KEY_A).await;
    assert_eq!(flights_group(&catalog)["available"], true);

    let (code, _) = app
        .json(
            Method::DELETE,
            "/v1/admin/integrations/serpapi",
            Some(KEY_B),
            None,
        )
        .await;
    assert_eq!(code, StatusCode::NO_CONTENT);
    let (_, catalog) = app.get("/v1/tools", KEY_A).await;
    assert_eq!(flights_group(&catalog)["available"], false);
}

#[tokio::test]
async fn a_key_from_the_environment_or_with_flights_off_is_reported() {
    let app = TestApp::with_env(
        vec![],
        &admin_env(&[("SERPAPI_API_KEY", "env-serpapi-3333")]),
    )
    .await;
    let (_, status) = app.get("/v1/admin/integrations", KEY_B).await;
    assert_eq!(
        status["serpapi"],
        json!({ "configured": true, "source": "env", "last4": "3333", "enabled": true })
    );
    let (code, _) = app
        .json(
            Method::PUT,
            "/v1/admin/integrations/serpapi",
            Some(KEY_B),
            Some(json!({ "api_key": "x" })),
        )
        .await;
    assert_eq!(code, StatusCode::CONFLICT);

    let app = TestApp::with_env(
        vec![Step::Reply(text("hi"))],
        &admin_env(&[("FLIGHTS", "off"), ("SERPAPI_API_KEY", "env-serpapi-3333")]),
    )
    .await;
    let (_, status) = app.get("/v1/admin/integrations", KEY_B).await;
    assert_eq!(status["serpapi"]["enabled"], false);
    let (_, catalog) = app.get("/v1/tools", KEY_A).await;
    assert_eq!(
        flights_group(&catalog)["reason"],
        "Turned off on the server (FLIGHTS=off)"
    );
}
