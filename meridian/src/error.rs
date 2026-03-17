use tracing;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),

    #[error("Payment required")]
    PaymentRequired {
        body: crate::billing::PaymentRequired,
    },

    #[error("Unsupported media type: {0}")]
    UnsupportedMediaType(String),

    #[error("Payload too large")]
    PayloadTooLarge,

    #[error("Operation timed out")]
    Timeout,
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<serde_json::Value>,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::NotFound(msg) => (
                StatusCode::NOT_FOUND,
                Json(ErrorBody { error: msg, detail: None }),
            ).into_response(),
            AppError::BadRequest(msg) => (
                StatusCode::BAD_REQUEST,
                Json(ErrorBody { error: msg, detail: None }),
            ).into_response(),
            AppError::Internal(e) => {
                tracing::error!(error = %e, "Internal server error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorBody { error: "Internal server error. Include the request_id when reporting.".into(), detail: None }),
                ).into_response()
            }
            AppError::PaymentRequired { body } => {
                // Return full x402 body as JSON with 402 status
                (StatusCode::PAYMENT_REQUIRED, Json(body)).into_response()
            }
            AppError::UnsupportedMediaType(msg) => (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                Json(ErrorBody { error: msg, detail: None }),
            ).into_response(),
            AppError::PayloadTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(ErrorBody { error: "Payload too large".into(), detail: None }),
            ).into_response(),
            AppError::Timeout => (
                StatusCode::REQUEST_TIMEOUT,
                Json(ErrorBody { error: "Operation timed out (30s limit).".into(), detail: None }),
            ).into_response(),
        }
    }
}
