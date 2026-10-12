//! API error type. Every error response is `{"error": {"code": ..., "message": ...}}`.

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("missing or invalid API key")]
    Unauthorized,
    #[error("{0}")]
    NotFound(&'static str),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    TooManyRequests(String),
    #[error("{0}")]
    PayloadTooLarge(String),
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
        if r.status() == StatusCode::PAYLOAD_TOO_LARGE {
            AppError::PayloadTooLarge(r.body_text())
        } else {
            AppError::BadRequest(r.body_text())
        }
    }
}

impl From<QueryRejection> for AppError {
    fn from(r: QueryRejection) -> Self {
        AppError::BadRequest(r.body_text())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, "bad_request", m.clone()),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized", self.to_string()),
            AppError::NotFound(m) => (StatusCode::NOT_FOUND, "not_found", (*m).to_string()),
            AppError::Conflict(m) => (StatusCode::CONFLICT, "conflict", m.clone()),
            AppError::TooManyRequests(m) => (StatusCode::TOO_MANY_REQUESTS, "too_many_requests", m.clone()),
            AppError::PayloadTooLarge(m) => (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large", m.clone()),
            AppError::Internal(e) => {
                tracing::error!(error = ?e, "internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal server error".to_string())
            }
        };
        let mut resp = (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response();
        if status == StatusCode::UNAUTHORIZED {
            resp.headers_mut().insert(header::WWW_AUTHENTICATE, "Bearer".parse().unwrap());
        }
        resp
    }
}
