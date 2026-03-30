//! POST /v1/raster-georeference — Georeference a JPEG using Ground Control Points (GCPs).
//! Produces a Cloud Optimized GeoTIFF via GDAL warp using gdalwarp CLI.

use axum::{extract::Extension, http::HeaderMap, Json};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use std::{fs::File, io::Write, process::Command};
use tempfile::TempDir;
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
pub struct GeorefParams {
    /// Path to the input JPEG image (on server filesystem)
    pub image_path: String,
    /// Ground Control Points
    pub gcps: Vec<GCPInput>,
}

#[derive(Deserialize, ToSchema, Clone)]
pub struct GCPInput {
    /// Pixel X coordinate (column)
    pub pixel_x: f64,
    /// Pixel Y coordinate (row)
    pub pixel_y: f64,
    /// Longitude (X in map units)
    pub lon: f64,
    /// Latitude (Y in map units)
    pub lat: f64,
}

#[derive(Serialize, ToSchema)]
pub struct GeorefResponse {
    pub request_id: String,
    pub cog_url: String,
    pub width: u32,
    pub height: u32,
    pub crs: String,
}

/// POST /v1/raster-georeference
///
/// Accepts a JPEG + ground control points and produces a Cloud Optimized GeoTIFF.
///
/// **Input (JSON):**
/// ```json
/// {
///   "image_path": "/tmp/page1.jpg",
///   "gcps": [
///     { "pixel_x": 100, "pixel_y": 200, "lon": -122.4194, "lat": 37.7749 }
///   ]
/// }
/// ```
///
/// **Output (JSON):**
/// ```json
/// {
///   "request_id": "...",
///   "cog_url": "/tmp/georef_abc123.tif",
///   "width": 800,
///   "height": 600,
///   "crs": "EPSG:4326"
/// }
/// ```
#[utoipa::path(
    post,
    path = "/v1/raster-georeference",
    tag = "GIS",
    request_body(content = GeorefParams, content_type = "application/json"),
    responses(
        (status = 200, description = "Georeferenced COG", body = GeorefResponse),
        (status = 400, description = "Bad request"),
        (status = 402, description = "Payment required", body = crate::billing::PaymentRequired),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn raster_georeference(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    Json(params): Json<GeorefParams>,
) -> Result<Json<GeorefResponse>, AppError> {
    if params.gcps.is_empty() {
        return Err(AppError::BadRequest("At least one GCP is required".into()));
    }
    if params.gcps.len() < 3 {
        return Err(AppError::BadRequest("At least 3 GCPs are required for georeferencing".into()));
    }

    let image_path = &params.image_path;
    if !image_path.starts_with("/tmp/") {
        return Err(AppError::BadRequest("image_path must be in /tmp/".into()));
    }

    let file_size = std::fs::metadata(image_path)
        .map(|m| m.len() as usize)
        .unwrap_or(0);

    let price = compute_price(file_size.max(1024));
    let t0 = Instant::now();
    metrics::record_request("raster-georeference", "received");

    // Payment gate — skip if MCP key
    payment_gate("raster-georeference", file_size, price, &request_id, &headers, &state).await?;

    // Build GCP list before spawning thread (clone for thread safety)
    let gcps = params.gcps.clone();
    let image_path_owned = image_path.to_string();
    let request_id_owned = request_id.to_string();

    // Run in blocking thread
    let result = tokio::task::spawn_blocking(move || {
        run_georef(&image_path_owned, &gcps, &request_id_owned)
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Thread panic: {e}")))?
    .map_err(|e| e)?;

    metrics::record_request("raster-georeference", "ok");
    metrics::record_request_duration("raster-georeference", t0.elapsed().as_secs_f64());

    Ok(Json(GeorefResponse {
        request_id,
        cog_url: result.cog_url,
        width: result.width,
        height: result.height,
        crs: "EPSG:4326".to_string(),
    }))
}

struct GeorefResult {
    cog_url: String,
    width: u32,
    height: u32,
}

fn run_georef(
    image_path: &str,
    gcps: &[GCPInput],
    request_id: &str,
) -> Result<GeorefResult, AppError> {
    let tmp = TempDir::new()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("TempDir: {e}")))?;

    let tmp_gtiff = tmp.path().join("georef_tmp.tif");
    let out_path = format!("/tmp/georef_{}_{}.tif", request_id.replace("-", "_"), std::process::id());

    // Build gdal_translate args with all GCPs (owned Strings to avoid temporaries)
    let mut translate_args: Vec<String> = vec!["-of".to_string(), "GTiff".to_string()];
    for g in gcps {
        translate_args.push("-gcp".to_string());
        translate_args.push(format!("{:.6}", g.pixel_x));
        translate_args.push(format!("{:.6}", g.pixel_y));
        translate_args.push(format!("{:.10}", g.lon));
        translate_args.push(format!("{:.10}", g.lat));
    }
    translate_args.push(image_path.to_string());
    translate_args.push(tmp_gtiff.to_str().unwrap().to_string());

    let translate_status = Command::new("gdal_translate")
        .args(&translate_args)
        .status()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("gdal_translate exec: {e}")))?;

    if !translate_status.success() {
        return Err(AppError::Internal(anyhow::anyhow!(
            "gdal_translate failed with status: {}", translate_status
        )));
    }

    if !tmp_gtiff.exists() {
        return Err(AppError::Internal(anyhow::anyhow!(
            "gdal_translate completed but temp output not created"
        )));
    }

    // Step 2: gdalwarp to create COG from the GCP-affixed TIFF
    let warp_status = Command::new("gdalwarp")
        .args([
            "-t_srs", "EPSG:4326",
            "-r", "bilinear",
            "-of", "GTiff",
            "-co", "TILED=YES",
            "-co", "COMPRESS=DEFLATE",
            "-co", "COPY_SRC_OVERVIEWS=YES",
            "--config", "GDAL_TIFF_INTERNAL_MASK", "YES",
        ])
        .arg(tmp_gtiff.as_os_str())
        .arg(&out_path)
        .status()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("gdalwarp exec: {e}")))?;

    if !warp_status.success() {
        return Err(AppError::Internal(anyhow::anyhow!(
            "gdalwarp failed with status: {}", warp_status
        )));
    }

    if !std::path::Path::new(&out_path).exists() {
        return Err(AppError::Internal(anyhow::anyhow!(
            "gdalwarp completed but output not created at {out_path}"
        )));
    }

    // Get dimensions using GDAL Rust
    let ds = gdal::Dataset::open(&out_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Open output COG: {e}")))?;
    let (width, height) = ds.raster_size();
    drop(ds);

    Ok(GeorefResult {
        cog_url: out_path,
        width: width as u32,
        height: height as u32,
    })
}
