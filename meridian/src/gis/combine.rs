use gdal::vector::Geometry;
use serde_json::{Map, Value};

use crate::error::AppError;
use super::reproject::extract_features;

// ── Append ────────────────────────────────────────────────────────────────────

/// Append features from layer_b into layer_a's schema.
/// Extra fields from B not in A are dropped. Missing A fields filled with null.
pub fn do_append(
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

    // Collect layer_a's schema (field names)
    let schema: Vec<String> = features_a
        .first()
        .and_then(|f| f.get("properties"))
        .and_then(|p| p.as_object())
        .map(|obj| obj.keys().cloned().collect())
        .unwrap_or_default();

    // Start with all of A
    let mut combined = features_a.clone();

    // Append B features, aligned to A schema
    for feat_b in &features_b {
        let props_b = feat_b
            .get("properties")
            .and_then(|p| p.as_object())
            .cloned()
            .unwrap_or_default();

        let mut aligned_props: Map<String, Value> = Map::new();
        for field in &schema {
            aligned_props.insert(
                field.clone(),
                props_b.get(field).cloned().unwrap_or(Value::Null),
            );
        }

        let mut new_feat = feat_b.clone();
        new_feat["properties"] = Value::Object(aligned_props);
        combined.push(new_feat);
    }

    let total = combined.len();

    Ok(serde_json::json!({
        "type": "FeatureCollection",
        "features": combined,
        "_meta": {
            "layer_a_features": count_a,
            "layer_b_features": count_b,
            "total_features": total,
            "schema": schema
        }
    }))
}

// ── Merge ─────────────────────────────────────────────────────────────────────

/// Combine both layers preserving all fields from both (union of schemas). Fill nulls.
pub fn do_merge(
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

    // Collect union of all field names from both layers
    let mut all_fields: indexmap::IndexMap<String, ()> = indexmap::IndexMap::new();
    for feat in features_a.iter().chain(features_b.iter()) {
        if let Some(props) = feat.get("properties").and_then(|p| p.as_object()) {
            for k in props.keys() {
                all_fields.insert(k.clone(), ());
            }
        }
    }
    let merged_fields: Vec<String> = all_fields.keys().cloned().collect();

    let normalize = |feat: Value| -> Value {
        let props = feat
            .get("properties")
            .and_then(|p| p.as_object())
            .cloned()
            .unwrap_or_default();
        let mut normalized: Map<String, Value> = Map::new();
        for field in &merged_fields {
            normalized.insert(
                field.clone(),
                props.get(field).cloned().unwrap_or(Value::Null),
            );
        }
        let mut new_feat = feat.clone();
        new_feat["properties"] = Value::Object(normalized);
        new_feat
    };

    let combined: Vec<Value> = features_a
        .into_iter()
        .chain(features_b)
        .map(normalize)
        .collect();
    let total = combined.len();

    Ok(serde_json::json!({
        "type": "FeatureCollection",
        "features": combined,
        "_meta": {
            "layer_a_features": count_a,
            "layer_b_features": count_b,
            "total_features": total,
            "merged_fields": merged_fields
        }
    }))
}

// ── Spatial Join ──────────────────────────────────────────────────────────────

/// Join attributes from layer_b onto layer_a based on spatial relationship.
///
/// how: "left" (keep all A), "inner" (only matching), "right" (keep all B)
/// predicate: "intersects" | "within" | "contains" | "crosses" | "touches" | "overlaps" | "nearest"
pub fn do_spatial_join(
    geojson_a: String,
    geojson_b: String,
    how: String,
    predicate: String,
) -> Result<Value, AppError> {
    let valid_hows = ["left", "right", "inner"];
    let valid_predicates = ["intersects", "within", "contains", "crosses", "touches", "overlaps", "nearest"];

    if !valid_hows.contains(&how.as_str()) {
        return Err(AppError::BadRequest(format!(
            "how must be one of: {}",
            valid_hows.join(", ")
        )));
    }
    if !valid_predicates.contains(&predicate.as_str()) {
        return Err(AppError::BadRequest(format!(
            "predicate must be one of: {}",
            valid_predicates.join(", ")
        )));
    }

    let fc_a: Value = serde_json::from_str(&geojson_a)
        .map_err(|e| AppError::BadRequest(format!("layer_a: Invalid JSON: {e}")))?;
    let fc_b: Value = serde_json::from_str(&geojson_b)
        .map_err(|e| AppError::BadRequest(format!("layer_b: Invalid JSON: {e}")))?;

    let features_a = extract_features(&fc_a)?;
    let features_b = extract_features(&fc_b)?;
    let count_a = features_a.len();
    let count_b = features_b.len();

    // Pre-parse B geometries
    let geoms_b: Vec<Option<Geometry>> = features_b
        .iter()
        .map(|f| {
            f.get("geometry")
                .filter(|g| !g.is_null())
                .and_then(|g| Geometry::from_geojson(&g.to_string()).ok())
        })
        .collect();

    let mut out_features: Vec<Value> = Vec::new();

    for feat_a in &features_a {
        let geom_a_opt = feat_a
            .get("geometry")
            .filter(|g| !g.is_null())
            .and_then(|g| Geometry::from_geojson(&g.to_string()).ok());

        // Find matching B features
        let mut matched_b_indices: Vec<usize> = Vec::new();

        if let Some(ref geom_a) = geom_a_opt {
            if predicate == "nearest" {
                // Find nearest B by envelope midpoint distance (GDAL 0.19 has no centroid fn)
                let env_a = geom_a.envelope();
                let cx_a = (env_a.MinX + env_a.MaxX) / 2.0;
                let cy_a = (env_a.MinY + env_a.MaxY) / 2.0;
                let mut min_dist = f64::INFINITY;
                let mut nearest_idx = None;
                for (i, geom_b_opt) in geoms_b.iter().enumerate() {
                    if let Some(gb) = geom_b_opt {
                        let env_b = gb.envelope();
                        let cx_b = (env_b.MinX + env_b.MaxX) / 2.0;
                        let cy_b = (env_b.MinY + env_b.MaxY) / 2.0;
                        let d = ((cx_a - cx_b).powi(2) + (cy_a - cy_b).powi(2)).sqrt();
                        if d < min_dist {
                            min_dist = d;
                            nearest_idx = Some(i);
                        }
                    }
                }
                if let Some(idx) = nearest_idx {
                    matched_b_indices.push(idx);
                }
            } else {
                for (i, geom_b_opt) in geoms_b.iter().enumerate() {
                    if let Some(gb) = geom_b_opt {
                        let matches = match predicate.as_str() {
                            "intersects" => geom_a.intersects(gb),
                            "within" => geom_a.within(gb),
                            "contains" => geom_a.contains(gb),
                            "crosses" => geom_a.crosses(gb),
                            "touches" => geom_a.touches(gb),
                            "overlaps" => geom_a.overlaps(gb),
                            _ => false,
                        };
                        if matches {
                            matched_b_indices.push(i);
                        }
                    }
                }
            }
        }

        if matched_b_indices.is_empty() {
            // No match
            match how.as_str() {
                "left" => {
                    // Keep A with null B attributes
                    out_features.push(feat_a.clone());
                }
                "inner" | "right" => {
                    // Drop unmatched A for inner; right is handled separately
                }
                _ => {}
            }
        } else {
            // For each matching B, output a joined feature
            for &bi in &matched_b_indices {
                let feat_b = &features_b[bi];
                let joined = merge_feature_properties(feat_a, feat_b)?;
                out_features.push(joined);
            }
        }
    }

    // For "right" join: also keep unmatched B features
    if how == "right" {
        let matched_b_set: std::collections::HashSet<usize> = features_a
            .iter()
            .flat_map(|fa| {
                let geom_a_opt = fa
                    .get("geometry")
                    .filter(|g| !g.is_null())
                    .and_then(|g| Geometry::from_geojson(&g.to_string()).ok());
                geoms_b
                    .iter()
                    .enumerate()
                    .filter_map(|(i, gb_opt)| {
                        if let (Some(ref ga), Some(gb)) = (&geom_a_opt, gb_opt) {
                            let matches = match predicate.as_str() {
                                "intersects" => ga.intersects(gb),
                                "within" => ga.within(gb),
                                "contains" => ga.contains(gb),
                                "crosses" => ga.crosses(gb),
                                "touches" => ga.touches(gb),
                                "overlaps" => ga.overlaps(gb),
                                "nearest" => false, // handled separately
                                _ => false,
                            };
                            if matches { Some(i) } else { None }
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect();

        for (i, feat_b) in features_b.iter().enumerate() {
            if !matched_b_set.contains(&i) {
                out_features.push(feat_b.clone());
            }
        }
    }

    let joined_count = out_features.len();

    Ok(serde_json::json!({
        "type": "FeatureCollection",
        "features": out_features,
        "_meta": {
            "layer_a_features": count_a,
            "layer_b_features": count_b,
            "joined_features": joined_count,
            "how": how,
            "predicate": predicate
        }
    }))
}

/// Merge properties from feat_a and feat_b. B properties suffixed with "_right" on conflict.
fn merge_feature_properties(feat_a: &Value, feat_b: &Value) -> Result<Value, AppError> {
    let props_a = feat_a
        .get("properties")
        .and_then(|p| p.as_object())
        .cloned()
        .unwrap_or_default();
    let props_b = feat_b
        .get("properties")
        .and_then(|p| p.as_object())
        .cloned()
        .unwrap_or_default();

    let mut merged: Map<String, Value> = props_a.clone();
    for (k, v) in props_b {
        if merged.contains_key(&k) {
            merged.insert(format!("{k}_right"), v);
        } else {
            merged.insert(k, v);
        }
    }

    let mut new_feat = feat_a.clone();
    new_feat["properties"] = Value::Object(merged);
    Ok(new_feat)
}
