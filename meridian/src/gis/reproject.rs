use axum::{extract::Extension, http::HeaderMap, Json};
use serde::Deserialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio::time::timeout;
use utoipa::ToSchema;

use crate::{
    billing::{build_payment_required, log_dev_operation, verify_payment, PaymentError},
    error::AppError,
    gis::{compute_price, normalize_crs_string, normalize_geom_to_wgs84, validate_geojson_bytes, GeoJsonInput, GeoJsonOutput},
    metrics,
    middleware::request_id::RequestId,
    AppState,
};

const OP_TIMEOUT: Duration = Duration::from_secs(30);

/// Global semaphore cap: at most 8 concurrent GDAL blocking threads.
pub(crate) static GDAL_SEMAPHORE: std::sync::LazyLock<Arc<Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(Semaphore::new(8)));

// ── Request schema ─────────────────────────────────────────────────────────────

/// Reproject request — multipart form:
/// - `file`: GeoJSON file (≤ 200 MB, .geojson or .json)
/// - `target_crs`: CRS string, e.g. "EPSG:4326", "EPSG:3857"
/// - `source_crs` (optional): source CRS string (default: "EPSG:4326")
#[derive(Deserialize, ToSchema)]
#[allow(dead_code)]
pub struct ReprojectParams {
    pub target_crs: String,
    pub source_crs: Option<String>,
}

// ── Handler ────────────────────────────────────────────────────────────────────

/// Reproject GeoJSON features to a target CRS.
///
/// Accepts multipart/form-data with:
/// - `file`: GeoJSON file (≤200 MB, .geojson/.json)
/// - `target_crs`: CRS string (e.g. "EPSG:4326", "EPSG:3857")
/// - `source_crs` (optional): source CRS string, default EPSG:4326
///
/// Returns reprojected GeoJSON FeatureCollection.
/// Per-operation timeout: 30s. Max 8 concurrent GDAL threads.
/// Requires x402/Base USDC payment via facilitator unless dev_mode is enabled.
#[utoipa::path(
    post,
    path = "/v1/reproject",
    tag = "GIS",
    request_body(
        content_type = "multipart/form-data",
        description = "Multipart form: `file` (GeoJSON), `target_crs` (string), optional `source_crs` (string)",
        content = ReprojectParams
    ),
    responses(
        (status = 200, description = "Reprojected GeoJSON FeatureCollection", body = GeoJsonOutput),
        (status = 400, description = "Bad request — missing file/crs, invalid JSON, or unsupported type"),
        (status = 402, description = "Payment required", body = crate::billing::PaymentRequired),
        (status = 413, description = "Payload too large (>200 MB)"),
        (status = 429, description = "Rate limit exceeded — 60 requests/min per IP"),
        (status = 500, description = "Internal server error"),
    )
)]
pub async fn reproject(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let mut file_input: Option<GeoJsonInput> = None;
    let mut target_crs: Option<String> = None;
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
            Some("target_crs") => {
                target_crs = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| AppError::BadRequest(format!("Error reading target_crs: {e}")))?,
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

    let input = file_input.ok_or_else(|| AppError::BadRequest("Missing 'file' field".into()))?;
    let crs = target_crs
        .ok_or_else(|| AppError::BadRequest("Missing 'target_crs' field".into()))?;

    if crs.trim().is_empty() {
        return Err(AppError::BadRequest("'target_crs' cannot be empty".into()));
    }
    let target_crs_normalized = normalize_crs_string(crs.trim())?;

    let request_start = Instant::now();
    metrics::record_request("reproject", "received");

    let src_crs = source_crs
        .map(|s| normalize_crs_string(&s))
        .transpose()?
        .unwrap_or_else(|| "EPSG:4326".to_string());
    let geojson_str = validate_geojson_bytes(&input.bytes)?;
    let price = compute_price(input.size);

    // ── Payment gate ──────────────────────────────────────────────────────────
    let payment_result = payment_gate(
        "reproject",
        input.size,
        price,
        &request_id,
        &headers,
        &state,
    ).await;
    match &payment_result {
        Ok(_) => metrics::record_payment("reproject", if state.config.dev_mode { "dev" } else { "success" }),
        Err(_) => metrics::record_payment("reproject", "failed"),
    }
    payment_result?;

    // ── GIS operation ─────────────────────────────────────────────────────────
    let _permit = GDAL_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Semaphore error: {e}")))?;

    let gdal_start = Instant::now();
    let result = timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
        do_reproject(geojson_str, src_crs, target_crs_normalized)
    }))
    .await
    .map_err(|_| AppError::Timeout)?
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Thread panic: {e}")))?
    .map_err(|e: AppError| e)?;
    metrics::record_gdal_duration("reproject", gdal_start.elapsed().as_secs_f64());

    metrics::record_request("reproject", "ok");
    metrics::record_request_duration("reproject", request_start.elapsed().as_secs_f64());

    Ok(Json(GeoJsonOutput {
        request_id,
        price_usd: price,
        result,
    }))
}

// ── Core blocking logic ────────────────────────────────────────────────────────

pub fn do_reproject_blocking(
    geojson_str: String,
    source_crs: String,
    target_crs: String,
) -> Result<serde_json::Value, AppError> {
    do_reproject(geojson_str, source_crs, target_crs)
}

fn do_reproject(
    geojson_str: String,
    source_crs: String,
    target_crs: String,
) -> Result<serde_json::Value, AppError> {
    use gdal::spatial_ref::{AxisMappingStrategy, CoordTransform, SpatialRef};
    use gdal::vector::Geometry;

    let fc: serde_json::Value = serde_json::from_str(&geojson_str)
        .map_err(|e| AppError::BadRequest(format!("Invalid JSON: {e}")))?;

    let features = extract_features(&fc)?;

    // Build WGS84 → target transform (we normalize to WGS84 first, then reproject)
    let mut wgs84_srs = SpatialRef::from_epsg(4326)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Failed to create WGS84 SRS: {e}")))?;
    wgs84_srs.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);

    let mut target_srs = SpatialRef::from_definition(&target_crs)
        .map_err(|e| AppError::BadRequest(format!("Invalid target_crs '{target_crs}': {e}")))?;
    target_srs.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);

    let wgs84_to_target = CoordTransform::new(&wgs84_srs, &target_srs)
        .map_err(|e| AppError::BadRequest(format!("Cannot create transform: {e}")))?;

    let mut out_features: Vec<serde_json::Value> = Vec::with_capacity(features.len());

    for feat in features {
        let geom_val = match feat.get("geometry") {
            Some(g) if !g.is_null() => g,
            _ => {
                out_features.push(feat.clone());
                continue;
            }
        };

        let geom_str = geom_val.to_string();
        let mut geom = Geometry::from_geojson(&geom_str)
            .map_err(|e| AppError::BadRequest(format!("Invalid geometry: {e}")))?;

        // Step 1: normalize input to WGS84 (no-op if already EPSG:4326)
        normalize_geom_to_wgs84(&mut geom, &source_crs)?;

        // Step 2: reproject WGS84 → target
        geom.transform_inplace(&wgs84_to_target)
            .map_err(|e| AppError::BadRequest(format!("Transform to target failed: {e}")))?;

        let new_geom_json: serde_json::Value = serde_json::from_str(&geom.json()
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Geometry serialization failed: {e}")))?)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Geometry JSON parse failed: {e}")))?;

        let mut new_feat = feat.clone();
        new_feat["geometry"] = new_geom_json;
        out_features.push(new_feat);
    }

    Ok(serde_json::json!({
        "type": "FeatureCollection",
        "features": out_features
    }))
}

// ── Utility ────────────────────────────────────────────────────────────────────

pub(crate) fn extract_features(
    fc: &serde_json::Value,
) -> Result<Vec<serde_json::Value>, AppError> {
    match fc.get("type").and_then(|t| t.as_str()) {
        Some("FeatureCollection") => {
            let feats = fc
                .get("features")
                .and_then(|f| f.as_array())
                .ok_or_else(|| AppError::BadRequest("Missing 'features' array".into()))?;
            Ok(feats.clone())
        }
        Some("Feature") => Ok(vec![fc.clone()]),
        Some(_t) => {
            // Bare geometry — wrap in a Feature
            Ok(vec![serde_json::json!({
                "type": "Feature",
                "properties": {},
                "geometry": fc
            })])
        }
        None => Err(AppError::BadRequest(
            "Input must be a GeoJSON FeatureCollection, Feature, or geometry".into(),
        )),
    }
}

// ── Payment gate helper (shared by all GIS handlers) ─────────────────────────

/// Check payment header and verify/log accordingly.
/// Returns Ok(()) to proceed, or Err(AppError) with 402/400/503.
pub(crate) async fn payment_gate(
    operation: &str,
    file_size_bytes: usize,
    price_usd: f64,
    request_id: &str,
    headers: &HeaderMap,
    state: &AppState,
) -> Result<(), AppError> {
    // ── X402_ENABLED gate ─────────────────────────────────────────────────────
    // When X402_ENABLED is not "true", skip all payment checks entirely.
    if std::env::var("X402_ENABLED").unwrap_or_default() != "true" {
        return Ok(());
    }

    let x_payment = headers
        .get("x-payment")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // ── MCP bypass ────────────────────────────────────────────────────────────
    // If a valid MCP API key is configured and the request presents a matching
    // `X-Mcp-Key` header, skip x402 verification entirely.
    if let Some(ref expected_key) = state.config.mcp_api_key {
        let provided_key = headers
            .get("x-mcp-key")
            .and_then(|v| v.to_str().ok());
        if provided_key == Some(expected_key.as_str()) {
            return Ok(());
        }
    }

    if state.config.dev_mode {
        // Dev mode: skip verification, log with status='dev' if DB available
        if let Some(db) = &state.db {
            log_dev_operation(request_id, operation, file_size_bytes, price_usd, db).await;
        }
        return Ok(());
    }

    // Production: wallet is configured
    let wallet = state
        .config
        .wallet_address
        .as_deref()
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!(
            "WALLET_ADDRESS not set - cannot accept payments in production mode"
        )))?;

    // Build a pseudo resource URL from operation name
    let resource_url = format!("https://api.meridian.tools/v1/{operation}");

    let payment_header = match x_payment {
        None => {
            // No payment header → 402
            let body = build_payment_required(operation, file_size_bytes, wallet, &resource_url);
            return Err(AppError::PaymentRequired { body });
        }
        Some(h) => h,
    };

    // Verify payment — requires DB
    let db = state.db.as_ref().ok_or_else(|| {
        AppError::Internal(anyhow::anyhow!(
            "DATABASE_URL not configured; cannot verify payments in production mode"
        ))
    })?;

    verify_payment(
        &payment_header,
        operation,
        &resource_url,
        wallet,
        &state.config.x402_facilitator_url,
        file_size_bytes,
        price_usd,
        request_id,
        db,
    )
    .await
    .map(|_payer| ())
    .map_err(|e| match e {
        PaymentError::AlreadyUsed => {
            AppError::BadRequest("Payment already used".into())
        }
        PaymentError::NotFound => {
            AppError::BadRequest("Transaction not found or not yet confirmed".into())
        }
        PaymentError::InsufficientAmount { expected, received } => AppError::BadRequest(
            format!("Insufficient payment: expected {expected} USDC atomic, received {received}"),
        ),
        PaymentError::InvalidPayload => AppError::BadRequest("Invalid X-PAYMENT payload".into()),
        PaymentError::FacilitatorError(msg) => AppError::Internal(anyhow::anyhow!("Facilitator error: {msg}")),
        PaymentError::DbError(msg) => AppError::Internal(anyhow::anyhow!("DB error: {msg}")),
    })
}
