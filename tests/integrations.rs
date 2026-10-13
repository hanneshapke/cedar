//! Per-run tool selection, the tool catalog, and integrations set up through the admin API (a
//! Brave Search key, "Connect Google") against local mocks of Brave and Google.

mod common;

use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use base64::Engine;
use cedar::config::DEFAULT_MODEL;
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
        names(&["fetch_url", "download_file", "weather_forecast", "show_map"])
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
            "browser",
            "gmail",
            "calendar"
        ]
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
        (
            Method::PUT,
            "/v1/admin/integrations/brave",
            Some(json!({ "api_key": "k" })),
        ),
        (Method::DELETE, "/v1/admin/integrations/brave", None),
        (
            Method::POST,
            "/v1/admin/integrations/google/authorize",
            Some(json!({ "redirect_uri": "http://x/cb" })),
        ),
        (Method::DELETE, "/v1/admin/integrations/google", None),
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
    );

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
    );
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

// -- defaults and the model server --------------------------------------------------------------

#[tokio::test]
async fn defaults_are_reported_to_any_key() {
    let app = TestApp::with_env(
        vec![],
        &[("SYSTEM_PROMPT", "Be terse."), ("LLM_MODEL", "m-9")],
    )
    .await;
    let (status, body) = app.get("/v1/defaults", KEY_A).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["system"], "Be terse.");
    assert_eq!(body["model"], "m-9");
    assert_eq!(body["max_turns_cap"], 10);
}

#[derive(Default)]
struct ModelServer {
    completions: Mutex<Vec<HeaderMap>>,
}

/// An OpenAI-compatible model server that lists `m-1` and, when `key` is set, wants it.
async fn start_model_server(key: Option<&'static str>) -> (String, Arc<ModelServer>) {
    async fn models(
        State((key, _)): State<(Option<&'static str>, Arc<ModelServer>)>,
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
        State((_, server)): State<(Option<&'static str>, Arc<ModelServer>)>,
        headers: HeaderMap,
    ) -> Json<Value> {
        server.completions.lock().unwrap().push(headers);
        Json(
            json!({ "choices": [{ "message": { "role": "assistant", "content": "from the new server" }, "finish_reason": "stop" }],
                     "usage": { "prompt_tokens": 3, "completion_tokens": 2 } }),
        )
    }
    let server = Arc::new(ModelServer::default());
    let app = Router::new()
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(completions))
        .with_state((key, server.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), server)
}

/// A test app whose model client is the real, swappable one, as in `main.rs`.
async fn app_with_real_llm(env: &[(&str, &str)]) -> TestApp {
    let cfg = test_config(env);
    let llm = Arc::new(cedar::llm::SwappableLlm::new(
        cedar::llm::ChatCompletionsClient::new(cfg.llm_base_url.clone(), None)
            .unwrap()
            .with_retry(1, std::time::Duration::from_millis(1)),
    ));
    let factory = FakeFactory::new();
    let browsers = Arc::new(FakeBrowserFactory::default());
    let state = cedar::api::AppState::with_browsers(
        cfg,
        Store::connect_memory().await.unwrap(),
        llm.clone(),
        factory.clone(),
        browsers.clone(),
    )
    .unwrap();
    state.integrations.attach_llm(llm);
    state.integrations.reload().await.unwrap();
    TestApp {
        router: cedar::api::router(state.clone()),
        state,
        llm: ScriptedLlm::new(vec![]),
        factory,
        browsers,
    }
}

#[tokio::test]
async fn an_admin_can_point_cedar_at_another_model_server() {
    let (server_url, server) = start_model_server(Some("llm-key")).await;
    let app = app_with_real_llm(&admin_env(&[("LLM_MODEL", "m-1")])).await;

    let (_, status) = app.get("/v1/admin/integrations", KEY_B).await;
    assert_eq!(status["llm"]["source"], "default");
    assert_eq!(status["llm"]["base_url"], "http://localhost:8000/v1");
    assert_eq!(status["llm"]["api_key_set"], false);

    let put = |body: Value| {
        app.json(
            Method::PUT,
            "/v1/admin/integrations/llm",
            Some(KEY_B),
            Some(body),
        )
    };
    // Nothing listening there.
    let (code, body) = put(json!({ "base_url": "http://127.0.0.1:1" })).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("could not reach"),
        "{body}"
    );
    // The server wants a key.
    let (code, body) = put(json!({ "base_url": server_url })).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]["message"].as_str().unwrap().contains("401"),
        "{body}"
    );
    let (code, body) = put(json!({ "base_url": "not a url" })).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{body}");

    // The path defaults to /v1.
    let (code, body) = put(json!({ "base_url": server_url, "api_key": "llm-key" })).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["base_url"], format!("{server_url}/v1"));
    assert_eq!(body["source"], "ui");
    assert_eq!(body["api_key_set"], true);
    assert_eq!(body["models"], json!(["m-1"]));
    let (_, _, raw) = app
        .call(Method::GET, "/v1/admin/integrations", Some(KEY_B), None)
        .await;
    assert!(!raw.contains("llm-key"));

    // The next run talks to the new server, with its key.
    let run = app.create_run(KEY_A, "hello").await;
    let done = app
        .wait_status(KEY_A, run["id"].as_str().unwrap(), "completed")
        .await;
    assert_eq!(done["final_text"], "from the new server");
    assert_eq!(
        server.completions.lock().unwrap()[0]["authorization"],
        "Bearer llm-key"
    );

    // Changing the address without api_key keeps the saved key.
    let (code, _) = put(json!({ "base_url": format!("{server_url}/v1/") })).await;
    assert_eq!(code, StatusCode::OK);

    let (code, _) = app
        .json(
            Method::DELETE,
            "/v1/admin/integrations/llm",
            Some(KEY_B),
            None,
        )
        .await;
    assert_eq!(code, StatusCode::NO_CONTENT);
    let (_, status) = app.get("/v1/admin/integrations", KEY_B).await;
    assert_eq!(status["llm"]["source"], "default");
    assert_eq!(status["llm"]["api_key_set"], false);

    let (code, _) = app
        .json(
            Method::PUT,
            "/v1/admin/integrations/llm",
            Some(KEY_A),
            Some(json!({ "base_url": server_url })),
        )
        .await;
    assert_eq!(code, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_admin_can_set_the_model_name_with_the_address() {
    let (server_url, _server) = start_model_server(None).await;
    let app = app_with_real_llm(&admin_env(&[])).await;
    let put = |body: Value| {
        app.json(
            Method::PUT,
            "/v1/admin/integrations/llm",
            Some(KEY_B),
            Some(body),
        )
    };

    let (_, status) = app.get("/v1/admin/integrations", KEY_B).await;
    assert_eq!(status["llm"]["model_source"], "default");
    assert_eq!(status["llm"]["default_model"], DEFAULT_MODEL);

    for bad in ["x".repeat(101), "two\nlines".to_string()] {
        let (code, body) = put(json!({ "base_url": server_url, "model": bad })).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{body}");
    }

    let (code, body) = put(json!({ "base_url": server_url, "model": " m-1 " })).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["default_model"], "m-1");
    assert_eq!(body["model_source"], "ui");

    // New runs, and the defaults clients show, use it.
    let (_, defaults) = app.get("/v1/defaults", KEY_A).await;
    assert_eq!(defaults["model"], "m-1");
    let run = app.create_run(KEY_A, "hello").await;
    assert_eq!(run["model"], "m-1");
    app.wait_status(KEY_A, run["id"].as_str().unwrap(), "completed")
        .await;

    // Leaving the model out keeps it; an empty one forgets it.
    let (code, body) = put(json!({ "base_url": server_url })).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["default_model"], "m-1");
    let (code, body) = put(json!({ "base_url": server_url, "model": "" })).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["default_model"], DEFAULT_MODEL);
    assert_eq!(body["model_source"], "default");

    // Going back to the default server forgets the name too.
    put(json!({ "base_url": server_url, "model": "m-1" })).await;
    let (code, _) = app
        .json(
            Method::DELETE,
            "/v1/admin/integrations/llm",
            Some(KEY_B),
            None,
        )
        .await;
    assert_eq!(code, StatusCode::NO_CONTENT);
    let (_, status) = app.get("/v1/admin/integrations", KEY_B).await;
    assert_eq!(status["llm"]["default_model"], DEFAULT_MODEL);
    assert_eq!(status["llm"]["model_source"], "default");
}

#[tokio::test]
async fn a_model_name_set_in_the_environment_is_locked() {
    let (server_url, _server) = start_model_server(None).await;
    let app = app_with_real_llm(&admin_env(&[("LLM_MODEL", "from-env")])).await;
    let (_, status) = app.get("/v1/admin/integrations", KEY_B).await;
    assert_eq!(status["llm"]["model_source"], "env");
    assert_eq!(status["llm"]["default_model"], "from-env");

    let put = |body: Value| {
        app.json(
            Method::PUT,
            "/v1/admin/integrations/llm",
            Some(KEY_B),
            Some(body),
        )
    };
    let (code, _) = put(json!({ "base_url": server_url, "model": "other" })).await;
    assert_eq!(code, StatusCode::CONFLICT);
    // The address can still be changed; the name stays.
    let (code, body) = put(json!({ "base_url": server_url })).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["default_model"], "from-env");
}

#[tokio::test]
async fn a_model_server_set_in_the_environment_is_locked() {
    let app = app_with_real_llm(&admin_env(&[("LLM_BASE_URL", "http://gpu:9000/v1")])).await;
    let (_, status) = app.get("/v1/admin/integrations", KEY_B).await;
    assert_eq!(status["llm"]["source"], "env");
    assert_eq!(status["llm"]["base_url"], "http://gpu:9000/v1");
    let (code, _) = app
        .json(
            Method::PUT,
            "/v1/admin/integrations/llm",
            Some(KEY_B),
            Some(json!({ "base_url": "http://other:8000" })),
        )
        .await;
    assert_eq!(code, StatusCode::CONFLICT);

    // A test app with a scripted model can't be repointed at all.
    let scripted = TestApp::with_env(vec![], &admin_env(&[])).await;
    let (_, status) = scripted.get("/v1/admin/integrations", KEY_B).await;
    assert_eq!(status["llm"]["changeable"], false);
}
