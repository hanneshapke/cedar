//! Being told that a response is waiting for you while it is still going: a risky call that
//! needs your OK, or a plan to agree to.
//!
//! cedar only waits so long for either (`APPROVAL_TIMEOUT_SECS`), and until now the request was
//! only on the page: with the tab closed, nobody knew. So the run of every message sent from the
//! page is followed here too, and a wait that is still unanswered after
//! `CHAT_ATTENTION_DELAY_SECS` goes out by the person's notification method, like a finished
//! response would (see `channels`). A wait answered sooner, by whoever is looking at the page,
//! is never announced. The open tabs hear of a wait at once, as a `turn.waiting` notice.
//!
//! Turns Slack started are not followed here: their requests are posted in the thread (`slack`).

use crate::api::AppState;
use crate::channels::{self, Message, Method};
use crate::error::AppError;
use crate::i18n::{self, Lang, Waiting};
use crate::store::Turn;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How many times in a row following a run may fail before it is given up.
const MAX_FAILURES: u32 = 20;

/// What a run is waiting for, by key (`plan`, or `approval:<id>`), while it is unanswered.
type Pending = Arc<Mutex<HashSet<String>>>;

/// Follow the run of a turn someone sent from the page until it ends, in the background.
/// `state` is the turn's owner's.
pub fn watch(state: AppState, turn: Turn) {
    tokio::spawn(async move {
        let shutdown = state.shutdown.clone();
        tokio::select! {
            _ = shutdown.cancelled() => {}
            _ = follow(&state, &turn) => {}
        }
    });
}

async fn follow(state: &AppState, turn: &Turn) {
    let pending: Pending = Arc::default();
    let mut after: i64 = 0;
    let mut failures: u32 = 0;
    loop {
        match follow_once(state, turn, &pending, &mut after).await {
            Ok(true) => return,
            Ok(false) => failures = 0,
            Err(e) => {
                failures += 1;
                if failures == 1 {
                    tracing::debug!(turn_id = %turn.id, error = %e, "could not follow a turn's run for what it waits on; retrying");
                }
            }
        }
        // The stream closed early, or broke: the run may have ended meanwhile.
        match state.cedar.get_run(turn.run_id).await {
            Ok(Some(run)) if !crate::cedar::is_terminal(&run.status) => {}
            Ok(_) => return,
            Err(_) => {}
        }
        if failures > MAX_FAILURES {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1 << failures.min(5))).await;
    }
}

/// Read the run's events from `after` on. `true` once the run's last event has come.
async fn follow_once(state: &AppState, turn: &Turn, pending: &Pending, after: &mut i64) -> Result<bool, AppError> {
    let resp = state.cedar.events(turn.run_id, *after).await?;
    let mut body = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|e| AppError::BadGateway(format!("cedar's event stream broke: {e}")))?;
        buf.extend_from_slice(&chunk);
        while let Some(end) = buf.windows(2).position(|w| w == b"\n\n") {
            let block: Vec<u8> = buf.drain(..end + 2).collect();
            let Some(event) = crate::notify::parse_sse(&block) else { continue };
            if let Some(seq) = event["seq"].as_i64() {
                *after = (*after).max(seq);
            }
            let kind = event["type"].as_str().unwrap_or_default();
            on_event(state, turn, pending, kind, &event["data"]);
            if matches!(kind, "run.completed" | "run.failed" | "run.cancelled") {
                pending.lock().unwrap().clear();
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn on_event(state: &AppState, turn: &Turn, pending: &Pending, kind: &str, data: &Value) {
    let (key, waiting) = match kind {
        "approval.required" => {
            let Some(id) = data["id"].as_str() else { return };
            let reason = data["reason"].as_str().unwrap_or_default().to_string();
            (format!("approval:{id}"), Waiting::Approval(reason))
        }
        "plan" if data["awaiting"] == true => ("plan".to_string(), Waiting::Plan),
        "approval.resolved" => {
            if let Some(id) = data["id"].as_str() {
                pending.lock().unwrap().remove(&format!("approval:{id}"));
            }
            return;
        }
        "plan.updated" => {
            pending.lock().unwrap().remove("plan");
            return;
        }
        _ => return,
    };
    // A replayed event (the stream was reopened) is one already being waited on.
    if !pending.lock().unwrap().insert(key.clone()) {
        return;
    }
    let what = match &waiting {
        Waiting::Approval(_) => "approval",
        Waiting::Plan => "plan",
    };
    let reason = match &waiting {
        Waiting::Approval(reason) => Some(reason.clone()),
        Waiting::Plan => None,
    };
    // No tab open is fine.
    let _ = state.notices.send((
        state.user(),
        json!({ "type": "turn.waiting", "turn_id": turn.id, "conversation_id": turn.conversation_id, "what": what, "reason": reason }),
    ));
    let (state, turn, pending) = (state.clone(), turn.clone(), pending.clone());
    tokio::spawn(async move {
        tokio::time::sleep(state.cfg.attention_delay).await;
        if !pending.lock().unwrap().contains(&key) {
            return;
        }
        match announce(&state, &turn, &key, &waiting).await {
            Ok(Some(Ok(detail))) => tracing::info!(turn_id = %turn.id, detail, "told the person that a response is waiting for them"),
            Ok(Some(Err(error))) => tracing::warn!(turn_id = %turn.id, error, "could not tell the person that a response is waiting for them"),
            Ok(None) => {}
            Err(e) => tracing::warn!(turn_id = %turn.id, error = %e, "could not tell the person that a response is waiting for them"),
        }
    });
}

/// Send the out-of-app message for a wait nobody answered yet, by the person's default method.
/// `None` when that is "only in the app". Returns what was done, for logs and tests.
pub async fn announce(state: &AppState, turn: &Turn, key: &str, waiting: &Waiting) -> Result<Option<Result<String, String>>, AppError> {
    let settings = state.store.notification_settings().await?;
    let method = settings.default_method;
    if method == Method::None {
        return Ok(None);
    }
    let conversation = state.store.get_conversation(turn.conversation_id).await?;
    // Nothing about a private conversation is sent anywhere.
    if conversation.as_ref().is_some_and(|c| c.private) {
        return Ok(None);
    }
    let locale = state.store.preferences().await?.locale;
    let (title, body) = i18n::waiting_message(locale.as_deref(), conversation.as_ref().and_then(|c| c.title.as_deref()), waiting);
    let message = Message {
        title,
        body,
        open_label: Lang::of(locale.as_deref()).open_in_cedar().into(),
        url: format!("{}/#/c/{}", state.cfg.public_url, turn.conversation_id),
        // One per wait, so a second approval in the same response is shown as well.
        tag: format!("{}:{key}", turn.id),
        conversation_id: Some(turn.conversation_id),
    };
    let outcome = channels::send(state, &settings, method, &message).await;
    channels::record(&state.store, method, false, &outcome).await?;
    Ok(Some(outcome))
}
