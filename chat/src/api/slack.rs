//! Settings -> Slack (`/api/slack`): whether the Slack app is set up, saving its tokens (admins,
//! when the environment doesn't set them), and linking one's Slack account to oneself here with
//! the code the app sent it (see `slack`).

use super::*;

/// `{available, source, workspace?, links}`: `source` is where the app's tokens come from (`env`,
/// `settings`, or `null` without an app); `workspace` is `{team, team_name, bot}` once the app is
/// connected; `links` are the caller's Slack accounts.
pub(super) async fn get_slack(Scoped(state): Scoped) -> Result<Json<Value>, AppError> {
    let links = state.store.slack_links().await?;
    Ok(Json(json!({
        "available": state.slack.get().is_some(),
        "source": state.slack.source().map(crate::slack::Source::as_str),
        "workspace": workspace(&state).await,
        "links": links,
    })))
}

async fn workspace(state: &AppState) -> Option<Value> {
    let me = state.slack.get()?.me().await?;
    Some(json!({ "team": me.team, "team_name": me.team_name, "bot": me.name }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AppBody {
    app_token: String,
    bot_token: String,
}

fn token_error(e: crate::slack::TokenError) -> AppError {
    match e {
        crate::slack::TokenError::FromEnv => {
            AppError::Conflict("the Slack app is set in the environment (CHAT_SLACK_APP_TOKEN, CHAT_SLACK_BOT_TOKEN); change it there".into())
        }
        crate::slack::TokenError::Invalid(why) => AppError::BadRequest(why),
        crate::slack::TokenError::Other(e) => AppError::Internal(e),
    }
}

/// Admin: save the Slack app's tokens, once Slack takes both, and connect with them.
/// `{app_token, bot_token}` -> `{source, workspace}`. `400` with Slack's reason for a token it
/// refuses; `409` while the environment sets the app.
pub(super) async fn put_slack_app(Scoped(state): Scoped, headers: HeaderMap, ApiJson(body): ApiJson<AppBody>) -> Result<Json<Value>, AppError> {
    require_admin(&state, &headers)?;
    let me = state.slack.save(&state.store, &body.app_token, &body.bot_token).await.map_err(token_error)?;
    tracing::info!(team = ?me.team_name, bot = ?me.name, "Slack app saved in Settings");
    Ok(Json(json!({ "source": "settings", "workspace": { "team": me.team, "team_name": me.team_name, "bot": me.name } })))
}

/// Admin: forget the Slack app's saved tokens and disconnect. Linked accounts stay linked, for
/// when an app is set up again.
pub(super) async fn delete_slack_app(Scoped(state): Scoped, headers: HeaderMap) -> Result<StatusCode, AppError> {
    require_admin(&state, &headers)?;
    state.slack.remove(&state.store).await.map_err(token_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LinkBody {
    code: String,
}

/// Link the Slack account a code was sent to. `400` for a code that isn't one, was used, or ran
/// out; `404` without a Slack app.
pub(super) async fn link_slack(Scoped(state): Scoped, ApiJson(body): ApiJson<LinkBody>) -> Result<(StatusCode, Json<Value>), AppError> {
    let slack = state.slack.get().ok_or(AppError::NotFound("Slack is not set up on this server"))?;
    let pending = slack
        .redeem(&body.code)
        .ok_or_else(|| AppError::BadRequest("that code is not valid or has run out; message the Slack app for a new one".into()))?;
    state.store.link_slack(&pending.team, pending.team_name.as_deref(), &pending.slack_user, pending.name.as_deref()).await?;
    // Tell the account in Slack, in this user's language.
    let name = state.store.get_user(state.user()).await?.map(|u| u.name).unwrap_or_default();
    let locale = state.store.preferences().await?.locale;
    let text = crate::slack::texts(Lang::of(locale.as_deref())).linked.replace("{name}", &name);
    if let Err(e) = slack.post(&pending.slack_user, None, &text, None).await {
        tracing::debug!(error = %e, "could not tell the Slack account it was linked");
    }
    let link = state.store.slack_links().await?.into_iter().find(|l| l.team == pending.team && l.slack_user == pending.slack_user);
    Ok((StatusCode::CREATED, Json(json!({ "link": link }))))
}

/// Unlink one of the caller's Slack accounts.
pub(super) async fn unlink_slack(Scoped(state): Scoped, Path((team, slack_user)): Path<(String, String)>) -> Result<StatusCode, AppError> {
    if state.store.unlink_slack(&team, &slack_user).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::NotFound("no such Slack account is linked"))
    }
}
