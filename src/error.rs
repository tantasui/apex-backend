use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::math::MathError;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    BadRequest(String),
    #[error("chain integration is not configured on this server")]
    ChainNotConfigured,
    #[error("upstream RPC error: {0}")]
    Upstream(#[from] anyhow::Error),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("math error: {0}")]
    Math(#[from] MathError),
}

impl AppError {
    fn code_and_status(&self) -> (&'static str, StatusCode) {
        match self {
            AppError::NotFound(_) => ("not_found", StatusCode::NOT_FOUND),
            AppError::BadRequest(_) => ("bad_request", StatusCode::BAD_REQUEST),
            AppError::ChainNotConfigured => ("chain_not_configured", StatusCode::SERVICE_UNAVAILABLE),
            AppError::Upstream(_) => ("upstream_error", StatusCode::BAD_GATEWAY),
            AppError::Db(_) => ("internal_error", StatusCode::INTERNAL_SERVER_ERROR),
            AppError::Math(_) => ("unrepresentable_range", StatusCode::UNPROCESSABLE_ENTITY),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (code, status) = self.code_and_status();
        // Internal details stay in the logs, not in client responses.
        let message = match &self {
            AppError::Db(e) => {
                tracing::error!(error = %e, "database error");
                "internal database error".to_string()
            }
            other => other.to_string(),
        };
        (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response()
    }
}
