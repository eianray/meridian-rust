pub mod buffer;
pub mod clip;
pub mod combine;
pub mod convert;
pub mod dissolve;
pub mod raster;
pub mod reproject;
pub mod schema;
pub mod topology;
pub mod transform;
pub mod vectorize;

use serde::Serialize;
use utoipa::ToSchema;

use crate::error::AppError;

/// Maximum accepted file size: 200 MB
pub const MAX_FILE_BYTES: usize = 200 * 1024 * 1024;

/// Allowed GeoJSON/JSON extensions (lower-cased)
const ALLOWED_EXTS: &[&str] = &["geojson", "json"];

// ── Pricing ───────────────────────────────────────────────────────────────────

/// Compute price in USD based on file size.
/// Rate: $0.01/MB, min $0.01, cap $2.00.
pub fn compute_price(bytes: usize) -> f64 {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    let raw = mb * 0.01_f64;
    raw.clamp(0.01_f64, 2.00_f64)
}

// ── Input / Output types ──────────────────────────────────────────────────────

/// Raw bytes extracted from a multipart file field.
pub struct GeoJsonInput {
    pub bytes: Vec<u8>,
    /// Original file size (used for pricing)
    pub size: usize,
}

impl GeoJsonInput {
    /// Parse a single named field from a multipart form.
    /// Validates extension and size before reading the body.
    pub async fn from_multipart_field(
        field: &mut axum::extract::multipart::Field<'_>,
    ) -> Result<Self, AppError> {
        // Validate extension from filename, if present
        if let Some(filename) = field.file_name() {
            let ext = filename
                .rsplit('.')
                .next()
                .unwrap_or("")
                .to_lowercase();
            if !ALLOWED_EXTS.contains(&ext.as_str()) {
                return Err(AppError::BadRequest(format!(
                    "Unsupported file type '.{ext}'. Accepted: .geojson, .json"
                )));
            }
        }

        // Stream and accumulate bytes with size guard
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = field.chunk().await.map_err(|e| {
            AppError::BadRequest(format!("Error reading upload: {e}"))
        })? {
            if buf.len() + chunk.len() > MAX_FILE_BYTES {
                return Err(AppError::PayloadTooLarge);
            }
            buf.extend_from_slice(&chunk);
        }

        if buf.is_empty() {
            return Err(AppError::BadRequest("Empty file".into()));
        }

        let size = buf.len();
        Ok(GeoJsonInput { bytes: buf, size })
    }
}

/// Validate that raw bytes are valid UTF-8 JSON (light check only).
pub fn validate_geojson_bytes(bytes: &[u8]) -> Result<String, AppError> {
    let s = std::str::from_utf8(bytes)
        .map_err(|_| AppError::BadRequest("File is not valid UTF-8".into()))?;
    // Cheap structural check — full parse happens inside GDAL
    if !s.trim_start().starts_with('{') && !s.trim_start().starts_with('[') {
        return Err(AppError::BadRequest(
            "File does not appear to be JSON".into(),
        ));
    }
    Ok(s.to_string())
}

/// Standard GIS endpoint response
#[derive(Serialize, ToSchema)]
pub struct GeoJsonOutput {
    pub request_id: String,
    pub price_usd: f64,
    pub result: serde_json::Value,
}

// ── GIS helpers ───────────────────────────────────────────────────────────────

/// Reject CRS strings that look like file paths or contain null bytes.
pub fn validate_crs_string(crs: &str) -> Result<(), AppError> {
    let s = crs.trim();
    if s.is_empty() {
        return Err(AppError::BadRequest("CRS string cannot be empty".into()));
    }
    if s.starts_with('/') || s.starts_with('\\') || s.contains("..") || s.contains('\0') {
        return Err(AppError::BadRequest(
            format!("Invalid CRS string (looks like a path): '{}'", &s[..s.len().min(50)])
        ));
    }
    Ok(())
}

/// Normalize a GDAL geometry to WGS84 (EPSG:4326) from the given source CRS, in-place.
/// No-op if source_crs is already EPSG:4326.
pub fn normalize_geom_to_wgs84(
    geom: &mut gdal::vector::Geometry,
    source_crs: &str,
) -> Result<(), AppError> {
    use gdal::spatial_ref::{AxisMappingStrategy, CoordTransform, SpatialRef};

    let crs = source_crs.trim();
    if crs.eq_ignore_ascii_case("EPSG:4326") {
        return Ok(());
    }

    validate_crs_string(crs)?;

    let mut src_srs = SpatialRef::from_definition(crs)
        .map_err(|e| AppError::BadRequest(format!("Invalid source_crs '{crs}': {e}")))?;
    src_srs.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);

    let mut wgs84 = SpatialRef::from_epsg(4326)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("WGS84 SRS init failed: {e}")))?;
    wgs84.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);

    let transform = CoordTransform::new(&src_srs, &wgs84)
        .map_err(|e| AppError::BadRequest(format!("Cannot create transform to WGS84: {e}")))?;

    geom.transform_inplace(&transform)
        .map_err(|e| AppError::BadRequest(format!("Reprojection to WGS84 failed: {e}")))?;

    Ok(())
}

/// Compute the EPSG code for the auto-UTM zone covering the given WGS84 lon/lat.
/// Returns e.g. 32610 (UTM 10N) or 32710 (UTM 10S).
pub fn auto_utm_epsg(lon: f64, lat: f64) -> u32 {
    let zone = ((lon + 180.0) / 6.0).floor() as u32 + 1;
    let zone = zone.clamp(1, 60);
    if lat >= 0.0 {
        32600 + zone
    } else {
        32700 + zone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_min() {
        assert_eq!(compute_price(0), 0.01);
        assert_eq!(compute_price(100), 0.01);
    }

    #[test]
    fn price_one_mb() {
        let p = compute_price(1024 * 1024);
        assert!((p - 0.01).abs() < 1e-6);
    }

    #[test]
    fn price_cap() {
        assert_eq!(compute_price(1024 * 1024 * 1024), 2.00);
    }

    #[test]
    fn price_200mb() {
        let p = compute_price(200 * 1024 * 1024);
        assert_eq!(p, 2.00);
    }

    #[test]
    fn auto_utm_north() {
        // San Francisco: lon=-122.4, lat=37.8 → zone 10N = 32610
        assert_eq!(auto_utm_epsg(-122.4, 37.8), 32610);
    }

    #[test]
    fn auto_utm_south() {
        // Sydney: lon=151, lat=-33 → zone 56S = 32756
        assert_eq!(auto_utm_epsg(151.0, -33.0), 32756);
    }

    #[test]
    fn auto_utm_equator_zero() {
        // lon=0, lat=0 → zone 31, lat>=0 → 32631
        assert_eq!(auto_utm_epsg(0.0, 0.0), 32631);
    }
}
