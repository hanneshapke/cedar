//! The `search_products` tool against a local mock of Best Buy's Products API.

mod common;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::{Json, Router};
use common::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

struct Mock {
    /// What to answer: the catalog, or a status to fail with.
    answer: Answer,
    /// Fail with 429 this many times before answering.
    throttle: AtomicUsize,
    /// Every request's decoded path and query, in order.
    seen: Mutex<Vec<(String, HashMap<String, String>)>>,
}

#[derive(Clone)]
enum Answer {
    Catalog,
    Empty,
    Status(StatusCode, Value),
}

/// A product as the API lists it. `rank` doubles as the sku.
fn product(
    rank: u64,
    name: &str,
    price: f64,
    regular: f64,
    avg: Option<f64>,
    count: u64,
    category: &str,
) -> Value {
    json!({
        "sku": rank, "name": name, "manufacturer": name.split(' ').next().unwrap(),
        "modelNumber": format!("MDL-{rank}"), "color": "Black", "condition": "New",
        "shortDescription": format!("{name}, in short."),
        "url": format!("https://www.bestbuy.com/site/{rank}.p"),
        "image": format!("https://pisces.example/{rank}.jpg;maxHeight=640"),
        "images": [
            { "href": format!("https://pisces.example/{rank}-alt.jpg"), "rel": "Alternate", "primary": false },
            { "href": format!("https://pisces.example/{rank}-front.jpg"), "rel": "Large", "primary": true },
        ],
        "salePrice": price, "regularPrice": regular,
        "customerReviewAverage": avg, "customerReviewCount": count,
        "onlineAvailability": true, "inStoreAvailability": rank.is_multiple_of(2), "freeShipping": true,
        "features": [{ "feature": "Active noise cancelling" }, { "feature": "30-hour battery" }],
        "details": [{ "name": "Weight", "value": "250 grams" }, { "name": "Bluetooth", "value": "5.3" }],
        "categoryPath": [{ "id": "cat00000", "name": "Best Buy" }, { "id": "abcat0200000", "name": "Audio" }, { "id": "pcmcat", "name": category }],
        "class": "HEADPHONES", "bestSellingRank": rank,
    })
}

fn catalog() -> Vec<Value> {
    vec![
        product(
            1,
            "Sony WH-1000XM5",
            329.99,
            399.99,
            Some(4.7),
            2345,
            "Over-Ear Headphones",
        ),
        product(
            2,
            "Bose QuietComfort Ultra",
            299.0,
            429.0,
            Some(4.6),
            980,
            "Over-Ear Headphones",
        ),
        product(
            3,
            "Soundcore Space One",
            79.99,
            99.99,
            Some(4.5),
            410,
            "Over-Ear Headphones",
        ),
        product(
            4,
            "JBL Tune 770NC",
            69.99,
            69.99,
            Some(5.0),
            3,
            "Over-Ear Headphones",
        ),
        product(
            5,
            "Apple AirPods Pro",
            199.0,
            249.0,
            Some(4.8),
            12000,
            "Earbuds",
        ),
        product(6, "Nothing Ear", 149.0, 149.0, None, 0, "Earbuds"),
    ]
}

async fn products_route(
    State(mock): State<Arc<Mock>>,
    uri: Uri,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    let agent = headers["user-agent"].to_str().unwrap();
    assert!(agent.starts_with("cedar/"), "{agent}");
    // The URL library encodes `<` and `>` in the path; the API decodes them.
    let path = uri.path().replace("%3C", "<").replace("%3E", ">");
    mock.seen.lock().unwrap().push((path, params));
    if mock.throttle.load(Ordering::SeqCst) > 0 {
        mock.throttle.fetch_sub(1, Ordering::SeqCst);
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": { "message": "slow down" } })),
        );
    }
    let products = match &mock.answer {
        Answer::Catalog => catalog(),
        Answer::Empty => vec![],
        Answer::Status(status, body) => return (*status, Json(body.clone())),
    };
    let total = products.len();
    (
        StatusCode::OK,
        Json(
            json!({ "from": 1, "to": total, "currentPage": 1, "total": total, "totalPages": 1,
                     "queryTime": "0.01", "totalTime": "0.02", "partial": false,
                     "canonicalUrl": uri.to_string(), "products": products }),
        ),
    )
}

async fn start(answer: Answer, throttle: usize) -> (String, Arc<Mock>) {
    let mock = Arc::new(Mock {
        answer,
        throttle: AtomicUsize::new(throttle),
        seen: Mutex::new(vec![]),
    });
    // The filter follows `/products` in parentheses, not under a slash, so no route matches it.
    let app = Router::new()
        .fallback(products_route)
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, mock)
}

/// Run one `search_products` call with `input` and return its tool result.
async fn call(input: Value, base: &str) -> Value {
    let api = format!("{base}/products");
    let app = TestApp::with_env(
        vec![
            Step::Reply(tool_use("t1", "search_products", input)),
            Step::Reply(text("I'd take the Sony.")),
        ],
        &[
            ("BESTBUY_API_KEY", "test-key"),
            ("PRODUCTS_API_URL", api.as_str()),
        ],
    )
    .await;
    let created = app.create_run(KEY_A, "which headphones?").await;
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

fn skus(list: &Value) -> Vec<u64> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|p| p["sku"].as_u64().unwrap())
        .collect()
}

#[tokio::test]
async fn products_are_searched_rated_and_given_comparable_options() {
    let (base, mock) = start(Answer::Catalog, 0).await;
    let r = report(
        &call(
            json!({ "query": "Best noise cancelling headphones under $350", "count": 3 }),
            &base,
        )
        .await,
    );

    let seen = mock.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    let (path, params) = &seen[0];
    assert_eq!(
        path,
        "/products((search=noise&search=cancelling&search=headphones)&(salePrice<=350)&(onlineAvailability=true))"
    );
    assert_eq!(params["apiKey"], "test-key");
    assert_eq!(params["format"], "json");
    assert_eq!(params["sort"], "bestSellingRank.asc");
    assert_eq!(params["pageSize"], "24");
    assert!(
        params["show"].contains("customerReviewAverage"),
        "{}",
        params["show"]
    );

    assert_eq!(r["query"], "Best noise cancelling headphones under $350");
    assert_eq!(r["store"], "Best Buy");
    assert_eq!(r["currency"], "USD");
    assert_eq!(r["total"], 6);
    assert_eq!(r["sort"], "best_selling");
    assert_eq!(
        r["filters"],
        json!({ "max_price": 350.0, "in_stock": true })
    );
    assert!(r["as_of"].as_str().unwrap().ends_with('Z'));
    assert!(r["disclaimer"].as_str().unwrap().contains("Best Buy"));

    assert_eq!(
        skus(&r["options"]),
        [1, 2, 3],
        "the first `count` in the store's order"
    );
    assert_eq!(skus(&r["others"]), [4, 5, 6]);

    let sony = &r["options"][0];
    assert_eq!(sony["name"], "Sony WH-1000XM5");
    assert_eq!(sony["brand"], "Sony");
    assert_eq!(sony["model"], "MDL-1");
    assert_eq!(sony["price"], 329.99);
    assert_eq!(sony["regular_price"], 399.99);
    assert_eq!(sony["savings_pct"], 17.5);
    assert_eq!(sony["url"], "https://www.bestbuy.com/site/1.p");
    assert_eq!(
        sony["images"],
        json!([
            "https://pisces.example/1.jpg;maxHeight=640",
            "https://pisces.example/1-front.jpg",
            "https://pisces.example/1-alt.jpg"
        ])
    );
    assert_eq!(sony["category"], "Over-Ear Headphones");
    assert_eq!(sony["categories"], json!(["Audio", "Over-Ear Headphones"]));
    assert_eq!(
        sony["availability"],
        json!({ "online": true, "in_store": false, "free_shipping": true })
    );
    assert_eq!(
        sony["features"],
        json!(["Active noise cancelling", "30-hour battery"])
    );
    assert_eq!(
        sony["specs"][0],
        json!({ "name": "Weight", "value": "250 grams" })
    );
    assert_eq!(sony["rank"], 1);
    let reviews = &sony["reviews"];
    assert_eq!(reviews["average"], 4.7);
    assert_eq!(reviews["count"], 2345);
    assert_eq!(reviews["adjusted"], 4.7);
    assert_eq!(reviews["verdict"], "excellent");
    assert_eq!(reviews["confidence"], "high");
    assert!(reviews["summary"]
        .as_str()
        .unwrap()
        .starts_with("4.7 stars from 2,345 reviews"));
    // Comparable to the Sony: the other over-ear sets (the Bose nearest in price), then the rest
    // of the class by price.
    assert_eq!(sony["comparable"], json!([2, 3, 4, 5]));
    assert_eq!(r["options"][2]["comparable"], json!([4, 2, 1, 6]));

    // The others carry the essentials only, with one picture.
    let jbl = &r["others"][0];
    assert_eq!(jbl["image"], "https://pisces.example/4.jpg;maxHeight=640");
    assert!(jbl.get("images").is_none() && jbl.get("features").is_none());
    assert_eq!(jbl["reviews"]["verdict"], "few");
    assert_eq!(r["others"][2]["reviews"]["verdict"], "none");
    assert_eq!(
        r["others"][2].get("savings_pct"),
        None,
        "no sale, no savings"
    );

    assert_eq!(
        r["picks"],
        json!({ "best_rated": 1, "best_value": 3, "most_popular": 1 }),
        "picks are among the options shown"
    );
}

#[tokio::test]
async fn filters_and_the_order_are_passed_on() {
    let (base, mock) = start(Answer::Catalog, 0).await;
    let r = report(
        &call(
            json!({ "query": "earbuds", "min_price": 100, "max_price": 250, "min_rating": 4.5,
                    "in_stock": false, "sort": "price_low", "count": 8 }),
            &base,
        )
        .await,
    );
    let seen = mock.seen.lock().unwrap();
    let (path, params) = &seen[0];
    assert_eq!(
        path,
        "/products((search=earbuds)&(salePrice>=100)&(salePrice<=250)&(customerReviewAverage>=4.5))"
    );
    assert_eq!(params["sort"], "salePrice.asc");
    assert_eq!(r["sort"], "price_low");
    assert_eq!(
        r["filters"],
        json!({ "min_price": 100.0, "max_price": 250.0, "min_rating": 4.5, "in_stock": false })
    );
    assert_eq!(
        skus(&r["options"]).len(),
        6,
        "every product fits in eight options"
    );
    assert_eq!(r["others"], json!([]));
}

#[tokio::test]
async fn a_search_that_finds_nothing_is_an_error_the_model_can_act_on() {
    let (base, _mock) = start(Answer::Empty, 0).await;
    let result = call(json!({ "query": "flux capacitor" }), &base).await;
    assert_eq!(result["is_error"], true, "{result}");
    let message = result["content"].as_str().unwrap();
    assert!(
        message.contains("no products found for \"flux capacitor\""),
        "{message}"
    );
}

#[tokio::test]
async fn a_rejected_key_and_a_bad_query_are_reported() {
    let (base, _mock) = start(
        Answer::Status(
            StatusCode::FORBIDDEN,
            json!({ "error": { "message": "Invalid API key" } }),
        ),
        0,
    )
    .await;
    let result = call(json!({ "query": "tv" }), &base).await;
    assert_eq!(result["is_error"], true, "{result}");
    assert!(result["content"]
        .as_str()
        .unwrap()
        .contains("rejected the API key (403"));

    let (base, _mock) = start(
        Answer::Status(
            StatusCode::BAD_REQUEST,
            json!({ "error": { "message": "Unknown attribute" } }),
        ),
        0,
    )
    .await;
    let result = call(json!({ "query": "tv" }), &base).await;
    assert_eq!(result["is_error"], true, "{result}");
    assert_eq!(
        result["content"],
        "Best Buy rejected the search: Unknown attribute"
    );
}

#[tokio::test]
async fn a_rate_limited_search_is_retried_once() {
    let (base, mock) = start(Answer::Catalog, 1).await;
    let r = report(&call(json!({ "query": "headphones", "count": 2 }), &base).await);
    assert_eq!(mock.seen.lock().unwrap().len(), 2, "one retry after a 429");
    assert_eq!(skus(&r["options"]), [1, 2]);

    let (base, mock) = start(Answer::Catalog, 2).await;
    let result = call(json!({ "query": "headphones" }), &base).await;
    assert_eq!(mock.seen.lock().unwrap().len(), 2, "but only once");
    assert_eq!(result["is_error"], true, "{result}");
    assert!(result["content"].as_str().unwrap().contains("rate limited"));
}

#[tokio::test]
async fn the_tool_is_only_offered_with_a_key() {
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
    assert_eq!(
        group["reason"],
        "Needs a Best Buy API key (BESTBUY_API_KEY)"
    );
    assert_eq!(group["tools"], json!([]));

    let app = TestApp::with_env(vec![Step::Repeat(text("ok"))], &[("BESTBUY_API_KEY", "k")]).await;
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
    assert_eq!(group["tools"][0]["writes"], false);
}
