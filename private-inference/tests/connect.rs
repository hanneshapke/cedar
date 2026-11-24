//! The connect flow against a stub that answers like cedar-plg's `docs/contract.md` (draft 1):
//! one grant per verifier and `invalid_grant` afterwards, `slow_down`, a 503 while the key is
//! being created, denial and expiry.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use cedar_private_inference::{Challenge, Client, Error, PollOutcome, Started, LARGE_ID, SMALL_ID};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Decision {
    Pending,
    Approved,
    Denied,
    Expired,
    Delivered,
}

#[derive(Default)]
struct Stub {
    /// challenge -> decision
    requests: Mutex<HashMap<String, Decision>>,
    /// What happens to a request once it has been polled this many times.
    decide_after: Mutex<Option<(usize, Decision)>>,
    polls: Mutex<HashMap<String, usize>>,
    /// Answer `slow_down` to these poll numbers (1-based).
    slow_down_on: Mutex<Vec<usize>>,
    /// Answer 503 this many times before handing over an approved grant.
    unavailable: Mutex<usize>,
    client_names: Mutex<Vec<Option<String>>>,
}

fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn error(status: u16, code: &str) -> (StatusCode, Json<Value>) {
    (
        StatusCode::from_u16(status).unwrap(),
        Json(json!({ "error": code })),
    )
}

async fn start(
    State(stub): State<Arc<Stub>>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let challenge = body["challenge"].as_str().unwrap_or_default().to_string();
    let valid = challenge.len() == 43
        && challenge
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                json!({ "error": "invalid_request", "error_description": "challenge must be base64url(sha256(verifier))" }),
            ),
        );
    }
    stub.client_names
        .lock()
        .unwrap()
        .push(body["client_name"].as_str().map(String::from));
    let mut requests = stub.requests.lock().unwrap();
    if requests.contains_key(&challenge) {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                json!({ "error": "invalid_request", "error_description": "challenge already used" }),
            ),
        );
    }
    requests.insert(challenge.clone(), Decision::Pending);
    (
        StatusCode::OK,
        Json(json!({
            "user_code": "NHKK-KZGJ",
            "verification_uri": "http://plg.test/connect",
            "verification_uri_complete": format!("http://plg.test/connect?challenge={challenge}"),
            "expires_in": 900,
            "interval": 0,
        })),
    )
}

async fn poll(State(stub): State<Arc<Stub>>, Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
    let challenge = challenge_for(body["verifier"].as_str().unwrap_or_default());
    let n = {
        let mut polls = stub.polls.lock().unwrap();
        let n = polls.entry(challenge.clone()).or_default();
        *n += 1;
        *n
    };
    let mut requests = stub.requests.lock().unwrap();
    let Some(decision) = requests.get_mut(&challenge) else {
        return error(400, "invalid_grant");
    };
    if let Some((after, then)) = *stub.decide_after.lock().unwrap() {
        if n >= after && *decision == Decision::Pending {
            *decision = then;
        }
    }
    match *decision {
        Decision::Delivered => error(400, "invalid_grant"),
        Decision::Denied => error(400, "access_denied"),
        Decision::Expired => error(400, "expired_token"),
        Decision::Pending if stub.slow_down_on.lock().unwrap().contains(&n) => {
            error(400, "slow_down")
        }
        Decision::Pending => error(400, "authorization_pending"),
        Decision::Approved => {
            let mut unavailable = stub.unavailable.lock().unwrap();
            if *unavailable > 0 {
                *unavailable -= 1;
                return error(503, "temporarily_unavailable");
            }
            *decision = Decision::Delivered;
            (
                StatusCode::OK,
                Json(json!({
                    "base_url": "https://api.doubleword.ai/v1",
                    "api_key": "sk-test",
                    "provider": "doubleword",
                    "models": [
                        { "id": "deepseek-v4.1-flash", "name": "DeepSeek V4.1 Flash 120B", "role": "small", "params_b": 120 },
                        { "id": "kimi-k3.1-pro", "name": "Kimi K3.1 Pro 2.8T", "role": "large", "params_b": 2800 },
                    ],
                    "account_url": "http://plg.test/account",
                })),
            )
        }
    }
}

async fn serve(stub: Arc<Stub>) -> String {
    let app = Router::new()
        .route("/api/connect/start", post(start))
        .route("/api/connect/poll", post(poll))
        .with_state(stub);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn client(url: &str) -> Client {
    Client::new(url)
        .unwrap()
        .with_poll_timing(Duration::from_millis(1), Duration::from_millis(5))
}

#[tokio::test]
async fn the_key_is_handed_over_once() {
    let stub = Arc::new(Stub::default());
    let url = serve(stub.clone()).await;
    let client = client(&format!("{url}/"));

    let challenge = Challenge::new();
    let started = client
        .start(
            &challenge,
            Some(&format!("Cedar Desktop on {}", "x".repeat(200))),
        )
        .await
        .unwrap();
    assert_eq!(started.user_code, "NHKK-KZGJ");
    assert!(started
        .verification_uri_complete
        .ends_with(challenge.challenge()));
    let name = stub.client_names.lock().unwrap()[0].clone().unwrap();
    assert_eq!(
        name.chars().count(),
        100,
        "client_name is cut to what the service keeps"
    );

    // The same challenge can't be started twice.
    let again = client.start(&challenge, None).await.unwrap_err();
    assert!(
        matches!(&again, Error::Refused { status: 400, message } if message == "challenge already used"),
        "{again:?}"
    );

    assert_eq!(
        client.poll(challenge.verifier()).await.unwrap(),
        PollOutcome::Pending
    );
    stub.requests
        .lock()
        .unwrap()
        .insert(challenge.challenge().into(), Decision::Approved);
    let PollOutcome::Granted(grant) = client.poll(challenge.verifier()).await.unwrap() else {
        panic!("approved, so granted");
    };
    assert_eq!(grant.base_url, "https://api.doubleword.ai/v1");
    assert_eq!(grant.api_key, "sk-test");
    assert_eq!(grant.account_url, "http://plg.test/account");
    let entries: Vec<_> = grant
        .entries()
        .iter()
        .map(|(id, m)| (*id, m.id.clone(), m.name.clone()))
        .collect();
    assert_eq!(
        entries,
        [
            (
                SMALL_ID,
                "deepseek-v4.1-flash".into(),
                "DeepSeek V4.1 Flash 120B".into()
            ),
            (
                LARGE_ID,
                "kimi-k3.1-pro".into(),
                "Kimi K3.1 Pro 2.8T".into()
            ),
        ]
    );

    // Once only, and an unknown verifier gets the same answer.
    assert!(matches!(
        client.poll(challenge.verifier()).await,
        Err(Error::InvalidGrant)
    ));
    assert!(matches!(
        client.poll(Challenge::new().verifier()).await,
        Err(Error::InvalidGrant)
    ));
}

#[tokio::test]
async fn connect_waits_through_pending_slow_down_and_a_key_still_being_made() {
    let stub = Arc::new(Stub::default());
    *stub.decide_after.lock().unwrap() = Some((4, Decision::Approved));
    *stub.slow_down_on.lock().unwrap() = vec![2];
    *stub.unavailable.lock().unwrap() = 2;
    let url = serve(stub.clone()).await;
    let client = client(&url);

    let challenge = Challenge::new();
    let started = client.start(&challenge, None).await.unwrap();
    assert_eq!(stub.client_names.lock().unwrap()[0], None);
    let grant = client.connect(&challenge, &started).await.unwrap();
    assert_eq!(grant.provider, "doubleword");
    // Pending, slow_down, pending, then approved: two 503s, then the grant.
    assert_eq!(stub.polls.lock().unwrap()[challenge.challenge()], 6);
}

#[tokio::test]
async fn connect_stops_when_declined_or_expired() {
    for (decision, expected) in [(Decision::Denied, "Denied"), (Decision::Expired, "Expired")] {
        let stub = Arc::new(Stub::default());
        *stub.decide_after.lock().unwrap() = Some((2, decision));
        let url = serve(stub.clone()).await;
        let client = client(&url);
        let challenge = Challenge::new();
        let started = client.start(&challenge, None).await.unwrap();
        let err = client.connect(&challenge, &started).await.unwrap_err();
        assert_eq!(format!("{err:?}"), expected);
        assert_eq!(stub.polls.lock().unwrap()[challenge.challenge()], 2);
    }

    // Nobody approves before expires_in runs out: stop without the service saying so.
    let stub = Arc::new(Stub::default());
    let url = serve(stub).await;
    let client = client(&url);
    let challenge = Challenge::new();
    let mut started: Started = client.start(&challenge, None).await.unwrap();
    started.expires_in = 0;
    assert!(matches!(
        client.connect(&challenge, &started).await,
        Err(Error::Expired)
    ));
}

#[tokio::test]
async fn an_unreachable_service_and_a_malformed_grant_are_errors() {
    let client = client("http://127.0.0.1:1");
    assert!(matches!(
        client.start(&Challenge::new(), None).await,
        Err(Error::Unreachable(_))
    ));
    assert!(matches!(
        Client::new("not a url"),
        Err(Error::BadServiceUrl(_))
    ));

    // A grant with no usable model is refused rather than half registered.
    let app = Router::new().route(
        "/api/connect/poll",
        post(|| async {
            Json(
                json!({ "base_url": "https://x/v1", "api_key": "k", "provider": "p",
                         "models": [], "account_url": "https://x/account" }),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = Client::new(&format!("http://{addr}")).unwrap();
    assert!(matches!(client.poll("v").await, Err(Error::Malformed(_))));
}
