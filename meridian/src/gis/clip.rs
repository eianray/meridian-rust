use axum::{extract::Extension, http::HeaderMap, Json};
use serde::Deserialize;
use std::time::{Duration, Instant};
use tokio::time::timeout;
use utoipa::ToSchema;

use crate::{
    error::AppError,
    gis::{compute_price, normalize_geom_to_wgs84, validate_geojson_bytes, GeoJsonInput, GeoJsonOutput},
    gis::reproject::{extract_features, payment_gate, GDAL_SEMAPHORE},
    metrics,
    middleware::request_id::RequestId,
    AppState,
};

/// Clip request — multipart form:
/// - `file`: GeoJSON features to clip (≤200 MB)
/// - `mask`: GeoJSON mask polygon (≤200 MB)
/// - `source_crs` (optional): source CRS, default EPSG:4326
#[derive(Deserialize, ToSchema)]
#[allow(dead_code)]
pub struct ClipParams {
    /// GeoJSON file to clip (≤200 MB, .geojson/.json)
    pub file: String,
    /// GeoJSON mask polygon (≤200 MB, .geojson/.json)
    pub mask: String,
    /// Source CRS (default EPSG:4326)
    pub source_crs: Option<String>,
}

const OP_TIMEOUT: Duration = Duration::from_secs(30);

/// Clip GeoJSON features to a mask polygon.
///
/// Accepts multipart/form-data with:
/// - `file`: GeoJSON features to clip (≤200 MB, .geojson/.json)
/// - `mask`: GeoJSON mask geometry (≤200 MB, .geojson/.json)
/// - `source_crs` (optional): source CRS of input geometries, default EPSG:4326
///
/// Requires x402/Base USDC payment via facilitator unless dev_mode is enabled.
#[utoipa::path(
    post,
    path = "/v1/clip",
    tag = "GIS",
    request_body(
        content_type = "multipart/form-data",
        description = "Multipart form: `file` (GeoJSON features), `mask` (GeoJSON polygon), optional `source_crs`",
        content = ClipParams
    ),
    responses(
        (status = 200, description = "Clipped GeoJSON FeatureCollection", body = GeoJsonOutput),
        (status = 400, description = "Bad request — missing file/mask, invalid JSON, GEOS unavailable"),
        (status = 402, description = "Payment required", body = crate::billing::PaymentRequired),
        (status = 413, description = "Payload too large (>200 MB per file)"),
        (status = 429, description = "Rate limit exceeded — 60 requests/min per IP"),
        (status = 500, description = "Internal server error"),
    )
)]
pub async fn clip(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let mut file_input: Option<GeoJsonInput> = None;
    let mut mask_input: Option<GeoJsonInput> = None;
    let mut source_crs: Option<String> = None;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        match field.name() {
            Some("file") => {
                file_input = Some(GeoJsonInput::from_multipart_field(&mut field).await?);
            }
            Some("mask") => {
                mask_input = Some(GeoJsonInput::from_multipart_field(&mut field).await?);
            }
            Some("source_crs") => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("Error reading source_crs: {e}")))?;
                if !v.trim().is_empty() {
                    source_crs = Some(v.trim().to_string());
                }
            }
            _ => {}
        }
    }

    let request_start = Instant::now();
    metrics::record_request("clip", "received");

    let input = file_input.ok_or_else(|| AppError::BadRequest("Missing 'file' field".into()))?;
    let mask = mask_input.ok_or_else(|| AppError::BadRequest("Missing 'mask' field".into()))?;

    let total_bytes = input.size + mask.size;
    let price = compute_price(total_bytes);

    let src_crs = source_crs.unwrap_or_else(|| "EPSG:4326".to_string());
    let features_str = validate_geojson_bytes(&input.bytes)?;
    let mask_str = validate_geojson_bytes(&mask.bytes)?;

    let payment_result = payment_gate("clip", total_bytes, price, &request_id, &headers, &state).await;
    match &payment_result {
        Ok(_) => metrics::record_payment("clip", if state.config.dev_mode { "dev" } else { "success" }),
        Err(_) => metrics::record_payment("clip", "failed"),
    }
    payment_result?;

    let _permit = GDAL_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Semaphore error: {e}")))?;

    let gdal_start = Instant::now();
    let result = timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
        do_clip(features_str, mask_str, src_crs)
    }))
    .await
    .map_err(|_| AppError::Timeout)?
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Thread panic: {e}")))?
    .map_err(|e: AppError| e)?;
    metrics::record_gdal_duration("clip", gdal_start.elapsed().as_secs_f64());

    metrics::record_request("clip", "ok");
    metrics::record_request_duration("clip", request_start.elapsed().as_secs_f64());

    Ok(Json(GeoJsonOutput {
        request_id,
        price_usd: price,
        result,
    }))
}

// ── Core blocking logic ────────────────────────────────────────────────────────

pub fn do_clip_blocking(
    features_str: String,
    mask_str: String,
    source_crs: String,
) -> Result<serde_json::Value, AppError> {
    do_clip(features_str, mask_str, source_crs)
}

fn do_clip(
    features_str: String,
    mask_str: String,
    source_crs: String,
) -> Result<serde_json::Value, AppError> {
    use gdal::vector::Geometry;

    let fc: serde_json::Value = serde_json::from_str(&features_str)
        .map_err(|e| AppError::BadRequest(format!("Invalid features JSON: {e}")))?;
    let mask_fc: serde_json::Value = serde_json::from_str(&mask_str)
        .map_err(|e| AppError::BadRequest(format!("Invalid mask JSON: {e}")))?;

    let features = extract_features(&fc)?;
    let mask_features = extract_features(&mask_fc)?;

    let mut mask_geoms_wgs84: Vec<Geometry> = Vec::with_capacity(mask_features.len());
    for feat in &mask_features {
        let geom_val = match feat.get("geometry") {
            Some(g) if !g.is_null() => g,
            _ => continue,
        };
        let mut geom = Geometry::from_geojson(&geom_val.to_string())
            .map_err(|e| AppError::BadRequest(format!("Invalid mask geometry: {e}")))?;
        normalize_geom_to_wgs84(&mut geom, &source_crs)?;
        mask_geoms_wgs84.push(geom);
    }

    let mask_geom = union_geometries(mask_geoms_wgs84)?;

    let mut out_features: Vec<serde_json::Value> = Vec::with_capacity(features.len());

    for feat in &features {
        let geom_val = match feat.get("geometry") {
            Some(g) if !g.is_null() => g,
            _ => continue,
        };

        let mut geom = Geometry::from_geojson(&geom_val.to_string())
            .map_err(|e| AppError::BadRequest(format!("Invalid geometry: {e}")))?;

        normalize_geom_to_wgs84(&mut geom, &source_crs)?;

        let clipped = match geom.intersection(&mask_geom) {
            Some(g) if !g.is_empty() => g,
            _ => continue,
        };

        let new_geom_json: serde_json::Value =
            serde_json::from_str(&clipped.json()
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Geometry serialization: {e}")))?)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Geometry JSON parse: {e}")))?;

        let mut new_feat = feat.clone();
        new_feat["geometry"] = new_geom_json;
        out_features.push(new_feat);
    }

    Ok(serde_json::json!({
        "type": "FeatureCollection",
        "features": out_features
    }))
}

fn union_geometries(geoms: Vec<gdal::vector::Geometry>) -> Result<gdal::vector::Geometry, AppError> {
    let mut union_geom: Option<gdal::vector::Geometry> = None;
    for geom in geoms {
        union_geom = Some(match union_geom {
            None => geom,
            Some(existing) => existing
                .union(&geom)
                .ok_or_else(|| AppError::BadRequest("Failed to union mask geometries (GEOS required)".into()))?,
        });
    }
    union_geom.ok_or_else(|| AppError::BadRequest("Mask has no valid geometries".into()))
}
