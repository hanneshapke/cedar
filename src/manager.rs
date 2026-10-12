//! Owns the lifecycle of runs inside this process: queueing, cancellation, timeouts, and making
//! sure every run ends in exactly one terminal state with its sandbox destroyed.

use crate::agent::{run_agent, AgentEnv, AgentError, Completion};
use crate::events::{Emitter, EventHub};
use crate::models::{event, Run, RunStatus};
use crate::sandbox::{Sandbox, SandboxFactory};
use crate::store::{Finish, Store};
use anyhow::Result;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use uuid::Uuid;

/// Where a run's sandbox is parked once created, so cleanup doesn't depend on the run task finishing.
type SandboxSlot = Arc<Mutex<Option<Arc<dyn Sandbox>>>>;

#[derive(Clone)]
pub struct RunManager(Arc<Inner>);

struct Inner {
    store: Store,
    hub: Arc<EventHub>,
    env: AgentEnv,
    sandboxes: Arc<dyn SandboxFactory>,
    /// Bounds how many runs execute at once; the rest stay `queued`.
    slots: Arc<Semaphore>,
    run_timeout: Duration,
    /// Cancellation handles for runs that are queued or running in this process.
    active: Mutex<HashMap<Uuid, CancellationToken>>,
    tracker: TaskTracker,
    shutting_down: AtomicBool,
}

impl RunManager {
    pub fn new(
        store: Store,
        hub: Arc<EventHub>,
        env: AgentEnv,
        sandboxes: Arc<dyn SandboxFactory>,
        max_concurrent_runs: usize,
        run_timeout: Duration,
    ) -> Self {
        Self(Arc::new(Inner {
            store,
            hub,
            env,
            sandboxes,
            slots: Arc::new(Semaphore::new(max_concurrent_runs)),
            run_timeout,
            active: Mutex::new(HashMap::new()),
            tracker: TaskTracker::new(),
            shutting_down: AtomicBool::new(false),
        }))
    }

    /// Fail runs a previous process left unfinished and remove their sandboxes.
    /// Call once at startup, before accepting requests.
    pub async fn recover(&self) -> Result<()> {
        let failed = self.0.store.fail_orphans().await?;
        if !failed.is_empty() {
            tracing::warn!(count = failed.len(), "marked runs left over from a previous process as failed");
        }
        match self.0.sandboxes.cleanup_orphans().await {
            Ok(0) => {}
            Ok(n) => tracing::warn!(count = n, "removed sandbox containers left over from a previous process"),
            // Docker may simply be down right now; /readyz reports that.
            Err(e) => tracing::warn!(error = %format!("{e:#}"), "could not clean up leftover sandboxes"),
        }
        Ok(())
    }

    /// Start executing a freshly inserted (`queued`) run in the background.
    pub fn spawn(&self, run: Run) {
        let token = CancellationToken::new();
        self.0.hub.register(run.id);
        self.0.active.lock().unwrap().insert(run.id, token.clone());
        let inner = self.0.clone();
        self.0.tracker.spawn(async move { inner.execute(run, token).await });
    }

    /// Ask a queued/running run to stop. Returns false if it isn't active in this process.
    /// Cancellation is asynchronous: the run reaches `cancelled` shortly after.
    pub fn cancel(&self, id: Uuid) -> bool {
        match self.0.active.lock().unwrap().get(&id) {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// First phase of shutdown: cancel every active run so streams and tasks wind down.
    pub fn begin_shutdown(&self) {
        self.0.shutting_down.store(true, Ordering::SeqCst);
        for token in self.0.active.lock().unwrap().values() {
            token.cancel();
        }
    }

    /// Second phase: wait (up to `grace`) for run tasks to finish recording their final state.
    pub async fn drain(&self, grace: Duration) {
        self.0.tracker.close();
        if tokio::time::timeout(grace, self.0.tracker.wait()).await.is_err() {
            tracing::warn!("runs were still finishing after the shutdown grace period");
        }
    }
}

impl Inner {
    async fn execute(self: Arc<Self>, run: Run, token: CancellationToken) {
        // The work runs in a child task so a panic still ends in a recorded failure and a
        // destroyed sandbox, instead of a run stuck `running` with streams that never close.
        let sandbox = SandboxSlot::default();
        let child = {
            let (inner, run, token, slot) = (self.clone(), run.clone(), token.clone(), sandbox.clone());
            tokio::spawn(async move { inner.drive(&run, &token, &slot).await })
        };
        let outcome = match child.await {
            Ok(outcome) => outcome,
            Err(e) => {
                tracing::error!(run_id = %run.id, error = ?e, "run task panicked");
                Err(AgentError::Failed("internal error".into()))
            }
        };
        let leftover = sandbox.lock().unwrap().take();
        if let Some(sandbox) = leftover {
            sandbox.destroy().await;
        }
        self.finalize(&run, outcome).await;
    }

    async fn drive(&self, run: &Run, token: &CancellationToken, slot: &SandboxSlot) -> Result<Completion, AgentError> {
        let _slot = tokio::select! {
            biased;
            _ = token.cancelled() => return Err(AgentError::Cancelled),
            slot = self.slots.clone().acquire_owned() => slot.map_err(|e| AgentError::Failed(e.to_string()))?,
        };

        let events = Emitter::new(self.store.clone(), self.hub.clone(), run.id);
        self.store.mark_running(run.id).await?;
        events.emit(event::RUN_STARTED, json!({ "model": run.model, "max_turns": run.max_turns })).await?;

        // Parked in `slot` so `execute` can destroy it whichever way this function ends.
        let sandbox = self.sandboxes.create(run.id).await?;
        *slot.lock().unwrap() = Some(sandbox.clone());

        match tokio::time::timeout(self.run_timeout, run_agent(&self.env, run, sandbox, &events, token)).await {
            Ok(result) => result,
            Err(_) => Err(AgentError::Failed(format!("run exceeded the {}s time limit", self.run_timeout.as_secs()))),
        }
    }

    /// Record the terminal state exactly once and release the run's in-memory bookkeeping.
    async fn finalize(&self, run: &Run, outcome: Result<Completion, AgentError>) {
        let finish = match &outcome {
            Ok(done) => Finish {
                status: RunStatus::Completed,
                final_text: Some(&done.final_text),
                stop_reason: done.stop_reason.as_deref(),
                error: None,
                event_kind: event::RUN_COMPLETED,
                event_data: json!({ "final_text": done.final_text, "stop_reason": done.stop_reason }),
            },
            Err(AgentError::Cancelled) => Finish {
                status: RunStatus::Cancelled,
                final_text: None,
                stop_reason: None,
                error: None,
                event_kind: event::RUN_CANCELLED,
                event_data: json!({
                    "reason": if self.shutting_down.load(Ordering::SeqCst) { "server shutting down" } else { "cancelled by client" }
                }),
            },
            Err(AgentError::Failed(message)) => Finish {
                status: RunStatus::Failed,
                final_text: None,
                stop_reason: None,
                error: Some(message),
                event_kind: event::RUN_FAILED,
                event_data: json!({ "error": message }),
            },
        };

        match self.store.finish_run(run.id, finish).await {
            Ok(Some(seq)) => self.hub.notify(run.id, seq),
            Ok(None) => {}
            // The run stays `running` in the database until the next startup recovery fails it.
            Err(e) => tracing::error!(run_id = %run.id, error = ?e, "could not record the run's final state"),
        }
        // Dropping the hub entry wakes any stream one last time so it can drain and close.
        self.hub.remove(run.id);
        self.active.lock().unwrap().remove(&run.id);
        tracing::info!(run_id = %run.id, "run finished");
    }
}
