//! Per-run tool selection, the tool catalog, and integrations set up through the admin API (the
//! model registry, a Brave Search key, "Connect Google") against local mocks of model servers,
//! Brave and Google.

mod common;

use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use base64::Engine;
use cedar::store::Store;
use common::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const STANDARD: [&str; 7] = [
    "bash",
    "read_file",
    "write_file",
    "run_python",
    "read_document",
    "generate_pdf",
    "generate_chart",
];
const SECRET_KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn tool_names(request: &Value) -> Vec<String> {
    request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

fn names(extra: &[&str]) -> Vec<String> {
    STANDARD
        .iter()
        .chain(extra)
        .map(|s| s.to_string())
        .collect()
}

fn group<'a>(catalog: &'a Value, id: &str) -> &'a Value {
    catalog["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == id)
        .unwrap_or_else(|| panic!("no group {id} in {catalog}"))
}

// -- tool selection ---------------------------------------------------------------------------

#[tokio::test]
async fn a_run_only_gets_and_can_only_call_the_tools_it_selects() {
    let app = TestApp::new(vec![
        Step::Reply(tool_use(
            "t1",
            "fetch_url",
            json!({ "url": "https://example.com" }),
        )),
        Step::Reply(text("done")),
    ])
    .await;
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "go", "tools": ["weather"] }))
        .await;
    assert_eq!(run["tools"], json!(["weather"]));
    let id = run["id"].as_str().unwrap();
    app.wait_status(KEY_A, id, "completed").await;

    let requests = app.llm.requests();
    assert_eq!(tool_names(&requests[0]), names(&["weather_forecast"]));
    let system = requests[0]["system"].as_str().unwrap();
    assert!(system.contains("use weather_forecast"), "{system}");
    assert!(!system.contains("fetch_url") && !system.contains("show_map"));
    assert!(!system.contains("show_recipe") && !system.contains("compare_stocks"));

    // The model asked for a tool outside the selection: it is as unknown as a made-up one.
    let result = &requests[1]["messages"].as_array().unwrap().last().unwrap()["content"][0];
    assert_eq!(result["is_error"], true);
    assert_eq!(result["content"], "unknown tool: fetch_url");
}

#[tokio::test]
async fn without_a_selection_every_tool_is_offered() {
    let app = TestApp::new(vec![Step::Reply(text("ok"))]).await;
    let run = app.create_run(KEY_A, "go").await;
    assert_eq!(run["tools"], Value::Null);
    app.wait_status(KEY_A, run["id"].as_str().unwrap(), "completed")
        .await;
    assert_eq!(
        tool_names(&app.llm.requests()[0]),
        names(&[
            "fetch_url",
            "download_file",
            "weather_forecast",
            "show_map",
            "show_recipe",
            "plan_trip",
            "compare_stocks",
            "ask_user"
        ])
    );
}

#[tokio::test]
async fn single_tools_can_be_selected_and_continuations_inherit_the_selection() {
    let app = TestApp::new(vec![Step::Repeat(text("ok"))]).await;
    let first = app
        .create_run_with(KEY_A, json!({ "prompt": "a", "tools": ["fetch_url"] }))
        .await;
    let first_id = first["id"].as_str().unwrap();
    app.wait_status(KEY_A, first_id, "completed").await;
    assert_eq!(tool_names(&app.llm.requests()[0]), names(&["fetch_url"]));

    let second = app
        .create_run_with(KEY_A, json!({ "prompt": "b", "parent_id": first_id }))
        .await;
    assert_eq!(second["tools"], json!(["fetch_url"]));
    let second_id = second["id"].as_str().unwrap();
    app.wait_status(KEY_A, second_id, "completed").await;

    // An explicit empty selection leaves only the sandbox tools.
    let third = app
        .create_run_with(
            KEY_A,
            json!({ "prompt": "c", "parent_id": second_id, "tools": [] }),
        )
        .await;
    assert_eq!(third["tools"], json!([]));
    app.wait_status(KEY_A, third["id"].as_str().unwrap(), "completed")
        .await;
    assert_eq!(tool_names(&app.llm.requests()[2]), names(&[]));
}

#[tokio::test]
async fn unknown_tools_are_rejected_but_groups_this_server_lacks_are_not() {
    let app = TestApp::new(vec![Step::Repeat(text("ok"))]).await;
    for bad in [json!(["nope"]), json!(["gmail_"]), json!(["calendar_x y"])] {
        let (status, body) = app
            .json(
                Method::POST,
                "/v1/runs",
                Some(KEY_A),
                Some(json!({ "prompt": "p", "tools": bad })),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown tool"));
    }
    let too_many: Vec<String> = (0..101).map(|_| "maps".to_string()).collect();
    let (status, _) = app
        .json(
            Method::POST,
            "/v1/runs",
            Some(KEY_A),
            Some(json!({ "prompt": "p", "tools": too_many })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The browser and Gmail are off here, but a saved selection naming them still works.
    let run = app
        .create_run_with(
            KEY_A,
            json!({ "prompt": "p", "tools": ["maps", "browser", "gmail_send_message", "maps"] }),
        )
        .await;
    assert_eq!(
        run["tools"],
        json!(["browser", "gmail_send_message", "maps"])
    );
}

#[tokio::test]
async fn the_catalog_lists_every_group_and_why_some_are_missing() {
    let app = TestApp::with_env(vec![], &[("MAPS", "off")]).await;
    let (status, catalog) = app.get("/v1/tools", KEY_A).await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> = catalog["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "web_search",
            "web_fetch",
            "weather",
            "maps",
            "recipes",
            "transit",
            "stocks",
            "products",
            "ask_user",
            "browser",
            "gmail",
            "calendar",
            "custom"
        ]
    );
    let products = group(&catalog, "products");
    assert_eq!(products["available"], false);
    assert_eq!(
        products["reason"],
        "Needs a Brave Search or Best Buy API key"
    );

    let search = group(&catalog, "web_search");
    assert_eq!(search["available"], false);
    assert_eq!(search["reason"], "Needs a Brave Search API key");
    assert_eq!(
        group(&catalog, "maps")["reason"],
        "Turned off on the server (MAPS=off)"
    );
    assert_eq!(
        group(&catalog, "gmail")["reason"],
        "Turned off on the server (GMAIL=off)"
    );
    assert_eq!(group(&catalog, "custom")["reason"], "No TOOLS_DIR is set");
    let fetch = group(&catalog, "web_fetch");
    assert_eq!(fetch["available"], true);
    assert_eq!(fetch["tools"][0]["name"], "fetch_url");
    assert_eq!(fetch["tools"][0]["writes"], false);
    assert!(fetch.get("reason").is_none());

    let (status, _) = app.json(Method::GET, "/v1/tools", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// -- admin: Brave Search ----------------------------------------------------------------------

/// A Brave Search mock that accepts one key.
async fn start_brave(valid_key: &'static str) -> String {
    async fn search(State(valid): State<&'static str>, headers: HeaderMap) -> impl IntoResponse {
        if headers["x-subscription-token"] == valid {
            (StatusCode::OK, Json(json!({ "web": { "results": [] } })))
        } else {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": { "detail": "bad key" } })),
            )
        }
    }
    let app = Router::new()
        .route("/search", get(search))
        .with_state(valid_key);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/search", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

fn admin_env(extra: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
    let mut env = vec![
        ("CEDAR_ADMIN_KEYS", KEY_B),
        ("CEDAR_SECRET_KEY", SECRET_KEY),
    ];
    env.extend_from_slice(extra);
    env
}

#[tokio::test]
async fn admin_routes_need_an_admin_key() {
    let app = TestApp::with_env(vec![], &admin_env(&[])).await;
    for (method, uri, body) in [
        (Method::GET, "/v1/admin/integrations", None),
        (Method::GET, "/v1/admin/models", None),
        (
            Method::PUT,
            "/v1/admin/models/m",
            Some(json!({ "base_url": "http://x", "model": "m" })),
        ),
        (Method::DELETE, "/v1/admin/models/m", None),
        (
            Method::POST,
            "/v1/admin/server-models",
            Some(json!({ "base_url": "http://x" })),
        ),
        (
            Method::PUT,
            "/v1/admin/integrations/brave",
            Some(json!({ "api_key": "k" })),
        ),
        (Method::DELETE, "/v1/admin/integrations/brave", None),
        (
            Method::PUT,
            "/v1/admin/integrations/bestbuy",
            Some(json!({ "api_key": "k" })),
        ),
        (Method::DELETE, "/v1/admin/integrations/bestbuy", None),
        (
            Method::POST,
            "/v1/admin/integrations/google/authorize",
            Some(json!({ "redirect_uri": "http://x/cb" })),
        ),
        (Method::DELETE, "/v1/admin/integrations/google", None),
        (Method::GET, "/v1/admin/settings", None),
        (
            Method::PUT,
            "/v1/admin/settings",
            Some(json!({ "max_image_side": 1024 })),
        ),
    ] {
        let (status, body) = app.json(method, uri, Some(KEY_A), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {body}");
        assert_eq!(body["error"]["code"], "forbidden");
    }
    let (status, _) = app.get("/v1/admin/integrations", KEY_B).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_brave_key_saved_by_an_admin_turns_web_search_on_without_a_restart() {
    let brave = start_brave("good-brave-key-9876").await;
    let brave: &'static str = Box::leak(brave.into_boxed_str());
    let store = Store::connect_memory().await.unwrap();
    let app = TestApp::build(
        store.clone(),
        ScriptedLlm::new(vec![Step::Repeat(text("ok"))]),
        FakeFactory::new(),
        &admin_env(&[("BRAVE_API_BASE_URL", brave)]),
    )
    .await;

    let (_, status) = app.get("/v1/admin/integrations", KEY_B).await;
    assert_eq!(status["storage"], true);
    assert_eq!(status["brave"]["configured"], false);

    // A key Brave rejects is not saved.
    let (code, body) = app
        .json(
            Method::PUT,
            "/v1/admin/integrations/brave",
            Some(KEY_B),
            Some(json!({ "api_key": "wrong" })),
        )
        .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("rejected the API key"));

    let (code, body) = app
        .json(
            Method::PUT,
            "/v1/admin/integrations/brave",
            Some(KEY_B),
            Some(json!({ "api_key": " good-brave-key-9876 " })),
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(
        body["brave"],
        json!({ "configured": true, "source": "ui", "last4": "9876" })
    );
    let (_, _, raw) = app
        .call(Method::GET, "/v1/admin/integrations", Some(KEY_B), None)
        .await;
    assert!(!raw.contains("good-brave-key"));

    let run = app.create_run(KEY_A, "search").await;
    app.wait_status(KEY_A, run["id"].as_str().unwrap(), "completed")
        .await;
    assert!(tool_names(&app.llm.requests()[0]).contains(&"web_search".to_string()));
    let (_, catalog) = app.get("/v1/tools", KEY_A).await;
    assert_eq!(group(&catalog, "web_search")["available"], true);

    // A restarted server picks the saved key up again.
    let restarted = TestApp::build(
        store,
        ScriptedLlm::new(vec![]),
        FakeFactory::new(),
        &admin_env(&[("BRAVE_API_BASE_URL", brave)]),
    )
    .await;
    restarted.state.integrations.reload().await.unwrap();
    let (_, catalog) = restarted.get("/v1/tools", KEY_A).await;
    assert_eq!(group(&catalog, "web_search")["available"], true);

    let (code, _) = app
        .json(
            Method::DELETE,
            "/v1/admin/integrations/brave",
            Some(KEY_B),
            None,
        )
        .await;
    assert_eq!(code, StatusCode::NO_CONTENT);
    let (_, catalog) = app.get("/v1/tools", KEY_A).await;
    assert_eq!(group(&catalog, "web_search")["available"], false);
    let (code, _) = app
        .json(
            Method::DELETE,
            "/v1/admin/integrations/brave",
            Some(KEY_B),
            None,
        )
        .await;
    assert_eq!(code, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn keys_from_the_environment_or_without_storage_cannot_be_changed() {
    let app = TestApp::with_env(vec![], &admin_env(&[("BRAVE_API_KEY", "env-key-1111")])).await;
    let (_, status) = app.get("/v1/admin/integrations", KEY_B).await;
    assert_eq!(
        status["brave"],
        json!({ "configured": true, "source": "env", "last4": "1111" })
    );
    let (code, body) = app
        .json(
            Method::PUT,
            "/v1/admin/integrations/brave",
            Some(KEY_B),
            Some(json!({ "api_key": "x" })),
        )
        .await;
    assert_eq!(code, StatusCode::CONFLICT);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("environment"));

    let app = TestApp::with_env(vec![], &[("CEDAR_ADMIN_KEYS", KEY_B)]).await;
    let (_, status) = app.get("/v1/admin/integrations", KEY_B).await;
    assert_eq!(status["storage"], false);
    let (code, body) = app
        .json(
            Method::PUT,
            "/v1/admin/integrations/brave",
            Some(KEY_B),
            Some(json!({ "api_key": "x" })),
        )
        .await;
    assert_eq!(code, StatusCode::CONFLICT);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("CEDAR_SECRET_KEY"));
}

// -- admin: Best Buy --------------------------------------------------------------------------

/// A Best Buy Products API mock that accepts one key, as its `apiKey` query parameter.
async fn start_bestbuy(valid_key: &'static str) -> String {
    async fn products(
        State(valid): State<&'static str>,
        axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
    ) -> impl IntoResponse {
        if params.get("apiKey").map(String::as_str) == Some(valid) {
            (StatusCode::OK, Json(json!({ "total": 0, "products": [] })))
        } else {
            (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": { "message": "The provided API Key is invalid." } })),
            )
        }
    }
    // The search filter follows `/products` in parentheses, so no route matches it.
    let app = Router::new().fallback(products).with_state(valid_key);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/products", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

#[tokio::test]
async fn a_best_buy_key_saved_by_an_admin_turns_product_search_on_without_a_restart() {
    let api = start_bestbuy("good-bestbuy-key-4321").await;
    let api: &'static str = Box::leak(api.into_boxed_str());
    let store = Store::connect_memory().await.unwrap();
    let app = TestApp::build(
        store.clone(),
        ScriptedLlm::new(vec![Step::Repeat(text("ok"))]),
        FakeFactory::new(),
        &admin_env(&[("PRODUCTS_API_URL", api)]),
    )
    .await;

    let (_, status) = app.get("/v1/admin/integrations", KEY_B).await;
    assert_eq!(
        status["bestbuy"],
        json!({ "configured": false, "source": null, "enabled": true })
    );
    let (_, catalog) = app.get("/v1/tools", KEY_A).await;
    assert_eq!(group(&catalog, "products")["available"], false);

    // A key Best Buy rejects is not saved.
    let (code, body) = app
        .json(
            Method::PUT,
            "/v1/admin/integrations/bestbuy",
            Some(KEY_B),
            Some(json!({ "api_key": "wrong" })),
        )
        .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Best Buy rejected the API key"),
        "{body}"
    );

    let (code, body) = app
        .json(
            Method::PUT,
            "/v1/admin/integrations/bestbuy",
            Some(KEY_B),
            Some(json!({ "api_key": " good-bestbuy-key-4321 " })),
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(
        body["bestbuy"],
        json!({ "configured": true, "source": "ui", "last4": "4321", "enabled": true })
    );
    let (_, _, raw) = app
        .call(Method::GET, "/v1/admin/integrations", Some(KEY_B), None)
        .await;
    assert!(!raw.contains("good-bestbuy-key"));

    let run = app.create_run(KEY_A, "which headphones?").await;
    app.wait_status(KEY_A, run["id"].as_str().unwrap(), "completed")
        .await;
    let request = &app.llm.requests()[0];
    assert!(tool_names(request).contains(&"search_products".to_string()));
    assert!(request["system"]
        .as_str()
        .unwrap()
        .contains("use search_products"));
    let (_, catalog) = app.get("/v1/tools", KEY_A).await;
    assert_eq!(group(&catalog, "products")["available"], true);

    // A restarted server picks the saved key up again.
    let restarted = TestApp::build(
        store,
        ScriptedLlm::new(vec![]),
        FakeFactory::new(),
        &admin_env(&[("PRODUCTS_API_URL", api)]),
    )
    .await;
    let (_, catalog) = restarted.get("/v1/tools", KEY_A).await;
    assert_eq!(group(&catalog, "products")["available"], true);

    let (code, _) = app
        .json(
            Method::DELETE,
            "/v1/admin/integrations/bestbuy",
            Some(KEY_B),
            None,
        )
        .await;
    assert_eq!(code, StatusCode::NO_CONTENT);
    let (_, catalog) = app.get("/v1/tools", KEY_A).await;
    assert_eq!(group(&catalog, "products")["available"], false);
    let (code, _) = app
        .json(
            Method::DELETE,
            "/v1/admin/integrations/bestbuy",
            Some(KEY_B),
            None,
        )
        .await;
    assert_eq!(code, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_best_buy_key_from_the_environment_or_with_products_off_is_reported() {
    let app = TestApp::with_env(
        vec![],
        &admin_env(&[("BESTBUY_API_KEY", "env-bestbuy-2222")]),
    )
    .await;
    let (_, status) = app.get("/v1/admin/integrations", KEY_B).await;
    assert_eq!(
        status["bestbuy"],
        json!({ "configured": true, "source": "env", "last4": "2222", "enabled": true })
    );
    let (code, body) = app
        .json(
            Method::PUT,
            "/v1/admin/integrations/bestbuy",
            Some(KEY_B),
            Some(json!({ "api_key": "x" })),
        )
        .await;
    assert_eq!(code, StatusCode::CONFLICT);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("environment"));

    let app = TestApp::with_env(vec![], &admin_env(&[("PRODUCTS", "off")])).await;
    let (_, status) = app.get("/v1/admin/integrations", KEY_B).await;
    assert_eq!(status["bestbuy"]["enabled"], false);
    let (_, catalog) = app.get("/v1/tools", KEY_A).await;
    assert_eq!(
        group(&catalog, "products")["reason"],
        "Turned off on the server (PRODUCTS=off)"
    );
}

// -- admin: Google ----------------------------------------------------------------------------

#[derive(Default)]
struct GoogleMock {
    forms: Mutex<Vec<HashMap<String, String>>>,
}

async fn start_google() -> (String, Arc<GoogleMock>) {
    async fn token(
        State(mock): State<Arc<GoogleMock>>,
        Form(form): Form<HashMap<String, String>>,
    ) -> impl IntoResponse {
        mock.forms.lock().unwrap().push(form.clone());
        if form.get("code").map(String::as_str) != Some("good-code") {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "invalid_grant", "error_description": "Bad Request" })),
            );
        }
        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"email":"ana@example.com"}"#);
        (
            StatusCode::OK,
            Json(json!({
                "access_token": "at", "expires_in": 3600, "refresh_token": "rt-secret",
                "id_token": format!("h.{claims}.s"),
                "scope": "openid email https://www.googleapis.com/auth/gmail.readonly",
            })),
        )
    }
    let mock = Arc::new(GoogleMock::default());
    let app = Router::new()
        .route("/token", post(token))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/token", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, mock)
}

fn state_param(url: &str) -> String {
    reqwest::Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .to_string()
}

#[tokio::test]
async fn connecting_google_stores_the_account_and_turns_gmail_on() {
    let (token_url, mock) = start_google().await;
    let token_url: &'static str = Box::leak(token_url.into_boxed_str());
    let app = TestApp::with_env(
        vec![],
        &admin_env(&[
            ("GMAIL", "on"),
            ("GOOGLE_CLIENT_ID", "client-1"),
            ("GOOGLE_CLIENT_SECRET", "client-secret"),
            ("GOOGLE_TOKEN_URL", token_url),
        ]),
    )
    .await;

    let (_, catalog) = app.get("/v1/tools", KEY_A).await;
    assert_eq!(
        group(&catalog, "gmail")["reason"],
        "No Google account is connected"
    );
    let (_, status) = app.get("/v1/admin/integrations", KEY_B).await;
    assert_eq!(status["google"]["enabled"], true);
    assert_eq!(status["google"]["configured"], false);
    assert_eq!(status["google"]["services"], json!(["gmail"]));

    let authorize = |redirect: &str| {
        app.json(
            Method::POST,
            "/v1/admin/integrations/google/authorize",
            Some(KEY_B),
            Some(json!({ "redirect_uri": redirect })),
        )
    };
    let (code, _) = authorize("javascript:alert(1)").await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    let (code, body) = authorize("http://chat.local/api/oauth/google/callback").await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let url = body["url"].as_str().unwrap();
    assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
    let params: HashMap<String, String> = reqwest::Url::parse(url)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect();
    assert_eq!(params["client_id"], "client-1");
    assert_eq!(params["access_type"], "offline");
    assert_eq!(
        params["redirect_uri"],
        "http://chat.local/api/oauth/google/callback"
    );
    assert!(params["scope"].contains("gmail.readonly"));
    let state = state_param(url);

    let callback = |state: &str, code: &str| {
        app.json(
            Method::POST,
            "/v1/admin/integrations/google/callback",
            Some(KEY_B),
            Some(json!({ "state": state, "code": code })),
        )
    };
    let (code, _) = callback("forged", "good-code").await;
    assert_eq!(code, StatusCode::BAD_REQUEST);

    let (code, body) = callback(&state, "good-code").await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["account"], "ana@example.com");
    let form = mock.forms.lock().unwrap().last().cloned().unwrap();
    assert_eq!(form["grant_type"], "authorization_code");
    assert_eq!(form["client_secret"], "client-secret");
    assert_eq!(
        form["redirect_uri"],
        "http://chat.local/api/oauth/google/callback"
    );

    // The state works once.
    let (code, _) = callback(&state, "good-code").await;
    assert_eq!(code, StatusCode::BAD_REQUEST);

    let (_, status) = app.get("/v1/admin/integrations", KEY_B).await;
    assert_eq!(status["google"]["configured"], true);
    assert_eq!(status["google"]["source"], "ui");
    assert_eq!(status["google"]["account"], "ana@example.com");
    assert!(!status.to_string().contains("rt-secret"));
    let (_, catalog) = app.get("/v1/tools", KEY_A).await;
    let gmail = group(&catalog, "gmail");
    assert_eq!(gmail["available"], true, "{gmail}");
    let draft = gmail["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "gmail_create_draft")
        .unwrap();
    assert_eq!(draft["writes"], true);

    // A code Google refuses reports why.
    let (_, body) = authorize("http://chat.local/cb").await;
    let (code, body) = callback(&state_param(body["url"].as_str().unwrap()), "bad").await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Bad Request"));

    let (code, _) = app
        .json(
            Method::DELETE,
            "/v1/admin/integrations/google",
            Some(KEY_B),
            None,
        )
        .await;
    assert_eq!(code, StatusCode::NO_CONTENT);
    let (_, catalog) = app.get("/v1/tools", KEY_A).await;
    assert_eq!(group(&catalog, "gmail")["available"], false);
}

// -- defaults and the model registry ----------------------------------------------------------

#[tokio::test]
async fn defaults_are_reported_to_any_key() {
    let app = TestApp::with_env(vec![], &[("SYSTEM_PROMPT", "Be terse.")]).await;
    let (status, body) = app.get("/v1/defaults", KEY_A).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["system"], "Be terse.");
    assert_eq!(body["model"], TEST_MODEL);
    assert_eq!(body["model_id"], TEST_MODEL);
    assert_eq!(body["max_turns_cap"], 10);
}

#[tokio::test]
async fn attached_images_are_downscaled_to_the_admins_max_image_side() {
    let b64 = base64::engine::general_purpose::STANDARD;
    let app = TestApp::with_env(vec![Step::Repeat(text("a photo"))], &admin_env(&[])).await;
    let (status, settings) = app.get("/v1/admin/settings", KEY_B).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        settings,
        json!({
            "max_image_side": null,
            "sandbox_idle_minutes": null,
            "sandbox_runtime": null,
            "openshell_supported": cfg!(not(windows)),
            "server_os": std::env::consts::OS,
            "defaults": {
                "sandbox_runtime": "docker",
                "max_image_side": 1568, "min_max_image_side": 256, "max_max_image_side": 8192,
                "sandbox_idle_minutes": 10, "max_sandbox_idle_minutes": 1440,
            },
        })
    );
    let put = |side: Value| {
        app.json(
            Method::PUT,
            "/v1/admin/settings",
            Some(KEY_B),
            Some(json!({ "max_image_side": side })),
        )
    };
    let (status, body) = put(json!(100)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, body) = put(json!(512)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["max_image_side"], 512);
    let (_, defaults) = app.get("/v1/defaults", KEY_A).await;
    assert_eq!(defaults["attachments"]["max_image_side"], 512);

    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        2000,
        1000,
        image::Rgb([9, 99, 199]),
    ))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .unwrap();
    let photo = json!({ "name": "photo.png", "media_type": "image/png", "data": b64.encode(&png) });
    let run = app
        .create_run_with(
            KEY_A,
            json!({ "prompt": "what is this?", "attachments": [photo] }),
        )
        .await;
    let id = run["id"].as_str().unwrap().to_string();
    app.wait_status(KEY_A, &id, "completed").await;

    // The model sees, and the workspace holds, the smaller image, renamed for its new format.
    let block = &app.llm.requests()[0]["messages"][0]["content"][0];
    assert_eq!(block["title"], "photo.jpg");
    assert_eq!(block["source"]["media_type"], "image/jpeg");
    let small = b64
        .decode(block["source"]["data"].as_str().unwrap())
        .unwrap();
    let img = image::load_from_memory(&small).unwrap();
    assert_eq!((img.width(), img.height()), (512, 256));
    {
        let created = app.factory.created.lock().unwrap();
        let files = created[0].1.files.lock().unwrap();
        assert_eq!(files.get("attachments/photo.jpg"), Some(&small));
        assert!(!files.contains_key("attachments/photo.png"));
    }

    // `null` goes back to the default.
    let (_, body) = put(Value::Null).await;
    assert_eq!(body["max_image_side"], Value::Null);
    let (_, defaults) = app.get("/v1/defaults", KEY_A).await;
    assert_eq!(defaults["attachments"]["max_image_side"], 1568);
}

#[tokio::test]
async fn an_admin_sets_how_long_an_idle_sandbox_keeps_running() {
    let app = TestApp::with_env(vec![], &admin_env(&[])).await;
    let put = |body: Value| app.json(Method::PUT, "/v1/admin/settings", Some(KEY_B), Some(body));
    let (status, body) = put(json!({ "sandbox_idle_minutes": 5000 })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("sandbox_idle_minutes"));
    // Nothing is saved when any field is refused.
    let (status, body) = put(json!({ "max_image_side": 512, "sandbox_idle_minutes": 5000 })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        app.get("/v1/admin/settings", KEY_B).await.1["max_image_side"],
        Value::Null
    );

    let (status, body) = put(json!({ "sandbox_idle_minutes": 30, "max_image_side": 512 })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        (
            body["sandbox_idle_minutes"].clone(),
            body["max_image_side"].clone()
        ),
        (json!(30), json!(512))
    );
    assert_eq!(
        cedar::manager::idle_minutes(&app.state.store)
            .await
            .unwrap(),
        30
    );

    // A field left out keeps its value; `null` goes back to the default.
    let (_, body) = put(json!({ "max_image_side": null })).await;
    assert_eq!(
        (
            body["sandbox_idle_minutes"].clone(),
            body["max_image_side"].clone()
        ),
        (json!(30), Value::Null)
    );
    let (_, body) = put(json!({ "sandbox_idle_minutes": 0 })).await;
    assert_eq!(
        body["sandbox_idle_minutes"], 0,
        "0 pauses as soon as a turn ends"
    );
    let (_, body) = put(json!({ "sandbox_idle_minutes": null })).await;
    assert_eq!(body["sandbox_idle_minutes"], Value::Null);
    assert_eq!(
        cedar::manager::idle_minutes(&app.state.store)
            .await
            .unwrap(),
        10
    );
}

#[tokio::test]
async fn an_admin_turns_openshell_on_for_the_next_runs_sandboxes() {
    let app = TestApp::with_env(
        vec![
            Step::Reply(text("one")),
            Step::Reply(text("two")),
            Step::Reply(text("three")),
        ],
        &admin_env(&[]),
    )
    .await;
    cedar::manager::set_idle_minutes(&app.state.store, Some(30))
        .await
        .unwrap();
    let put = |body: Value| app.json(Method::PUT, "/v1/admin/settings", Some(KEY_B), Some(body));
    let runtime = |run: &Value| run["id"].as_str().unwrap().to_string();
    let started = |events: &[SseEvent]| {
        events
            .iter()
            .find(|e| e.data["type"] == "run.started")
            .unwrap()
            .data["data"]["sandbox"]["runtime"]
            .clone()
    };

    // Docker by default.
    let first = app.create_run(KEY_A, "1").await;
    app.wait_status(KEY_A, &runtime(&first), "completed").await;
    let events = app.events(KEY_A, &runtime(&first), None).await;
    assert_eq!(started(&events), "docker");

    let (status, body) = put(json!({ "sandbox_runtime": "podman" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    // Without a gateway that answers, nothing is switched.
    app.factory
        .openshell_down
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (status, body) =
        put(json!({ "sandbox_runtime": "openshell", "sandbox_idle_minutes": 5 })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let message = body["error"]["message"].as_str().unwrap();
    if cfg!(windows) {
        assert!(message.contains("Windows"), "{message}");
        return;
    }
    assert!(message.contains("gateway is not answering"), "{message}");
    let (_, body) = app.get("/v1/admin/settings", KEY_B).await;
    assert_eq!(
        (
            body["sandbox_runtime"].clone(),
            body["sandbox_idle_minutes"].clone()
        ),
        (Value::Null, json!(30))
    );
    app.factory
        .openshell_down
        .store(false, std::sync::atomic::Ordering::SeqCst);

    let (status, body) = put(json!({ "sandbox_runtime": "openshell" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["sandbox_runtime"], "openshell");

    // The conversation's kept Docker sandbox is replaced by an OpenShell one on the same workspace.
    let second = app
        .create_run_with(KEY_A, json!({ "prompt": "2", "parent_id": first["id"] }))
        .await;
    let done = app.wait_status(KEY_A, &runtime(&second), "completed").await;
    assert_eq!(done["workspace_reused"], true);
    let events = app.events(KEY_A, &runtime(&second), None).await;
    assert_eq!(started(&events), "openshell");
    assert_eq!(app.factory.created.lock().unwrap().len(), 2);
    assert!(app
        .factory
        .sandbox(0)
        .destroyed
        .load(std::sync::atomic::Ordering::SeqCst));
    assert!(app.factory.sandbox(1).openshell);

    // Off again: Docker is the default, so nothing is saved.
    let (_, body) = put(json!({ "sandbox_runtime": "docker" })).await;
    assert_eq!(body["sandbox_runtime"], Value::Null);
    let third = app
        .create_run_with(KEY_A, json!({ "prompt": "3", "parent_id": second["id"] }))
        .await;
    app.wait_status(KEY_A, &runtime(&third), "completed").await;
    assert!(!app.factory.sandbox(2).openshell);
}

#[derive(Default)]
struct ModelServer {
    /// The headers and body of every chat-completions request.
    completions: Mutex<Vec<(HeaderMap, Value)>>,
}

/// An OpenAI-compatible model server that lists `m-1` and, when `key` is set, wants it. Its
/// replies say `answer`.
async fn start_model_server(
    key: Option<&'static str>,
    answer: &'static str,
) -> (String, Arc<ModelServer>) {
    async fn models(
        State((key, _, _)): State<(Option<&'static str>, &'static str, Arc<ModelServer>)>,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        let presented = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        if key.is_some_and(|k| presented != Some(format!("Bearer {k}"))) {
            return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "no" })));
        }
        (
            StatusCode::OK,
            Json(json!({ "object": "list", "data": [{ "id": "m-1", "object": "model" }] })),
        )
    }
    async fn completions(
        State((_, answer, server)): State<(Option<&'static str>, &'static str, Arc<ModelServer>)>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        server.completions.lock().unwrap().push((headers, body));
        Json(
            json!({ "choices": [{ "message": { "role": "assistant", "content": answer }, "finish_reason": "stop" }],
                     "usage": { "prompt_tokens": 3, "completion_tokens": 2 } }),
        )
    }
    let server = Arc::new(ModelServer::default());
    let app = Router::new()
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(completions))
        .with_state((key, answer, server.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), server)
}

/// A test app whose model clients are real ones built from the registry, as in `main.rs`. The
/// registry starts as `store` has it (empty for a fresh one).
async fn app_with_real_llm_on(store: Store, env: &[(&str, &str)]) -> TestApp {
    let clients = Arc::new(cedar::llm::LlmRouter::new());
    let factory = FakeFactory::new();
    let browsers = Arc::new(FakeBrowserFactory::default());
    let state = cedar::api::AppState::with_browsers(
        test_config(env),
        store,
        clients.clone(),
        factory.clone(),
        browsers.clone(),
    )
    .unwrap();
    state.integrations.attach_router(clients);
    state.integrations.reload().await.unwrap();
    TestApp {
        router: cedar::api::router(state.clone()),
        state,
        llm: ScriptedLlm::new(vec![]),
        factory,
        browsers,
    }
}

async fn app_with_real_llm(env: &[(&str, &str)]) -> TestApp {
    app_with_real_llm_on(Store::connect_memory().await.unwrap(), env).await
}

async fn put_model(app: &TestApp, id: &str, body: Value) -> (StatusCode, Value) {
    let uri = format!("/v1/admin/models/{id}");
    app.json(Method::PUT, &uri, Some(KEY_B), Some(body)).await
}

#[tokio::test]
async fn an_admin_registers_a_model_with_its_server_and_key() {
    let (server_url, server) = start_model_server(Some("llm-key"), "from the new server").await;
    let app = app_with_real_llm(&admin_env(&[])).await;

    // Nothing is registered: runs are refused, and the lists are empty.
    let (_, status) = app.get("/v1/admin/integrations", KEY_B).await;
    assert_eq!(status["models"], 0);
    assert_eq!(
        app.get("/v1/admin/models", KEY_B).await.1,
        json!({ "models": [] })
    );
    assert_eq!(
        app.get("/v1/models", KEY_A).await.1,
        json!({ "models": [] })
    );
    let (code, body) = app
        .json(
            Method::POST,
            "/v1/runs",
            Some(KEY_A),
            Some(json!({ "prompt": "x" })),
        )
        .await;
    assert_eq!(code, StatusCode::CONFLICT, "{body}");

    // Nothing listening there.
    let (code, body) = put_model(
        &app,
        "gpu",
        json!({ "base_url": "http://127.0.0.1:1", "model": "m-1" }),
    )
    .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("could not reach"),
        "{body}"
    );
    // The server wants a key.
    let (code, body) = put_model(
        &app,
        "gpu",
        json!({ "base_url": server_url, "model": "m-1" }),
    )
    .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]["message"].as_str().unwrap().contains("401"),
        "{body}"
    );
    for (id, body) in [
        ("gpu", json!({ "base_url": "not a url", "model": "m-1" })),
        (
            "gpu",
            json!({ "base_url": "http://user:pw@box:8000/v1", "model": "m-1" }),
        ),
        ("gpu", json!({ "base_url": server_url, "model": "" })),
        (
            "gpu",
            json!({ "base_url": server_url, "model": "two\nlines" }),
        ),
        ("GPU", json!({ "base_url": server_url, "model": "m-1" })),
        (
            "gpu",
            json!({ "base_url": server_url, "model": "m-1", "bogus": 1 }),
        ),
        (
            "gpu",
            json!({ "base_url": server_url, "model": "m-1", "reasoning_levels": ["lo w"] }),
        ),
        (
            "gpu",
            json!({ "base_url": server_url, "model": "m-1", "reasoning_levels": ["low"], "default_reasoning": "high" }),
        ),
        (
            "gpu",
            json!({ "base_url": server_url, "model": "m-1", "max_tokens": 0 }),
        ),
        (
            "gpu",
            json!({ "base_url": server_url, "model": "m-1", "max_tokens": 2_000_000 }),
        ),
    ] {
        let (code, resp) = put_model(&app, id, body.clone()).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{body} -> {resp}");
    }

    // The path defaults to /v1; the first entry is the default model.
    let (code, body) = put_model(
        &app,
        "gpu",
        json!({ "base_url": server_url, "model": " m-1 ", "api_key": "llm-key", "label": "GPU box",
                "reasoning_levels": ["none", "high"], "multimodal": true }),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["id"], "gpu");
    assert_eq!(body["label"], "GPU box");
    assert_eq!(body["model"], "m-1");
    assert_eq!(body["base_url"], format!("{server_url}/v1"));
    assert_eq!(body["api_key_set"], true);
    assert_eq!(body["is_default"], true);
    assert_eq!(body["reasoning_levels"], json!(["none", "high"]));
    assert_eq!(body["default_reasoning"], Value::Null);
    assert_eq!(body["multimodal"], true);
    assert_eq!(
        body["max_tokens"], 16384,
        "a new entry gets the default reply limit"
    );
    assert_eq!(body["listed_models"], json!(["m-1"]));
    let (_, _, raw) = app
        .call(Method::GET, "/v1/admin/models", Some(KEY_B), None)
        .await;
    assert!(!raw.contains("llm-key"));
    assert_eq!(
        app.get("/v1/models", KEY_A).await.1["models"][0],
        json!({ "id": "gpu", "label": "GPU box", "model": "m-1", "is_default": true,
                "reasoning_levels": ["none", "high"], "default_reasoning": null, "multimodal": true,
                "max_tokens": 16384, "price": null })
    );

    // The next run talks to the server, with its key and the level asked for.
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "hello", "reasoning": "high" }))
        .await;
    assert_eq!(run["model_id"], "gpu");
    let done = app
        .wait_status(KEY_A, run["id"].as_str().unwrap(), "completed")
        .await;
    assert_eq!(done["final_text"], "from the new server");
    let (headers, body) = server.completions.lock().unwrap()[0].clone();
    assert_eq!(headers["authorization"], "Bearer llm-key");
    assert_eq!(body["model"], "m-1");
    assert_eq!(body["reasoning_effort"], "high");
    assert_eq!(body["max_tokens"], 16384);

    // Changing the entry without api_key keeps the saved key; absent levels stay too.
    let (code, body) = put_model(
        &app,
        "gpu",
        json!({ "base_url": format!("{server_url}/v1/"), "model": "m-1" }),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["api_key_set"], true);
    assert_eq!(body["label"], "GPU box");
    assert_eq!(body["reasoning_levels"], json!(["none", "high"]));
    assert_eq!(body["multimodal"], true, "absent keeps the saved value");
    let (code, body) = put_model(
        &app,
        "gpu",
        json!({ "base_url": server_url, "model": "m-1", "multimodal": false, "max_tokens": 2048 }),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["multimodal"], false);
    assert_eq!(body["max_tokens"], 2048);
    let (code, body) = put_model(
        &app,
        "gpu",
        json!({ "base_url": server_url, "model": "m-1" }),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["max_tokens"], 2048, "absent keeps the saved limit");
    // The next model call takes the entry's new limit.
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "again" }))
        .await;
    app.wait_status(KEY_A, run["id"].as_str().unwrap(), "completed")
        .await;
    let (_, body) = server.completions.lock().unwrap().last().unwrap().clone();
    assert_eq!(body["max_tokens"], 2048);
    // An empty api_key removes it (and the server then refuses the check).
    let (code, body) = put_model(
        &app,
        "gpu",
        json!({ "base_url": server_url, "model": "m-1", "api_key": "" }),
    )
    .await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{body}");

    let (code, _) = app
        .json(Method::DELETE, "/v1/admin/models/gpu", Some(KEY_B), None)
        .await;
    assert_eq!(code, StatusCode::NO_CONTENT);
    let (code, _) = app
        .json(Method::DELETE, "/v1/admin/models/gpu", Some(KEY_B), None)
        .await;
    assert_eq!(code, StatusCode::NOT_FOUND);
    assert_eq!(
        app.get("/v1/admin/models", KEY_B).await.1,
        json!({ "models": [] })
    );
    assert!(!app
        .state
        .store
        .has_secret("model_api_key:gpu")
        .await
        .unwrap());

    let (code, _) = app
        .json(
            Method::PUT,
            "/v1/admin/models/gpu",
            Some(KEY_A),
            Some(json!({ "base_url": server_url, "model": "m-1" })),
        )
        .await;
    assert_eq!(code, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_admin_lists_a_servers_models_before_registering_one() {
    let (server_url, _) = start_model_server(Some("llm-key"), "hi").await;
    let app = app_with_real_llm(&admin_env(&[])).await;
    let list = |body: Value| {
        let app = &app;
        async move {
            app.json(
                Method::POST,
                "/v1/admin/server-models",
                Some(KEY_B),
                Some(body),
            )
            .await
        }
    };

    // The server wants a key; nothing listening; a malformed address.
    let (code, body) = list(json!({ "base_url": server_url })).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]["message"].as_str().unwrap().contains("401"),
        "{body}"
    );
    let (code, body) = list(json!({ "base_url": "http://127.0.0.1:1" })).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("could not reach"),
        "{body}"
    );
    let (code, _) = list(json!({ "base_url": "not a url" })).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);

    // With the key, the names come back with the address as it would be stored. Nothing is saved.
    let (code, body) = list(json!({ "base_url": server_url, "api_key": "llm-key" })).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({ "base_url": format!("{server_url}/v1"), "models": ["m-1"] })
    );
    assert_eq!(
        app.get("/v1/admin/models", KEY_B).await.1,
        json!({ "models": [] })
    );

    // A registered model's saved key is used when none is typed.
    let (code, body) = put_model(
        &app,
        "gpu",
        json!({ "base_url": server_url, "model": "m-1", "api_key": "llm-key" }),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let (code, body) = list(json!({ "base_url": server_url, "id": "gpu" })).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["models"], json!(["m-1"]));
    let (code, _) = list(json!({ "base_url": server_url, "id": "other" })).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn each_model_has_its_own_server_levels_and_default() {
    let (url_a, server_a) = start_model_server(None, "from a").await;
    let (url_b, server_b) = start_model_server(None, "from b").await;
    let app = app_with_real_llm(&admin_env(&[])).await;

    let (code, a) = put_model(
        &app,
        "a",
        json!({ "base_url": url_a, "model": "m-1", "reasoning_levels": ["none", "high"], "default_reasoning": "high" }),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{a}");
    assert_eq!(a["is_default"], true);
    let (code, b) = put_model(
        &app,
        "b",
        json!({ "base_url": url_b, "model": "m-1", "is_default": true }),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{b}");
    assert_eq!(b["is_default"], true);
    let (_, list) = app.get("/v1/admin/models", KEY_B).await;
    assert_eq!(list["models"][0]["id"], "a");
    assert_eq!(list["models"][0]["is_default"], false);
    assert_eq!(list["models"][1]["id"], "b");
    assert_eq!(app.get("/v1/defaults", KEY_A).await.1["model_id"], "b");

    // Unnamed: the default. Named: its own server, with its default level.
    let run = app.create_run(KEY_A, "hello").await;
    assert_eq!(run["model_id"], "b");
    let done = app
        .wait_status(KEY_A, run["id"].as_str().unwrap(), "completed")
        .await;
    assert_eq!(done["final_text"], "from b");
    let run = app
        .create_run_with(KEY_A, json!({ "prompt": "hello", "model": "a" }))
        .await;
    assert_eq!(run["reasoning"], "high");
    let done = app
        .wait_status(KEY_A, run["id"].as_str().unwrap(), "completed")
        .await;
    assert_eq!(done["final_text"], "from a");
    assert_eq!(
        server_a.completions.lock().unwrap()[0].1["reasoning_effort"],
        "high"
    );
    assert_eq!(
        server_b.completions.lock().unwrap()[0]
            .1
            .get("reasoning_effort"),
        None
    );
    // Both entries name m-1, so the name alone is ambiguous.
    let (code, body) = app
        .json(
            Method::POST,
            "/v1/runs",
            Some(KEY_A),
            Some(json!({ "prompt": "x", "model": "m-1" })),
        )
        .await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{body}");

    // Removing the default hands it to the first remaining entry.
    let (code, _) = app
        .json(Method::DELETE, "/v1/admin/models/b", Some(KEY_B), None)
        .await;
    assert_eq!(code, StatusCode::NO_CONTENT);
    assert_eq!(app.get("/v1/defaults", KEY_A).await.1["model_id"], "a");
    // A run on the removed model fails at its next call rather than hanging.
    let (code, body) = app
        .json(
            Method::POST,
            "/v1/runs",
            Some(KEY_A),
            Some(json!({ "prompt": "x", "model": "b" })),
        )
        .await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn an_api_key_needs_the_secret_key_but_a_model_without_one_does_not() {
    let (url, _server) = start_model_server(None, "ok").await;
    let app = app_with_real_llm(&[("CEDAR_ADMIN_KEYS", KEY_B)]).await;
    let (code, body) = put_model(
        &app,
        "m",
        json!({ "base_url": url, "model": "m-1", "api_key": "k" }),
    )
    .await;
    assert_eq!(code, StatusCode::CONFLICT, "{body}");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("CEDAR_SECRET_KEY"));
    let (code, body) = put_model(&app, "m", json!({ "base_url": url, "model": "m-1" })).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["api_key_set"], false);
    let run = app.create_run(KEY_A, "hello").await;
    let done = app
        .wait_status(KEY_A, run["id"].as_str().unwrap(), "completed")
        .await;
    assert_eq!(done["final_text"], "ok");
}

#[tokio::test]
async fn a_model_server_saved_by_an_earlier_version_becomes_the_default_entry() {
    let store = Store::connect_memory().await.unwrap();
    let key: [u8; 32] = hex::decode(SECRET_KEY).unwrap().try_into().unwrap();
    let secrets = cedar::secrets::SecretStore::new(store.clone(), &key);
    secrets
        .put(
            "llm_base_url",
            "http://gpu:9000/v1",
            &json!({ "base_url": "http://gpu:9000/v1", "models": ["m-1"] }),
        )
        .await
        .unwrap();
    secrets
        .put("llm_api_key", "old-key", &json!({ "last4": "-key" }))
        .await
        .unwrap();
    secrets
        .put(
            "llm_model",
            "deepseek-ai/DeepSeek-V4-Flash",
            &json!({ "model": "deepseek-ai/DeepSeek-V4-Flash" }),
        )
        .await
        .unwrap();

    let app = app_with_real_llm_on(store.clone(), &admin_env(&[])).await;
    let (_, list) = app.get("/v1/admin/models", KEY_B).await;
    let entry = &list["models"][0];
    assert_eq!(list["models"].as_array().unwrap().len(), 1);
    assert_eq!(entry["id"], "default");
    assert_eq!(entry["label"], "Default");
    assert_eq!(entry["model"], "deepseek-ai/DeepSeek-V4-Flash");
    assert_eq!(entry["base_url"], "http://gpu:9000/v1");
    assert_eq!(entry["api_key_set"], true);
    assert_eq!(entry["is_default"], true);
    assert_eq!(
        entry["reasoning_levels"],
        json!(["none", "low", "high", "max"])
    );
    assert_eq!(entry["multimodal"], false);
    assert_eq!(entry["listed_models"], json!(["m-1"]));
    for old in ["llm_base_url", "llm_api_key", "llm_model"] {
        assert!(!store.has_secret(old).await.unwrap(), "{old} is gone");
    }
    assert_eq!(
        secrets
            .get("model_api_key:default")
            .await
            .unwrap()
            .as_deref(),
        Some("old-key")
    );

    // A restart doesn't make a second one.
    let again = app_with_real_llm_on(store, &admin_env(&[])).await;
    assert_eq!(
        again.get("/v1/admin/models", KEY_B).await.1["models"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
