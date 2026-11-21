//! Private conversations: how one ends.
//!
//! A private conversation is in no list, so nobody can find it again to delete it. It ends when
//! its client leaves it (`DELETE /api/conversations/{id}`), and for the ones a client never
//! ended (a closed tab, a phone that lost its connection) this module looks in the background
//! and ends those nobody has written in for `CHAT_PRIVATE_TTL_SECS`. Ending one stops its
//! response if one is going, removes it here at once, and deletes its runs on cedar, which
//! frees their messages and the workspace.

use crate::api::AppState;
use crate::cedar;
use crate::error::AppError;
use crate::store::Conversation;
use chrono::Utc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// How often the idle ones are looked for.
const SWEEP_EVERY: Duration = Duration::from_secs(300);
/// How many are ended per look; the rest wait for the next.
const SWEEP_AT_ONCE: u32 = 100;
/// How often, and how long apart, a run that is still stopping is tried again.
const DELETE_TRIES: u32 = 30;
const DELETE_WAIT: Duration = Duration::from_secs(2);

/// End `conversation`, as its owner (`state` is theirs). A response still going is stopped, not
/// waited for: the conversation is gone when this returns, and its runs on cedar are deleted in
/// the background as soon as they have stopped.
pub async fn end(state: &AppState, conversation: &Conversation) -> Result<(), AppError> {
    let turns = state.store.list_turns(conversation.id).await?;
    for turn in turns.iter().filter(|t| !cedar::is_terminal(&t.status)) {
        // A run that ended meanwhile, or is gone, is fine: deleting comes next.
        if let Err(e) = state.cedar.cancel_run(turn.run_id).await {
            tracing::debug!(run_id = %turn.run_id, error = %e, "could not stop a private conversation's run");
        }
    }
    state.store.delete_conversation(conversation.id).await?;
    let runs: Vec<Uuid> = turns.iter().map(|t| t.run_id).collect();
    if !runs.is_empty() {
        let state = state.clone();
        tokio::spawn(async move { delete_runs(&state, runs).await });
    }
    Ok(())
}

/// Delete `runs` on cedar, oldest first, trying again while one is still stopping.
async fn delete_runs(state: &AppState, mut runs: Vec<Uuid>) {
    for attempt in 0..DELETE_TRIES {
        let mut left = Vec::new();
        for run in runs {
            match state.cedar.delete_run(run).await {
                Ok(()) | Err(AppError::Upstream { status: reqwest::StatusCode::NOT_FOUND, .. }) => {}
                Err(e) => {
                    if attempt + 1 == DELETE_TRIES {
                        tracing::warn!(run_id = %run, error = %e, "could not delete a private conversation's run on cedar");
                    }
                    left.push(run);
                }
            }
        }
        if left.is_empty() {
            return;
        }
        runs = left;
        tokio::select! {
            _ = state.shutdown.cancelled() => return,
            _ = tokio::time::sleep(DELETE_WAIT) => {}
        }
    }
}

/// End every private conversation that has been idle for longer than `CHAT_PRIVATE_TTL_SECS`,
/// whoever's it is. Returns how many were ended.
pub async fn sweep(state: &AppState) -> Result<usize, AppError> {
    let ttl = chrono::Duration::from_std(state.cfg.private_ttl).unwrap_or(chrono::Duration::MAX);
    let Some(cutoff) = Utc::now().checked_sub_signed(ttl) else { return Ok(0) };
    let mut ended = 0;
    for (id, user) in state.store.idle_private_conversations(cutoff, SWEEP_AT_ONCE).await? {
        let state = state.as_user(user);
        let Some(conversation) = state.store.get_conversation(id).await? else { continue };
        match end(&state, &conversation).await {
            Ok(()) => ended += 1,
            Err(e) => tracing::warn!(conversation_id = %id, error = %e, "could not end an idle private conversation"),
        }
    }
    Ok(ended)
}

/// Look for idle private conversations until `stop` is cancelled. The first look happens right
/// away, so the ones that went idle while the server was down are ended when it starts.
pub fn start(state: AppState, stop: CancellationToken) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match sweep(&state).await {
                Ok(0) => {}
                Ok(n) => tracing::info!(count = n, "ended idle private conversations"),
                Err(e) => tracing::warn!(error = %e, "could not look for idle private conversations"),
            }
            tokio::select! {
                _ = stop.cancelled() => return,
                _ = tokio::time::sleep(SWEEP_EVERY) => {}
            }
        }
    })
}
