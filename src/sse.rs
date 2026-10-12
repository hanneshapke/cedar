//! Server-Sent Events for a run: replay from the database, then follow live.

use crate::events::EventHub;
use crate::models::Event;
use crate::store::Store;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures_util::Stream;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

const BATCH: u32 = 200;

fn to_sse(e: &Event) -> SseEvent {
    SseEvent::default()
        .id(e.seq.to_string())
        .event(e.kind.clone())
        .data(serde_json::to_string(e).unwrap_or_else(|_| "{}".into()))
}

/// Stream a run's events with `seq > after`, ending after the terminal event.
///
/// The stream subscribes *before* its first read. Anything appended afterwards either shows up in
/// that read or triggers a wake-up, so no event is missed or delivered twice.
pub fn event_stream(
    store: Store,
    hub: Arc<EventHub>,
    run_id: Uuid,
    after: i64,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let stream = async_stream::stream! {
        // `None` means the run isn't live here: replay what's stored, then close.
        let mut live = hub.subscribe(run_id);
        let mut cursor = after;
        loop {
            let batch = match store.events_after(run_id, cursor, BATCH).await {
                Ok(batch) => batch,
                Err(e) => {
                    tracing::error!(%run_id, error = ?e, "reading events for stream");
                    break;
                }
            };
            let more_pending = batch.len() as u32 == BATCH;
            let mut terminal = false;
            for e in &batch {
                cursor = e.seq;
                terminal |= e.is_terminal();
                yield Ok::<_, Infallible>(to_sse(e));
            }
            if terminal {
                break;
            }
            if more_pending {
                continue;
            }
            let woke = match live.as_mut() {
                None => break,
                Some(rx) => rx.changed().await.is_ok(),
            };
            if !woke {
                // The run ended and was removed from the hub: read once more, then close.
                live = None;
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
