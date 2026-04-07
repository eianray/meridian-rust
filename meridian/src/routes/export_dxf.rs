//! POST /v1/export/dxf — Export GeoJSON features as a DXF drawing (.dxf).
//! Accepts a multipart form upload with a GeoJSON FeatureCollection.

use axum::{extract::Extension, http::HeaderMap, routing::post, Json, Router};
use serde::Serialize;
use std::time::{Duration, Instant};
use tokio::time::timeout;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    error::AppError,
    gis::compute_price,
    metrics,
    middleware::request_id::RequestId,
    AppState,
};
use crate::gis::reproject::payment_gate;

const OP_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(ToSchema)]
pub struct ExportDxfParams {
    /// GeoJSON FeatureCollection as a string (multipart field name: "geojson")
    pub geojson: String,
    /// Optional source CRS, default EPSG:4326
    pub input_crs: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ExportDxfResponse {
    pub request_id: String,
    pub price_usd: f64,
    /// DXF file bytes as base64
    #[schema(value_type = String)]
    pub dxf_base64: String,
    pub filename: String,
}

#[utoipa::path(
    post,
    path = "/v1/export/dxf",
    tag = "GIS",
    request_body(
        content_type = "multipart/form-data",
        description = "Multipart form: `geojson` (GeoJSON string), optional `input_crs` (default EPSG:4326)",
        content = ExportDxfParams
    ),
    responses(
        (status = 200, description = "DXF file output", body = ExportDxfResponse),
        (status = 400, description = "Bad request"),
        (status = 402, description = "Payment required", body = crate::billing::PaymentRequired),
        (status = 413, description = "Payload too large (>200 MB)"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn export_dxf(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<ExportDxfResponse>, AppError> {
    let mut geojson_text: Option<String> = None;
    let mut input_crs: Option<String> = None;

    while let Some(field) = multipart.next_field().await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        match field.name() {
            Some("geojson") => {
                geojson_text = Some(field.text().await
                    .map_err(|e| AppError::BadRequest(format!("geojson field read error: {e}")))?);
            }
            Some("input_crs") => {
                let v = field.text().await
                    .map_err(|e| AppError::BadRequest(format!("input_crs: {e}")))?;
                if !v.trim().is_empty() {
                    input_crs = Some(v.trim().to_string());
                }
            }
            _ => {}
        }
    }

    let geojson_text = geojson_text.ok_or_else(|| AppError::BadRequest("Missing required field: geojson".into()))?;
    let input_crs = input_crs.unwrap_or_else(|| "EPSG:4326".to_string());

    let geojson_bytes: Vec<u8> = geojson_text.into_bytes();
    let file_size = geojson_bytes.len();

    let price = compute_price(file_size);
    let t0 = Instant::now();
    metrics::record_request("export-dxf", "received");

    payment_gate("export-dxf", file_size, price, &request_id, &headers, &state).await?;

    let _permit = crate::gis::reproject::GDAL_SEMAPHORE.acquire().await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Semaphore: {e}")))?;

    let uuid = Uuid::new_v4().to_string();
    let result = timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
        run_export_dxf(&uuid, &geojson_bytes, &input_crs)
    }))
    .await
    .map_err(|_| AppError::Timeout)?
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Thread panic: {e}")))?
    .map_err(|e| e)?;

    metrics::record_request("export-dxf", "ok");
    metrics::record_request_duration("export-dxf", t0.elapsed().as_secs_f64());

    let encoded = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &result,
    );

    let response_body = ExportDxfResponse {
        request_id,
        price_usd: price,
        dxf_base64: encoded,
        filename: "output.dxf".to_string(),
    };

    Ok(Json(response_body))
}

fn run_export_dxf(uuid: &str, geojson_bytes: &[u8], _input_crs: &str) -> Result<Vec<u8>, AppError> {
    use gdal::DriverManager;
    use gdal::vector::{LayerOptions, LayerAccess};

    let tmp_dir = tempfile::TempDir::new()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("TempDir: {e}")))?;

    // Write input GeoJSON to a temp file
    let input_path = tmp_dir.path().join(format!("{}_input.geojson", uuid));
    std::fs::write(&input_path, geojson_bytes)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Write GeoJSON: {e}")))?;

    // Open GeoJSON as GDAL dataset
    let src_ds = gdal::Dataset::open(&input_path)
        .map_err(|e| AppError::BadRequest(format!("Cannot open GeoJSON: {e}")))?;

    let src_layer = src_ds.layer(0)
        .map_err(|e| AppError::BadRequest(format!("No layers in GeoJSON: {e}")))?;

    let defn = src_layer.defn();
    let srs = src_layer.spatial_ref();
    let geom_type = defn.geometry_type();

    // Use DXF driver
    let driver = DriverManager::get_driver_by_name("DXF")
        .map_err(|e| AppError::BadRequest(format!("DXF driver not available: {e}")))?;

    let out_path = tmp_dir.path().join(format!("{}_output.dxf", uuid));
    let mut dst_ds = driver
        .create_vector_only(out_path.to_str().unwrap_or("output.dxf"))
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Create DXF dataset: {e}")))?;

    let layer_opts = LayerOptions {
        name: "entities",
        srs: srs.as_ref(),
        ty: geom_type,
        options: None,
    };
    let dst_layer = dst_ds.create_layer(layer_opts)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Create DXF layer: {e}")))?;

    // Copy field definitions
    for field in defn.fields() {
        let field_defn = gdal::vector::FieldDefn::new(field.name().as_str(), field.field_type())
            .map_err(|e| AppError::Internal(anyhow::anyhow!("FieldDefn: {e}")))?;
        field_defn.add_to_layer(&dst_layer)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Add field: {e}")))?;
    }

    // Copy features
    let mut src_layer2 = src_ds.layer(0)
        .map_err(|e| AppError::BadRequest(format!("Re-open source layer: {e}")))?;
    for feature in src_layer2.features() {
        feature.create(&dst_layer)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Create feature: {e}")))?;
    }

    drop(dst_ds);
    drop(src_ds);

    let dxf_bytes = std::fs::read(&out_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Read DXF output: {e}")))?;

    Ok(dxf_bytes)
}

/// Registers the /v1/export/dxf route.
pub fn routes() -> Router {
    Router::new().route("/v1/export/dxf", post(export_dxf))
}
