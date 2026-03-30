//! POST /v1/export/jgw — Export a raster as JPEG + ESRI world file (.jgw).
//! Accepts a multipart form upload. Pure math: converts lon/lat + scale/rotation
//! to 6-parameter ESRI world file.

use axum::{extract::Extension, http::HeaderMap, routing::post, Json, Router};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::time::Instant;
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

#[derive(Deserialize, ToSchema)]
pub struct Placement {
    /// Center longitude
    pub lon: f64,
    /// Center latitude
    pub lat: f64,
    /// Pixel scale (degrees per pixel for lat/lon)
    pub scale: f64,
    /// Rotation in degrees
    pub rotation: f64,
}

#[derive(Serialize, ToSchema)]
pub struct ExportJgwResponse {
    pub request_id: String,
    /// JPEG bytes as base64
    pub jpeg_base64: String,
    /// JGW world file content (6 lines, one value per line)
    pub jgw_content: String,
}

/// POST /v1/export/jgw
///
/// Accepts a raster image via multipart form and produces a JPEG + ESRI world file (.jgw).
/// Caller uploads raw bytes; server never needs a pre-existing file path.
///
/// **Input (multipart/form-data):**
/// - `image`: raw raster bytes (GeoTIFF or any GDAL-readable format)
/// - `placement`: JSON string — `{"lon": N, "lat": N, "scale": N, "rotation": N}`
///
/// **Output (JSON):**
/// ```json
/// {
///   "request_id": "...",
///   "jpeg_base64": "<base64-encoded JPEG>",
///   "jgw_content": "A\nD\nB\nE\nC\nF\n"
/// }
/// ```
///
/// ESRI world file formula (standard 6-parameter):
/// - A = scale * cos(rotation)
/// - D = -scale * sin(rotation)
/// - B = scale * sin(rotation)
/// - E = scale * cos(rotation)
/// - C = lon
/// - F = lat
#[utoipa::path(
    post,
    path = "/v1/export/jgw",
    tag = "Utility",
    request_body(content = ExportJgwResponse, content_type = "multipart/form-data"),
    responses(
        (status = 200, description = "JPEG + world file output", body = ExportJgwResponse),
        (status = 400, description = "Bad request"),
        (status = 402, description = "Payment required", body = crate::billing::PaymentRequired),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn export_jgw(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<ExportJgwResponse>, AppError> {
    let mut image_bytes: Option<Bytes> = None;
    let mut placement_json: Option<String> = None;

    // Parse multipart fields
    while let Some(field) = multipart.next_field().await.map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))? {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "image" => {
                image_bytes = Some(field.bytes().await.map_err(|e| AppError::BadRequest(format!("Failed to read image bytes: {e}")))?);
            }
            "placement" => {
                placement_json = Some(field.text().await.map_err(|e| AppError::BadRequest(format!("Failed to read placement: {e}")))?);
            }
            _ => {}
        }
    }

    let image_bytes = image_bytes.ok_or_else(|| AppError::BadRequest("Missing required field: image".into()))?;
    let placement_json = placement_json.ok_or_else(|| AppError::BadRequest("Missing required field: placement".into()))?;

    let placement: Placement = serde_json::from_str(&placement_json)
        .map_err(|e| AppError::BadRequest(format!("Invalid placement JSON: {e}")))?;

    let file_size = image_bytes.len();
    let price = compute_price(file_size.max(1024));
    let t0 = Instant::now();
    metrics::record_request("export-jgw", "received");

    // Payment gate — skip if MCP key
    payment_gate("export-jgw", file_size, price, &request_id, &headers, &state).await?;

    let uuid = Uuid::new_v4().to_string();
    let result = tokio::task::spawn_blocking(move || {
        run_export_jgw(&uuid, &image_bytes, &placement)
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Thread panic: {e}")))?
    .map_err(|e| e)?;

    metrics::record_request("export-jgw", "ok");
    metrics::record_request_duration("export-jgw", t0.elapsed().as_secs_f64());

    Ok(Json(ExportJgwResponse {
        request_id,
        jpeg_base64: result.jpeg_base64,
        jgw_content: result.jgw_content,
    }))
}

struct ExportResult {
    jpeg_base64: String,
    jgw_content: String,
}

fn run_export_jgw(
    uuid: &str,
    image_bytes: &[u8],
    placement: &Placement,
) -> Result<ExportResult, AppError> {
    let input_path = format!("/tmp/{}_input.tif", uuid);

    // Write uploaded bytes to temp input file
    let mut input_file = std::fs::File::create(&input_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Create input file: {e}")))?;
    std::io::Write::write_all(&mut input_file, image_bytes)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Write input file: {e}")))?;
    drop(input_file);

    // Open GeoTIFF to get dimensions
    let ds = gdal::Dataset::open(&input_path)
        .map_err(|e| AppError::BadRequest(format!("Cannot open input raster: {e}")))?;

    let (width, height) = ds.raster_size();

    // Compute ESRI world file parameters
    // A = scale * cos(rot), D = -scale * sin(rot), B = scale * sin(rot), E = scale * cos(rot), C = lon, F = lat
    let rot_rad = placement.rotation.to_radians();
    let cos_r = rot_rad.cos();
    let sin_r = rot_rad.sin();
    let scale = placement.scale;

    let a = scale * cos_r;
    let d = -scale * sin_r;
    let b = scale * sin_r;
    let e = scale * cos_r;
    let c = placement.lon;
    let f = placement.lat;

    // Build world file content (one value per line, no trailing newline expected by ESRI)
    let jgw_content = format!(
        "{:.15}\n{:.15}\n{:.15}\n{:.15}\n{:.15}\n{:.15}\n",
        a, d, b, e, c, f
    );

    // Read first band of GeoTIFF and encode as JPEG
    let band = ds.rasterband(1)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Get band 1: {e}")))?;

    let mut buf: Vec<u8> = vec![0u8; width * height];

    band.read_into_slice::<u8>(
        (0, 0),
        (width, height),
        (width, height),
        &mut buf,
        None,
    )
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Read band: {e}")))?;
    drop(ds);

    // Convert to image::GrayImage and encode as JPEG
    let img = image::GrayImage::from_raw(width as u32, height as u32, buf)
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("Create GrayImage failed")))?;

    let mut jpeg_bytes: Vec<u8> = Vec::new();
    {
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut jpeg_bytes), 90);
        encoder
            .encode_image(&img)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("JPEG encode: {e}")))?;
    }

    // Clean up input file
    let _ = std::fs::remove_file(&input_path);

    let jpeg_base64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &jpeg_bytes);

    Ok(ExportResult {
        jpeg_base64,
        jgw_content,
    })
}

/// Registers the export/jgw routes.
pub fn routes() -> Router {
    Router::new().route("/v1/export/jgw", post(export_jgw))
}
