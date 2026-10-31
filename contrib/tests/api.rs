//! The ingest's request path, against the in-memory store.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use cedar_contrib::config::Config;
use cedar_contrib::store::MemoryStore;
use cedar_contrib::{router, AppState};
use cedar_contrib_schema::example;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tower::ServiceExt;

struct Harness {
    app: Router,
    store: Arc<MemoryStore>,
}

impl Harness {
    fn new(extra: &[(&str, &str)]) -> Self {
        let mut env: HashMap<String, String> = [("CONTRIB_HMAC_KEY", "0123456789abcdef0123456789abcdef-test")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        for (k, v) in extra {
            env.insert(k.to_string(), v.to_string());
        }
        let cfg = Config::from_lookup(|k| env.get(k).cloned()).unwrap();
        let store = Arc::new(MemoryStore::default());
        let state = AppState::new(cfg, store.clone());
        Harness { app: router(state), store }
    }

    async fn call(&self, method: Method, uri: &str, token: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let req = match body {
            Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn submit(&self, bundle: &Value) -> (StatusCode, Value) {
        self.call(Method::POST, "/v1/contributions", None, Some(bundle.clone())).await
    }
}

fn bundle() -> Value {
    serde_json::to_value(example::bundle()).unwrap()
}

#[tokio::test]
async fn a_good_bundle_is_quarantined_with_a_receipt_and_can_be_withdrawn() {
    let h = Harness::new(&[]);
    let (status, receipt) = h.submit(&bundle()).await;
    assert_eq!(status, StatusCode::CREATED, "{receipt}");
    let id = receipt["receipt_id"].as_str().unwrap().to_string();
    let token = receipt["withdraw_token"].as_str().unwrap().to_string();
    assert_eq!(receipt["status"], "quarantined");
    assert_eq!(h.store.bundles.lock().unwrap().len(), 1);
    assert!(h.store.ledgers.lock().unwrap().contains_key("p_0123456789abcdef"));

    // Status needs the token; a wrong one looks like an unknown receipt.
    let (status, body) = h.call(Method::GET, &format!("/v1/contributions/{id}"), Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "quarantined");
    assert_eq!(body["released_in"], json!([]));
    let (status, _) = h.call(Method::GET, &format!("/v1/contributions/{id}"), Some("nope"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = h.call(Method::GET, &format!("/v1/contributions/{id}"), None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // The same conversation again is a duplicate, whoever sends it and whatever they say about it.
    let mut again = bundle();
    again["contributor"] = json!("p_someoneelse1234");
    again["outcome"] = json!({ "result": "failed" });
    let (status, err) = h.submit(&again).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(err["error"]["code"], "duplicate");

    // Withdrawn: gone from quarantine, hash freed and noted, status says so; always 204.
    let (status, _) = h.call(Method::DELETE, &format!("/v1/contributions/{id}"), Some(&token), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(h.store.bundles.lock().unwrap().len(), 0);
    assert_eq!(h.store.withdrawn.lock().unwrap().len(), 1);
    let (_, body) = h.call(Method::GET, &format!("/v1/contributions/{id}"), Some(&token), None).await;
    assert_eq!(body["status"], "withdrawn");
    let (status, _) = h.call(Method::DELETE, "/v1/contributions/no-such", Some("x"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // And it may be contributed again after a withdrawal.
    let (status, _) = h.submit(&bundle()).await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn bad_bundles_are_refused_with_coarse_codes_and_never_stored() {
    let h = Harness::new(&[("CONTRIB_MODELS", "deepseek-ai/*")]);
    let (status, err) = h.call(Method::POST, "/v1/contributions", None, Some(json!({ "hello": 1 }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["error"]["code"], "schema");

    let mut b = bundle();
    b["canary"] = json!("other");
    let (status, err) = h.submit(&b).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["error"]["code"], "schema");

    let mut b = bundle();
    b["turns"][0]["messages"][2]["content"][0]["tool_use_id"] = json!("zzz");
    let (status, err) = h.submit(&b).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["error"]["code"], "not_consistent");

    let mut b = bundle();
    b["model"]["name"] = json!("openai/gpt-5");
    let (status, err) = h.submit(&b).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["error"]["code"], "model_not_allowed");

    let mut b = bundle();
    b["turns"][0]["prompt"] = json!("use sk-abcdefghijklmnopqrstuvwxyz please");
    let (status, err) = h.submit(&b).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["error"]["code"], "secret_detected");
    assert!(err["error"]["message"].as_str().unwrap().starts_with("turn 1 prompt"));

    assert_eq!(h.store.bundles.lock().unwrap().len(), 0);
    assert_eq!(h.store.hashes.lock().unwrap().len(), 0);

    let (status, _) = h.call(Method::GET, "/v1/nothing", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn quotas_apply_per_pseudonym_and_per_lookalike_cluster() {
    let h = Harness::new(&[("CONTRIB_QUOTA_NEW", "2"), ("CONTRIB_QUOTA_CLUSTER", "1")]);
    // A new pseudonym gets two on its first day.
    for i in 0..3 {
        let mut b = bundle();
        b["turns"][0]["prompt"] = json!(format!("list the files {i}"));
        let (status, err) = h.submit(&b).await;
        if i < 2 {
            assert_eq!(status, StatusCode::CREATED, "{err}");
        } else {
            assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(err["error"]["code"], "quota");
            assert!(err["error"]["message"].as_str().unwrap().contains("first day"));
        }
    }
    // Unlinked bundles that look alike (same prompts, different answers) share a daily allowance.
    let mut b = bundle();
    b["contributor"] = Value::Null;
    b["turns"][0]["prompt"] = json!("please write a python script that prints the first ten primes and run it");
    b["turns"][0]["messages"][0]["content"][0]["text"] = b["turns"][0]["prompt"].clone();
    let (status, _) = h.submit(&b).await;
    assert_eq!(status, StatusCode::CREATED);
    b["turns"][0]["messages"][3]["content"][0]["text"] = json!("Here are the primes: 2 3 5 7 11 13 17 19 23 29");
    let (status, err) = h.submit(&b).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{err}");
    assert!(err["error"]["message"].as_str().unwrap().contains("look alike"));
}

#[tokio::test]
async fn the_policy_says_what_is_taken_and_the_rate_limit_bites() {
    let h = Harness::new(&[("CONTRIB_MODELS", "deepseek-ai/*"), ("CONTRIB_IP_PER_MINUTE", "2")]);
    let (status, policy) = h.call(Method::GET, "/v1/policy", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(policy["models"], json!(["deepseek-ai/*"]));
    assert_eq!(policy["licenses"], json!(["cc0", "cc-by-4.0"]));
    assert_eq!(policy["schema_versions"], json!([1]));
    // Two requests per minute from one client (no peer address in tests: all "unknown").
    for i in 0..3 {
        let (status, _) = h.call(Method::GET, "/v1/contributions/x", Some("t"), None).await;
        assert_eq!(status, if i < 2 { StatusCode::UNAUTHORIZED } else { StatusCode::TOO_MANY_REQUESTS });
    }
}
