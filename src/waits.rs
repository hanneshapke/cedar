//! Where a run waits for its client: for the plan to be agreed, or for a risky tool call to be
//! approved. The run registers what it waits for before it emits the event that asks; the API
//! hands the answer over. Time spent waiting doesn't count toward the run's time limit (see
//! [`Waits::paused`]), but each wait is bounded by `APPROVAL_TIMEOUT_SECS`.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// What a wait ended with.
#[derive(Debug)]
pub enum Answer {
    Given(Value),
    TimedOut,
    Cancelled,
}

#[derive(Default)]
pub struct Waits {
    pending: Mutex<HashMap<(Uuid, String), oneshot::Sender<Value>>>,
    /// How long each run has spent waiting so far, and since when it is waiting now.
    paused: Mutex<HashMap<Uuid, (Duration, Option<Instant>)>>,
}

impl Waits {
    /// Start waiting for `key` (`plan`, or `approval:<id>`) in `run`. Call before emitting the
    /// event that asks, so an answer can't arrive before anyone listens for it.
    pub fn register(&self, run: Uuid, key: &str) -> oneshot::Receiver<Value> {
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap()
            .insert((run, key.to_string()), tx);
        rx
    }

    /// Wait for the answer registered under `key`, at most `timeout`.
    pub async fn wait(
        &self,
        run: Uuid,
        key: &str,
        rx: oneshot::Receiver<Value>,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Answer {
        let started = Instant::now();
        self.paused.lock().unwrap().entry(run).or_default().1 = Some(started);
        let answer = tokio::select! {
            biased;
            _ = cancel.cancelled() => Answer::Cancelled,
            r = tokio::time::timeout(timeout, rx) => match r {
                Ok(Ok(value)) => Answer::Given(value),
                Ok(Err(_)) | Err(_) => Answer::TimedOut,
            },
        };
        self.pending.lock().unwrap().remove(&(run, key.to_string()));
        let mut paused = self.paused.lock().unwrap();
        let entry = paused.entry(run).or_default();
        entry.0 += started.elapsed();
        entry.1 = None;
        answer
    }

    /// Hand `value` to whatever `run` is waiting for under `key`. False if nothing is waiting.
    pub fn answer(&self, run: Uuid, key: &str, value: Value) -> bool {
        let tx = self.pending.lock().unwrap().remove(&(run, key.to_string()));
        tx.is_some_and(|tx| tx.send(value).is_ok())
    }

    /// Whether `run` is waiting for `key` right now.
    pub fn is_waiting(&self, run: Uuid, key: &str) -> bool {
        self.pending
            .lock()
            .unwrap()
            .contains_key(&(run, key.to_string()))
    }

    /// How long `run` has spent waiting for its client so far, including a wait still going.
    pub fn paused(&self, run: Uuid) -> Duration {
        match self.paused.lock().unwrap().get(&run) {
            Some((done, since)) => *done + since.map_or(Duration::ZERO, |s| s.elapsed()),
            None => Duration::ZERO,
        }
    }

    /// The run is over: forget its waits.
    pub fn forget(&self, run: Uuid) {
        self.pending.lock().unwrap().retain(|(r, _), _| *r != run);
        self.paused.lock().unwrap().remove(&run);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    #[tokio::test]
    async fn an_answer_reaches_the_waiting_run_and_the_wait_is_paused_time() {
        let waits = Arc::new(Waits::default());
        let run = Uuid::new_v4();
        let rx = waits.register(run, "plan");
        assert!(waits.is_waiting(run, "plan"));
        let w = waits.clone();
        let handle = tokio::spawn(async move {
            w.wait(
                run,
                "plan",
                rx,
                Duration::from_secs(5),
                &CancellationToken::new(),
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waits.answer(run, "approval:x", json!(1)));
        assert!(waits.answer(run, "plan", json!({ "ok": true })));
        assert!(matches!(handle.await.unwrap(), Answer::Given(v) if v["ok"] == true));
        assert!(!waits.is_waiting(run, "plan"));
        assert!(waits.paused(run) >= Duration::from_millis(20));
        waits.forget(run);
        assert_eq!(waits.paused(run), Duration::ZERO);
    }

    #[tokio::test]
    async fn waits_time_out_and_cancel() {
        let waits = Waits::default();
        let run = Uuid::new_v4();
        let rx = waits.register(run, "a");
        let cancel = CancellationToken::new();
        assert!(matches!(
            waits
                .wait(run, "a", rx, Duration::from_millis(10), &cancel)
                .await,
            Answer::TimedOut
        ));
        assert!(!waits.answer(run, "a", json!(1)));
        let rx = waits.register(run, "b");
        cancel.cancel();
        assert!(matches!(
            waits
                .wait(run, "b", rx, Duration::from_secs(5), &cancel)
                .await,
            Answer::Cancelled
        ));
    }
}
