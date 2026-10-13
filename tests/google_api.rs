//! The Gmail and Calendar tools through Google's REST APIs (`GOOGLE_BACKEND=api`, the default),
//! against a local mock of the token endpoint and the APIs.

mod common;

use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use base64::Engine;
use common::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Mock {
    /// Every API request: method, path, query string, and authorization header.
    seen: Mutex<Vec<(String, String, String, String)>>,
    tokens_issued: Mutex<usize>,
    /// Reject the first access token issued with 401.
    reject_first_token: bool,
    /// The bodies posted to the drafts and freeBusy endpoints.
    posted: Mutex<Vec<Value>>,
}

impl Mock {
    fn record(&self, method: &str, path: &str, query: Option<String>, headers: &HeaderMap) -> bool {
        let auth = headers["authorization"].to_str().unwrap().to_string();
        let rejected = self.reject_first_token && auth == "Bearer tok-1";
        self.seen.lock().unwrap().push((
            method.into(),
            path.into(),
            query.unwrap_or_default(),
            auth,
        ));
        !rejected
    }

    fn requests(&self, path_prefix: &str) -> Vec<(String, String)> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, p, _, _)| p.starts_with(path_prefix))
            .map(|(_, p, q, _)| (p.clone(), q.clone()))
            .collect()
    }
}

fn b64(s: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s)
}

fn headers(pairs: &[(&str, &str)]) -> Value {
    json!(pairs
        .iter()
        .map(|(n, v)| json!({ "name": n, "value": v }))
        .collect::<Vec<_>>())
}

async fn token(
    State(mock): State<Arc<Mock>>,
    Form(_): Form<HashMap<String, String>>,
) -> Json<Value> {
    let mut n = mock.tokens_issued.lock().unwrap();
    *n += 1;
    Json(json!({ "access_token": format!("tok-{n}"), "expires_in": 3600 }))
}

async fn gmail_get(
    State(mock): State<Arc<Mock>>,
    Path(path): Path<String>,
    RawQuery(query): RawQuery,
    headers_in: HeaderMap,
) -> Response {
    if !mock.record("GET", &format!("/gmail/{path}"), query, &headers_in) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let parts: Vec<&str> = path.split('/').collect();
    let body = match parts.as_slice() {
        ["threads"] => json!({ "threads": [{ "id": "t1" }, { "id": "t2" }], "nextPageToken": "page-2" }),
        ["threads", id] => json!({ "id": id, "messages": [
            { "id": format!("{id}-a"), "labelIds": ["INBOX"], "snippet": "first",
              "payload": { "headers": headers(&[("Subject", &format!("Subject {id}")), ("From", "Alice <alice@example.com>"), ("Date", "Mon, 12 Oct 2026 09:00:00 -0700")]) } },
            { "id": format!("{id}-b"), "labelIds": ["INBOX", "UNREAD"], "snippet": "latest",
              "payload": { "headers": headers(&[("From", "Bob <bob@example.com>"), ("Date", "Mon, 12 Oct 2026 10:00:00 -0700")]),
                           "mimeType": "text/plain", "body": { "data": b64("See you then.") } } },
        ] }),
        ["messages", "m1"] => json!({ "id": "m1", "threadId": "t1", "payload": { "headers": headers(&[
            ("Subject", "Lunch?"), ("From", "Alice <alice@example.com>"),
            ("Message-ID", "<abc@mail.example.com>"),
        ]) } }),
        ["labels"] => {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": { "code": 403, "status": "PERMISSION_DENIED",
                    "message": "Gmail API has not been used in project 1 before or it is disabled." } })),
            )
                .into_response()
        }
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    Json(body).into_response()
}

async fn gmail_drafts(
    State(mock): State<Arc<Mock>>,
    headers_in: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    mock.record("POST", "/gmail/drafts", None, &headers_in);
    mock.posted.lock().unwrap().push(body.clone());
    Json(json!({ "id": "d1", "message": { "id": "m9", "threadId": body["message"]["threadId"] } }))
        .into_response()
}

async fn calendar_get(
    State(mock): State<Arc<Mock>>,
    Path(path): Path<String>,
    RawQuery(query): RawQuery,
    headers_in: HeaderMap,
) -> Response {
    if !mock.record("GET", &format!("/calendar/{path}"), query, &headers_in) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let body = match path.as_str() {
        "users/me/settings/timezone" => json!({ "value": "America/Los_Angeles" }),
        "calendars/primary/events" => json!({ "items": [{
            "id": "e1", "summary": "Standup", "status": "confirmed",
            "start": { "dateTime": "2026-10-13T09:00:00-07:00" },
            "end": { "dateTime": "2026-10-13T09:15:00-07:00" },
            "attendees": [{ "email": "me@example.com", "self": true, "responseStatus": "accepted" }],
        }] }),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    Json(body).into_response()
}

async fn free_busy(
    State(mock): State<Arc<Mock>>,
    headers_in: HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    mock.record("POST", "/calendar/freeBusy", None, &headers_in);
    mock.posted.lock().unwrap().push(body);
    Json(json!({ "calendars": {
        "primary": { "busy": [{ "start": "2026-10-19T17:00:00Z", "end": "2026-10-19T18:00:00Z" }] },
        "bob@example.com": { "busy": [{ "start": "2026-10-19T20:00:00Z", "end": "2026-10-19T23:00:00Z" }] },
        "carol@other.com": { "errors": [{ "domain": "global", "reason": "notFound" }] },
    } }))
}

async fn start(mock: Mock) -> (String, Arc<Mock>) {
    let mock = Arc::new(mock);
    let app = Router::new()
        .route("/token", post(token))
        .route("/gmail/drafts", post(gmail_drafts))
        .route("/gmail/{*path}", get(gmail_get))
        .route("/calendar/freeBusy", post(free_busy))
        .route("/calendar/{*path}", get(calendar_get))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, mock)
}

async fn app(script: Vec<Step>, base: &str, extra: &[(&str, &str)]) -> TestApp {
    let gmail = format!("{base}/gmail");
    let calendar = format!("{base}/calendar");
    let token = format!("{base}/token");
    let mut env = vec![
        ("GMAIL", "on"),
        ("CALENDAR", "on"),
        ("GMAIL_API_URL", gmail.as_str()),
        ("CALENDAR_API_URL", calendar.as_str()),
        ("GOOGLE_CLIENT_ID", "client"),
        ("GOOGLE_CLIENT_SECRET", "secret"),
        ("GOOGLE_REFRESH_TOKEN", "refresh"),
        ("GOOGLE_TOKEN_URL", token.as_str()),
    ];
    env.extend_from_slice(extra);
    TestApp::with_env(script, &env).await
}

async fn run_to_completion(app: &TestApp) {
    let created = app.create_run(KEY_A, "check my mail and calendar").await;
    let id = created["id"].as_str().unwrap().to_string();
    app.wait_status(KEY_A, &id, "completed").await;
}

/// The result of the tool call answered in LLM request `request`.
fn tool_result(app: &TestApp, request: usize) -> Value {
    app.llm.requests()[request]["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["content"][0]
        .clone()
}

fn result_json(app: &TestApp, request: usize) -> Value {
    let result = tool_result(app, request);
    assert!(result.get("is_error").is_none(), "{result}");
    serde_json::from_str(result["content"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn mail_is_searched_and_a_reply_is_drafted() {
    let (base, mock) = start(Mock::default()).await;
    let app = app(
        vec![
            Step::Reply(tool_use(
                "t1",
                "gmail_search_threads",
                json!({ "query": "is:unread", "pageSize": 2 }),
            )),
            Step::Reply(tool_use(
                "t2",
                "gmail_create_draft",
                json!({ "replyToMessageId": "m1", "body": "Sure, noon works." }),
            )),
            Step::Reply(tool_use("t3", "gmail_list_labels", json!({}))),
            Step::Reply(text("done")),
        ],
        &base,
        &[],
    )
    .await;
    run_to_completion(&app).await;

    let first = &app.llm.requests()[0];
    let names: Vec<&str> = first["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .filter(|n| n.starts_with("gmail_") || n.starts_with("calendar_"))
        .collect();
    // The default tools, available from the first request on: nothing is discovered.
    assert_eq!(
        names,
        [
            "gmail_search_threads",
            "gmail_get_thread",
            "gmail_list_labels",
            "gmail_list_drafts",
            "gmail_get_draft",
            "gmail_create_draft",
            "calendar_list_calendars",
            "calendar_list_events",
            "calendar_get_event",
            "calendar_search_events",
            "calendar_suggest_time",
        ]
    );
    let system = first["system"].as_str().unwrap();
    assert!(system.contains("use the gmail_ tools"), "{system}");
    assert!(system.contains("use the calendar_ tools"), "{system}");

    let search = result_json(&app, 1);
    assert_eq!(search["nextPageToken"], "page-2");
    let threads = search["threads"].as_array().unwrap();
    assert_eq!(threads.len(), 2);
    assert_eq!(threads[0]["id"], "t1");
    assert_eq!(threads[0]["subject"], "Subject t1");
    assert_eq!(threads[0]["from"], "Alice <alice@example.com>");
    assert_eq!(threads[0]["lastFrom"], "Bob <bob@example.com>");
    assert_eq!(threads[0]["messageCount"], 2);
    assert_eq!(threads[0]["unread"], true);
    assert_eq!(threads[0]["snippet"], "latest");
    let gmail = mock.requests("/gmail/threads");
    assert_eq!(
        gmail[0],
        ("/gmail/threads".into(), "maxResults=2&q=is%3Aunread".into())
    );
    assert!(
        gmail[1]
            .1
            .starts_with("format=metadata&metadataHeaders=Subject"),
        "{gmail:?}"
    );

    let draft = result_json(&app, 2);
    assert_eq!(draft["draftId"], "d1");
    assert_eq!(draft["threadId"], "t1");
    assert!(draft["note"].as_str().unwrap().contains("not been sent"));
    let posted = mock.posted.lock().unwrap()[0].clone();
    assert_eq!(posted["message"]["threadId"], "t1");
    let raw = base64::engine::general_purpose::URL_SAFE
        .decode(posted["message"]["raw"].as_str().unwrap())
        .unwrap();
    let raw = String::from_utf8(raw).unwrap();
    assert!(raw.contains("To: alice@example.com\r\n"), "{raw}");
    assert!(raw.contains("Subject: Re: Lunch?\r\n"), "{raw}");
    assert!(
        raw.contains("In-Reply-To: <abc@mail.example.com>\r\n"),
        "{raw}"
    );
    assert!(raw.contains(&base64::engine::general_purpose::STANDARD.encode("Sure, noon works.")));

    // Google's own explanation reaches the model.
    let labels = tool_result(&app, 3);
    assert_eq!(labels["is_error"], true);
    let message = labels["content"].as_str().unwrap();
    assert!(
        message.contains("Gmail API error (403 Forbidden)"),
        "{message}"
    );
    assert!(
        message.contains("has not been used in project"),
        "{message}"
    );

    assert_eq!(
        *mock.tokens_issued.lock().unwrap(),
        1,
        "the access token is cached"
    );
}

#[tokio::test]
async fn calendar_times_are_read_in_the_users_time_zone() {
    let (base, mock) = start(Mock {
        reject_first_token: true,
        ..Mock::default()
    })
    .await;
    let app = app(
        vec![
            Step::Reply(tool_use(
                "t1",
                "calendar_list_events",
                json!({ "startTime": "2026-10-12", "endTime": "2026-10-19T00:00:00" }),
            )),
            Step::Reply(tool_use(
                "t2",
                "calendar_suggest_time",
                json!({
                    "attendeeEmails": ["bob@example.com", "carol@other.com"],
                    "startTime": "2026-10-17T00:00:00",
                    "endTime": "2026-10-20T00:00:00",
                    "durationMinutes": 60,
                }),
            )),
            Step::Reply(text("done")),
        ],
        &base,
        &[("GMAIL", "off")],
    )
    .await;
    run_to_completion(&app).await;

    let events = result_json(&app, 1);
    assert_eq!(events["timeZone"], "America/Los_Angeles");
    assert!(events["now"].as_str().is_some());
    assert_eq!(events["from"], "Mon 2026-10-12T00:00:00-07:00");
    assert_eq!(events["events"][0]["summary"], "Standup");
    assert_eq!(events["events"][0]["myResponse"], "accepted");
    let (path, query) = mock.requests("/calendar/calendars")[0].clone();
    assert_eq!(path, "/calendar/calendars/primary/events");
    assert!(
        query.contains("timeMin=2026-10-12T07%3A00%3A00%2B00%3A00"),
        "{query}"
    );
    assert!(
        query.contains("timeMax=2026-10-19T07%3A00%3A00%2B00%3A00"),
        "{query}"
    );
    assert!(query.contains("singleEvents=true"), "{query}");
    assert!(query.contains("timeZone=America%2FLos_Angeles"), "{query}");

    // Saturday and Sunday are skipped; on Monday the user is busy 10-11 and Bob 13-16.
    let suggestion = result_json(&app, 2);
    assert_eq!(
        suggestion["freeWindows"],
        json!([
            { "start": "Mon 2026-10-19T09:00:00-07:00", "end": "Mon 2026-10-19T10:00:00-07:00" },
            { "start": "Mon 2026-10-19T11:00:00-07:00", "end": "Mon 2026-10-19T13:00:00-07:00" },
            { "start": "Mon 2026-10-19T16:00:00-07:00", "end": "Mon 2026-10-19T17:00:00-07:00" },
        ])
    );
    assert_eq!(
        suggestion["unknownAvailability"],
        json!([{ "id": "carol@other.com", "reason": "notFound" }])
    );
    let posted = mock.posted.lock().unwrap()[0].clone();
    assert_eq!(
        posted["items"],
        json!([{ "id": "primary" }, { "id": "bob@example.com" }, { "id": "carol@other.com" }])
    );

    // The rejected first token was replaced once, and the time zone was looked up once (the
    // rejected request, then its retry) and then kept.
    assert_eq!(*mock.tokens_issued.lock().unwrap(), 2);
    assert_eq!(
        mock.requests("/calendar/users/me/settings/timezone").len(),
        2
    );
    let seen = mock.seen.lock().unwrap();
    assert_eq!(seen[0].3, "Bearer tok-1");
    assert!(seen[1..]
        .iter()
        .all(|(_, _, _, auth)| auth == "Bearer tok-2"));
}

#[tokio::test]
async fn only_the_allowed_tools_are_offered() {
    let (base, _mock) = start(Mock::default()).await;
    let app = app(
        vec![Step::Reply(text("done"))],
        &base,
        &[
            ("GMAIL_TOOLS", "get_thread,send_message"),
            ("CALENDAR", "off"),
        ],
    )
    .await;
    run_to_completion(&app).await;
    let names: Vec<String> = app.llm.requests()[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .filter(|n| n.starts_with("gmail_") || n.starts_with("calendar_"))
        .collect();
    // send_message isn't implemented for the REST API, so it is left out rather than offered.
    assert_eq!(names, ["gmail_get_thread"]);
}
