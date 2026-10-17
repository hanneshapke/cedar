//! Event publication for live runs.
//!
//! The `events` table is the source of truth. The hub only tells SSE streams *that* something new
//! was appended (the latest seq), so a stream can wake up and read from the database. Because the
//! watch channel remembers its latest value, a wake-up sent between a stream's subscribe and its
//! first query is never lost.

use crate::store::Store;
use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{watch, Notify};
use uuid::Uuid;

#[derive(Default)]
pub struct EventHub {
    channels: Mutex<HashMap<Uuid, watch::Sender<i64>>>,
    /// Per owner, the id of the latest notification (a finished run), for `GET /v1/events`.
    /// Entries are made by the first stream and dropped once no stream is left.
    owners: Mutex<HashMap<String, watch::Sender<i64>>>,
    /// Wakes the webhook worker when a run with a callback has finished.
    deliveries: Notify,
}

impl EventHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark a run as live in this process.
    pub fn register(&self, run_id: Uuid) {
        self.channels
            .lock()
            .unwrap()
            .entry(run_id)
            .or_insert_with(|| watch::channel(0).0);
    }

    /// Follow a live run. `None` means the run isn't live here (finished, or from an earlier process).
    pub fn subscribe(&self, run_id: Uuid) -> Option<watch::Receiver<i64>> {
        self.channels
            .lock()
            .unwrap()
            .get(&run_id)
            .map(|tx| tx.subscribe())
    }

    pub fn notify(&self, run_id: Uuid, seq: i64) {
        if let Some(tx) = self.channels.lock().unwrap().get(&run_id) {
            tx.send_replace(seq);
        }
    }

    /// The run is over. Dropping the sender wakes every subscriber one last time.
    pub fn remove(&self, run_id: Uuid) {
        self.channels.lock().unwrap().remove(&run_id);
    }

    /// Follow an owner's finished runs. The receiver carries the latest notification id, which
    /// a stream only uses as a wake-up: it reads the rows from the database.
    pub fn subscribe_owner(&self, owner: &str) -> watch::Receiver<i64> {
        let mut owners = self.owners.lock().unwrap();
        // A sender nobody receives from any more is stale: start over.
        owners.retain(|_, tx| tx.receiver_count() > 0);
        owners
            .entry(owner.to_string())
            .or_insert_with(|| watch::channel(0).0)
            .subscribe()
    }

    /// A run of `owner` ended with notification `id`: wake its streams and, if it has a
    /// callback, the webhook worker.
    pub fn finished(&self, owner: &str, id: i64, callback: bool) {
        if let Some(tx) = self.owners.lock().unwrap().get(owner) {
            tx.send_replace(id);
        }
        if callback {
            self.deliveries.notify_one();
        }
    }

    /// Resolves once [`EventHub::finished`] has been called with a callback since the last
    /// time (a permit is kept, so a call before the wait is not lost).
    pub async fn delivery_due(&self) {
        self.deliveries.notified().await;
    }
}

/// Appends events for one run and wakes its subscribers.
#[derive(Clone)]
pub struct Emitter {
    store: Store,
    hub: Arc<EventHub>,
    run_id: Uuid,
}

impl Emitter {
    pub fn new(store: Store, hub: Arc<EventHub>, run_id: Uuid) -> Self {
        Self { store, hub, run_id }
    }

    pub async fn emit(&self, kind: &str, data: Value) -> Result<i64> {
        let seq = self.store.append_event(self.run_id, kind, &data).await?;
        self.hub.notify(self.run_id, seq);
        Ok(seq)
    }

    pub fn notify(&self, seq: i64) {
        self.hub.notify(self.run_id, seq);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subscribers_see_notifications_and_removal() {
        let hub = EventHub::new();
        let id = Uuid::new_v4();
        assert!(hub.subscribe(id).is_none());

        hub.register(id);
        let mut rx = hub.subscribe(id).unwrap();
        hub.notify(id, 3);
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow_and_update(), 3);

        hub.remove(id);
        assert!(rx.changed().await.is_err());
        assert!(hub.subscribe(id).is_none());
    }

    #[tokio::test]
    async fn owner_streams_are_woken_by_finished_runs() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe_owner("a");
        assert_eq!(*rx.borrow(), 0);
        hub.finished("b", 7, false);
        assert!(!rx.has_changed().unwrap(), "another owner's run");
        hub.finished("a", 3, true);
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow_and_update(), 3);
        // The worker's permit was kept for it.
        tokio::time::timeout(std::time::Duration::from_millis(50), hub.delivery_due())
            .await
            .unwrap();
        // Once the last stream is gone, the next subscriber starts from a fresh channel.
        drop(rx);
        let rx = hub.subscribe_owner("a");
        assert_eq!(*rx.borrow(), 0);
    }

    #[tokio::test]
    async fn notification_before_first_wait_is_not_lost() {
        let hub = EventHub::new();
        let id = Uuid::new_v4();
        hub.register(id);
        let mut rx = hub.subscribe(id).unwrap();
        hub.notify(id, 1); // lands while the subscriber is still reading its backlog
        tokio::time::timeout(std::time::Duration::from_secs(1), rx.changed())
            .await
            .unwrap()
            .unwrap();
    }
}
