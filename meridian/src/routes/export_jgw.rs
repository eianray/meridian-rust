//! POST /v1/export/jgw — Export a GeoTIFF as JPEG + ESRI world file (.jgw).
//! Pure math: converts lon/lat + scale/rotation to 6-parameter ESRI world file.

use axum::{extract::Extension, http::HeaderMap, Json};
use gdal::Dataset;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use utoipa::ToSchema;

use crate::{
    error::AppError,
    gis::compute_price,
    metrics,
    middleware::request_id::RequestId,
    AppState,
};
use crate::gis::reproject::payment_gate;

#[derive(Deserialize, ToSchema)]
pub struct ExportJgwParams {
    /// Path to the input GeoTIFF (on server filesystem)
    pub image_path: String,
    /// Placement data
    pub placement: Placement,
}

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
    pub jpeg_url: String,
    pub jgw_url: String,
}

/// POST /v1/export/jgw
///
/// Produces a JPEG + ESRI world file (.jgw) from a GeoTIFF and placement data.
///
/// **Input (JSON):**
/// ```json
/// {
///   "image_path": "/tmp/georef.tif",
///   "placement": {
///     "lon": -122.4194,
///     "lat": 37.7749,
///     "scale": 0.1,
///     "rotation": 0.0
///   }
/// }
/// ```
///
/// **Output (JSON):**
/// ```json
/// {
///   "request_id": "...",
///   "jpeg_url": "/tmp/output.jpg",
///   "jgw_url": "/tmp/output.jgw"
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
    request_body(content = ExportJgwParams, content_type = "application/json"),
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
    Json(params): Json<ExportJgwParams>,
) -> Result<Json<ExportJgwResponse>, AppError> {
    // Clone now so we can move params into the async block without borrow conflict
    let image_path = params.image_path.clone();
    let image_path_for_check = &image_path;
    if !image_path_for_check.starts_with("/tmp/") {
        return Err(AppError::BadRequest("image_path must be in /tmp/".into()));
    }

    let file_size = std::fs::metadata(&image_path)
        .map(|m| m.len() as usize)
        .unwrap_or(0);

    let price = compute_price(file_size.max(1024));
    let t0 = Instant::now();
    metrics::record_request("export-jgw", "received");

    // Clone request_id so we can move it into both payment_gate and the async block
    let request_id_for_gate = request_id.clone();

    // Payment gate — skip if MCP key
    payment_gate("export-jgw", file_size, price, &request_id_for_gate, &headers, &state).await?;

    // Clone again for spawn_blocking since request_id is consumed by the return value
    let request_id_for_block = request_id.clone();

    let result = tokio::task::spawn_blocking(move || {
        run_export_jgw(&image_path, &params.placement, &request_id_for_block)
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Thread panic: {e}")))?
    .map_err(|e| e)?;

    metrics::record_request("export-jgw", "ok");
    metrics::record_request_duration("export-jgw", t0.elapsed().as_secs_f64());

    Ok(Json(ExportJgwResponse {
        request_id,
        jpeg_url: result.jpeg_url,
        jgw_url: result.jgw_url,
    }))
}

struct ExportResult {
    jpeg_url: String,
    jgw_url: String,
}

fn run_export_jgw(
    image_path: &str,
    placement: &Placement,
    request_id: &str,
) -> Result<ExportResult, AppError> {
    // Open GeoTIFF to get dimensions
    let ds = Dataset::open(image_path)
        .map_err(|e| AppError::BadRequest(format!("Cannot open {image_path}: {e}")))?;

    let (width, height) = ds.raster_size();

    // Compute ESRI world file parameters
    // A = scale * cos(rot), D = -scale * sin(rot), B = scale * sin(rot), E = scale * cos(rot), C = lon, F = lat
    let rot_rad = placement.rotation.to_radians();
    let cos_r = rot_rad.cos();
    let sin_r = rot_rad.sin();
    let scale = placement.scale;

    let A = scale * cos_r;
    let D = -scale * sin_r;
    let B = scale * sin_r;
    let E = scale * cos_r;
    let C = placement.lon;
    let F = placement.lat;

    // Build base output name
    let base = format!("/tmp/export_{}_{}x{}", request_id, width, height);
    let jpeg_path = format!("{}.jpg", base);
    let jgw_path = format!("{}.jgw", base);

    // Read first band of GeoTIFF and save as JPEG
    // (This is a simple approach: read band as buffer, encode as JPEG)
    // For multi-band, we'd need to composite or select first band
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

    // Convert to image::GrayImage and encode as JPEG
    let img = image::GrayImage::from_raw(width as u32, height as u32, buf)
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("Create GrayImage failed")))?;

    let jpeg_file = std::fs::File::create(&jpeg_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Create JPEG: {e}")))?;
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(jpeg_file, 90);
    encoder
        .encode_image(&img)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("JPEG encode: {e}")))?;

    // Write .jgw world file
    let jgw_content = format!(
        "{:.15}\n{:.15}\n{:.15}\n{:.15}\n{:.15}\n{:.15}\n",
        A, D, B, E, C, F
    );
    std::fs::write(&jgw_path, jgw_content)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Write JGW: {e}")))?;

    Ok(ExportResult {
        jpeg_url: jpeg_path,
        jgw_url: jgw_path,
    })
}
