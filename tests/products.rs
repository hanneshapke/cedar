//! `search_products` end to end, against one local server that plays Brave Search, a review
//! article, two shop pages, a broken page, and Best Buy's API; the scripted model answers both
//! the run and the tool's own question.

mod common;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use common::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Mock {
    base: Mutex<String>,
    /// Brave answers nothing at all, to show the tool says so.
    empty: bool,
    /// Every Brave query, and every page read.
    searches: Mutex<Vec<String>>,
    pages: Mutex<Vec<String>>,
}

async fn brave(
    State(mock): State<Arc<Mock>>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Value> {
    assert_eq!(headers["x-subscription-token"], "brave-key");
    let q = params["q"].clone();
    mock.searches.lock().unwrap().push(q.clone());
    let base = mock.base.lock().unwrap().clone();
    if mock.empty {
        return Json(json!({ "web": { "results": [] } }));
    }
    let results = if q.ends_with(" review") {
        json!([
            { "title": "The <strong>best</strong> trail shoes of 2026", "url": format!("{base}/review/trail"),
              "description": "We ran 300 miles. The Speedgoat 6 costs $155 and grips best." },
            { "title": "Trail shoe video", "url": "https://www.youtube.com/watch?v=1" }
        ])
    } else {
        json!([
            { "title": "Trail Runner 5", "url": format!("{base}/shop/trail-5"), "description": "Free returns.",
              "thumbnail": { "src": "https://imgs.search.brave.com/tr5.jpg" } },
            { "title": "Speedgoat 6 | Outlet", "url": format!("{base}/shop/speedgoat") },
            { "title": "Broken shop", "url": format!("{base}/shop/broken") }
        ])
    };
    Json(json!({ "web": { "results": results } }))
}

const REVIEW: &str = r#"<html><head><title>The best trail shoes of 2026</title></head><body>
<h1>The best trail shoes</h1><p>Our top pick is the Trail Runner 5: grippy, cushioned, and
roomy in the toe box. The Speedgoat 6 is the pick for long runs.</p></body></html>"#;

const TRAIL_5: &str = r#"<html><head><title>Trail Runner 5</title>
<script type="application/ld+json">{"@context":"https://schema.org","@type":"Product",
 "name":"Trail Runner 5","brand":{"name":"Hoka"},"image":["https://img.example/tr5.jpg","https://img.example/tr5-side.jpg"],
 "description":"A cushioned trail shoe.",
 "offers":{"@type":"Offer","price":"129.95","priceCurrency":"USD","availability":"https://schema.org/InStock"},
 "aggregateRating":{"ratingValue":4.6,"reviewCount":1204},
 "review":[{"reviewBody":"Great grip on wet rock.","reviewRating":{"ratingValue":5}}]}</script>
</head><body><h1>Trail Runner 5</h1></body></html>"#;

const SPEEDGOAT: &str = r#"<html><head><meta property="og:type" content="product">
<meta property="og:title" content="Speedgoat 6">
<meta property="og:image" content="https://img.example/sg6.jpg">
<meta property="product:price:amount" content="155.00">
<meta property="product:price:currency" content="USD"></head><body>Speedgoat 6</body></html>"#;

async fn page(State(mock): State<Arc<Mock>>, uri: Uri) -> Response {
    mock.pages.lock().unwrap().push(uri.path().to_string());
    match uri.path() {
        "/review/trail" => Html(REVIEW).into_response(),
        "/shop/trail-5" => Html(TRAIL_5).into_response(),
        "/shop/speedgoat" => Html(SPEEDGOAT).into_response(),
        "/shop/broken" => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        // Best Buy's Products API: the filter follows `/bestbuy/products` in parentheses.
        p if p.starts_with("/bestbuy/products(") => Json(json!({ "total": 1, "products": [{
            "sku": 7, "name": "Trail Runner 5 Wide", "manufacturer": "Hoka",
            "salePrice": 139.99, "regularPrice": 159.99,
            "customerReviewAverage": 4.8, "customerReviewCount": 88,
            "url": "https://www.bestbuy.com/site/7.p", "image": "https://img.example/bb7.jpg",
            "onlineAvailability": true, "inStoreAvailability": true, "freeShipping": true }] }))
        .into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn start(empty: bool) -> (String, Arc<Mock>) {
    let mock = Arc::new(Mock {
        empty,
        ..Mock::default()
    });
    let app = Router::new()
        .route("/brave", get(brave))
        .fallback(page)
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    *mock.base.lock().unwrap() = base.clone();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, mock)
}

/// What the model answers when the tool asks it to weigh the options.
fn ranking() -> String {
    json!({
        "summary": "Get the Trail Runner 5: the best grip and a roomy toe box for the money.",
        "options": [
            { "name": "Trail Runner 5", "candidate": "C1", "sources": ["S1"], "label": "top_pick",
              "why": "Reviewers' top pick, rated 4.6 by over a thousand buyers.",
              "pros": ["grip", "cushioning"], "cons": ["runs warm"], "best_for": "muddy trails",
              "compare_with": [2] },
            { "name": "Speedgoat 6", "candidate": "C2", "sources": ["S1", "S3"], "label": "premium",
              "why": "Best for long runs.", "pros": ["light"], "cons": ["pricey"] },
            { "name": "Imaginary Shoe 9", "candidate": null, "sources": [] }
        ]
    })
    .to_string()
}

struct Outcome {
    result: Value,
    tool_request: Value,
    events: Vec<SseEvent>,
}

/// One run: the model calls `search_products` with `input`, the tool asks the model `answer`,
/// and the model then replies.
async fn run(input: Value, base: &str, answer: &str, extra: &[(&str, &str)]) -> Outcome {
    let brave = format!("{base}/brave");
    let mut env = vec![
        ("BRAVE_API_KEY", "brave-key"),
        ("BRAVE_API_BASE_URL", brave.as_str()),
        ("WEB_FETCH_ALLOW_PRIVATE", "on"),
    ];
    env.extend_from_slice(extra);
    let app = TestApp::with_env(
        vec![
            Step::Reply(tool_use("t1", "search_products", input)),
            Step::Reply(text(answer)),
            Step::Reply(text("The Trail Runner 5 is my pick.")),
        ],
        &env,
    )
    .await;
    let created = app
        .create_run(KEY_A, "which trail shoes should I buy?")
        .await;
    let id = created["id"].as_str().unwrap().to_string();
    app.wait_status(KEY_A, &id, "completed").await;
    let requests = app.llm.requests();
    let tool_result = requests.last().unwrap()["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["content"][0]
        .clone();
    assert!(tool_result.get("is_error").is_none(), "{tool_result}");
    Outcome {
        result: serde_json::from_str(tool_result["content"].as_str().unwrap()).unwrap(),
        tool_request: requests[1].clone(),
        events: app.events(KEY_A, &id, None).await,
    }
}

#[tokio::test]
async fn the_web_is_searched_the_pages_read_and_the_model_picks() {
    let (base, mock) = start(false).await;
    let out = run(
        json!({ "query": "trail running shoes", "needs": "wide feet", "max_price": 200, "count": 3 }),
        &base,
        &ranking(),
        &[],
    )
    .await;

    assert_eq!(
        *mock.searches.lock().unwrap(),
        [
            "best trail running shoes under 200 review",
            "trail running shoes under 200 buy online"
        ]
    );
    let mut pages = mock.pages.lock().unwrap().clone();
    pages.sort();
    assert_eq!(
        pages,
        [
            "/review/trail",
            "/shop/broken",
            "/shop/speedgoat",
            "/shop/trail-5"
        ],
        "the video is skipped; a broken page doesn't stop the rest"
    );

    // The tool's own question: no tools, the evidence, and the instructions.
    let req = &out.tool_request;
    assert_eq!(req["tools"], json!([]));
    assert!(req["system"]
        .as_str()
        .unwrap()
        .starts_with("You are a careful, independent shopping advisor."));
    let prompt = req["messages"][0]["content"].as_str().unwrap();
    assert!(
        prompt.contains("What matters to them: wide feet"),
        "{prompt}"
    );
    assert!(prompt.contains(
        "[C1] Trail Runner 5 | brand: Hoka | price: 129.95 USD | rating: 4.6/5 from 1,204 reviews | in stock"
    ), "{prompt}");
    assert!(
        prompt.contains("[C2] Speedgoat 6 | price: 155.00 USD"),
        "{prompt}"
    );
    assert!(
        prompt.contains("\"5/5: Great grip on wet rock.\""),
        "{prompt}"
    );
    assert!(
        prompt.contains("roomy in the toe box"),
        "the article's text: {prompt}"
    );
    assert!(prompt.contains("Pick the 3 best options"));

    let r = &out.result;
    // For looking at the card by hand: CEDAR_PRODUCTS_SAMPLE=path writes the result there.
    if let Ok(path) = std::env::var("CEDAR_PRODUCTS_SAMPLE") {
        std::fs::write(path, r.to_string()).unwrap();
    }
    assert_eq!(r["method"], "model");
    assert_eq!(r["store"], "the web");
    assert_eq!(
        r["summary"],
        "Get the Trail Runner 5: the best grip and a roomy toe box for the money."
    );
    assert_eq!(r["considered"]["pages"], 3);
    assert_eq!(r["considered"]["products"], 2);
    let names: Vec<&str> = r["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["Trail Runner 5", "Speedgoat 6"],
        "a product the evidence doesn't hold is dropped"
    );
    let first = &r["options"][0];
    assert_eq!(first["sku"], "P1");
    assert_eq!(first["url"], format!("{base}/shop/trail-5"));
    assert_eq!(
        first["images"],
        json!([
            "https://img.example/tr5.jpg",
            "https://img.example/tr5-side.jpg"
        ])
    );
    assert_eq!(first["price"], 129.95);
    assert_eq!(first["price_checked"], true);
    assert_eq!(first["availability"], json!({ "online": true }));
    assert_eq!(first["reviews"]["verdict"], "excellent");
    assert_eq!(first["badges"], json!(["Top pick", "Best rated"]));
    assert_eq!(
        first["why"],
        "Reviewers' top pick, rated 4.6 by over a thousand buyers."
    );
    assert_eq!(first["pros"], json!(["grip", "cushioning"]));
    assert_eq!(first["best_for"], "muddy trails");
    assert_eq!(first["comparable"], json!(["P2"]));
    let goat = &r["options"][1];
    assert_eq!(goat["images"], json!(["https://img.example/sg6.jpg"]));
    assert_eq!(goat["badges"], json!(["Premium pick"]));
    assert_eq!(goat["reviews"]["verdict"], "none");
    assert!(
        goat.get("availability").is_none(),
        "unknown stock isn't shown as out of stock"
    );
    let cited: Vec<&str> = r["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["host"].as_str().unwrap())
        .collect();
    assert_eq!(cited.len(), 3);

    // The tool's question counts toward the run's tokens.
    let tool_usage: Vec<&Value> = out
        .events
        .iter()
        .filter(|e| e.event == "usage" && e.data["data"]["purpose"] == "tool")
        .map(|e| &e.data["data"])
        .collect();
    assert_eq!(
        tool_usage.len(),
        1,
        "{:?}",
        out.events.iter().map(|e| &e.event).collect::<Vec<_>>()
    );
    assert_eq!(tool_usage[0]["tool"], "search_products");
    assert_eq!(tool_usage[0]["turn"], 1);
}

#[tokio::test]
async fn best_buy_joins_the_search_when_it_has_a_key() {
    let (base, _mock) = start(false).await;
    let bestbuy = format!("{base}/bestbuy/products");
    let answer = json!({ "summary": "The wide version.", "options": [
        { "name": "Trail Runner 5 Wide", "candidate": "C3", "label": "top_pick" }
    ] })
    .to_string();
    let out = run(
        json!({ "query": "trail running shoes" }),
        &base,
        &answer,
        &[
            ("BESTBUY_API_KEY", "bb-key"),
            ("PRODUCTS_API_URL", bestbuy.as_str()),
        ],
    )
    .await;
    let prompt = out.tool_request["messages"][0]["content"].as_str().unwrap();
    assert!(
        prompt.contains("[C3] Trail Runner 5 Wide | brand: Hoka | price: 139.99 USD (was 159.99)"),
        "{prompt}"
    );
    let only = &out.result["options"][0];
    assert_eq!(only["store"], "Best Buy");
    assert_eq!(only["savings_pct"], 12.5);
    assert_eq!(
        only["availability"],
        json!({ "online": true, "in_store": true, "free_shipping": true })
    );
}

#[tokio::test]
async fn when_the_model_cant_weigh_them_the_products_are_ranked_by_ratings() {
    let (base, _mock) = start(false).await;
    let out = run(
        json!({ "query": "trail running shoes" }),
        &base,
        "I'm not sure, sorry.",
        &[],
    )
    .await;
    let r = &out.result;
    assert_eq!(r["method"], "ratings");
    assert!(r.get("summary").is_none());
    assert!(r["note"]
        .as_str()
        .unwrap()
        .starts_with("Ranked by customer ratings, because the options could not be weighed:"));
    assert_eq!(r["options"][0]["name"], "Trail Runner 5");
    assert_eq!(r["options"][0]["badges"], json!(["Best rated"]));
}

#[tokio::test]
async fn a_search_that_finds_nothing_is_an_error_the_model_can_act_on() {
    let (base, _mock) = start(true).await;
    let brave = format!("{base}/brave");
    let app = TestApp::with_env(
        vec![
            Step::Reply(tool_use(
                "t1",
                "search_products",
                json!({ "query": "flux capacitor" }),
            )),
            Step::Reply(text("Nothing found.")),
        ],
        &[
            ("BRAVE_API_KEY", "brave-key"),
            ("BRAVE_API_BASE_URL", brave.as_str()),
            ("WEB_FETCH_ALLOW_PRIVATE", "on"),
        ],
    )
    .await;
    let created = app.create_run(KEY_A, "buy one").await;
    let id = created["id"].as_str().unwrap().to_string();
    app.wait_status(KEY_A, &id, "completed").await;
    let result = app.llm.requests()[1]["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["content"][0]
        .clone();
    assert_eq!(result["is_error"], true, "{result}");
    assert!(result["content"]
        .as_str()
        .unwrap()
        .contains("no products found for \"flux capacitor\""));
    assert_eq!(
        app.llm.call_count(),
        2,
        "the model isn't asked to weigh nothing"
    );
}

#[tokio::test]
async fn the_tool_needs_a_search_key() {
    let app = TestApp::with_env(vec![Step::Repeat(text("ok"))], &[]).await;
    let (status, tools) = app.get("/v1/tools", KEY_A).await;
    assert_eq!(status, StatusCode::OK);
    let group = tools["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == "products")
        .unwrap()
        .clone();
    assert_eq!(group["label"], "Shopping");
    assert_eq!(group["available"], false);
    assert_eq!(group["reason"], "Needs a Brave Search or Best Buy API key");

    let app = TestApp::with_env(vec![Step::Repeat(text("ok"))], &[("BRAVE_API_KEY", "k")]).await;
    let (_, tools) = app.get("/v1/tools", KEY_A).await;
    let group = tools["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == "products")
        .unwrap()
        .clone();
    assert_eq!(group["available"], true);
    assert_eq!(group["tools"][0]["name"], "search_products");
}
