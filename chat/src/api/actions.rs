//! Saved actions (`/api/actions`): what the person showed Cedar how to do, by recording their
//! screen while they did it. cedar keeps the actions, turns a recording into steps, and keeps
//! the passwords stored with them (see `actions` in cedar's README); this service passes the
//! requests on for whoever is signed in. A recording is made small by the page before it gets
//! here, and nothing of it is kept: it goes to cedar's model once and is gone.

use super::*;

/// A recording as the page sends it: its key frames and video as base64 (see cedar's limits).
pub(super) const MAX_RECORDING_BODY_BYTES: usize = 40 << 20;

/// An action as the page saves it: its text and the few pictures of its recording kept with it,
/// as base64 (cedar's limit is the same).
pub(super) const MAX_ACTION_BODY_BYTES: usize = 10 << 20;

fn action_id(raw: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(raw).map_err(|_| AppError::NotFound("action not found"))
}

/// `GET /api/actions`: the person's actions, with what a recording may carry and whether this
/// server can watch one and keep a password. A cedar from before actions answers
/// `{"supported": false}`.
pub(super) async fn list(Scoped(state): Scoped) -> Result<Json<Value>, AppError> {
    let Some(mut body) = state.cedar.actions().await? else {
        return Ok(Json(json!({ "supported": false, "actions": [] })));
    };
    body["supported"] = json!(true);
    Ok(Json(body))
}

/// `POST /api/actions/draft`: the steps cedar's model reads from a recording. Saves nothing.
pub(super) async fn draft(Scoped(state): Scoped, ApiJson(recording): ApiJson<Value>) -> Result<Json<Value>, AppError> {
    Ok(Json(state.cedar.draft_action(&recording).await?))
}

/// `POST /api/actions`: save an action as the person approved it.
pub(super) async fn create(Scoped(state): Scoped, ApiJson(action): ApiJson<Value>) -> Result<(StatusCode, Json<Value>), AppError> {
    let saved = state.cedar.save_action(None, &action).await?;
    let _ = state.notices.send((state.user(), json!({ "type": "actions.changed" })));
    Ok((StatusCode::CREATED, Json(saved)))
}

pub(super) async fn get(Scoped(state): Scoped, Path(id): Path<String>) -> Result<Json<Value>, AppError> {
    Ok(Json(state.cedar.action(action_id(&id)?).await?))
}

/// `GET /api/actions/{id}/frames`: the pictures kept with an action, for its form.
pub(super) async fn frames(Scoped(state): Scoped, Path(id): Path<String>) -> Result<Json<Value>, AppError> {
    Ok(Json(state.cedar.action_frames(action_id(&id)?).await?))
}

/// `PUT /api/actions/{id}`: change an action. A secret sent without a value keeps its saved one.
pub(super) async fn update(Scoped(state): Scoped, Path(id): Path<String>, ApiJson(action): ApiJson<Value>) -> Result<Json<Value>, AppError> {
    let saved = state.cedar.save_action(Some(action_id(&id)?), &action).await?;
    let _ = state.notices.send((state.user(), json!({ "type": "actions.changed" })));
    Ok(Json(saved))
}

/// `DELETE /api/actions/{id}`: delete an action and the passwords stored with it.
pub(super) async fn delete(Scoped(state): Scoped, Path(id): Path<String>) -> Result<StatusCode, AppError> {
    state.cedar.delete_action(action_id(&id)?).await?;
    let _ = state.notices.send((state.user(), json!({ "type": "actions.changed" })));
    Ok(StatusCode::NO_CONTENT)
}
