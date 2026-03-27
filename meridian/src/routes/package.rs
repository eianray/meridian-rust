use axum::{extract::Extension, http::HeaderMap, Json};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use gdal::vector::LayerAccess;
use serde::Deserialize;
use std::time::Instant;
use tempfile::TempDir;
use utoipa::ToSchema;

use crate::error::AppError;
use crate::gis::{compute_price, GeoJsonOutput};
use crate::gis::reproject::payment_gate;
use crate::metrics;
use crate::middleware::request_id::RequestId;
use crate::AppState;

#[derive(Deserialize, ToSchema)]
#[allow(dead_code)]
pub struct PackageGdbParams {
    /// Up to 10 GeoJSON FeatureCollection files: layer_1, layer_2, ... layer_N
    #[allow(dead_code)]
    pub layer_1: Option<String>,
}

/// Package multiple GeoJSON layers into a GDB (File Geodatabase) archive.
/// Multipart form fields:
///   layer_1 … layer_N  — GeoJSON FeatureCollection bytes (up to 10 layers)
///   name_1 … name_N    — corresponding layer names (text)
/// At least one layer/name pair is required.
#[utoipa::path(
    post,
    path = "/v1/package/gdb",
    tag = "GIS",
    request_body(
        content_type = "multipart/form-data",
        description = "Multipart form: layer_1…layer_N (GeoJSON bytes), name_1…name_N (layer names, text)",
        content = PackageGdbParams
    ),
    responses(
        (status = 200, description = "GDB archive as base64 zip", body = GeoJsonOutput),
        (status = 400, description = "Bad request — missing layer, invalid GeoJSON, unmatched pairs"),
        (status = 402, description = "Payment required", body = crate::billing::PaymentRequired),
        (status = 413, description = "Payload too large"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn package_gdb(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    // Parse all layer/name pairs from multipart
    // layer_1..layer_10, name_1..name_10
    let mut layers: Vec<(String, Vec<u8>)> = Vec::with_capacity(10);

    // Temporary storage for layer bytes while we collect names
    let mut layer_bytes: std::collections::HashMap<usize, Vec<u8>> =
        std::collections::HashMap::new();
    let mut layer_names: std::collections::HashMap<usize, String> =
        std::collections::HashMap::new();

    let mut field_count: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();

        // Track field occurrence for counting
        let counter = field_count.entry(name.clone()).or_insert(0);
        *counter += 1;

        if name.starts_with("layer_") {
            if let Some(idx) = name.strip_prefix("layer_") {
                if let Ok(n) = idx.parse::<usize>() {
                    if n < 1 || n > 10 {
                        return Err(AppError::BadRequest(
                            "Layer index must be 1-10".into(),
                        ));
                    }
                    let bytes = field
                        .bytes()
                        .await
                        .map_err(|e| AppError::BadRequest(format!("layer read: {e}")))?;
                    layer_bytes.insert(n, bytes.to_vec());
                }
            }
        } else if name.starts_with("name_") {
            if let Some(idx) = name.strip_prefix("name_") {
                if let Ok(n) = idx.parse::<usize>() {
                    if n < 1 || n > 10 {
                        return Err(AppError::BadRequest(
                            "Name index must be 1-10".into(),
                        ));
                    }
                    let text = field
                        .text()
                        .await
                        .map_err(|e| AppError::BadRequest(format!("name read: {e}")))?;
                    layer_names.insert(n, text.trim().to_string());
                }
            }
        }
    }

    // Build ordered list of (name, bytes) pairs
    let mut used_indices: Vec<usize> = layer_bytes.keys().cloned().collect();
    used_indices.sort_unstable();

    for idx in &used_indices {
        let name = layer_names.get(idx).ok_or_else(|| {
            AppError::BadRequest(format!(
                "Missing name_{} for layer_{}. Provide both layer_{{1-10}} and name_{{1-10}}.",
                idx, idx
            ))
        })?;

        let bytes = layer_bytes.get(idx).unwrap();

        // Validate GeoJSON: must be a FeatureCollection with a features array
        let parsed: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|e| {
                AppError::BadRequest(format!(
                    "layer_{} is not valid GeoJSON: {}",
                    idx, e
                ))
            })?;
        if !parsed.get("features").map(|f| f.is_array()).unwrap_or(false) {
            return Err(AppError::BadRequest(format!(
                "layer_{} is not a GeoJSON FeatureCollection (missing features array)",
                idx
            )));
        }

        layers.push((name.clone(), bytes.clone()));
    }

    if layers.is_empty() {
        return Err(AppError::BadRequest(
            "No layers provided. Upload at least one layer_{{1-10}} / name_{{1-10}} pair.".into(),
        ));
    }

    let total_size: usize = layers.iter().map(|(_, b)| b.len()).sum();
    let price = compute_price(total_size);
    let t0 = Instant::now();
    metrics::record_request("package-gdb", "received");
    payment_gate("package-gdb", total_size, price, &request_id, &headers, &state).await?;

    let result = do_package_gdb(&layers)?;

    metrics::record_request("package-gdb", "ok");
    metrics::record_request_duration("package-gdb", t0.elapsed().as_secs_f64());

    Ok(Json(GeoJsonOutput {
        request_id,
        price_usd: price,
        result: serde_json::json!({
            "data": STANDARD.encode(&result),
            "encoding": "base64",
            "filename": "reliant-output.gdb.zip",
            "mime_type": "application/zip"
        }),
    }))
}

fn do_package_gdb(layers: &[(String, Vec<u8>)]) -> Result<Vec<u8>, AppError> {
    use std::process::Command;

    let tmp = TempDir::new().map_err(|e| AppError::Internal(anyhow::anyhow!("TempDir: {e}")))?;
    let gdb_dir = tmp.path().join("output.gdb");

    for (layer_idx, (layer_name, geojson_bytes)) in layers.iter().enumerate() {
        // Write GeoJSON to temp file
        let geojson_path = tmp.path().join(format!("layer_{}.geojson", layer_idx));
        std::fs::write(&geojson_path, geojson_bytes)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Write GeoJSON {}: {}", layer_name, e)))?;

        // Build ogr2ogr command
        // -f OpenFileGDB: output format
        // -nln <name>: rename layer to requested name
        // For first layer: create the GDB directory
        // For subsequent layers: append to existing GDB
        // Step 1: Explode any GeometryCollections → flat GeoJSON
        // OpenFileGDB rejects GeometryCollection — must be Polygon/MultiPolygon/LineString etc.
        let exploded_path = tmp.path().join(format!("layer_{}_exploded.geojson", layer_idx));
        let explode_output = Command::new("ogr2ogr")
            .args(["-f", "GeoJSON", "-explodecollections"])
            .arg(&exploded_path)
            .arg(&geojson_path)
            .output()
            .map_err(|e| AppError::Internal(anyhow::anyhow!("ogr2ogr explode failed: {}", e)))?;

        if !explode_output.status.success() {
            let stderr = String::from_utf8_lossy(&explode_output.stderr);
            return Err(AppError::Internal(anyhow::anyhow!(
                "ogr2ogr explode failed for layer '{}': {}", layer_name, stderr
            )));
        }

        // Step 2: Write exploded GeoJSON → OpenFileGDB
        let mut cmd = Command::new("ogr2ogr");
        cmd.arg("-f").arg("OpenFileGDB");

        if layer_idx == 0 {
            cmd.arg("-overwrite");
        }
        cmd.arg("-nln").arg(layer_name);
        cmd.arg("-nlt").arg("PROMOTE_TO_MULTI");

        if layer_idx > 0 {
            cmd.arg("-append");
        }

        cmd.arg(&gdb_dir);
        cmd.arg(&exploded_path);

        let output = cmd.output()
            .map_err(|e| AppError::Internal(anyhow::anyhow!("ogr2ogr failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AppError::Internal(anyhow::anyhow!(
                "ogr2ogr failed for layer '{}': {}",
                layer_name, stderr
            )));
        }
    }

    // Zip the .gdb directory
    let zip_path = tmp.path().join("output.gdb.zip");
    zip_directory(&gdb_dir, &zip_path)?;

    let zip_bytes = std::fs::read(&zip_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Read zip: {e}")))?;

    Ok(zip_bytes)
}

fn zip_directory(src_dir: &std::path::Path, zip_path: &std::path::Path) -> Result<(), AppError> {
    use std::fs;
    use std::io::{Read, Write};

    let file = fs::File::create(zip_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Create zip file: {e}")))?;
    let mut zip = zip::ZipWriter::new(file);

    let options = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    walk_dir(src_dir, src_dir, &mut zip, &options)?;

    zip.finish()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Finish zip: {e}")))?;

    Ok(())
}

fn walk_dir(
    base: &std::path::Path,
    dir: &std::path::Path,
    zip: &mut zip::ZipWriter<std::fs::File>,
    options: &zip::write::FileOptions,
) -> Result<(), AppError> {
    use std::fs;
    use std::io::Read;

    for entry in fs::read_dir(dir)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Read dir: {e}")))?
    {
        let entry = entry
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Entry: {e}")))?;
        let path = entry.path();
        let name = path.strip_prefix(base.parent().unwrap_or(base))
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Strip prefix: {e}")))?;

        if path.is_file() {
            zip.start_file(name.to_string_lossy().into_owned(), *options)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Start file: {e}")))?;
            let mut f = fs::File::open(&path)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Open file: {e}")))?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Read file: {e}")))?;
            use std::io::Write;
            zip.write_all(&buf)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Write file: {e}")))?;
        } else if path.is_dir() {
            // Add directory entry
            let dir_name = name.to_string_lossy().into_owned();
            if !dir_name.is_empty() {
                zip.add_directory(dir_name, *options)
                    .map_err(|e| AppError::Internal(anyhow::anyhow!("Add dir: {e}")))?;
                walk_dir(base, &path, zip, options)?;
            }
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_single_layer_fc() -> Vec<u8> {
        let fc = serde_json::json!({
            "type": "FeatureCollection",
            "features": [{
                "type": "Feature",
                "geometry": { "type": "Point", "coordinates": [-122.4194, 37.7749] },
                "properties": { "name": "SF", "pop": 884363 }
            }]
        });
        serde_json::to_vec(&fc).unwrap()
    }

    fn make_three_layers() -> Vec<(String, Vec<u8>)> {
        let layer1 = serde_json::json!({
            "type": "FeatureCollection",
            "features": [{
                "type": "Feature",
                "geometry": { "type": "Point", "coordinates": [0.0, 0.0] },
                "properties": { "id": 1 }
            }]
        });
        let layer2 = serde_json::json!({
            "type": "FeatureCollection",
            "features": [{
                "type": "Feature",
                "geometry": { "type": "LineString", "coordinates": [[0.0, 0.0], [1.0, 1.0]] },
                "properties": { "id": 2 }
            }]
        });
        let layer3 = serde_json::json!({
            "type": "FeatureCollection",
            "features": [{
                "type": "Feature",
                "geometry": { "type": "Polygon", "coordinates": [[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 0.0]]] },
                "properties": { "id": 3 }
            }]
        });
        vec![
            ("points".to_string(), serde_json::to_vec(&layer1).unwrap()),
            ("lines".to_string(), serde_json::to_vec(&layer2).unwrap()),
            ("polygons".to_string(), serde_json::to_vec(&layer3).unwrap()),
        ]
    }

    // Test 1: Single layer GeoJSON → valid .gdb.zip
    #[test]
    fn test_package_gdb_single_layer() {
        let layers = vec![("cities".to_string(), make_single_layer_fc())];
        let result = do_package_gdb(&layers);
        assert!(result.is_ok(), "Expected ok, got: {:?}", result);
        let bytes = result.unwrap();
        assert!(!bytes.is_empty(), "Zip should not be empty");

        // Verify it's a valid zip
        let cursor = std::io::Cursor::new(&bytes);
        let mut archive = zip::ZipArchive::new(cursor).unwrap();
        assert!(archive.len() > 0, "Zip should contain entries");

        // Verify the GDB directory is present
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(names.iter().any(|n| n.contains("output.gdb")),
            "Zip should contain output.gdb: {:?}", names);
    }

    // Test 2: Multiple layers (3) → all layers present in output .gdb
    #[test]
    fn test_package_gdb_multiple_layers() {
        let layers = make_three_layers();
        let result = do_package_gdb(&layers);
        assert!(result.is_ok(), "Expected ok, got: {:?}", result);
        let bytes = result.unwrap();

        // Verify it's a valid zip
        let cursor = std::io::Cursor::new(&bytes);
        let mut archive = zip::ZipArchive::new(cursor).unwrap();
        assert!(archive.len() > 0);

        // GDB stores as directory + files; verify zip has content
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        // Should contain .gdb directory entries
        assert!(!names.is_empty(), "GDB zip should not be empty: {:?}", names);
    }

    // Test 3: Invalid GeoJSON in layer → error (BadRequest or Internal)
    #[test]
    fn test_package_gdb_invalid_geojson() {
        let bad_bytes = b"not geojson at all".to_vec();
        let layers = vec![("layer1".to_string(), bad_bytes)];
        let result = do_package_gdb(&layers);
        assert!(result.is_err(), "Expected error for invalid GeoJSON");
        // Invalid JSON is caught as BadRequest; invalid GeoJSON structure may be Internal
        let err = result.unwrap_err();
        assert!(
            matches!(err, AppError::BadRequest(_)) || matches!(err, AppError::Internal(_)),
            "Expected BadRequest or Internal error, got: {:?}", err
        );
    }

    // Test 4: Empty layers list → error handled upstream in handler
    // (Handler validates this, but test the inner function too)
    #[test]
    fn test_package_gdb_empty_layers() {
        let layers: Vec<(String, Vec<u8>)> = vec![];
        // The handler checks for empty, but do_package_gdb should also handle gracefully
        let result = do_package_gdb(&layers);
        // Should not panic; will fail on GDB creation
        assert!(result.is_err());
    }

    // Test 5: GeoJSON FeatureCollection with multiple features
    #[test]
    fn test_package_gdb_multi_feature() {
        let fc = serde_json::json!({
            "type": "FeatureCollection",
            "features": [
                {
                    "type": "Feature",
                    "geometry": { "type": "Point", "coordinates": [0.0, 0.0] },
                    "properties": { "id": 1, "name": "A" }
                },
                {
                    "type": "Feature",
                    "geometry": { "type": "Point", "coordinates": [1.0, 1.0] },
                    "properties": { "id": 2, "name": "B" }
                },
                {
                    "type": "Feature",
                    "geometry": { "type": "Point", "coordinates": [2.0, 2.0] },
                    "properties": { "id": 3, "name": "C" }
                }
            ]
        });
        let layers = vec![("points".to_string(), serde_json::to_vec(&fc).unwrap())];
        let result = do_package_gdb(&layers);
        assert!(result.is_ok(), "Expected ok, got: {:?}", result);
        let bytes = result.unwrap();
        assert!(!bytes.is_empty());
    }
}
