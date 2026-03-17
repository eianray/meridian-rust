use axum::{extract::Extension, Json};
use serde::Serialize;
use utoipa::ToSchema;

use crate::middleware::request_id::RequestId;

/// Health check response
#[derive(Serialize, ToSchema)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
    pub request_id: String,
}

/// Health check endpoint — always returns 200 OK
///
/// Free endpoint, no payment required.
#[utoipa::path(
    get,
    path = "/v1/health",
    tag = "Info",
    responses(
        (status = 200, description = "Service is healthy", body = HealthResponse),
        (status = 500, description = "Internal server error"),
    )
)]
pub async fn health(Extension(RequestId(id)): Extension<RequestId>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        request_id: id,
    })
}
