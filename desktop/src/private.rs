//! Cedar Private Inference on the setup page: connecting (the code the user approves in the
//! browser, then the key and models the service hands over) and disconnecting.
//!
//! The flow itself is `cedar_private_inference`'s, shared with cedar-chat and the `cedar` CLI.
//! Only the hand-off goes through the service; cedar sends prompts to the provider directly.

use crate::app::{self, AppState, Chosen, ConnectStatus};
use crate::config::{PrivateInference, PrivateModel};
use crate::i18n::{t, tf};
use crate::models;
use anyhow::{anyhow, Result};
use cedar_private_inference::{Challenge, Client, Error, Grant, Started};
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;

/// Asks the service for a code, opens the page where the user approves it, and waits for the
/// approval in the background. The setup page shows the code from `private_inference.connect`.
pub async fn connect(app: AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    app::refuse_when_remote(&state.config())?;
    let url = cedar_private_inference::service_url().map_err(|e| anyhow!(message(&e)))?;
    let cancel = state.begin_private_connect().ok_or_else(|| anyhow!(t("private.busy")))?;
    let begun = async {
        let client = Client::new(&url)?;
        let challenge = Challenge::new();
        let name = tf("private.client_name", &[("computer", &computer_name())]);
        let started = client.start(&challenge, Some(&name)).await?;
        Ok::<_, Error>((client, challenge, started))
    }
    .await;
    let (client, challenge, started) = match begun {
        Ok(begun) => begun,
        Err(e) => {
            let text = message(&e);
            state.set_private_connect(ConnectStatus::Failed(text.clone()));
            return Err(anyhow!(text));
        }
    };
    state.set_private_connect(ConnectStatus::Waiting {
        user_code: started.user_code.clone(),
        url: started.verification_uri_complete.clone(),
    });
    match url::Url::parse(&started.verification_uri_complete) {
        Ok(link) => {
            if let Err(e) = crate::window::open_web_link(&link) {
                // The page shows the link too.
                eprintln!("opening the approval page: {e:#}");
            }
        }
        Err(e) => eprintln!("the approval page's address: {e}"),
    }
    tauri::async_runtime::spawn(wait(app.clone(), client, challenge, started, cancel));
    Ok(())
}

/// Polls until the user approves, declines or the code expires; an approval is saved and
/// registered with cedar, and the small model becomes the one new chats use.
async fn wait(app: AppHandle, client: Client, challenge: Challenge, started: Started, cancel: CancellationToken) {
    let state = app.state::<AppState>();
    let outcome = tokio::select! {
        _ = cancel.cancelled() => return,
        outcome = client.connect(&challenge, &started) => outcome,
    };
    let grant = match outcome {
        Ok(grant) => grant,
        Err(e) => {
            state.set_private_connect(ConnectStatus::Failed(message(&e)));
            return;
        }
    };
    if let Err(e) = state.update_config(|c| c.private_inference = Some(saved(&grant))) {
        state.set_private_connect(ConnectStatus::Failed(format!("{e:#}")));
        return;
    }
    state.set_private_connect(ConnectStatus::Idle);
    state.choose(Chosen::Private);
    app::apply_model_server(app.clone()).await;
}

/// What the config keeps of a grant.
fn saved(grant: &Grant) -> PrivateInference {
    PrivateInference {
        base_url: grant.base_url.clone(),
        api_key: grant.api_key.clone(),
        provider: grant.provider.clone(),
        account_url: grant.account_url.clone(),
        models: grant
            .entries()
            .into_iter()
            .map(|(id, m)| PrivateModel { id: id.to_string(), model: m.id.clone(), label: m.name.clone() })
            .collect(),
    }
}

/// Stops waiting for an approval.
pub fn cancel(app: &AppHandle) {
    app.state::<AppState>().cancel_private_connect();
}

/// Removes both models from cedar and forgets the key. The key itself stays valid: the account
/// page revokes it. cedar must be running, so that its entries go too.
pub async fn disconnect(app: AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let cfg = state.config();
    app::refuse_when_remote(&cfg)?;
    state.cancel_private_connect();
    if cfg.private_inference.is_none() {
        return Ok(());
    }
    for id in models::PRIVATE_IDS {
        models::unregister(&cfg.cedar_url(), &cfg.secrets.cedar_api_key, id).await?;
    }
    let cfg = state.update_config(|c| c.private_inference = None)?;
    if !cfg.configured() {
        state.set_model(app::ModelStatus::Unset);
    }
    Ok(())
}

/// What the page says when connecting fails, in the user's language.
fn message(e: &Error) -> String {
    match e {
        Error::Denied => t("private.denied"),
        Error::Expired | Error::InvalidGrant => t("private.expired"),
        Error::NotConfigured | Error::BadServiceUrl(_) => t("private.unavailable"),
        Error::Unreachable(_) => t("private.unreachable"),
        other => other.to_string(),
    }
}

/// This computer's name, for the approval page ("Cedar Desktop on Anna's MacBook").
fn computer_name() -> String {
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("scutil").args(["--get", "ComputerName"]).output() {
        let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if out.status.success() && !name.is_empty() {
            return name;
        }
    }
    ["COMPUTERNAME", "HOSTNAME"]
        .into_iter()
        .find_map(|var| std::env::var(var).ok().filter(|v| !v.trim().is_empty()))
        .unwrap_or_else(|| t("private.this_computer"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grant_is_saved_with_the_small_model_first() {
        let grant: Grant = serde_json::from_value(serde_json::json!({
            "base_url": "https://api.doubleword.ai/v1", "api_key": "sk", "provider": "doubleword",
            "account_url": "https://plg.example/account",
            "models": [
                { "id": "kimi-k3.1-pro", "name": "Kimi K3.1 Pro 2.8T", "role": "large", "params_b": 2800 },
                { "id": "deepseek-v4.1-flash", "name": "DeepSeek V4.1 Flash 120B", "role": "small", "params_b": 120 },
            ],
        }))
        .unwrap();
        let saved = saved(&grant);
        assert_eq!(saved.account_url, "https://plg.example/account");
        assert_eq!(
            saved.models,
            [
                PrivateModel { id: "private-small".into(), model: "deepseek-v4.1-flash".into(), label: "DeepSeek V4.1 Flash 120B".into() },
                PrivateModel { id: "private-large".into(), model: "kimi-k3.1-pro".into(), label: "Kimi K3.1 Pro 2.8T".into() },
            ]
        );
    }

    #[test]
    fn failures_are_said_in_the_users_words() {
        assert_eq!(message(&Error::Denied), t("private.denied"));
        assert_eq!(message(&Error::InvalidGrant), t("private.expired"));
        assert!(!computer_name().is_empty());
    }
}
