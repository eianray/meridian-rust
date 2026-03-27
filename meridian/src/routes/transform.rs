use axum::{extract::Extension, http::HeaderMap, Json};
use std::time::{Duration, Instant};
use tokio::time::timeout;

use crate::{
    error::AppError,
    gis::{
        compute_price,
        transform::{
            do_add_field, do_erase, do_feature_to_line, do_feature_to_point,
            do_feature_to_polygon, do_multipart_to_singlepart,
        },
        validate_geojson_bytes, GeoJsonInput, GeoJsonOutput,
    },
    metrics,
    middleware::request_id::RequestId,
    AppState,
};
use crate::gis::reproject::{payment_gate, GDAL_SEMAPHORE};

const OP_TIMEOUT: Duration = Duration::from_secs(30);

// ── Erase ──────────────────────────────────────────────────────────────────────

pub async fn erase(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let mut file_input: Option<GeoJsonInput> = None;

    while let Some(mut field) = multipart
        .next_field().await
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
    metrics::record_request("erase", "received");

    payment_gate("erase", input.size, price, &request_id, &headers, &state).await?;

    let _permit = GDAL_SEMAPHORE.acquire().await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Semaphore: {e}")))?;

    let result = timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
        do_erase(geojson_str)
    }))
    .await
    .map_err(|_| AppError::BadRequest("Operation timed out".into()))?
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Thread panic: {e}")))?
    .map_err(|e: AppError| e)?;

    metrics::record_request("erase", "ok");
    metrics::record_request_duration("erase", t0.elapsed().as_secs_f64());

    Ok(Json(GeoJsonOutput { request_id, price_usd: price, result }))
}

// ── Feature to Point ──────────────────────────────────────────────────────────

pub async fn feature_to_point(
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
    metrics::record_request("feature_to_point", "received");
    payment_gate("feature-to-point", input.size, price, &request_id, &headers, &state).await?;
    let _permit = GDAL_SEMAPHORE.acquire().await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Semaphore: {e}")))?;
    let result = timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
        do_feature_to_point(geojson_str)
    })).await
        .map_err(|_| AppError::BadRequest("Timed out".into()))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?
        .map_err(|e: AppError| e)?;
    metrics::record_request("feature_to_point", "ok");
    metrics::record_request_duration("feature_to_point", t0.elapsed().as_secs_f64());
    Ok(Json(GeoJsonOutput { request_id, price_usd: price, result }))
}

// ── Feature to Line ───────────────────────────────────────────────────────────

pub async fn feature_to_line(
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
    metrics::record_request("feature_to_line", "received");
    payment_gate("feature-to-line", input.size, price, &request_id, &headers, &state).await?;
    let _permit = GDAL_SEMAPHORE.acquire().await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Semaphore: {e}")))?;
    let result = timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
        do_feature_to_line(geojson_str)
    })).await
        .map_err(|_| AppError::BadRequest("Timed out".into()))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?
        .map_err(|e: AppError| e)?;
    metrics::record_request("feature_to_line", "ok");
    metrics::record_request_duration("feature_to_line", t0.elapsed().as_secs_f64());
    Ok(Json(GeoJsonOutput { request_id, price_usd: price, result }))
}

// ── Feature to Polygon ────────────────────────────────────────────────────────

pub async fn feature_to_polygon(
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
    metrics::record_request("feature_to_polygon", "received");
    payment_gate("feature-to-polygon", input.size, price, &request_id, &headers, &state).await?;
    let _permit = GDAL_SEMAPHORE.acquire().await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Semaphore: {e}")))?;
    let result = timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
        do_feature_to_polygon(geojson_str)
    })).await
        .map_err(|_| AppError::BadRequest("Timed out".into()))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?
        .map_err(|e: AppError| e)?;
    metrics::record_request("feature_to_polygon", "ok");
    metrics::record_request_duration("feature_to_polygon", t0.elapsed().as_secs_f64());
    Ok(Json(GeoJsonOutput { request_id, price_usd: price, result }))
}

// ── Multipart to Singlepart ───────────────────────────────────────────────────

pub async fn multipart_to_singlepart(
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
    metrics::record_request("multipart_to_singlepart", "received");
    payment_gate("multipart-to-singlepart", input.size, price, &request_id, &headers, &state).await?;
    let _permit = GDAL_SEMAPHORE.acquire().await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Semaphore: {e}")))?;
    let result = timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
        do_multipart_to_singlepart(geojson_str)
    })).await
        .map_err(|_| AppError::BadRequest("Timed out".into()))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?
        .map_err(|e: AppError| e)?;
    metrics::record_request("multipart_to_singlepart", "ok");
    metrics::record_request_duration("multipart_to_singlepart", t0.elapsed().as_secs_f64());
    Ok(Json(GeoJsonOutput { request_id, price_usd: price, result }))
}

// ── Add Field ─────────────────────────────────────────────────────────────────

pub async fn add_field(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let mut file_input: Option<GeoJsonInput> = None;
    let mut field_name: Option<String> = None;
    let mut field_type: Option<String> = None;
    let mut default_value: Option<String> = None;

    while let Some(mut field) = multipart.next_field().await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        match field.name() {
            Some("file") => {
                file_input = Some(GeoJsonInput::from_multipart_field(&mut field).await?);
            }
            Some("field_name") => {
                field_name = Some(field.text().await
                    .map_err(|e| AppError::BadRequest(format!("field_name: {e}")))?);
            }
            Some("field_type") => {
                field_type = Some(field.text().await
                    .map_err(|e| AppError::BadRequest(format!("field_type: {e}")))?);
            }
            Some("default_value") => {
                let v = field.text().await
                    .map_err(|e| AppError::BadRequest(format!("default_value: {e}")))?;
                if !v.trim().is_empty() {
                    default_value = Some(v);
                }
            }
            _ => {}
        }
    }

    let input = file_input.ok_or_else(|| AppError::BadRequest("Missing 'file' field".into()))?;
    let fname = field_name.ok_or_else(|| AppError::BadRequest("Missing 'field_name'".into()))?;
    let ftype = field_type.unwrap_or_else(|| "str".to_string());

    let geojson_str = validate_geojson_bytes(&input.bytes)?;
    let price = compute_price(input.size);
    let t0 = Instant::now();
    metrics::record_request("add_field", "received");
    payment_gate("add-field", input.size, price, &request_id, &headers, &state).await?;
    let _permit = GDAL_SEMAPHORE.acquire().await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Semaphore: {e}")))?;
    let result = timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
        do_add_field(geojson_str, fname, ftype, default_value)
    })).await
        .map_err(|_| AppError::BadRequest("Timed out".into()))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?
        .map_err(|e: AppError| e)?;
    metrics::record_request("add_field", "ok");
    metrics::record_request_duration("add_field", t0.elapsed().as_secs_f64());
    Ok(Json(GeoJsonOutput { request_id, price_usd: price, result }))
}

// ── Calculate Geometry ────────────────────────────────────────────────────────

pub async fn calculate_geometry(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let mut file_input: Option<GeoJsonInput> = None;
    let mut property   = "area".to_string();
    let mut field_name: Option<String> = None;
    let mut area_unit   = "sqm".to_string();
    let mut length_unit = "m".to_string();

    while let Some(mut field) = multipart.next_field().await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        match field.name() {
            Some("file") => {
                file_input = Some(GeoJsonInput::from_multipart_field(&mut field).await?);
            }
            Some("property") => {
                let v = field.text().await.map_err(|e| AppError::BadRequest(format!("property: {e}")))?;
                if !v.trim().is_empty() { property = v.trim().to_string(); }
            }
            Some("field_name") => {
                let v = field.text().await.map_err(|e| AppError::BadRequest(format!("field_name: {e}")))?;
                if !v.trim().is_empty() { field_name = Some(v.trim().to_string()); }
            }
            Some("area_unit") => {
                let v = field.text().await.map_err(|e| AppError::BadRequest(format!("area_unit: {e}")))?;
                if !v.trim().is_empty() { area_unit = v.trim().to_string(); }
            }
            Some("length_unit") => {
                let v = field.text().await.map_err(|e| AppError::BadRequest(format!("length_unit: {e}")))?;
                if !v.trim().is_empty() { length_unit = v.trim().to_string(); }
            }
            // Legacy compat: accept 'units' as area_unit alias
            Some("units") => {
                let v = field.text().await.map_err(|e| AppError::BadRequest(format!("units: {e}")))?;
                if !v.trim().is_empty() { area_unit = v.trim().to_string(); }
            }
            _ => {}
        }
    }

    // Default field_name to property name if not provided
    let fname = field_name.unwrap_or_else(|| property.clone());

    let input = file_input.ok_or_else(|| AppError::BadRequest("Missing 'file' field".into()))?;
    let geojson_str = validate_geojson_bytes(&input.bytes)?;
    let price = compute_price(input.size);
    let t0 = Instant::now();
    metrics::record_request("calculate_geometry", "received");
    payment_gate("calculate-geometry", input.size, price, &request_id, &headers, &state).await?;
    let _permit = GDAL_SEMAPHORE.acquire().await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Semaphore: {e}")))?;
    let result = timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
        crate::gis::transform::do_calculate_geometry(geojson_str, property, fname, area_unit, length_unit)
    })).await
        .map_err(|_| AppError::BadRequest("Timed out".into()))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?
        .map_err(|e: AppError| e)?;
    metrics::record_request("calculate_geometry", "ok");
    metrics::record_request_duration("calculate_geometry", t0.elapsed().as_secs_f64());
    Ok(Json(GeoJsonOutput { request_id, price_usd: price, result }))
}
