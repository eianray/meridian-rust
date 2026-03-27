use axum::{extract::Extension, http::HeaderMap, Json};
use serde::Deserialize;
use std::collections::HashMap;
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

/// Dissolve request — multipart form:
/// - `file`: GeoJSON file (≤200 MB)
/// - `field` (optional): attribute field name to group by
/// - `source_crs` (optional): source CRS, default EPSG:4326
#[derive(Deserialize, ToSchema)]
#[allow(dead_code)]
pub struct DissolveParams {
    /// GeoJSON file (≤200 MB, .geojson/.json)
    pub file: String,
    /// Attribute field to group by before dissolving (optional)
    pub field: Option<String>,
    /// Source CRS (default EPSG:4326)
    pub source_crs: Option<String>,
}

const OP_TIMEOUT: Duration = Duration::from_secs(120);

/// Dissolve GeoJSON features, optionally grouping by an attribute field.
///
/// Accepts multipart/form-data with:
/// - `file`: GeoJSON file (≤200 MB, .geojson/.json)
/// - `field` (optional): attribute field name to group by before dissolving
/// - `source_crs` (optional): source CRS of input geometries, default EPSG:4326
///
/// Requires x402/Base USDC payment via facilitator unless dev_mode is enabled.
#[utoipa::path(
    post,
    path = "/v1/dissolve",
    tag = "GIS",
    request_body(
        content_type = "multipart/form-data",
        description = "Multipart form: `file` (GeoJSON), optional `field` (group attribute), optional `source_crs`",
        content = DissolveParams
    ),
    responses(
        (status = 200, description = "Dissolved GeoJSON FeatureCollection", body = GeoJsonOutput),
        (status = 400, description = "Bad request — missing file, invalid JSON, GEOS unavailable"),
        (status = 402, description = "Payment required", body = crate::billing::PaymentRequired),
        (status = 413, description = "Payload too large (>200 MB)"),
        (status = 429, description = "Rate limit exceeded — 60 requests/min per IP"),
        (status = 500, description = "Internal server error"),
    )
)]
pub async fn dissolve(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let mut file_input: Option<GeoJsonInput> = None;
    let mut field_name: Option<String> = None;
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
            Some("field") => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("Error reading field param: {e}")))?;
                if !v.trim().is_empty() {
                    field_name = Some(v.trim().to_string());
                }
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
    metrics::record_request("dissolve", "received");

    let input = file_input.ok_or_else(|| AppError::BadRequest("Missing 'file' field".into()))?;
    let price = compute_price(input.size);
    let src_crs = source_crs.unwrap_or_else(|| "EPSG:4326".to_string());
    let geojson_str = validate_geojson_bytes(&input.bytes)?;

    let payment_result = payment_gate("dissolve", input.size, price, &request_id, &headers, &state).await;
    match &payment_result {
        Ok(_) => metrics::record_payment("dissolve", if state.config.dev_mode { "dev" } else { "success" }),
        Err(_) => metrics::record_payment("dissolve", "failed"),
    }
    payment_result?;

    let _permit = GDAL_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Semaphore error: {e}")))?;

    let gdal_start = Instant::now();
    let result = timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
        do_dissolve(geojson_str, field_name, src_crs)
    }))
    .await
    .map_err(|_| AppError::Timeout)?
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Thread panic: {e}")))?
    .map_err(|e: AppError| e)?;
    metrics::record_gdal_duration("dissolve", gdal_start.elapsed().as_secs_f64());

    metrics::record_request("dissolve", "ok");
    metrics::record_request_duration("dissolve", request_start.elapsed().as_secs_f64());

    Ok(Json(GeoJsonOutput {
        request_id,
        price_usd: price,
        result,
    }))
}

// ── Core blocking logic ────────────────────────────────────────────────────────

pub fn do_dissolve_blocking(
    geojson_str: String,
    field_name: Option<String>,
    source_crs: String,
) -> Result<serde_json::Value, AppError> {
    do_dissolve(geojson_str, field_name, source_crs)
}

fn do_dissolve(
    geojson_str: String,
    field_name: Option<String>,
    source_crs: String,
) -> Result<serde_json::Value, AppError> {
    use gdal::vector::Geometry;

    let fc: serde_json::Value = serde_json::from_str(&geojson_str)
        .map_err(|e| AppError::BadRequest(format!("Invalid JSON: {e}")))?;

    let features = extract_features(&fc)?;

    let mut groups: HashMap<String, (Vec<serde_json::Value>, Option<serde_json::Value>)> =
        HashMap::new();

    for feat in &features {
        let key = match &field_name {
            Some(f) => {
                let props = feat.get("properties").and_then(|p| p.as_object());
                match props.and_then(|p| p.get(f)) {
                    Some(v) => value_to_key(v),
                    None => "_null_".to_string(),
                }
            }
            None => "_all_".to_string(),
        };

        let entry = groups.entry(key.clone()).or_insert_with(|| (vec![], None));

        if entry.1.is_none() {
            let mut props = feat
                .get("properties")
                .cloned()
                .unwrap_or(serde_json::json!({}));
            if let Some(ref f) = field_name {
                let val = props
                    .as_object()
                    .and_then(|p| p.get(f))
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                props = serde_json::json!({ f: val });
            }
            entry.1 = Some(props);
        }

        if let Some(g) = feat.get("geometry") {
            if !g.is_null() {
                entry.0.push(g.clone());
            }
        }
    }

    let mut out_features: Vec<serde_json::Value> = Vec::with_capacity(groups.len());

    for (_, (geom_vals, props)) in groups {
        if geom_vals.is_empty() {
            continue;
        }

        // Build a MultiPolygon then use OGR_G_UnionCascaded for efficient
        // spatial-tree union — OGR_G_UnionCascaded requires MultiPolygon.
        // We use OGR C API directly to clone each sub-geometry into the collection
        // to avoid gdal-rs ownership tracking issues.
        let multi_raw = unsafe {
            gdal_sys::OGR_G_CreateGeometry(gdal_sys::OGRwkbGeometryType::wkbMultiPolygon)
        };
        if multi_raw.is_null() {
            return Err(AppError::Internal(anyhow::anyhow!("Failed to create MultiPolygon")));
        }

        for gv in &geom_vals {
            let mut geom = Geometry::from_geojson(&gv.to_string())
                .map_err(|e| AppError::BadRequest(format!("Invalid geometry: {e}")))?;
            normalize_geom_to_wgs84(&mut geom, &source_crs)?;

            let geom_type = unsafe { gdal_sys::OGR_G_GetGeometryType(geom.c_geometry()) };
            if geom_type == gdal_sys::OGRwkbGeometryType::wkbMultiPolygon
                || geom_type == gdal_sys::OGRwkbGeometryType::wkbMultiPolygon25D
            {
                // Flatten sub-polygons
                let count = unsafe { gdal_sys::OGR_G_GetGeometryCount(geom.c_geometry()) };
                for i in 0..count {
                    let sub = unsafe { gdal_sys::OGR_G_GetGeometryRef(geom.c_geometry(), i) };
                    if !sub.is_null() {
                        let cloned = unsafe { gdal_sys::OGR_G_Clone(sub) };
                        if !cloned.is_null() {
                            unsafe { gdal_sys::OGR_G_AddGeometryDirectly(multi_raw, cloned) };
                        }
                    }
                }
            } else {
                // Single polygon — clone and add directly
                let cloned = unsafe { gdal_sys::OGR_G_Clone(geom.c_geometry()) };
                if !cloned.is_null() {
                    unsafe { gdal_sys::OGR_G_AddGeometryDirectly(multi_raw, cloned) };
                }
            }
        }

        // Call OGR_G_UnionCascaded — returns a new geometry (caller owns it)
        let unioned_raw = unsafe { gdal_sys::OGR_G_UnionCascaded(multi_raw) };
        unsafe { gdal_sys::OGR_G_DestroyGeometry(multi_raw) };
        if unioned_raw.is_null() {
            return Err(AppError::BadRequest(
                "UnionCascaded failed — GEOS is required for dissolve operations".into(),
            ));
        }

        // Export to GeoJSON via C API, then free the raw geometry
        let geojson_cstr = unsafe {
            let ptr = gdal_sys::OGR_G_ExportToJson(unioned_raw);
            gdal_sys::OGR_G_DestroyGeometry(unioned_raw);
            if ptr.is_null() {
                return Err(AppError::Internal(anyhow::anyhow!("OGR_G_ExportToJson returned null")));
            }
            std::ffi::CStr::from_ptr(ptr)
                .to_string_lossy()
                .into_owned()
        };

        let new_geom_json: serde_json::Value = serde_json::from_str(&geojson_cstr)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Geometry JSON parse: {e}")))?;

        out_features.push(serde_json::json!({
            "type": "Feature",
            "properties": props.unwrap_or(serde_json::json!({})),
            "geometry": new_geom_json
        }));
    }

    Ok(serde_json::json!({
        "type": "FeatureCollection",
        "features": out_features
    }))
}

fn value_to_key(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "_null_".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        _ => v.to_string(),
    }
}
