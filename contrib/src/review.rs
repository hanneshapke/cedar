//! The review page: held bundles with what the screener found, and Accept / Reject. Served at
//! `/review` when `CONTRIB_REVIEW_TOKEN` is set, to whoever presents the token (the design puts
//! IAP in front for org accounts; the token is the floor). Every decision is an audit line.
//!
//! Reviewers see the bundle's text (stand-ins and all), the flags, scores and notes, never a
//! receipt's withdraw token.

use crate::api::{ApiError, AppState};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde_json::{json, Value};
use subtle::ConstantTimeEq;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/review", get(page))
        .route("/review/api/held", get(held))
        .route("/review/api/{id}", get(one))
        .route("/review/api/{id}/accept", post(accept))
        .route("/review/api/{id}/reject", post(reject))
}

fn authorized(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(expected) = &state.cfg.review_token else { return Err(ApiError::NotFound) };
    let given = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).unwrap_or_default();
    if expected.as_bytes().ct_eq(given.as_bytes()).into() {
        Ok(())
    } else {
        Err(ApiError::Unauthorized)
    }
}

async fn page(State(state): State<AppState>) -> Response {
    if state.cfg.review_token.is_none() {
        return ApiError::NotFound.into_response();
    }
    Html(PAGE).into_response()
}

/// Held receipts, oldest first: id, day, pseudonym (hashed), flags, scores, notes, tags.
async fn held(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    authorized(&state, &headers)?;
    let list: Vec<Value> = state
        .store
        .list_receipts()
        .await?
        .into_iter()
        .filter(|r| r.status == "held")
        .map(|r| {
            json!({
                "receipt_id": r.receipt_id,
                "day": r.day,
                "contributor": r.pseudonym.as_deref().map(hash8),
                "flags": r.verdict.as_ref().map(|v| v.flags.clone()).unwrap_or_default(),
                "scores": r.verdict.as_ref().map(|v| v.scores.clone()).unwrap_or_default(),
                "notes": r.verdict.as_ref().map(|v| v.notes.clone()).unwrap_or_default(),
                "tags": r.tags,
            })
        })
        .collect();
    Ok(Json(json!({ "held": list })))
}

/// The reviewer sees who, only as a hash: enough to tell two contributors apart, not to look one up.
fn hash8(pseudonym: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(&Sha256::digest(pseudonym.as_bytes())[..4])
}

async fn one(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    authorized(&state, &headers)?;
    let Some(r) = state.store.get_receipt(&id).await? else { return Err(ApiError::NotFound) };
    let bundle: Value = match state.store.get_bundle(&r).await? {
        Some(bytes) => serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        None => Value::Null,
    };
    Ok(Json(json!({ "receipt_id": r.receipt_id, "status": r.status, "verdict": r.verdict, "tags": r.tags, "bundle": bundle })))
}

async fn decide(state: &AppState, id: &str, status: &str) -> Result<Json<Value>, ApiError> {
    let Some(mut r) = state.store.get_receipt(id).await? else { return Err(ApiError::NotFound) };
    if r.status != "held" {
        return Err(ApiError::Refused { status: StatusCode::CONFLICT, code: "not_held", message: format!("the bundle is {}", r.status) });
    }
    let day = Utc::now().format("%Y-%m-%d").to_string();
    r.status = status.into();
    if let Some(v) = &mut r.verdict {
        v.status = status.into();
        v.by = "reviewer".into();
        v.decided_day = day.clone();
    }
    state.store.update_receipt(&r).await?;
    state
        .store
        .put_audit(&json!({ "day": day, "receipt_id": r.receipt_id, "sha256": r.sha256, "by": "reviewer", "status": status, "flags": r.verdict.as_ref().map(|v| v.flags.clone()).unwrap_or_default() }))
        .await?;
    tracing::info!(receipt = %r.receipt_id, status, "reviewed");
    Ok(Json(json!({ "receipt_id": r.receipt_id, "status": status })))
}

async fn accept(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    authorized(&state, &headers)?;
    decide(&state, &id, "accepted").await
}

async fn reject(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    authorized(&state, &headers)?;
    decide(&state, &id, "rejected").await
}

/// The page: one file, no dependencies. Asks for the token once (kept for the tab), lists what
/// is held, shows a bundle's turns and the screener's notes, and offers Accept and Reject.
const PAGE: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>cedar-contrib review</title>
<style>
  body { margin: 0; font: 14px/1.45 system-ui, sans-serif; color: #24292f; background: #fff; }
  header { display: flex; gap: 12px; align-items: center; padding: 10px 16px; border-bottom: 1px solid #d0d7de; }
  main { display: grid; grid-template-columns: 340px 1fr; min-height: calc(100vh - 45px); }
  nav { border-right: 1px solid #d0d7de; overflow: auto; }
  nav button { display: block; width: 100%; text-align: left; padding: 10px 14px; border: 0; border-bottom: 1px solid #eaeef2; background: none; font: inherit; cursor: pointer; }
  nav button[aria-current="true"] { background: #f6f8fa; }
  .flag { display: inline-block; margin: 2px 4px 0 0; padding: 0 6px; border: 1px solid #d0d7de; border-radius: 999px; font: 500 11px/1.7 ui-monospace, monospace; }
  .flag.bad { border-color: #cf222e; color: #cf222e; }
  section { padding: 14px 18px; overflow: auto; }
  pre { padding: 8px 10px; background: #f6f8fa; border: 1px solid #d0d7de; border-radius: 6px; white-space: pre-wrap; overflow-wrap: anywhere; font-size: 12.5px; }
  .turn { margin: 10px 0; padding: 8px 12px; border: 1px solid #d0d7de; border-radius: 8px; }
  .actions button { padding: 6px 14px; margin-right: 8px; border: 1px solid #d0d7de; border-radius: 6px; background: #fff; font: inherit; cursor: pointer; }
  .actions .ok { border-color: #1a7f37; color: #1a7f37; } .actions .no { border-color: #cf222e; color: #cf222e; }
  .muted { color: #57606a; }
  input { padding: 5px 8px; border: 1px solid #d0d7de; border-radius: 6px; font: inherit; }
</style></head>
<body>
<header><strong>cedar-contrib review</strong><span class="muted" id="count"></span><span style="flex:1"></span>
  <input id="token" type="password" placeholder="review token" size="28"><button id="go">Load</button></header>
<main><nav id="list"></nav><section id="view"><p class="muted">Enter the review token and load the queue.</p></section></main>
<script>
const $ = (s) => document.querySelector(s);
let token = sessionStorage.getItem('contrib-review-token') || '';
$('#token').value = token;
async function api(path, method = 'GET') {
  const r = await fetch(path, { method, headers: { authorization: 'Bearer ' + token } });
  if (!r.ok) throw new Error((await r.json().catch(() => ({}))).error?.message || r.status);
  return r.json();
}
function el(tag, cls, text) { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; }
let held = [];
async function load() {
  token = $('#token').value.trim(); sessionStorage.setItem('contrib-review-token', token);
  try { held = (await api('/review/api/held')).held; } catch (e) { $('#view').replaceChildren(el('p', 'muted', 'Could not load: ' + e.message)); return; }
  $('#count').textContent = held.length + ' held';
  const nav = $('#list'); nav.replaceChildren();
  for (const h of held) {
    const b = el('button'); b.append(el('div', '', h.day + ' · ' + h.receipt_id.slice(0, 8) + (h.contributor ? ' · ' + h.contributor : ' · unlinked')));
    const f = el('div'); for (const x of h.flags) f.append(el('span', 'flag' + (/complied|destructive|exfil|harm/.test(x) ? ' bad' : ''), x)); b.append(f);
    b.addEventListener('click', () => show(h, b)); nav.append(b);
  }
  if (!held.length) $('#view').replaceChildren(el('p', 'muted', 'Nothing is held.'));
}
async function show(h, btn) {
  for (const b of $('#list').children) b.removeAttribute('aria-current'); btn.setAttribute('aria-current', 'true');
  const v = $('#view'); v.replaceChildren(el('p', 'muted', 'Loading…'));
  const d = await api('/review/api/' + h.receipt_id);
  v.replaceChildren();
  v.append(el('h3', '', h.receipt_id));
  const acts = el('div', 'actions');
  const ok = el('button', 'ok', 'Accept'), no = el('button', 'no', 'Reject');
  ok.onclick = () => decide(h, 'accept'); no.onclick = () => decide(h, 'reject');
  acts.append(ok, no); v.append(acts);
  v.append(el('h4', '', 'What the screener found')); v.append(el('pre', '', (h.notes || []).join('\n') || '(no notes)'));
  v.append(el('pre', '', 'scores: ' + JSON.stringify(h.scores) + '\ntags: ' + JSON.stringify(h.tags) + '\nmodel: ' + (d.bundle?.model?.name || '') + '\nredactions: ' + JSON.stringify(d.bundle?.redactions || {})));
  for (const t of d.bundle?.turns || []) {
    const c = el('div', 'turn'); c.append(el('strong', '', 'Turn ' + t.seq + ' · ' + t.status));
    c.append(el('pre', '', 'USER: ' + t.prompt));
    for (const m of (t.messages || []).slice(1)) for (const b of m.content || []) {
      if (b.type === 'text') c.append(el('pre', '', 'AGENT: ' + b.text));
      else if (b.type === 'tool_use') c.append(el('pre', '', 'CALL ' + b.name + ' ' + JSON.stringify(b.input)));
      else if (b.type === 'tool_result') c.append(el('pre', '', 'RESULT: ' + (typeof b.content === 'string' ? b.content : JSON.stringify(b.content))));
    }
    v.append(c);
  }
  if (d.bundle?.outcome) v.append(el('pre', '', 'outcome: ' + JSON.stringify(d.bundle.outcome)));
}
async function decide(h, what) {
  try { await api('/review/api/' + h.receipt_id + '/' + what, 'POST'); } catch (e) { alert(e.message); return; }
  await load();
}
$('#go').onclick = load;
if (token) load();
</script></body></html>"#;
