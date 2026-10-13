//! API error type. Every error response is `{"error": {"code": ..., "message": ...}}`, the same
//! shape cedar uses, so cedar's own errors can be passed through unchanged.

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("missing or invalid access token")]
    Unauthorized,
    /// Settings only an admin may change, without the admin token.
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    NotFound(&'static str),
    #[error("{0}")]
    Conflict(String),
    /// cedar answered with an error; relay it.
    #[error("{message}")]
    Upstream { status: StatusCode, code: String, message: String },
    /// cedar could not be reached or answered with something unreadable.
    #[error("{0}")]
    BadGateway(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        AppError::Internal(e.into())
    }
}

impl From<JsonRejection> for AppError {
    fn from(r: JsonRejection) -> Self {
        AppError::BadRequest(r.body_text())
    }
}

impl From<QueryRejection> for AppError {
    fn from(r: QueryRejection) -> Self {
        AppError::BadRequest(r.body_text())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, "bad_request".to_string(), m),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".into(), self.to_string()),
            AppError::Forbidden(m) => (StatusCode::FORBIDDEN, "forbidden".into(), m),
            AppError::NotFound(m) => (StatusCode::NOT_FOUND, "not_found".into(), m.to_string()),
            AppError::Conflict(m) => (StatusCode::CONFLICT, "conflict".into(), m),
            AppError::Upstream { status, code, message } => (status, code, message),
            AppError::BadGateway(m) => {
                tracing::warn!(error = %m, "cedar request failed");
                (StatusCode::BAD_GATEWAY, "bad_gateway".into(), m)
            }
            AppError::Internal(e) => {
                tracing::error!(error = ?e, "internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal".into(), "internal server error".into())
            }
        };
        (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response()
    }
}
