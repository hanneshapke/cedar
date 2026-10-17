//! The `compare_stocks` tool against local mocks of Yahoo Finance's chart and search APIs.

mod common;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use common::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The last bar: 2026-10-16 20:00 UTC, 16:00 in New York.
const LAST: i64 = 1_792_180_800;
const DAY: i64 = 86_400;

struct Mock {
    /// Daily growth in percent per known symbol; anything else is an unknown symbol.
    growth: HashMap<&'static str, f64>,
    /// Make the news search fail, to show the prices still come back.
    news_broken: bool,
    seen: Mutex<Vec<String>>,
}

/// A year and a bit of daily bars growing by `daily` percent, zig-zagging a little.
fn chart(symbol: &str, daily: f64) -> Value {
    let n = 300;
    let stamps: Vec<i64> = (0..n).map(|i| LAST - (n - 1 - i) * DAY).collect();
    let closes: Vec<f64> = (0..n)
        .map(|i| {
            let wiggle = if i % 2 == 0 { 1.0 } else { 1.004 };
            (100.0 * (1.0 + daily / 100.0).powi(i as i32) * wiggle * 100.0).round() / 100.0
        })
        .collect();
    let last = *closes.last().unwrap();
    let kind = if symbol.starts_with('^') {
        "INDEX"
    } else {
        "EQUITY"
    };
    json!({ "chart": { "error": null, "result": [{
        "meta": { "symbol": symbol, "longName": format!("{symbol} Corp"), "currency": "USD",
                  "fullExchangeName": "NasdaqGS", "instrumentType": kind,
                  "regularMarketPrice": last, "previousClose": closes[n as usize - 2],
                  "regularMarketTime": LAST, "gmtoffset": -14400 },
        "timestamp": stamps,
        "indicators": { "quote": [{ "close": closes }], "adjclose": [{ "adjclose": closes }] }
    }] } })
}

async fn chart_route(
    State(mock): State<Arc<Mock>>,
    Path(symbol): Path<String>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    let agent = headers["user-agent"].to_str().unwrap();
    assert!(agent.contains("cedar/"), "{agent}");
    assert_eq!(params["interval"], "1d");
    mock.seen
        .lock()
        .unwrap()
        .push(format!("chart {symbol} {}", params["range"]));
    match mock.growth.get(symbol.as_str()) {
        Some(g) => (StatusCode::OK, Json(chart(&symbol, *g))),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "chart": { "result": null, "error": {
                "code": "Not Found", "description": "No data found, symbol may be delisted" } } })),
        ),
    }
}

async fn search_route(
    State(mock): State<Arc<Mock>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let q = params["q"].clone();
    mock.seen
        .lock()
        .unwrap()
        .push(format!("news {q} {}", params["newsCount"]));
    if mock.news_broken {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    Ok(Json(json!({ "news": [
        { "title": format!("{q} beats estimates"), "publisher": "Reuters",
          "link": format!("https://news.example/{q}"), "providerPublishTime": LAST - 3600 * (q.len() as i64),
          "thumbnail": { "resolutions": [{ "url": format!("https://img.example/{q}.jpg"), "width": 140, "height": 140 }] } },
        { "title": "Chip stocks rally", "publisher": "AP",
          "link": "https://news.example/chips", "providerPublishTime": LAST - 86_400 },
    ] })))
}

async fn start(news_broken: bool) -> (String, Arc<Mock>) {
    let mock = Arc::new(Mock {
        growth: HashMap::from([("NVDA", 0.25), ("INTC", -0.2), ("^GSPC", 0.05)]),
        news_broken,
        seen: Mutex::new(vec![]),
    });
    let app = Router::new()
        .route("/chart/{symbol}", get(chart_route))
        .route("/search", get(search_route))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, mock)
}

/// Run one `compare_stocks` call with `input` and return its tool result.
async fn call(input: Value, base: &str) -> Value {
    let chart = format!("{base}/chart");
    let search = format!("{base}/search");
    let app = TestApp::with_env(
        vec![
            Step::Reply(tool_use("t1", "compare_stocks", input)),
            Step::Reply(text("Here is my take.")),
        ],
        &[
            ("STOCKS_CHART_URL", chart.as_str()),
            ("STOCKS_NEWS_URL", search.as_str()),
        ],
    )
    .await;
    let created = app.create_run(KEY_A, "should I buy?").await;
    let id = created["id"].as_str().unwrap().to_string();
    app.wait_status(KEY_A, &id, "completed").await;
    app.llm.requests()[1]["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["content"][0]
        .clone()
}

fn report(result: &Value) -> Value {
    assert!(result.get("is_error").is_none(), "{result}");
    serde_json::from_str(result["content"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn stocks_are_compared_with_the_benchmark_rated_and_given_news() {
    let (base, mock) = start(false).await;
    let r = report(
        &call(
            json!({ "symbols": ["nvda", "INTC"], "range": "6mo" }),
            &base,
        )
        .await,
    );

    let mut seen = mock.seen.lock().unwrap().clone();
    seen.sort();
    assert_eq!(
        seen,
        [
            "chart INTC 1y",
            "chart NVDA 1y",
            "chart ^GSPC 1y",
            "news INTC 4",
            "news NVDA 4"
        ],
        "a year is fetched for the rating, the benchmark has no news"
    );

    assert_eq!(r["range"], "6mo");
    assert_eq!(
        r["benchmark"],
        json!({ "symbol": "^GSPC", "name": "^GSPC Corp", "in_symbols": false })
    );
    assert_eq!(r["benchmark_series"]["symbol"], "^GSPC");
    assert_eq!(r["as_of"], "2026-10-16T16:00:00-04:00");
    assert!(r["disclaimer"]
        .as_str()
        .unwrap()
        .contains("not financial advice"));

    let nvda = &r["symbols"][0];
    assert_eq!(nvda["symbol"], "NVDA");
    assert_eq!(nvda["chart"]["dates"].as_array().unwrap().len(), 300);
    assert_eq!(nvda["chart"]["dates"][299], "2026-10-16");
    assert!(nvda["change_pct"].as_f64().is_some());
    assert!(
        nvda["returns"]["1y"].as_f64().unwrap() > 50.0,
        "{}",
        nvda["returns"]
    );
    let rating = &nvda["rating"];
    assert_eq!(rating["call"], "buy", "{rating}");
    let factors: Vec<&str> = rating["factors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert!(factors.contains(&"vs. ^GSPC Corp"), "{factors:?}");
    assert!(rating["factors"][0]["detail"]
        .as_str()
        .unwrap()
        .contains("200-day average"));
    assert!(!rating["watch"].as_array().unwrap().is_empty());

    let intc = &r["symbols"][1];
    assert_eq!(intc["rating"]["call"], "sell", "{}", intc["rating"]);

    // The article both searches found is listed once, for both symbols; newest first.
    let news = r["news"].as_array().unwrap();
    assert_eq!(news.len(), 3, "{news:?}");
    assert_eq!(news[0]["title"], "NVDA beats estimates");
    assert_eq!(news[0]["thumbnail"], "https://img.example/NVDA.jpg");
    assert_eq!(news[2]["symbols"], json!(["NVDA", "INTC"]));
}

#[tokio::test]
async fn an_unknown_symbol_is_listed_and_broken_news_does_not_stop_the_prices() {
    let (base, _) = start(true).await;
    let r = report(
        &call(
            json!({ "symbols": ["NVDA", "NOPE"], "benchmark": "none", "news_count": 2 }),
            &base,
        )
        .await,
    );
    assert!(r.get("benchmark").is_none());
    assert_eq!(r["symbols"].as_array().unwrap().len(), 1);
    assert_eq!(
        r["unavailable"],
        json!([{ "symbol": "NOPE", "reason": "No data found, symbol may be delisted" }])
    );
    assert_eq!(r["news"], json!([]));
    assert!(r["news_note"].as_str().unwrap().contains("503"));
    let factors = r["symbols"][0]["rating"]["factors"].as_array().unwrap();
    assert!(factors
        .iter()
        .all(|f| !f["name"].as_str().unwrap().starts_with("vs. ")));
}

#[tokio::test]
async fn no_prices_at_all_is_an_error_for_the_model() {
    let (base, _) = start(false).await;
    let result = call(json!({ "symbols": ["NOPE"] }), &base).await;
    assert_eq!(result["is_error"], true);
    let content = result["content"].as_str().unwrap();
    assert!(content.starts_with("no prices found (NOPE: "), "{content}");
    let result = call(json!({ "symbols": ["DROP TABLE"] }), &base).await;
    assert_eq!(result["is_error"], true);
    assert!(result["content"]
        .as_str()
        .unwrap()
        .contains("is not a ticker symbol"));
}

#[tokio::test]
async fn with_stocks_off_the_tool_is_not_offered() {
    let app = TestApp::with_env(vec![Step::Reply(text("hi"))], &[("STOCKS", "off")]).await;
    let created = app.create_run(KEY_A, "hi").await;
    app.wait_status(KEY_A, created["id"].as_str().unwrap(), "completed")
        .await;
    let request = &app.llm.requests()[0];
    let names: Vec<_> = request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(!names.contains(&"compare_stocks"), "{names:?}");
    assert!(!request["system"]
        .as_str()
        .unwrap()
        .contains("compare_stocks"));
}
