//! Settings -> Slack (`/api/slack`): whether the Slack app is set up, and linking one's Slack
//! account to oneself here with the code the app sent it (see `slack`).

use super::*;

/// `{available, workspace?, links}`: `workspace` is `{team, team_name, bot}` once the app is
/// connected; `links` are the caller's Slack accounts.
pub(super) async fn get_slack(Scoped(state): Scoped) -> Result<Json<Value>, AppError> {
    let links = state.store.slack_links().await?;
    let workspace = match &state.slack {
        Some(slack) => slack.me().await.map(|me| json!({ "team": me.team, "team_name": me.team_name, "bot": me.name })),
        None => None,
    };
    Ok(Json(json!({ "available": state.slack.is_some(), "workspace": workspace, "links": links })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LinkBody {
    code: String,
}

/// Link the Slack account a code was sent to. `400` for a code that isn't one, was used, or ran
/// out; `404` without a Slack app.
pub(super) async fn link_slack(Scoped(state): Scoped, ApiJson(body): ApiJson<LinkBody>) -> Result<(StatusCode, Json<Value>), AppError> {
    let slack = state.slack.clone().ok_or(AppError::NotFound("Slack is not set up on this server"))?;
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
