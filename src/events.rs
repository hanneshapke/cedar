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
use tokio::sync::watch;
use uuid::Uuid;

#[derive(Default)]
pub struct EventHub {
    channels: Mutex<HashMap<Uuid, watch::Sender<i64>>>,
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
