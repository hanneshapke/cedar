//! The Gmail, Calendar, Drive, Sheets and Slides tools through Google's REST APIs (`GOOGLE_BACKEND=api`,
//! the default), against a local mock of the token endpoint and the APIs.

mod common;

use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, Method, StatusCode};
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
    /// The bodies posted to the Slides API: creating a presentation, then its batch updates.
    slides_posted: Mutex<Vec<(String, Value)>>,
    /// Refuse a batch update that creates an image, as Slides does for one it can't fetch.
    reject_images: bool,
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

/// `events.insert`: the event comes back as posted, with an ID and a link.
async fn calendar_post(
    State(mock): State<Arc<Mock>>,
    Path(path): Path<String>,
    RawQuery(query): RawQuery,
    headers_in: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    mock.record("POST", &format!("/calendar/{path}"), query, &headers_in);
    if path != "calendars/primary/events" {
        return StatusCode::NOT_FOUND.into_response();
    }
    mock.posted.lock().unwrap().push(body.clone());
    let mut event = body;
    event["id"] = json!("new-1");
    event["status"] = json!("confirmed");
    event["htmlLink"] = json!("https://calendar.google.com/event?eid=new-1");
    Json(event).into_response()
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

async fn drive_get(
    State(mock): State<Arc<Mock>>,
    Path(path): Path<String>,
    RawQuery(query): RawQuery,
    headers_in: HeaderMap,
) -> Response {
    let q = query.clone().unwrap_or_default();
    mock.record("GET", &format!("/drive/{path}"), query, &headers_in);
    let doc = json!({ "id": "doc-1", "name": "Q3 plan", "mimeType": "application/vnd.google-apps.document",
        "modifiedTime": "2026-10-01T10:00:00Z", "webViewLink": "https://docs.google.com/document/d/doc-1",
        "owners": [{ "displayName": "Ana", "emailAddress": "ana@example.com" }], "parents": ["folder-1"] });
    let sheet = json!({ "id": "sheet-1", "name": "Budget", "mimeType": "application/vnd.google-apps.spreadsheet",
        "modifiedTime": "2026-09-01T10:00:00Z" });
    let pdf = json!({ "id": "pdf-1", "name": "invoice.pdf", "mimeType": "application/pdf", "size": "11" });
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        ["files"] => {
            Json(json!({ "files": [doc, sheet, pdf], "nextPageToken": "page-2" })).into_response()
        }
        ["files", "doc-1"] => Json(doc).into_response(),
        ["files", "sheet-1"] => Json(sheet).into_response(),
        ["files", "pdf-1"] if q.contains("alt=media") => {
            ([("content-type", "application/pdf")], "%PDF-1.4 fake").into_response()
        }
        ["files", "pdf-1"] => Json(pdf).into_response(),
        ["files", "doc-1", "export"] if q.contains("text%2Fmarkdown") => (
            [("content-type", "text/markdown")],
            "# Q3 plan\n\nShip **cedar** 0.4.\n",
        )
            .into_response(),
        ["files", "doc-1", "export"] => {
            let body = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
            ([("content-type", body)], "PK fake docx").into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn sheets_get(
    State(mock): State<Arc<Mock>>,
    Path(path): Path<String>,
    RawQuery(query): RawQuery,
    headers_in: HeaderMap,
) -> Response {
    mock.record("GET", &format!("/sheets/{path}"), query, &headers_in);
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        ["sheet-1"] => Json(json!({ "spreadsheetId": "sheet-1", "spreadsheetUrl": "https://docs.google.com/spreadsheets/d/sheet-1",
            "properties": { "title": "Budget", "locale": "en_US", "timeZone": "Europe/Zurich" },
            "sheets": [
                { "properties": { "sheetId": 0, "title": "2026", "index": 0, "gridProperties": { "rowCount": 1000, "columnCount": 26 } } },
                { "properties": { "sheetId": 7, "title": "Notes", "index": 1, "hidden": true, "gridProperties": { "rowCount": 20, "columnCount": 5 } } },
            ] })).into_response(),
        ["sheet-1", "values", range] => {
            let rows: Vec<Value> = (1..=3)
                .map(|i| json!([format!("row {i}"), i * 10, "x"]))
                .collect();
            Json(json!({ "range": range, "majorDimension": "ROWS", "values": rows })).into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn slides_post(
    State(mock): State<Arc<Mock>>,
    path: Option<Path<String>>,
    headers_in: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let path = path.map(|Path(p)| format!("/slides/{p}")).unwrap_or("/slides".into());
    mock.record("POST", &path, None, &headers_in);
    mock.slides_posted.lock().unwrap().push((path.clone(), body.clone()));
    match path.as_str() {
        "/slides" => Json(json!({ "presentationId": "deck-1", "title": body["title"],
            "slides": [{ "objectId": "p" }] }))
        .into_response(),
        "/slides/deck-1:batchUpdate" => {
            let image = body["requests"].as_array().unwrap().iter().position(|r| r.get("createImage").is_some());
            match image {
                Some(i) if mock.reject_images => (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": { "code": 400, "message": format!("Invalid requests[{i}].createImage: There was a problem retrieving the image.") } })),
                )
                    .into_response(),
                _ => Json(json!({ "presentationId": "deck-1", "replies": [] })).into_response(),
            }
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn slides_get(
    State(mock): State<Arc<Mock>>,
    Path(path): Path<String>,
    RawQuery(query): RawQuery,
    headers_in: HeaderMap,
) -> Response {
    let q = query.clone().unwrap_or_default();
    mock.record("GET", &format!("/slides/{path}"), query, &headers_in);
    if path != "deck-1" {
        return StatusCode::NOT_FOUND.into_response();
    }
    // The slides made so far, each with the notes box Slides gives it.
    let made: Vec<String> = mock
        .slides_posted
        .lock()
        .unwrap()
        .iter()
        .flat_map(|(_, body)| body["requests"].as_array().cloned().unwrap_or_default())
        .filter_map(|r| r["createSlide"]["objectId"].as_str().map(String::from))
        .collect();
    if q.contains("speakerNotesObjectId") {
        let slides: Vec<Value> = made
            .iter()
            .map(|id| json!({ "objectId": id, "slideProperties": { "notesPage": { "notesProperties": {
                "speakerNotesObjectId": format!("{id}_notes") } } } }))
            .collect();
        return Json(json!({ "slides": slides })).into_response();
    }
    let run = |s: &str| json!({ "text": { "textElements": [{ "textRun": { "content": s } }] } });
    Json(json!({ "presentationId": "deck-1", "title": "Q3 review", "slides": [
        { "objectId": "s1", "pageElements": [{ "shape": run("Q3 review\n") }] },
        { "objectId": "s2", "pageElements": [{ "shape": run("Highlights\n") }, { "shape": run("Revenue up\nChurn down\n") }] },
    ] }))
    .into_response()
}

async fn start(mock: Mock) -> (String, Arc<Mock>) {
    let mock = Arc::new(mock);
    let app = Router::new()
        .route("/token", post(token))
        .route("/gmail/drafts", post(gmail_drafts))
        .route("/gmail/{*path}", get(gmail_get))
        .route("/calendar/freeBusy", post(free_busy))
        .route("/calendar/{*path}", get(calendar_get).post(calendar_post))
        .route("/drive/{*path}", get(drive_get))
        .route("/sheets/{*path}", get(sheets_get))
        .route("/slides", post(slides_post))
        .route("/slides/{*path}", get(slides_get).post(slides_post))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, mock)
}

async fn app(script: Vec<Step>, base: &str, extra: &[(&str, &str)]) -> TestApp {
    let gmail = format!("{base}/gmail");
    let calendar = format!("{base}/calendar");
    let drive = format!("{base}/drive");
    let sheets = format!("{base}/sheets");
    let slides = format!("{base}/slides");
    let token = format!("{base}/token");
    let mut env = vec![
        ("GMAIL", "on"),
        ("CALENDAR", "on"),
        ("GMAIL_API_URL", gmail.as_str()),
        ("CALENDAR_API_URL", calendar.as_str()),
        ("DRIVE_API_URL", drive.as_str()),
        ("SHEETS_API_URL", sheets.as_str()),
        ("SLIDES_API_URL", slides.as_str()),
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
            "calendar_create_event",
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
async fn an_event_is_added_to_the_calendar() {
    let (base, mock) = start(Mock::default()).await;
    let app = app(
        vec![
            Step::Reply(tool_use(
                "t1",
                "calendar_create_event",
                json!({ "summary": "Dentist", "startTime": "2026-10-13T15:00", "location": "Main St 1" }),
            )),
            Step::Reply(tool_use(
                "t2",
                "calendar_create_event",
                json!({ "summary": "Offsite", "startTime": "2026-10-20", "endTime": "2026-10-21",
                        "attendeeEmails": ["bob@example.com"] }),
            )),
            Step::Reply(tool_use(
                "t3",
                "calendar_create_event",
                json!({ "summary": "Backwards", "startTime": "2026-10-13T15:00", "endTime": "2026-10-13T14:00" }),
            )),
            Step::Reply(text("done")),
        ],
        &base,
        &[("GMAIL", "off")],
    )
    .await;
    run_to_completion(&app).await;

    let made = result_json(&app, 1);
    assert_eq!(made["event"]["id"], "new-1");
    assert_eq!(made["event"]["summary"], "Dentist");
    assert_eq!(made["event"]["start"], "2026-10-13T15:00:00-07:00");
    assert_eq!(made["event"]["link"], "https://calendar.google.com/event?eid=new-1");
    assert_eq!(made["note"], "The event is on the calendar.");
    let invited = result_json(&app, 2);
    assert_eq!(invited["event"]["allDay"], true);
    assert!(invited["note"].as_str().unwrap().contains("emailed the guest"), "{invited}");

    // A mistake in the input is the model's to fix: nothing was posted for it.
    let refused = tool_result(&app, 3);
    assert_eq!(refused["is_error"], true);
    assert!(refused["content"].as_str().unwrap().contains("endTime must be after startTime"));

    let posted = mock.posted.lock().unwrap().clone();
    assert_eq!(posted.len(), 2);
    assert_eq!(
        posted[0],
        json!({ "summary": "Dentist", "location": "Main St 1",
            "start": { "dateTime": "2026-10-13T15:00:00-07:00", "timeZone": "America/Los_Angeles" },
            "end": { "dateTime": "2026-10-13T16:00:00-07:00", "timeZone": "America/Los_Angeles" } })
    );
    assert_eq!(posted[1]["start"], json!({ "date": "2026-10-20" }));
    assert_eq!(posted[1]["end"], json!({ "date": "2026-10-22" }));
    assert_eq!(posted[1]["attendees"], json!([{ "email": "bob@example.com" }]));
    // Only an event with guests has Google send mail.
    let inserts = mock.requests("/calendar/calendars/primary/events");
    assert_eq!(inserts[0].1, "sendUpdates=none");
    assert_eq!(inserts[1].1, "sendUpdates=all");
}

#[tokio::test]
async fn drive_files_are_found_read_and_saved_and_sheets_are_read() {
    let (base, mock) = start(Mock::default()).await;
    let app = app(
        vec![
            Step::Reply(tool_use(
                "t1",
                "drive_search_files",
                json!({ "query": "Q3 plan", "kind": "document", "modifiedAfter": "2026-01-01" }),
            )),
            Step::Reply(tool_use(
                "t2",
                "drive_read_document",
                json!({ "fileId": "doc-1" }),
            )),
            Step::Reply(tool_use(
                "t3",
                "drive_read_document",
                json!({ "fileId": "pdf-1" }),
            )),
            Step::Reply(tool_use(
                "t4",
                "drive_download_file",
                json!({ "fileId": "pdf-1", "path": "in/invoice.pdf" }),
            )),
            Step::Reply(tool_use(
                "t5",
                "drive_download_file",
                json!({ "fileId": "doc-1", "path": "plan.docx" }),
            )),
            Step::Reply(tool_use(
                "t6",
                "sheets_get_spreadsheet",
                json!({ "spreadsheetId": "sheet-1" }),
            )),
            Step::Reply(tool_use(
                "t7",
                "sheets_read_range",
                json!({ "spreadsheetId": "sheet-1", "range": "Ana's", "maxRows": 3 }),
            )),
            Step::Reply(tool_use(
                "t8",
                "sheets_read_range",
                json!({ "spreadsheetId": "sheet-1", "range": "2026!A1:C2", "valueRender": "raw" }),
            )),
            Step::Reply(text("done")),
        ],
        &base,
        &[("GMAIL", "off"), ("CALENDAR", "off")],
    )
    .await;
    run_to_completion(&app).await;

    let first = app.llm.requests()[0].clone();
    let names: Vec<&str> = first["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .filter(|n| n.starts_with("drive_") || n.starts_with("sheets_"))
        .collect();
    assert_eq!(
        names,
        [
            "drive_search_files",
            "drive_get_file",
            "drive_read_document",
            "drive_download_file",
            "sheets_get_spreadsheet",
            "sheets_read_range",
        ]
    );
    let system = first["system"].as_str().unwrap();
    assert!(system.contains("drive_search_files"), "{system}");
    assert!(system.contains("sheets_ tools"), "{system}");

    // The search builds Drive's query language, with trashed files left out.
    let found = result_json(&app, 1);
    assert_eq!(found["files"][0]["id"], "doc-1");
    assert_eq!(found["files"][0]["kind"], "Google Doc");
    assert_eq!(found["files"][0]["owner"], "Ana <ana@example.com>");
    assert_eq!(found["files"][2]["size"], 11);
    assert_eq!(found["nextPageToken"], "page-2");
    let (_, query) = mock.requests("/drive/files")[0].clone();
    let decoded = urlencoding_decode(&query);
    assert!(decoded.contains("trashed = false and fullText contains 'Q3 plan' and mimeType = 'application/vnd.google-apps.document' and modifiedTime > '2026-01-01T00:00:00Z'"), "{decoded}");
    assert!(
        !decoded.contains("orderBy"),
        "relevance order for full-text searches: {decoded}"
    );

    // A Google Doc is exported as Markdown.
    let doc = result_json(&app, 2);
    assert_eq!(doc["format"], "markdown");
    assert_eq!(doc["content"], "# Q3 plan\n\nShip **cedar** 0.4.\n");
    assert_eq!(doc["file"]["name"], "Q3 plan");
    assert_eq!(doc["truncated"], false);
    assert!(doc.get("nextOffset").is_none());

    // A PDF isn't text: the error says what to do instead.
    let err = tool_result(&app, 3);
    assert_eq!(err["is_error"], true);
    assert!(
        err["content"]
            .as_str()
            .unwrap()
            .contains("drive_download_file"),
        "{err}"
    );

    // Downloads land in the workspace, a Doc as docx unless asked otherwise.
    assert_eq!(
        tool_result(&app, 4)["content"],
        "saved invoice.pdf to in/invoice.pdf: 13 bytes, application/pdf"
    );
    assert_eq!(tool_result(&app, 5)["content"].as_str().unwrap(), "saved Q3 plan to plan.docx: 12 bytes, application/vnd.openxmlformats-officedocument.wordprocessingml.document");
    let files = app.factory.sandbox(0).files.lock().unwrap().clone();
    assert_eq!(files["in/invoice.pdf"], b"%PDF-1.4 fake");
    assert_eq!(files["plan.docx"], b"PK fake docx");

    // The spreadsheet's tabs, then a tab by name in row windows, then an explicit range.
    let spreadsheet = result_json(&app, 6);
    assert_eq!(spreadsheet["title"], "Budget");
    assert_eq!(spreadsheet["sheets"][1]["title"], "Notes");
    assert_eq!(spreadsheet["sheets"][1]["hidden"], true);
    let rows = result_json(&app, 7);
    assert_eq!(rows["rows"][0], json!(["row 1", 10, "x"]));
    assert_eq!(rows["rowCount"], 3);
    assert_eq!(rows["more"], true);
    assert_eq!(rows["nextStartRow"], 4);
    let ranges = mock.requests("/sheets/sheet-1/values");
    assert_eq!(ranges[0].0, "/sheets/sheet-1/values/'Ana''s'!1:3");
    assert!(
        ranges[0].1.contains("valueRenderOption=FORMATTED_VALUE"),
        "{}",
        ranges[0].1
    );
    assert_eq!(ranges[1].0, "/sheets/sheet-1/values/2026!A1:C2");
    assert!(
        ranges[1].1.contains("valueRenderOption=UNFORMATTED_VALUE"),
        "{}",
        ranges[1].1
    );
    let explicit = result_json(&app, 8);
    assert_eq!(explicit["more"], false);
    assert!(explicit.get("nextStartRow").is_none());
}

/// A query string with its percent-encoding and `+` undone, for assertions.
fn urlencoding_decode(q: &str) -> String {
    let bytes = q.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap();
                out.push(u8::from_str_radix(hex, 16).unwrap());
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap()
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

#[tokio::test]
async fn mail_and_calendar_can_be_read_outside_a_run() {
    let (base, mock) = start(Mock::default()).await;
    let app = app(vec![], &base, &[]).await;
    let (status, body) = app
        .json(
            Method::POST,
            "/v1/tools/gmail_search_threads/call",
            Some(KEY_A),
            Some(json!({ "input": { "query": "is:unread" } })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["is_error"], false, "{body}");
    assert_eq!(body["json"]["threads"][0]["id"], "t1");
    assert_eq!(body["json"]["threads"][0]["messageCount"], 2);
    assert!(!mock.requests("/gmail/threads").is_empty());

    let (status, body) = app
        .json(
            Method::POST,
            "/v1/tools/calendar_list_events/call",
            Some(KEY_A),
            Some(json!({ "input": {} })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["is_error"], false, "{body}");
    assert!(body["json"]["events"].is_array(), "{body}");

    // Reading one thread, or writing a draft, stays inside runs.
    for name in ["gmail_get_thread", "gmail_create_draft", "calendar_get_event", "calendar_create_event"] {
        let (status, _) = app
            .json(Method::POST, &format!("/v1/tools/{name}/call"), Some(KEY_A), Some(json!({})))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name}");
    }
    assert_eq!(app.llm.call_count(), 0);
    assert!(app.factory.created.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_deck_is_made_from_an_outline_added_to_and_read_back() {
    let (base, mock) = start(Mock::default()).await;
    let app = app(
        vec![
            Step::Reply(tool_use(
                "t1",
                "slides_create_presentation",
                json!({ "title": "Q3 review", "theme": "editorial", "accent": "#0F766E", "footer": "Acme", "slides": [
                    { "title": "Q3 business review", "subtitle": "Board meeting" },
                    { "title": "Revenue grew 18%", "bullets": ["**Enterprise** led", "Churn fell"], "notes": "Start with enterprise." },
                    { "title": "The quarter in numbers", "stats": [{ "value": "$4.2M", "label": "Revenue" }] },
                    { "layout": "closing", "title": "Thank you", "notes": "Take questions." },
                ] }),
            )),
            Step::Reply(tool_use(
                "t2",
                "slides_append_slides",
                json!({ "presentationId": "https://docs.google.com/presentation/d/deck-1/edit", "insertAt": 4,
                    "slides": [{ "title": "By region", "table": { "headers": ["Region", "Revenue"], "rows": [["EMEA", "1,200"]] } }] }),
            )),
            Step::Reply(tool_use("t3", "slides_get_presentation", json!({ "presentationId": "deck-1" }))),
            Step::Reply(tool_use(
                "t4",
                "slides_create_presentation",
                json!({ "title": "Bad", "theme": "neon", "slides": [{ "title": "T" }] }),
            )),
            Step::Reply(tool_use(
                "t5",
                "slides_create_presentation",
                json!({ "title": "Bad", "slides": [{ "title": "Cover" }, { "title": "T", "columns": [{ "heading": "Only one" }] }] }),
            )),
            Step::Reply(text("done")),
        ],
        &base,
        &[("GMAIL", "off"), ("CALENDAR", "off")],
    )
    .await;
    run_to_completion(&app).await;

    let first = app.llm.requests()[0].clone();
    let names: Vec<&str> = first["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .filter(|n| n.starts_with("slides_"))
        .collect();
    assert_eq!(
        names,
        ["slides_create_presentation", "slides_append_slides", "slides_get_presentation"]
    );
    let system = first["system"].as_str().unwrap();
    assert!(system.contains("slides_create_presentation"), "{system}");

    let made = result_json(&app, 1);
    assert_eq!(made["presentationId"], "deck-1");
    assert_eq!(made["link"], "https://docs.google.com/presentation/d/deck-1/edit");
    assert_eq!(made["slideCount"], 4);
    assert_eq!(made["theme"], "editorial");
    assert!(made.get("warnings").is_none(), "{made}");

    let posted = mock.slides_posted.lock().unwrap().clone();
    // The presentation is created 16:9 with its name, drawn in one batch, then given its notes.
    assert_eq!(posted[0].0, "/slides");
    assert_eq!(posted[0].1["title"], "Q3 review");
    assert_eq!(posted[0].1["pageSize"]["width"], json!({ "magnitude": 9144000, "unit": "EMU" }));
    assert_eq!(posted[1].0, "/slides/deck-1:batchUpdate");
    let requests = posted[1].1["requests"].as_array().unwrap();
    let slide_ids: Vec<&str> =
        requests.iter().filter_map(|r| r["createSlide"]["objectId"].as_str()).collect();
    assert_eq!(slide_ids.len(), 4);
    // Slides' own first slide goes last, once the deck's slides exist.
    assert_eq!(requests.last().unwrap(), &json!({ "deleteObject": { "objectId": "p" } }));
    let texts: Vec<&str> =
        requests.iter().filter_map(|r| r["insertText"]["text"].as_str()).collect();
    assert!(texts.contains(&"Enterprise led\nChurn fell"), "{texts:?}");
    assert!(texts.contains(&"$4.2M") && texts.contains(&"Acme"), "{texts:?}");
    // The accent the call gave colours the deck.
    let teal = json!({ "red": 0.059, "green": 0.463, "blue": 0.431 });
    assert!(requests.iter().any(|r| r["updateShapeProperties"]["shapeProperties"]
        ["shapeBackgroundFill"]["solidFill"]["color"]["rgbColor"]
        == teal));
    assert_eq!(posted[2].0, "/slides/deck-1:batchUpdate");
    assert_eq!(
        posted[2].1["requests"],
        json!([
            { "insertText": { "objectId": format!("{}_notes", slide_ids[1]), "insertionIndex": 0, "text": "Start with enterprise." } },
            { "insertText": { "objectId": format!("{}_notes", slide_ids[3]), "insertionIndex": 0, "text": "Take questions." } },
        ])
    );

    // Added slides go where asked, with object IDs of their own.
    let added = result_json(&app, 2);
    assert_eq!(added["slidesAdded"], 1);
    let requests = posted[3].1["requests"].as_array().unwrap();
    assert_eq!(requests[0]["createSlide"]["insertionIndex"], 3);
    assert!(!slide_ids.contains(&requests[0]["createSlide"]["objectId"].as_str().unwrap()));
    assert!(requests.iter().any(|r| r.get("createTable").is_some()));
    assert!(!requests.iter().any(|r| r.get("deleteObject").is_some()));

    let outline = result_json(&app, 3);
    assert_eq!(outline["title"], "Q3 review");
    assert_eq!(outline["slides"][1]["text"], json!(["Highlights", "Revenue up / Churn down"]));

    // What can't be laid out is refused before anything is made in Drive.
    for (request, needle) in [(4, "unknown theme"), (5, "slide 2: a columns slide takes 2 to 4")] {
        let refused = tool_result(&app, request);
        assert_eq!(refused["is_error"], true, "{refused}");
        assert!(refused["content"].as_str().unwrap().contains(needle), "{refused}");
    }
    assert_eq!(mock.slides_posted.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn a_deck_whose_image_cannot_be_fetched_says_what_is_left_behind() {
    let (base, mock) = start(Mock { reject_images: true, ..Mock::default() }).await;
    let app = app(
        vec![
            Step::Reply(tool_use(
                "t1",
                "slides_create_presentation",
                json!({ "title": "Launch", "slides": [
                    { "title": "Launch" },
                    { "title": "The product", "imageUrl": "https://example.com/private.png" },
                ] }),
            )),
            Step::Reply(text("done")),
        ],
        &base,
        &[("GMAIL", "off"), ("CALENDAR", "off"), ("SLIDES_TOOLS", "create_presentation")],
    )
    .await;
    run_to_completion(&app).await;
    let offered = app.llm.requests()[0]["tools"].clone();
    let names: Vec<&str> = offered
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .filter(|n| n.starts_with("slides_"))
        .collect();
    assert_eq!(names, ["slides_create_presentation"]);
    let failed = tool_result(&app, 1);
    assert_eq!(failed["is_error"], true, "{failed}");
    let message = failed["content"].as_str().unwrap();
    assert!(message.contains("createImage"), "{message}");
    assert!(message.contains("public https link"), "{message}");
    assert!(message.contains("https://docs.google.com/presentation/d/deck-1/edit"), "{message}");
    assert_eq!(mock.slides_posted.lock().unwrap().len(), 2);
}
