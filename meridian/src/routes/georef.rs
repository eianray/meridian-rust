//! POST /v1/raster-georeference — Georeference a raster using Ground Control Points (GCPs).
//! Accepts a multipart form upload and produces a Cloud Optimized GeoTIFF via GDAL warp.

use axum::{extract::Extension, http::HeaderMap, response::Response, routing::post, Router};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use std::{fs::File, io::Write, process::Command};
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

const OP_TIMEOUT: Duration = Duration::from_secs(30);

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
pub struct GeorefResponse {
    pub request_id: String,
    pub cog_url: String,
    pub width: u32,
    pub height: u32,
    pub crs: String,
}

/// POST /v1/raster-georeference
///
/// Accepts a raster image + ground control points via multipart form and produces a
/// Cloud Optimized GeoTIFF. Caller uploads raw bytes; server never needs a pre-existing
/// file path.
///
/// **Input (multipart/form-data):**
/// - `file`: raw TIFF/raster bytes
/// - `gcps`: JSON string — array of `{"pixel_x": N, "pixel_y": N, "geo_x": N, "geo_y": N}`
/// - `output_crs`: optional string, default `"EPSG:4326"`
///
/// **Output:** raw georeferenced TIFF as `application/octet-stream`
/// with `Content-Disposition: attachment; filename="georef_<request_id>.tif"`
#[utoipa::path(
    post,
    path = "/v1/raster-georeference",
    tag = "GIS",
    request_body(content = GeorefResponse, content_type = "multipart/form-data"),
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
    mut multipart: axum::extract::Multipart,
) -> Result<Response, AppError> {
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

    const MAX_IMAGE_BYTES: usize = 50 * 1024 * 1024; // 50 MB
    if image_bytes.len() > MAX_IMAGE_BYTES {
        return Err(AppError::BadRequest("Image too large (max 50 MB)".into()));
    }

    let gcps_json = gcps_json.ok_or_else(|| AppError::BadRequest("Missing required field: gcps".into()))?;

    let gcps: Vec<GCPInput> = serde_json::from_str(&gcps_json)
        .map_err(|e| AppError::BadRequest(format!("Invalid gcps JSON: {e}")))?;

    if gcps.is_empty() {
        return Err(AppError::BadRequest("At least one GCP is required".into()));
    }
    if gcps.len() < 3 {
        return Err(AppError::BadRequest("At least 3 GCPs are required for georeferencing".into()));
    }

    let file_size = image_bytes.len();
    let price = compute_price(file_size.max(1024));
    let t0 = Instant::now();
    metrics::record_request("raster-georeference", "received");

    // Payment gate — skip if MCP key
    payment_gate("raster-georeference", file_size, price, &request_id, &headers, &state).await?;

    let uuid = Uuid::new_v4().to_string();
    let request_id_clone = request_id.clone();
    let result = timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
        run_georef(&uuid, &image_bytes, &gcps, &request_id_clone, &output_crs)
    }))
    .await
    .map_err(|_| AppError::Timeout)?
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Thread panic: {e}")))?
    .map_err(|e| e)?;

    metrics::record_request("raster-georeference", "ok");
    metrics::record_request_duration("raster-georeference", t0.elapsed().as_secs_f64());

    // Read output and return as binary
    let output_bytes = tokio::fs::read(&result.cog_url).await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Read output file: {e}")))?;

    // Clean up output file
    let _ = tokio::fs::remove_file(&result.cog_url).await;

    let filename = format!("georef_{}.tif", request_id.replace("-", "_"));
    let mut response = Response::new(output_bytes.into());
    response.headers_mut().insert(axum::http::header::CONTENT_TYPE, "application/octet-stream".parse().unwrap());
    response.headers_mut().insert(
        axum::http::header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{}\"", filename).parse().unwrap(),
    );

    Ok(response)
}

struct GeorefResult {
    cog_url: String,
}

fn run_georef(
    uuid: &str,
    image_bytes: &[u8],
    gcps: &[GCPInput],
    request_id: &str,
    output_crs: &str,
) -> Result<GeorefResult, AppError> {
    let input_path = format!("/tmp/{}_input.tif", uuid);

    // Write uploaded bytes to temp input file
    let mut input_file = File::create(&input_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Create input file: {e}")))?;
    input_file.write_all(image_bytes)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Write input file: {e}")))?;
    drop(input_file);

    let tmp = match tempfile::TempDir::new() {
        Ok(t) => t,
        Err(e) => {
            let _ = std::fs::remove_file(&input_path);
            return Err(AppError::Internal(anyhow::anyhow!("TempDir: {e}")));
        }
    };

    let tmp_gtiff = tmp.path().join("georef_tmp.tif");
    let out_path = tmp.path().join("georef_out.tif");

    // Build gdal_translate args with all GCPs
    let mut translate_args: Vec<String> = vec!["-of".to_string(), "GTiff".to_string()];
    for g in gcps {
        translate_args.push("-gcp".to_string());
        translate_args.push(format!("{:.6}", g.pixel_x));
        translate_args.push(format!("{:.6}", g.pixel_y));
        translate_args.push(format!("{:.10}", g.geo_x));
        translate_args.push(format!("{:.10}", g.geo_y));
    }
    translate_args.push(input_path.clone());
    translate_args.push(tmp_gtiff.to_str().unwrap().to_string());

    let translate_status = Command::new("gdal_translate")
        .args(&translate_args)
        .status()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("gdal_translate exec: {e}")))?;

    if !translate_status.success() {
        // Clean up input — out_path is inside tmp and auto-cleaned
        let _ = std::fs::remove_file(&input_path);
        return Err(AppError::Internal(anyhow::anyhow!(
            "gdal_translate failed with status: {}", translate_status
        )));
    }

    if !tmp_gtiff.exists() {
        // Clean up input — out_path is inside tmp and auto-cleaned
        let _ = std::fs::remove_file(&input_path);
        return Err(AppError::Internal(anyhow::anyhow!(
            "gdal_translate completed but temp output not created"
        )));
    }

    // Step 2: gdalwarp to create COG from the GCP-affixed TIFF
    // Allow only alphanumeric, colon, plus, hyphen (covers EPSG:XXXX, +proj=..., etc.)
    if !output_crs.is_empty() && !output_crs.chars().all(|c| c.is_alphanumeric() || ":/+=_-. ".contains(c)) {
        return Err(AppError::BadRequest("Invalid output_crs value".into()));
    }
    let t_srs = if output_crs.is_empty() || output_crs == "EPSG:4326" {
        "EPSG:4326"
    } else {
        output_crs
    };

    let warp_status = Command::new("gdalwarp")
        .args([
            "-t_srs", t_srs,
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

    // Always clean up input
    let _ = std::fs::remove_file(&input_path);

    if !warp_status.success() {
        return Err(AppError::Internal(anyhow::anyhow!(
            "gdalwarp failed with status: {}", warp_status
        )));
    }

    if !out_path.exists() {
        return Err(AppError::Internal(anyhow::anyhow!(
            "gdalwarp completed but output not created at {:?}", out_path
        )));
    }

    Ok(GeorefResult {
        cog_url: out_path.to_string_lossy().to_string(),
    })
}

/// Registers the georef routes.
pub fn routes() -> Router {
    Router::new().route("/v1/raster-georeference", post(raster_georeference))
}
