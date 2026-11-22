//! The people who use this server (`/api/users`), managed with the admin token, and who the
//! caller is (`/api/me`).
//!
//! A user signs in with an access token of their own, made here and shown once; only its
//! SHA-256 is kept. What they make is theirs alone: their conversations, agents, settings and
//! devices here (see `store`), and their runs, memories and records in cedar, which are filed
//! under the user's id (`X-Cedar-User`). The primary user is the one the server started with:
//! whoever presents `CHAT_ACCESS_TOKEN`.

use super::*;
use crate::store::User;

/// The longest name a user can have.
const MAX_NAME_CHARS: usize = 80;

fn require_users_admin(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    if state.admin_digest.is_none() {
        return Err(AppError::Forbidden("users are managed with the admin token, and CHAT_ADMIN_TOKEN is not set".into()));
    }
    require_admin(state, headers)
}

/// A new access token: 32 random bytes in hex, and the SHA-256 (hex) that is stored.
fn new_token() -> Result<(String, String), AppError> {
    let mut buf = [0u8; 32];
    openssl::rand::rand_bytes(&mut buf).map_err(|e| AppError::Internal(e.into()))?;
    let token = hex::encode(buf);
    let digest = hex::encode(Sha256::digest(token.as_bytes()));
    Ok((token, digest))
}

fn clean_name(raw: &str) -> Result<String, AppError> {
    let name = raw.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return Err(AppError::BadRequest(format!("name must be 1 to {MAX_NAME_CHARS} characters")));
    }
    Ok(name.to_string())
}

async fn find_user(state: &AppState, raw_id: &str) -> Result<User, AppError> {
    let id = parse_id(raw_id, "user not found")?;
    state.store.get_user(id).await?.ok_or(AppError::NotFound("user not found"))
}

/// Who the caller is.
pub(super) async fn get_me(Scoped(state): Scoped) -> Result<Json<User>, AppError> {
    Ok(Json(state.store.get_user(state.user()).await?.ok_or(AppError::Unauthorized)?))
}

pub(super) async fn list_users(Scoped(state): Scoped, headers: HeaderMap) -> Result<Json<Value>, AppError> {
    require_users_admin(&state, &headers)?;
    Ok(Json(json!({ "users": state.store.list_users().await? })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UserBody {
    name: String,
}

/// Add a user. The answer carries their access token, this once.
pub(super) async fn create_user(Scoped(state): Scoped, headers: HeaderMap, ApiJson(body): ApiJson<UserBody>) -> Result<(StatusCode, Json<Value>), AppError> {
    require_users_admin(&state, &headers)?;
    let name = clean_name(&body.name)?;
    // Before there is a second user: cedar has to let this service act for one, or their runs
    // would be refused (or, worse, filed with the primary user's).
    match state.cedar.as_user(Some(Uuid::new_v4())).check_user().await {
        Err(AppError::Upstream { status: StatusCode::FORBIDDEN, .. }) => {
            return Err(AppError::Conflict(
                "cedar does not let this service act for users: add cedar-chat's CEDAR_API_KEY to cedar's CEDAR_SERVICE_KEYS and restart cedar".into(),
            ))
        }
        other => other?,
    }
    let (token, digest) = new_token()?;
    let user = state.store.create_user(&name, &digest).await?;
    Ok((StatusCode::CREATED, Json(json!({ "user": user, "token": token }))))
}

pub(super) async fn rename_user(Scoped(state): Scoped, headers: HeaderMap, Path(id): Path<String>, ApiJson(body): ApiJson<UserBody>) -> Result<Json<User>, AppError> {
    require_users_admin(&state, &headers)?;
    let user = find_user(&state, &id).await?;
    let name = clean_name(&body.name)?;
    state.store.rename_user(user.id, &name).await?;
    Ok(Json(User { name, ..user }))
}

/// Give a user a new access token; the old one stops working at once.
pub(super) async fn rotate_user_token(Scoped(state): Scoped, headers: HeaderMap, Path(id): Path<String>) -> Result<Json<Value>, AppError> {
    require_users_admin(&state, &headers)?;
    let user = find_user(&state, &id).await?;
    if user.primary {
        return Err(AppError::Conflict("the primary user signs in with CHAT_ACCESS_TOKEN".into()));
    }
    let (token, digest) = new_token()?;
    state.store.set_user_token(user.id, &digest).await?;
    Ok(Json(json!({ "user": user, "token": token })))
}

/// Remove a user with everything of theirs: here, and their runs, memories, records and Google
/// account in cedar
/// (best effort: what cedar can't be asked to delete now stays there, out of anyone's reach).
pub(super) async fn delete_user(Scoped(state): Scoped, headers: HeaderMap, Path(id): Path<String>) -> Result<StatusCode, AppError> {
    require_users_admin(&state, &headers)?;
    let user = find_user(&state, &id).await?;
    if user.primary {
        return Err(AppError::Conflict("the primary user can't be removed".into()));
    }
    let theirs = state.as_user(user.id);
    for run_id in theirs.store.list_run_ids().await? {
        if let Err(e) = theirs.cedar.delete_run(run_id).await {
            tracing::warn!(%run_id, error = %e, "could not delete cedar run");
        }
    }
    if let Err(e) = theirs.cedar.delete_all_memory().await {
        tracing::warn!(user_id = %user.id, error = %e, "could not delete the user's memories in cedar");
    }
    // Their Google account, if they connected one (404 when they didn't).
    match theirs.cedar.remove_own_google().await {
        Ok(()) | Err(AppError::Upstream { status: StatusCode::NOT_FOUND, .. }) => {}
        Err(e) => tracing::warn!(user_id = %user.id, error = %e, "could not disconnect the user's Google account in cedar"),
    }
    if let Err(e) = theirs.cedar.delete_all_records().await {
        tracing::warn!(user_id = %user.id, error = %e, "could not delete the user's records in cedar");
    }
    state.store.delete_user(user.id).await?;
    Ok(StatusCode::NO_CONTENT)
}
