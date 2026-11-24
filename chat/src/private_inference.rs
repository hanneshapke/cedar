//! Cedar Private Inference from Settings -> Models: the connect flow of `cedar_private_inference`,
//! run on this server, so the browser that approves the code can be on another machine than the
//! one cedar runs on. The grant's models go to cedar through its admin API, like any other model;
//! after that, cedar talks to the provider directly and nothing goes through the service.
//!
//! One connection at a time. Its state is kept here and read by the page (`GET
//! /api/integrations/private-inference`) while it waits for the approval.

use crate::api::AppState;
use cedar_private_inference::{Challenge, Client, Error, Grant, Started};
use serde::Serialize;
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Where a connection started from Settings is.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConnectStatus {
    Idle,
    /// Waiting for the user to approve `user_code` at `url`.
    Waiting { user_code: String, url: String },
    /// Approved: registering the models with cedar (the new key can take a minute to work).
    Registering,
    /// Done: these models are registered.
    Connected { models: Vec<String> },
    /// `reason` is `denied`, `expired`, `unreachable`, `unavailable` or `error`, for the page
    /// to say in the user's language; `error` is the detail.
    Failed { reason: &'static str, error: String },
}

#[derive(Default)]
pub struct Connector {
    status: Mutex<Option<ConnectStatus>>,
    cancel: Mutex<CancellationToken>,
}

impl Connector {
    pub fn status(&self) -> ConnectStatus {
        self.status.lock().unwrap().clone().unwrap_or(ConnectStatus::Idle)
    }

    fn set(&self, s: ConnectStatus) {
        *self.status.lock().unwrap() = Some(s);
    }

    /// Marks a connection as under way, unless one is: the token that stops it.
    fn begin(&self) -> Option<CancellationToken> {
        let mut status = self.status.lock().unwrap();
        if matches!(*status, Some(ConnectStatus::Waiting { .. } | ConnectStatus::Registering)) {
            return None;
        }
        // Until the code is there; a failed start sets Failed.
        *status = Some(ConnectStatus::Registering);
        let token = CancellationToken::new();
        *self.cancel.lock().unwrap() = token.clone();
        Some(token)
    }

    /// Stops waiting for the approval. Registering, once begun, finishes.
    pub fn cancel(&self) {
        self.cancel.lock().unwrap().cancel();
        let mut status = self.status.lock().unwrap();
        if matches!(*status, Some(ConnectStatus::Waiting { .. })) {
            *status = Some(ConnectStatus::Idle);
        }
    }
}

/// Whether this build or environment knows where the service is.
pub fn available() -> bool {
    cedar_private_inference::service_url().is_ok()
}

/// Why starting failed, for the API's answer.
pub enum StartError {
    Unavailable,
    Busy,
    Failed(ConnectStatus),
}

/// Asks the service for a code and waits for its approval in the background. The status it
/// returns (`waiting`) has the code and the page to approve it on.
pub async fn start(state: &AppState) -> Result<ConnectStatus, StartError> {
    let Ok(url) = cedar_private_inference::service_url() else {
        return Err(StartError::Unavailable);
    };
    let connector = state.private_inference.clone();
    let cancel = connector.begin().ok_or(StartError::Busy)?;
    let begun = async {
        let client = Client::new(&url)?;
        let challenge = Challenge::new();
        let started = client.start(&challenge, Some(&client_name(&state.cfg.public_url))).await?;
        Ok::<_, Error>((client, challenge, started))
    }
    .await;
    let (client, challenge, started) = match begun {
        Ok(begun) => begun,
        Err(e) => {
            let failed = failed(&e);
            connector.set(failed.clone());
            return Err(StartError::Failed(failed));
        }
    };
    let waiting = ConnectStatus::Waiting {
        user_code: started.user_code.clone(),
        url: started.verification_uri_complete.clone(),
    };
    connector.set(waiting.clone());
    tokio::spawn(wait(state.clone(), client, challenge, started, cancel));
    Ok(waiting)
}

async fn wait(state: AppState, client: Client, challenge: Challenge, started: Started, cancel: CancellationToken) {
    let connector = state.private_inference.clone();
    let outcome = tokio::select! {
        _ = cancel.cancelled() => return,
        _ = state.shutdown.cancelled() => return,
        outcome = client.connect(&challenge, &started) => outcome,
    };
    match outcome {
        Ok(grant) => {
            connector.set(ConnectStatus::Registering);
            connector.set(register(&state, &grant).await);
        }
        Err(e) => connector.set(failed(&e)),
    }
}

/// Puts the grant's models into cedar's registry, trying again while the new key activates.
async fn register(state: &AppState, grant: &Grant) -> ConnectStatus {
    let mut models = Vec::new();
    for (id, spec) in grant.model_specs() {
        let saved = cedar_private_inference::retry_while_activating(|| state.cedar.set_model(id, &spec)).await;
        match saved {
            Ok(entry) => models.push(entry["label"].as_str().unwrap_or(id).to_string()),
            Err(e) => {
                tracing::warn!(model = id, error = %e, "could not register Cedar Private Inference's model");
                return ConnectStatus::Failed { reason: "error", error: e.to_string() };
            }
        }
    }
    tracing::info!(provider = %grant.provider, ?models, "Cedar Private Inference connected");
    ConnectStatus::Connected { models }
}

/// Removes both of Cedar Private Inference's models from cedar. The key stays valid until it is
/// revoked on the account page.
pub async fn disconnect(state: &AppState) -> Result<(), crate::error::AppError> {
    state.private_inference.cancel();
    for id in [cedar_private_inference::SMALL_ID, cedar_private_inference::LARGE_ID] {
        match state.cedar.remove_model(id).await {
            Ok(()) => {}
            Err(crate::error::AppError::Upstream { status, .. }) if status == axum::http::StatusCode::NOT_FOUND => {}
            Err(e) => return Err(e),
        }
    }
    state.private_inference.set(ConnectStatus::Idle);
    Ok(())
}

fn failed(e: &Error) -> ConnectStatus {
    let reason = match e {
        Error::Denied => "denied",
        Error::Expired | Error::InvalidGrant => "expired",
        Error::Unreachable(_) => "unreachable",
        Error::NotConfigured | Error::BadServiceUrl(_) => "unavailable",
        _ => "error",
    };
    ConnectStatus::Failed { reason, error: e.to_string() }
}

/// What the approval page calls this server: "Cedar at chat.example.com".
fn client_name(public_url: &str) -> String {
    let host = reqwest::Url::parse(public_url).ok().and_then(|u| u.host_str().map(String::from));
    match host {
        Some(host) => format!("Cedar at {host}"),
        None => "Cedar".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_connection_at_a_time_and_cancel_ends_the_wait() {
        let c = Connector::default();
        assert_eq!(c.status(), ConnectStatus::Idle);
        let token = c.begin().unwrap();
        assert!(c.begin().is_none(), "one at a time");
        c.set(ConnectStatus::Waiting { user_code: "A-B".into(), url: "https://x".into() });
        assert!(c.begin().is_none());
        c.cancel();
        assert!(token.is_cancelled());
        assert_eq!(c.status(), ConnectStatus::Idle);
        // A failed one can be tried again.
        c.set(ConnectStatus::Failed { reason: "denied", error: String::new() });
        assert!(c.begin().is_some());
    }

    #[test]
    fn failures_have_reasons_the_page_can_word() {
        assert_eq!(failed(&Error::Denied), ConnectStatus::Failed { reason: "denied", error: Error::Denied.to_string() });
        assert!(matches!(failed(&Error::InvalidGrant), ConnectStatus::Failed { reason: "expired", .. }));
        assert_eq!(client_name("https://chat.example.com:8443/"), "Cedar at chat.example.com");
        assert_eq!(client_name("nonsense"), "Cedar");
    }
}
