use gdal::vector::Geometry;
use serde_json::Value;

use crate::error::AppError;
use super::reproject::extract_features;

/// Align all geometries in `features_b` to the CRS of `features_a`.
/// Since we operate on GeoJSON (always WGS84), this is effectively a no-op
/// but we validate both inputs parse correctly.
fn align_crs_noop(
    features_b: Vec<Value>,
) -> Result<Vec<Value>, AppError> {
    // GeoJSON spec requires WGS84 — both inputs are already aligned.
    // If source_crs support is added later, reproject here.
    Ok(features_b)
}

// ── Union ─────────────────────────────────────────────────────────────────────

/// Combine all features from two layers. If dissolve=true, merge all geometries.
pub fn do_union(
    geojson_a: String,
    geojson_b: String,
    dissolve: bool,
) -> Result<Value, AppError> {
    let fc_a: Value = serde_json::from_str(&geojson_a)
        .map_err(|e| AppError::BadRequest(format!("layer_a: Invalid JSON: {e}")))?;
    let fc_b: Value = serde_json::from_str(&geojson_b)
        .map_err(|e| AppError::BadRequest(format!("layer_b: Invalid JSON: {e}")))?;

    let features_a = extract_features(&fc_a)?;
    let features_b = extract_features(&fc_b)?;
    let features_b = align_crs_noop(features_b)?;

    let count_a = features_a.len();
    let count_b = features_b.len();

    if dissolve {
        // Merge all geometries into a single dissolved feature
        let all_features: Vec<Value> = features_a.into_iter().chain(features_b).collect();
        let mut union_geom: Option<Geometry> = None;
        for feat in &all_features {
            if let Some(gv) = feat.get("geometry") {
                if !gv.is_null() {
                    if let Ok(g) = Geometry::from_geojson(&gv.to_string()) {
                        union_geom = Some(match union_geom {
                            None => g,
                            Some(u) => u.union(&g)
                                .ok_or_else(|| AppError::Internal(anyhow::anyhow!("Union failed")))?,
                        });
                    }
                }
            }
        }
        let dissolved = union_geom.ok_or_else(|| AppError::BadRequest("No geometries to union".into()))?;
        let geom_json: Value = serde_json::from_str(
            &dissolved.json().map_err(|e| AppError::Internal(anyhow::anyhow!("Geom JSON: {e}")))?,
        )
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Geom parse: {e}")))?;

        return Ok(serde_json::json!({
            "type": "FeatureCollection",
            "features": [{
                "type": "Feature",
                "properties": {},
                "geometry": geom_json
            }],
            "_meta": {
                "layer_a_features": count_a,
                "layer_b_features": count_b,
                "total_features": 1,
                "dissolved": true
            }
        }));
    }

    // Simple concatenation
    let mut combined: Vec<Value> = features_a;
    combined.extend(features_b);
    let total = combined.len();

    Ok(serde_json::json!({
        "type": "FeatureCollection",
        "features": combined,
        "_meta": {
            "layer_a_features": count_a,
            "layer_b_features": count_b,
            "total_features": total,
            "dissolved": false
        }
    }))
}

// ── Intersect ─────────────────────────────────────────────────────────────────

/// Return features/areas common to both layers. Raises 400 if empty.
pub fn do_intersect(
    geojson_a: String,
    geojson_b: String,
) -> Result<Value, AppError> {
    let fc_a: Value = serde_json::from_str(&geojson_a)
        .map_err(|e| AppError::BadRequest(format!("layer_a: Invalid JSON: {e}")))?;
    let fc_b: Value = serde_json::from_str(&geojson_b)
        .map_err(|e| AppError::BadRequest(format!("layer_b: Invalid JSON: {e}")))?;

    let features_a = extract_features(&fc_a)?;
    let features_b = extract_features(&fc_b)?;
    let count_a = features_a.len();
    let count_b = features_b.len();

    // Union all B geometries into a single mask
    let mask = union_geometries(&features_b).ok_or_else(|| {
        AppError::BadRequest("layer_b has no valid geometries".into())
    })?;

    let mut out_features: Vec<Value> = Vec::new();

    for feat_a in &features_a {
        let geom_val = match feat_a.get("geometry") {
            Some(g) if !g.is_null() => g,
            _ => continue,
        };
        let geom_a = Geometry::from_geojson(&geom_val.to_string())
            .map_err(|e| AppError::BadRequest(format!("layer_a geometry: {e}")))?;

        if !geom_a.intersects(&mask) {
            continue;
        }

        let intersection = geom_a
            .intersection(&mask)
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("Intersection failed")))?;

        if intersection.is_empty() {
            continue;
        }

        let geom_json: Value = serde_json::from_str(
            &intersection.json().map_err(|e| AppError::Internal(anyhow::anyhow!("Geom JSON: {e}")))?,
        )
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Geom parse: {e}")))?;

        let mut new_feat = feat_a.clone();
        new_feat["geometry"] = geom_json;
        out_features.push(new_feat);
    }

    if out_features.is_empty() {
        return Err(AppError::BadRequest(
            "Intersection is empty — the two layers do not overlap.".into(),
        ));
    }

    Ok(serde_json::json!({
        "type": "FeatureCollection",
        "features": out_features,
        "_meta": {
            "layer_a_features": count_a,
            "layer_b_features": count_b,
            "output_features": out_features.len()
        }
    }))
}

// ── Difference ────────────────────────────────────────────────────────────────

/// Return parts of layer_a not covered by layer_b. Raises 400 if empty.
pub fn do_difference(
    geojson_a: String,
    geojson_b: String,
) -> Result<Value, AppError> {
    let fc_a: Value = serde_json::from_str(&geojson_a)
        .map_err(|e| AppError::BadRequest(format!("layer_a: Invalid JSON: {e}")))?;
    let fc_b: Value = serde_json::from_str(&geojson_b)
        .map_err(|e| AppError::BadRequest(format!("layer_b: Invalid JSON: {e}")))?;

    let features_a = extract_features(&fc_a)?;
    let features_b = extract_features(&fc_b)?;
    let count_a = features_a.len();
    let count_b = features_b.len();

    let mask = union_geometries(&features_b).ok_or_else(|| {
        AppError::BadRequest("layer_b has no valid geometries".into())
    })?;

    let mut out_features: Vec<Value> = Vec::new();

    for feat_a in &features_a {
        let geom_val = match feat_a.get("geometry") {
            Some(g) if !g.is_null() => g,
            _ => {
                out_features.push(feat_a.clone());
                continue;
            }
        };
        let geom_a = Geometry::from_geojson(&geom_val.to_string())
            .map_err(|e| AppError::BadRequest(format!("layer_a geometry: {e}")))?;

        // If no intersection, keep as-is
        if !geom_a.intersects(&mask) {
            out_features.push(feat_a.clone());
            continue;
        }

        let diff = geom_a
            .difference(&mask)
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("Difference failed")))?;

        if diff.is_empty() {
            continue; // Entirely covered — exclude
        }

        let geom_json: Value = serde_json::from_str(
            &diff.json().map_err(|e| AppError::Internal(anyhow::anyhow!("Geom JSON: {e}")))?,
        )
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Geom parse: {e}")))?;

        let mut new_feat = feat_a.clone();
        new_feat["geometry"] = geom_json;
        out_features.push(new_feat);
    }

    if out_features.is_empty() {
        return Err(AppError::BadRequest(
            "Difference is empty — layer_a is entirely covered by layer_b.".into(),
        ));
    }

    Ok(serde_json::json!({
        "type": "FeatureCollection",
        "features": out_features,
        "_meta": {
            "layer_a_features": count_a,
            "layer_b_features": count_b,
            "output_features": out_features.len()
        }
    }))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn union_geometries(features: &[Value]) -> Option<Geometry> {
    let mut result: Option<Geometry> = None;
    for feat in features {
        if let Some(gv) = feat.get("geometry") {
            if !gv.is_null() {
                if let Ok(g) = Geometry::from_geojson(&gv.to_string()) {
                    result = Some(match result {
                        None => g,
                        Some(u) => u.union(&g)?,
                    });
                }
            }
        }
    }
    result
}
