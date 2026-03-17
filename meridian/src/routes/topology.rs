use axum::{extract::Extension, http::HeaderMap, Json};
use std::time::{Duration, Instant};
use tokio::time::timeout;

use crate::{
    error::AppError,
    gis::{
        compute_price,
        topology::{do_difference, do_intersect, do_union},
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

    let a = file_a.ok_or_else(|| AppError::BadRequest("Missing 'file_a' field".into()))?;
    let b = file_b.ok_or_else(|| AppError::BadRequest("Missing 'file_b' field".into()))?;
    Ok((a, b))
}

// ── Union ─────────────────────────────────────────────────────────────────────

pub async fn union(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let mut file_a: Option<GeoJsonInput> = None;
    let mut file_b: Option<GeoJsonInput> = None;
    let mut dissolve = false;

    while let Some(mut field) = multipart.next_field().await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        match field.name() {
            Some("file_a") => { file_a = Some(GeoJsonInput::from_multipart_field(&mut field).await?); }
            Some("file_b") => { file_b = Some(GeoJsonInput::from_multipart_field(&mut field).await?); }
            Some("dissolve") => {
                let v = field.text().await.unwrap_or_default();
                dissolve = matches!(v.trim().to_lowercase().as_str(), "true" | "1" | "yes");
            }
            _ => {}
        }
    }

    let a = file_a.ok_or_else(|| AppError::BadRequest("Missing 'file_a' field".into()))?;
    let b = file_b.ok_or_else(|| AppError::BadRequest("Missing 'file_b' field".into()))?;
    let str_a = validate_geojson_bytes(&a.bytes)?;
    let str_b = validate_geojson_bytes(&b.bytes)?;
    let price = compute_price(a.size + b.size);
    let t0 = Instant::now();
    metrics::record_request("union", "received");
    payment_gate("union", a.size + b.size, price, &request_id, &headers, &state).await?;
    let _permit = GDAL_SEMAPHORE.acquire().await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Semaphore: {e}")))?;
    let result = timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
        do_union(str_a, str_b, dissolve)
    })).await
        .map_err(|_| AppError::BadRequest("Timed out".into()))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?
        .map_err(|e: AppError| e)?;
    metrics::record_request("union", "ok");
    metrics::record_request_duration("union", t0.elapsed().as_secs_f64());
    Ok(Json(GeoJsonOutput { request_id, price_usd: price, result }))
}

// ── Intersect ─────────────────────────────────────────────────────────────────

pub async fn intersect(
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
    metrics::record_request("intersect", "received");
    payment_gate("intersect", a.size + b.size, price, &request_id, &headers, &state).await?;
    let _permit = GDAL_SEMAPHORE.acquire().await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Semaphore: {e}")))?;
    let result = timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
        do_intersect(str_a, str_b)
    })).await
        .map_err(|_| AppError::BadRequest("Timed out".into()))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?
        .map_err(|e: AppError| e)?;
    metrics::record_request("intersect", "ok");
    metrics::record_request_duration("intersect", t0.elapsed().as_secs_f64());
    Ok(Json(GeoJsonOutput { request_id, price_usd: price, result }))
}

// ── Difference ────────────────────────────────────────────────────────────────

pub async fn difference(
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
    metrics::record_request("difference", "received");
    payment_gate("difference", a.size + b.size, price, &request_id, &headers, &state).await?;
    let _permit = GDAL_SEMAPHORE.acquire().await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Semaphore: {e}")))?;
    let result = timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
        do_difference(str_a, str_b)
    })).await
        .map_err(|_| AppError::BadRequest("Timed out".into()))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?
        .map_err(|e: AppError| e)?;
    metrics::record_request("difference", "ok");
    metrics::record_request_duration("difference", t0.elapsed().as_secs_f64());
    Ok(Json(GeoJsonOutput { request_id, price_usd: price, result }))
}
