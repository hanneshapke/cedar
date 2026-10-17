//! Being told when a response finishes: cedar's stream of the key's finished runs
//! (`GET /v1/events`), followed in the background, turned into unread turns and live notices
//! for the page.
//!
//! The stream is resumed from the id of the last notification handled, kept in the database,
//! so a restart of this service or of cedar loses nothing and a notification is handled at
//! most twice (once if the cursor was saved). Handling one brings the turn up to date (the
//! page's own stream may already have), and broadcasts a notice that every open tab's
//! `GET /api/events` relays. Whether the response is *unread* is a matter of the turn's `seen`
//! flag, which `POST /api/conversations/{id}/seen` sets, so a response that finished while the
//! conversation was open and the tab visible is never announced as new.

use crate::api::{refresh, AppState};
use crate::error::AppError;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// How many notices a slow tab may fall behind before it misses some (it then re-reads the inbox).
pub const NOTICE_BUFFER: usize = 64;

/// Follow cedar's stream until `stop`, reconnecting with a growing wait when it ends or fails.
pub fn start(state: AppState, stop: CancellationToken) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut failures: u32 = 0;
        loop {
            match sync(&state, &stop).await {
                Ok(n) => {
                    failures = 0;
                    if n > 0 {
                        tracing::debug!(count = n, "handled finished runs from cedar");
                    }
                }
                Err(e) => {
                    failures += 1;
                    // The first failure and then one in ten, so a cedar that is down for a
                    // while doesn't fill the log.
                    if failures == 1 || failures.is_multiple_of(10) {
                        tracing::warn!(error = %e, failures, "could not follow cedar's event stream; retrying");
                    }
                }
            }
            if stop.is_cancelled() {
                return;
            }
            let wait = Duration::from_secs(1 << failures.min(5));
            tokio::select! {
                _ = stop.cancelled() => return,
                _ = tokio::time::sleep(wait) => {}
            }
        }
    })
}

/// Open cedar's stream from the saved cursor and handle every notification until the stream
/// ends (cedar closed it) or `stop` is cancelled. Returns how many were handled.
pub async fn sync(state: &AppState, stop: &CancellationToken) -> Result<usize, AppError> {
    let after = state.store.notifications_cursor().await?;
    let resp = state.cedar.notifications(after).await?;
    let mut body = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut handled = 0;
    loop {
        let chunk = tokio::select! {
            _ = stop.cancelled() => return Ok(handled),
            chunk = body.next() => chunk,
        };
        let Some(chunk) = chunk else { return Ok(handled) };
        let chunk = chunk.map_err(|e| AppError::BadGateway(format!("cedar's event stream broke: {e}")))?;
        buf.extend_from_slice(&chunk);
        while let Some(end) = buf.windows(2).position(|w| w == b"\n\n") {
            let block: Vec<u8> = buf.drain(..end + 2).collect();
            if let Some(notification) = parse_sse(&block) {
                handle(state, &notification).await?;
                handled += 1;
            }
        }
    }
}

/// The JSON of one SSE block's `data:` lines; `None` for a keep-alive comment or malformed data.
fn parse_sse(block: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(block).ok()?;
    let data: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(|rest| rest.strip_prefix(' ').unwrap_or(rest))
        .collect();
    if data.is_empty() {
        return None;
    }
    serde_json::from_str(&data.join("\n")).ok()
}

/// One of cedar's notifications: bring its turn up to date, tell the tabs, move the cursor.
/// A run this service doesn't know (a deleted conversation's, or another client's on the same
/// key) is skipped.
pub async fn handle(state: &AppState, notification: &Value) -> Result<(), AppError> {
    let id = notification["id"]
        .as_i64()
        .ok_or_else(|| AppError::BadGateway("a notification from cedar has no id".into()))?;
    let run_id = notification["run_id"].as_str().and_then(|s| Uuid::parse_str(s).ok());
    if let Some(turn) = match run_id {
        Some(run_id) => state.store.get_turn_by_run(run_id).await?,
        None => None,
    } {
        // What cedar's stream says is enough to announce, but the page wants the text too.
        let turn = refresh(state, turn).await?;
        let conversation = state.store.get_conversation(turn.conversation_id).await?;
        let notice = json!({
            "type": "turn.finished",
            "turn_id": turn.id,
            "conversation_id": turn.conversation_id,
            "run_id": turn.run_id,
            "status": turn.status,
            "stop_reason": turn.stop_reason,
            "error": turn.error,
            "needs_answer": turn.status == "completed" && turn.stop_reason.as_deref() == Some("ask_user"),
            "finished_at": turn.finished_at,
            "title": conversation.as_ref().and_then(|c| c.title.clone()),
            "schedule_id": conversation.as_ref().and_then(|c| c.schedule_id),
        });
        // No tab open is fine.
        let _ = state.notices.send(notice);
        // Out of the app too, unless a page marks it seen in the meantime (see `channels`).
        let state = state.clone();
        let turn_id = turn.id;
        tokio::spawn(async move {
            tokio::time::sleep(crate::channels::DISPATCH_DELAY).await;
            if let Err(e) = crate::channels::dispatch(&state, turn_id).await {
                tracing::warn!(%turn_id, error = %e, "could not send the notification");
            }
        });
    }
    state.store.set_notifications_cursor(id).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_blocks_are_parsed_and_comments_skipped() {
        assert_eq!(parse_sse(b": keep-alive\n\n"), None);
        assert_eq!(parse_sse(b"id: 3\nevent: run.completed\ndata: {\"id\":3}\n\n"), Some(json!({ "id": 3 })));
        assert_eq!(parse_sse(b"data:{\"a\":\ndata: 1}\n\n"), Some(json!({ "a": 1 })));
        assert_eq!(parse_sse(b"data: not json\n\n"), None);
    }
}
