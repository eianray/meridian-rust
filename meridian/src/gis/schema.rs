use gdal::vector::Geometry;
use serde_json::Value;

use crate::error::AppError;
use super::reproject::extract_features;

// ── Schema ─────────────────────────────────────────────────────────────────────

pub fn do_schema(geojson_str: String, filename: String) -> Result<Value, AppError> {
    let fc: Value = serde_json::from_str(&geojson_str)
        .map_err(|e| AppError::BadRequest(format!("Invalid JSON: {e}")))?;
    let features = extract_features(&fc)?;

    let mut field_map: indexmap::IndexMap<String, String> = indexmap::IndexMap::new();
    let mut geom_types: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;

    for feat in &features {
        if let Some(props) = feat.get("properties").and_then(|p| p.as_object()) {
            for (k, v) in props {
                field_map.entry(k.clone()).or_insert_with(|| json_type_name(v).to_string());
            }
        }
        if let Some(geom_val) = feat.get("geometry") {
            if let Some(gt) = geom_val.get("type").and_then(|t| t.as_str()) {
                geom_types.insert(gt.to_string());
            }
            if !geom_val.is_null() {
                if let Ok(geom_str) = serde_json::to_string(geom_val) {
                    if let Ok(geom) = Geometry::from_geojson(&geom_str) {
                        let env = geom.envelope();
                        min_x = min_x.min(env.MinX);
                        min_y = min_y.min(env.MinY);
                        max_x = max_x.max(env.MaxX);
                        max_y = max_y.max(env.MaxY);
                    }
                }
            }
        }
    }

    let fields: Vec<Value> = field_map.iter()
        .map(|(name, typ)| serde_json::json!({"name": name, "type": typ}))
        .collect();

    let mut geom_types_sorted: Vec<String> = geom_types.into_iter().collect();
    geom_types_sorted.sort();
    let geometry_type: Value = if geom_types_sorted.len() == 1 {
        Value::String(geom_types_sorted[0].clone())
    } else if geom_types_sorted.is_empty() {
        Value::String("Unknown".to_string())
    } else {
        Value::Array(geom_types_sorted.into_iter().map(Value::String).collect())
    };

    let bbox: Value = if min_x.is_finite() {
        serde_json::json!([min_x, min_y, max_x, max_y])
    } else {
        Value::Null
    };

    Ok(serde_json::json!({
        "filename": filename,
        "layer_count": 1,
        "layers": [{
            "layer": "default",
            "geometry_type": geometry_type,
            "feature_count": features.len(),
            "bbox": bbox,
            "crs_epsg": 4326,
            "crs_name": "WGS 84",
            "crs_wkt": null,
            "fields": fields
        }]
    }))
}

fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(n) => if n.is_f64() { "float" } else { "int" },
        Value::String(_) => "str",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

// ── Validate ───────────────────────────────────────────────────────────────────

pub fn do_validate(geojson_str: String) -> Result<Value, AppError> {
    let fc: Value = serde_json::from_str(&geojson_str)
        .map_err(|e| AppError::BadRequest(format!("Invalid JSON: {e}")))?;
    let features = extract_features(&fc)?;
    let total = features.len();
    let mut issues: Vec<Value> = Vec::new();

    for (i, feat) in features.iter().enumerate() {
        match feat.get("geometry") {
            None | Some(Value::Null) => {
                issues.push(serde_json::json!({"feature_index": i, "reason": "null geometry"}));
            }
            Some(gv) => {
                match Geometry::from_geojson(&gv.to_string()) {
                    Err(e) => {
                        issues.push(serde_json::json!({
                            "feature_index": i,
                            "reason": format!("Invalid GeoJSON geometry: {e}")
                        }));
                    }
                    Ok(geom) => {
                        if !geom.is_valid() {
                            issues.push(serde_json::json!({
                                "feature_index": i,
                                "reason": "Invalid geometry (self-intersection or other GEOS error)"
                            }));
                        }
                    }
                }
            }
        }
    }

    let invalid_count = issues.len();
    Ok(serde_json::json!({
        "total_features": total,
        "valid_count": total - invalid_count,
        "invalid_count": invalid_count,
        "all_valid": invalid_count == 0,
        "issues": issues
    }))
}

// ── Repair ─────────────────────────────────────────────────────────────────────

pub fn do_repair(geojson_str: String) -> Result<Value, AppError> {
    let fc: Value = serde_json::from_str(&geojson_str)
        .map_err(|e| AppError::BadRequest(format!("Invalid JSON: {e}")))?;
    let features = extract_features(&fc)?;
    let total = features.len();
    let mut fixed_count = 0usize;
    let mut out_features: Vec<Value> = Vec::with_capacity(total);

    for feat in &features {
        let geom_val = match feat.get("geometry") {
            Some(g) if !g.is_null() => g,
            _ => { out_features.push(feat.clone()); continue; }
        };

        match Geometry::from_geojson(&geom_val.to_string()) {
            Err(_) => { out_features.push(feat.clone()); }
            Ok(geom) => {
                if !geom.is_valid() {
                    fixed_count += 1;
                    let repaired = make_valid_geom(geom)?;
                    let rj: Value = serde_json::from_str(
                        &repaired.json().map_err(|e| AppError::Internal(anyhow::anyhow!("Geom JSON: {e}")))?,
                    ).map_err(|e| AppError::Internal(anyhow::anyhow!("Repaired parse: {e}")))?;
                    let mut new_feat = feat.clone();
                    new_feat["geometry"] = rj;
                    out_features.push(new_feat);
                } else {
                    out_features.push(feat.clone());
                }
            }
        }
    }

    Ok(serde_json::json!({
        "type": "FeatureCollection",
        "features": out_features,
        "_meta": { "total_features": total, "fixed_count": fixed_count }
    }))
}

fn make_valid_geom(geom: Geometry) -> Result<Geometry, AppError> {
    use gdal::cpl::CslStringList;
    // Try GDAL MakeValid (GEOS 3.8+)
    if let Ok(valid) = geom.make_valid(&CslStringList::new()) {
        return Ok(valid);
    }
    // Fallback: buffer by 0
    geom.buffer(0.0, 1)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Repair (buffer-by-zero) failed: {e}")))
}
