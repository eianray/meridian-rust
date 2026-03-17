use axum::{extract::Extension, http::HeaderMap, Json};
use std::time::{Duration, Instant};
use tokio::time::timeout;

use crate::{
    error::AppError,
    gis::{
        compute_price,
        combine::{do_append, do_merge, do_spatial_join},
        validate_geojson_bytes, GeoJsonInput, GeoJsonOutput,
    },
    metrics,
    middleware::request_id::RequestId,
    AppState,
};
use crate::gis::reproject::{payment_gate, GDAL_SEMAPHORE};

const OP_TIMEOUT: Duration = Duration::from_secs(60);

async fn read_two_files(
    multipart: &mut axum::extract::Multipart,
) -> Result<(GeoJsonInput, GeoJsonInput), AppError> {
    let mut file_a: Option<GeoJsonInput> = None;
    let mut file_b: Option<GeoJsonInput> = None;
    while let Some(mut field) = multipart.next_field().await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        match field.name() {
            Some("file_a") => { file_a = Some(GeoJsonInput::from_multipart_field(&mut field).await?); }
            Some("file_b") => { file_b = Some(GeoJsonInput::from_multipart_field(&mut field).await?); }
            _ => {}
        }
    }
    let a = file_a.ok_or_else(|| AppError::BadRequest("Missing 'file_a'".into()))?;
    let b = file_b.ok_or_else(|| AppError::BadRequest("Missing 'file_b'".into()))?;
    Ok((a, b))
}

// ── Append ────────────────────────────────────────────────────────────────────

pub async fn append(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let (a, b) = read_two_files(&mut multipart).await?;
    let str_a = validate_geojson_bytes(&a.bytes)?;
    let str_b = validate_geojson_bytes(&b.bytes)?;
    let price = compute_price(a.size + b.size);
    let t0 = Instant::now();
    metrics::record_request("append", "received");
    payment_gate("append", a.size + b.size, price, &request_id, &headers, &state).await?;
    let _permit = GDAL_SEMAPHORE.acquire().await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Semaphore: {e}")))?;
    let result = timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
        do_append(str_a, str_b)
    })).await
        .map_err(|_| AppError::BadRequest("Timed out".into()))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?
        .map_err(|e: AppError| e)?;
    metrics::record_request("append", "ok");
    metrics::record_request_duration("append", t0.elapsed().as_secs_f64());
    Ok(Json(GeoJsonOutput { request_id, price_usd: price, result }))
}

// ── Merge ─────────────────────────────────────────────────────────────────────

pub async fn merge(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let (a, b) = read_two_files(&mut multipart).await?;
    let str_a = validate_geojson_bytes(&a.bytes)?;
    let str_b = validate_geojson_bytes(&b.bytes)?;
    let price = compute_price(a.size + b.size);
    let t0 = Instant::now();
    metrics::record_request("merge", "received");
    payment_gate("merge", a.size + b.size, price, &request_id, &headers, &state).await?;
    let _permit = GDAL_SEMAPHORE.acquire().await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Semaphore: {e}")))?;
    let result = timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
        do_merge(str_a, str_b)
    })).await
        .map_err(|_| AppError::BadRequest("Timed out".into()))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?
        .map_err(|e: AppError| e)?;
    metrics::record_request("merge", "ok");
    metrics::record_request_duration("merge", t0.elapsed().as_secs_f64());
    Ok(Json(GeoJsonOutput { request_id, price_usd: price, result }))
}

// ── Spatial Join ──────────────────────────────────────────────────────────────

pub async fn spatial_join(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let mut file_a: Option<GeoJsonInput> = None;
    let mut file_b: Option<GeoJsonInput> = None;
    let mut how = "left".to_string();
    let mut predicate = "intersects".to_string();

    while let Some(mut field) = multipart.next_field().await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        match field.name() {
            Some("file_a") => { file_a = Some(GeoJsonInput::from_multipart_field(&mut field).await?); }
            Some("file_b") => { file_b = Some(GeoJsonInput::from_multipart_field(&mut field).await?); }
            Some("how") => {
                let v = field.text().await.unwrap_or_default();
                if !v.trim().is_empty() { how = v.trim().to_string(); }
            }
            Some("predicate") => {
                let v = field.text().await.unwrap_or_default();
                if !v.trim().is_empty() { predicate = v.trim().to_string(); }
            }
            _ => {}
        }
    }

    let a = file_a.ok_or_else(|| AppError::BadRequest("Missing 'file_a'".into()))?;
    let b = file_b.ok_or_else(|| AppError::BadRequest("Missing 'file_b'".into()))?;
    let str_a = validate_geojson_bytes(&a.bytes)?;
    let str_b = validate_geojson_bytes(&b.bytes)?;
    let price = compute_price(a.size + b.size);
    let t0 = Instant::now();
    metrics::record_request("spatial_join", "received");
    payment_gate("spatial-join", a.size + b.size, price, &request_id, &headers, &state).await?;
    let _permit = GDAL_SEMAPHORE.acquire().await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Semaphore: {e}")))?;
    let result = timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
        do_spatial_join(str_a, str_b, how, predicate)
    })).await
        .map_err(|_| AppError::BadRequest("Timed out".into()))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?
        .map_err(|e: AppError| e)?;
    metrics::record_request("spatial_join", "ok");
    metrics::record_request_duration("spatial_join", t0.elapsed().as_secs_f64());
    Ok(Json(GeoJsonOutput { request_id, price_usd: price, result }))
}
