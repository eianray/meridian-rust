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
pub struct GCPInput {
    /// Pixel X coordinate (column)
    pub pixel_x: f64,
    /// Pixel Y coordinate (row)
    pub pixel_y: f64,
    /// X in map units (longitude)
    pub geo_x: f64,
    /// Y in map units (latitude)
    pub geo_y: f64,
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
/// - `file`: raw raster bytes (GeoTIFF or any GDAL-readable format)
/// - `gcps`: JSON string — array of `{"pixel_x": N, "pixel_y": N, "geo_x": N, "geo_y": N}`
/// - `output_crs`: optional string, default `"EPSG:4326"`
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
    let mut gcps_json: Option<String> = None;
    let mut output_crs: String = "EPSG:4326".to_string();

    // Parse multipart fields
    while let Some(field) = multipart.next_field().await.map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))? {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                image_bytes = Some(field.bytes().await.map_err(|e| AppError::BadRequest(format!("Failed to read file bytes: {e}")))?);
            }
            "gcps" => {
                gcps_json = Some(field.text().await.map_err(|e| AppError::BadRequest(format!("Failed to read gcps: {e}")))?);
            }
            "output_crs" => {
                output_crs = field.text().await.map_err(|e| AppError::BadRequest(format!("Failed to read output_crs: {e}")))?.trim().to_string();
                if output_crs.is_empty() {
                    output_crs = "EPSG:4326".to_string();
                }
            }
            _ => {}
        }
    }

    let image_bytes = image_bytes.ok_or_else(|| AppError::BadRequest("Missing required field: file".into()))?;
    let gcps_json = gcps_json.ok_or_else(|| AppError::BadRequest("Missing required field: gcps".into()))?;

    let gcps: Vec<GCPInput> = serde_json::from_str(&gcps_json)
        .map_err(|e| AppError::BadRequest(format!("Invalid gcps JSON: {e}")))?;

    let file_size = image_bytes.len();
    let price = compute_price(file_size.max(1024));
    let t0 = Instant::now();
    metrics::record_request("export-jgw", "received");

    // Payment gate — skip if MCP key
    payment_gate("export-jgw", file_size, price, &request_id, &headers, &state).await?;

    let uuid = Uuid::new_v4().to_string();
    let result = tokio::task::spawn_blocking(move || {
        run_export_jgw(&uuid, &image_bytes, &gcps, &output_crs)
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
    gcps: &[GCPInput],
    _output_crs: &str,
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

    // Compute ESRI world file parameters via least-squares affine fit from GCPs
    let n = gcps.len();
    if n < 3 {
        return Err(AppError::BadRequest(
            "At least 3 GCPs are required to compute an affine world file".into(),
        ));
    }

    let mean_px = gcps.iter().map(|g| g.pixel_x).sum::<f64>() / n as f64;
    let mean_py = gcps.iter().map(|g| g.pixel_y).sum::<f64>() / n as f64;
    let mean_gx = gcps.iter().map(|g| g.geo_x).sum::<f64>() / n as f64;
    let mean_gy = gcps.iter().map(|g| g.geo_y).sum::<f64>() / n as f64;

    // Solve 2x2 normal equations for each axis independently
    // geo_x = a*px + b*py + c  →  [Spxpx Spxpy; Spypx Spypy] * [a;b] = [Sgpxgx; Sgpxgy]
    let mut s_px_px = 0.0_f64;
    let mut s_px_py = 0.0_f64;
    let mut s_py_py = 0.0_f64;
    let mut s_gx_px = 0.0_f64;
    let mut s_gx_py = 0.0_f64;
    let mut s_gy_px = 0.0_f64;
    let mut s_gy_py = 0.0_f64;

    for g in gcps {
        let dx = g.pixel_x - mean_px;
        let dy = g.pixel_y - mean_py;
        s_px_px += dx * dx;
        s_px_py += dx * dy;
        s_py_py += dy * dy;
        s_gx_px += (g.geo_x - mean_gx) * dx;
        s_gx_py += (g.geo_x - mean_gx) * dy;
        s_gy_px += (g.geo_y - mean_gy) * dx;
        s_gy_py += (g.geo_y - mean_gy) * dy;
    }

    let det = s_px_px * s_py_py - s_px_py * s_px_py;
    if det.abs() < 1e-12 {
        return Err(AppError::BadRequest(
            "GCPs are collinear or insufficient — cannot compute affine transformation".into(),
        ));
    }

    // geo_x coefficients: a, b
    let a = (s_gx_px * s_py_py - s_gx_py * s_px_py) / det;
    let b = (s_gx_py * s_px_px - s_gx_px * s_px_py) / det;
    let c = mean_gx - a * mean_px - b * mean_py;

    // geo_y coefficients: d, e
    let d = (s_gy_px * s_py_py - s_gy_py * s_px_py) / det;
    let e = (s_gy_py * s_px_px - s_gy_px * s_px_py) / det;
    let f = mean_gy - d * mean_px - e * mean_py;

    // JGW world file params: A=a, D=d, B=b, E=e, C=c, F=f
    let (a_jgw, d_jgw, b_jgw, e_jgw, c_jgw, f_jgw) = (a, d, b, e, c, f);

    // Build world file content (one value per line, no trailing newline expected by ESRI)
    let jgw_content = format!(
        "{:.15}\n{:.15}\n{:.15}\n{:.15}\n{:.15}\n{:.15}\n",
        a_jgw, d_jgw, b_jgw, e_jgw, c_jgw, f_jgw
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
