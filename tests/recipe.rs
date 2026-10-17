//! The `show_recipe` tool against local mocks of the Wikipedia and Wikimedia Commons APIs.

mod common;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use common::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

type Seen = Mutex<Vec<(String, HashMap<String, String>)>>;

struct Mock {
    /// Answers by `gsrsearch`; anything else finds nothing.
    wikipedia: HashMap<&'static str, Value>,
    commons: HashMap<&'static str, Value>,
    /// Make Wikipedia fail, to show the recipe still comes back.
    broken: bool,
    seen: Seen,
}

fn answer(mock: &Mock, api: &str, headers: &HeaderMap, params: HashMap<String, String>) -> Value {
    let agent = headers["user-agent"].to_str().unwrap();
    assert!(agent.starts_with("cedar/"), "{agent}");
    let answers = if api == "wikipedia" {
        &mock.wikipedia
    } else {
        &mock.commons
    };
    let found = answers.get(params["gsrsearch"].as_str()).cloned();
    mock.seen.lock().unwrap().push((api.to_string(), params));
    // MediaWiki answers without `query` when a search finds nothing.
    found.unwrap_or_else(|| json!({ "batchcomplete": true }))
}

async fn wikipedia(
    State(mock): State<Arc<Mock>>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    if mock.broken {
        mock.seen.lock().unwrap().push(("wikipedia".into(), params));
        return Err(StatusCode::BAD_GATEWAY);
    }
    Ok(Json(answer(&mock, "wikipedia", &headers, params)))
}

async fn commons(
    State(mock): State<Arc<Mock>>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Value> {
    Json(answer(&mock, "commons", &headers, params))
}

async fn start(broken: bool) -> (String, Arc<Mock>) {
    let mock = Arc::new(Mock {
        wikipedia: HashMap::from([(
            "Spaghetti carbonara",
            json!({ "query": { "pages": [{
                "title": "Carbonara", "index": 1,
                "fullurl": "https://en.wikipedia.org/wiki/Carbonara",
                "thumbnail": { "source": "https://upload.wikimedia.org/carbonara.jpg", "width": 960, "height": 640 }
            }] } }),
        )]),
        commons: HashMap::from([(
            "filetype:bitmap Shakshuka",
            json!({ "query": { "pages": [{
                "title": "File:Shakshuka in a pan.jpg", "index": 1,
                "imageinfo": [{
                    "url": "https://upload.wikimedia.org/full.jpg",
                    "thumburl": "https://upload.wikimedia.org/960px-shakshuka.jpg",
                    "descriptionurl": "https://commons.wikimedia.org/wiki/File:Shakshuka_in_a_pan.jpg"
                }]
            }] } }),
        )]),
        broken,
        seen: Mutex::new(vec![]),
    });
    let app = Router::new()
        .route("/wiki", get(wikipedia))
        .route("/commons", get(commons))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, mock)
}

/// Run one `show_recipe` call with `input` and return its tool result.
async fn call(input: Value, base: &str) -> Value {
    let wiki = format!("{base}/wiki");
    let commons = format!("{base}/commons");
    let app = TestApp::with_env(
        vec![
            Step::Reply(tool_use("t1", "show_recipe", input)),
            Step::Reply(text("Enjoy!")),
        ],
        &[
            ("RECIPE_WIKIPEDIA_URL", wiki.as_str()),
            ("RECIPE_COMMONS_URL", commons.as_str()),
        ],
    )
    .await;
    let created = app.create_run(KEY_A, "how do I make it?").await;
    let id = created["id"].as_str().unwrap().to_string();
    app.wait_status(KEY_A, &id, "completed").await;
    app.llm.requests()[1]["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["content"][0]
        .clone()
}

fn recipe(result: &Value) -> Value {
    assert!(result.get("is_error").is_none(), "{result}");
    serde_json::from_str(result["content"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn the_photo_comes_from_the_wikipedia_article() {
    let (base, mock) = start(false).await;
    let result = call(
        json!({
            "title": "Nonna's carbonara",
            "image_query": "Spaghetti carbonara",
            "servings": 2,
            "ingredients": [{ "quantity": "200 g", "item": "spaghetti" }, "pecorino"],
            "steps": [{ "title": "Boil", "text": "Cook the pasta.", "duration": "9 min" }, "Toss."]
        }),
        &base,
    )
    .await;

    let seen = mock.seen.lock().unwrap();
    assert_eq!(
        seen.len(),
        1,
        "Commons is not asked when Wikipedia has a photo"
    );
    let (api, params) = &seen[0];
    assert_eq!(api, "wikipedia");
    assert_eq!(params["gsrsearch"], "Spaghetti carbonara");
    assert_eq!(params["gsrnamespace"], "0");
    assert_eq!(params["prop"], "pageimages|info");
    assert_eq!(params["pithumbsize"], "960");

    assert_eq!(
        recipe(&result),
        json!({
            "title": "Nonna's carbonara",
            "servings": "2",
            "ingredients": [{ "quantity": "200 g", "item": "spaghetti" }, { "item": "pecorino" }],
            "steps": [{ "title": "Boil", "text": "Cook the pasta.", "duration": "9 min" }, { "text": "Toss." }],
            "image": {
                "url": "https://upload.wikimedia.org/carbonara.jpg",
                "page_url": "https://en.wikipedia.org/wiki/Carbonara",
                "title": "Carbonara",
                "source": "Wikipedia"
            }
        })
    );
}

#[tokio::test]
async fn without_an_article_photo_commons_is_searched() {
    let (base, mock) = start(false).await;
    let result = call(
        json!({ "title": "Shakshuka", "ingredients": ["eggs"], "steps": ["Poach the eggs."] }),
        &base,
    )
    .await;

    let seen = mock.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1].0, "commons");
    assert_eq!(seen[1].1["gsrnamespace"], "6");
    assert_eq!(seen[1].1["iiurlwidth"], "960");
    assert_eq!(
        recipe(&result)["image"],
        json!({
            "url": "https://upload.wikimedia.org/960px-shakshuka.jpg",
            "page_url": "https://commons.wikimedia.org/wiki/File:Shakshuka_in_a_pan.jpg",
            "title": "Shakshuka in a pan.jpg",
            "source": "Wikimedia Commons"
        })
    );
}

#[tokio::test]
async fn a_recipe_without_a_photo_is_still_shown() {
    let (base, _) = start(false).await;
    let result = call(
        json!({ "title": "Mystery stew", "ingredients": ["love"], "steps": ["Simmer."] }),
        &base,
    )
    .await;
    let r = recipe(&result);
    assert!(r.get("image").is_none(), "{r}");
    assert_eq!(r["image_note"], "no photo found for \"Mystery stew\"");

    let (base, _) = start(true).await;
    let result = call(
        json!({ "title": "Spaghetti carbonara", "ingredients": ["pasta"], "steps": ["Cook."] }),
        &base,
    )
    .await;
    let r = recipe(&result);
    assert!(r.get("image").is_none(), "{r}");
    let note = r["image_note"].as_str().unwrap();
    assert!(
        note.contains("could not be looked up") && note.contains("502"),
        "{note}"
    );
    assert_eq!(r["steps"], json!([{ "text": "Cook." }]));
}

#[tokio::test]
async fn bad_input_is_an_error_for_the_model() {
    let (base, mock) = start(false).await;
    let result = call(
        json!({ "title": "Toast", "ingredients": ["bread"], "steps": [{ "duration": "2 min" }] }),
        &base,
    )
    .await;
    assert_eq!(result["is_error"], true);
    assert_eq!(result["content"], "step 1: give the step's text");
    assert!(
        mock.seen.lock().unwrap().is_empty(),
        "no photo is looked up for a bad recipe"
    );
}

#[tokio::test]
async fn with_recipes_off_the_tool_is_not_offered() {
    let app = TestApp::with_env(vec![Step::Reply(text("hi"))], &[("RECIPES", "off")]).await;
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
    assert!(!names.contains(&"show_recipe"), "{names:?}");
    assert!(!request["system"].as_str().unwrap().contains("show_recipe"));
}
