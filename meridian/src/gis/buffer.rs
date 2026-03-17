use axum::{extract::Extension, http::HeaderMap, Json};
use serde::Deserialize;
use std::time::{Duration, Instant};
use tokio::time::timeout;
use utoipa::ToSchema;

use crate::{
    error::AppError,
    gis::{
        auto_utm_epsg, compute_price, normalize_geom_to_wgs84, validate_geojson_bytes, GeoJsonInput,
        GeoJsonOutput,
    },
    gis::reproject::{extract_features, payment_gate, GDAL_SEMAPHORE},
    metrics,
    middleware::request_id::RequestId,
    AppState,
};

/// Buffer request — multipart form:
/// - `file`: GeoJSON file (≤200 MB)
/// - `distance`: buffer distance in meters (numeric string)
/// - `source_crs` (optional): source CRS, default EPSG:4326
#[derive(Deserialize, ToSchema)]
#[allow(dead_code)]
pub struct BufferParams {
    /// GeoJSON file (≤200 MB, .geojson/.json)
    pub file: String,
    /// Buffer distance in meters
    pub distance: f64,
    /// Source CRS (default EPSG:4326)
    pub source_crs: Option<String>,
}

const OP_TIMEOUT: Duration = Duration::from_secs(30);

/// Buffer GeoJSON features by a given distance in **meters**, using auto-UTM projection.
///
/// Accepts multipart/form-data with:
/// - `file`: GeoJSON file (≤200 MB, .geojson/.json)
/// - `distance`: buffer distance in meters
/// - `source_crs` (optional): source CRS of input geometry, default EPSG:4326
///
/// Requires x402/Base USDC payment via facilitator unless dev_mode is enabled.
#[utoipa::path(
    post,
    path = "/v1/buffer",
    tag = "GIS",
    request_body(
        content_type = "multipart/form-data",
        description = "Multipart form: `file` (GeoJSON), `distance` (meters), optional `source_crs`",
        content = BufferParams
    ),
    responses(
        (status = 200, description = "Buffered GeoJSON FeatureCollection", body = GeoJsonOutput),
        (status = 400, description = "Bad request — missing file/distance, invalid JSON, non-numeric distance"),
        (status = 402, description = "Payment required", body = crate::billing::PaymentRequired),
        (status = 413, description = "Payload too large (>200 MB)"),
        (status = 429, description = "Rate limit exceeded — 60 requests/min per IP"),
        (status = 500, description = "Internal server error"),
    )
)]
pub async fn buffer(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let mut file_input: Option<GeoJsonInput> = None;
    let mut distance_str: Option<String> = None;
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
            Some("distance") => {
                distance_str = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| AppError::BadRequest(format!("Error reading distance: {e}")))?,
                );
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
    metrics::record_request("buffer", "received");

    let input = file_input.ok_or_else(|| AppError::BadRequest("Missing 'file' field".into()))?;
    let dist_s = distance_str
        .ok_or_else(|| AppError::BadRequest("Missing 'distance' field".into()))?;
    let distance: f64 = dist_s
        .trim()
        .parse()
        .map_err(|_| AppError::BadRequest(format!("'distance' must be a number, got '{dist_s}'")))?;

    // Validate distance before payment gate
    if distance <= 0.0 || distance > 500_000.0 {
        return Err(AppError::BadRequest(
            "'distance' must be between 0 and 500,000 meters".into(),
        ));
    }

    let src_crs = source_crs.unwrap_or_else(|| "EPSG:4326".to_string());
    let geojson_str = validate_geojson_bytes(&input.bytes)?;
    let price = compute_price(input.size);

    let payment_result = payment_gate("buffer", input.size, price, &request_id, &headers, &state).await;
    match &payment_result {
        Ok(_) => metrics::record_payment("buffer", if state.config.dev_mode { "dev" } else { "success" }),
        Err(_) => metrics::record_payment("buffer", "failed"),
    }
    payment_result?;

    let _permit = GDAL_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Semaphore error: {e}")))?;

    let gdal_start = Instant::now();
    let result = timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
        do_buffer(geojson_str, distance, src_crs)
    }))
    .await
    .map_err(|_| AppError::Timeout)?
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Thread panic: {e}")))?
    .map_err(|e: AppError| e)?;
    metrics::record_gdal_duration("buffer", gdal_start.elapsed().as_secs_f64());

    metrics::record_request("buffer", "ok");
    metrics::record_request_duration("buffer", request_start.elapsed().as_secs_f64());

    Ok(Json(GeoJsonOutput {
        request_id,
        price_usd: price,
        result,
    }))
}

// ── Core blocking logic ────────────────────────────────────────────────────────

pub fn do_buffer_blocking(
    geojson_str: String,
    distance: f64,
    source_crs: String,
) -> Result<serde_json::Value, AppError> {
    do_buffer(geojson_str, distance, source_crs)
}

fn do_buffer(
    geojson_str: String,
    distance: f64,
    source_crs: String,
) -> Result<serde_json::Value, AppError> {
    use gdal::spatial_ref::{AxisMappingStrategy, CoordTransform, SpatialRef};
    use gdal::vector::Geometry;

    // Validate distance range (enforced here to cover both HTTP handler and batch paths)
    if distance <= 0.0 || distance > 500_000.0 {
        return Err(AppError::BadRequest(
            "'distance' must be between 0 and 500,000 meters".into(),
        ));
    }

    let fc: serde_json::Value = serde_json::from_str(&geojson_str)
        .map_err(|e| AppError::BadRequest(format!("Invalid JSON: {e}")))?;

    let features = extract_features(&fc)?;
    let mut out_features: Vec<serde_json::Value> = Vec::with_capacity(features.len());

    let mut wgs84_srs = SpatialRef::from_epsg(4326)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("WGS84 SRS init failed: {e}")))?;
    wgs84_srs.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);

    for feat in &features {
        let geom_val = match feat.get("geometry") {
            Some(g) if !g.is_null() => g,
            _ => {
                out_features.push(feat.clone());
                continue;
            }
        };

        let mut geom = Geometry::from_geojson(&geom_val.to_string())
            .map_err(|e| AppError::BadRequest(format!("Invalid geometry: {e}")))?;

        normalize_geom_to_wgs84(&mut geom, &source_crs)?;

        let env = geom.envelope();
        let lon = (env.MinX + env.MaxX) / 2.0;
        let lat = (env.MinY + env.MaxY) / 2.0;
        let utm_epsg = auto_utm_epsg(lon, lat);

        let mut utm_srs = SpatialRef::from_epsg(utm_epsg)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("UTM SRS EPSG:{utm_epsg} failed: {e}")))?;
        utm_srs.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);

        let to_utm = CoordTransform::new(&wgs84_srs, &utm_srs)
            .map_err(|e| AppError::BadRequest(format!("Cannot create WGS84→UTM transform: {e}")))?;
        let to_wgs84 = CoordTransform::new(&utm_srs, &wgs84_srs)
            .map_err(|e| AppError::BadRequest(format!("Cannot create UTM→WGS84 transform: {e}")))?;

        geom.transform_inplace(&to_utm)
            .map_err(|e| AppError::BadRequest(format!("WGS84→UTM reprojection failed: {e}")))?;

        let mut buffered = geom
            .buffer(distance, 30)
            .map_err(|e| AppError::BadRequest(format!("Buffer operation failed: {e}")))?;

        buffered
            .transform_inplace(&to_wgs84)
            .map_err(|e| AppError::BadRequest(format!("UTM→WGS84 reprojection failed: {e}")))?;

        let new_geom_json: serde_json::Value =
            serde_json::from_str(&buffered.json()
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
