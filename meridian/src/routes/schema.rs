use axum::{extract::Extension, http::HeaderMap, Json};
use std::time::{Duration, Instant};
use tokio::time::timeout;

use crate::{
    error::AppError,
    gis::{
        compute_price,
        schema::{do_repair, do_schema, do_validate},
        validate_geojson_bytes, GeoJsonInput, GeoJsonOutput,
    },
    metrics,
    middleware::request_id::RequestId,
    AppState,
};
use crate::gis::reproject::{payment_gate, GDAL_SEMAPHORE};

const OP_TIMEOUT: Duration = Duration::from_secs(30);

// ── Schema ─────────────────────────────────────────────────────────────────────

pub async fn schema(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let mut file_input: Option<GeoJsonInput> = None;
    let mut filename = "file.geojson".to_string();

    while let Some(mut field) = multipart.next_field().await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        if field.name() == Some("file") {
            if let Some(fn_) = field.file_name().map(|s| s.to_string()) {
                filename = fn_;
            }
            file_input = Some(GeoJsonInput::from_multipart_field(&mut field).await?);
        }
    }

    let input = file_input.ok_or_else(|| AppError::BadRequest("Missing 'file' field".into()))?;
    let geojson_str = validate_geojson_bytes(&input.bytes)?;
    let price = compute_price(input.size);
    let t0 = Instant::now();
    metrics::record_request("schema", "received");
    payment_gate("schema", input.size, price, &request_id, &headers, &state).await?;
    let _permit = GDAL_SEMAPHORE.acquire().await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Semaphore: {e}")))?;
    let result = timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
        do_schema(geojson_str, filename)
    })).await
        .map_err(|_| AppError::BadRequest("Timed out".into()))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?
        .map_err(|e: AppError| e)?;
    metrics::record_request("schema", "ok");
    metrics::record_request_duration("schema", t0.elapsed().as_secs_f64());
    Ok(Json(GeoJsonOutput { request_id, price_usd: price, result }))
}

// ── Validate ───────────────────────────────────────────────────────────────────

pub async fn validate(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let mut file_input: Option<GeoJsonInput> = None;
    while let Some(mut field) = multipart.next_field().await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        if field.name() == Some("file") {
            file_input = Some(GeoJsonInput::from_multipart_field(&mut field).await?);
        }
    }
    let input = file_input.ok_or_else(|| AppError::BadRequest("Missing 'file' field".into()))?;
    let geojson_str = validate_geojson_bytes(&input.bytes)?;
    let price = compute_price(input.size);
    let t0 = Instant::now();
    metrics::record_request("validate", "received");
    payment_gate("validate", input.size, price, &request_id, &headers, &state).await?;
    let _permit = GDAL_SEMAPHORE.acquire().await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Semaphore: {e}")))?;
    let result = timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
        do_validate(geojson_str)
    })).await
        .map_err(|_| AppError::BadRequest("Timed out".into()))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?
        .map_err(|e: AppError| e)?;
    metrics::record_request("validate", "ok");
    metrics::record_request_duration("validate", t0.elapsed().as_secs_f64());
    Ok(Json(GeoJsonOutput { request_id, price_usd: price, result }))
}

// ── Repair ─────────────────────────────────────────────────────────────────────

pub async fn repair(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let mut file_input: Option<GeoJsonInput> = None;
    while let Some(mut field) = multipart.next_field().await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        if field.name() == Some("file") {
            file_input = Some(GeoJsonInput::from_multipart_field(&mut field).await?);
        }
    }
    let input = file_input.ok_or_else(|| AppError::BadRequest("Missing 'file' field".into()))?;
    let geojson_str = validate_geojson_bytes(&input.bytes)?;
    let price = compute_price(input.size);
    let t0 = Instant::now();
    metrics::record_request("repair", "received");
    payment_gate("repair", input.size, price, &request_id, &headers, &state).await?;
    let _permit = GDAL_SEMAPHORE.acquire().await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Semaphore: {e}")))?;
    let result = timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
        do_repair(geojson_str)
    })).await
        .map_err(|_| AppError::BadRequest("Timed out".into()))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?
        .map_err(|e: AppError| e)?;
    metrics::record_request("repair", "ok");
    metrics::record_request_duration("repair", t0.elapsed().as_secs_f64());
    Ok(Json(GeoJsonOutput { request_id, price_usd: price, result }))
}
