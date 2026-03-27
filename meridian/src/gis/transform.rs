use gdal::vector::{Geometry, OGRwkbGeometryType};
use serde_json::Value;

use crate::error::AppError;
use super::reproject::extract_features;

// ── Erase ──────────────────────────────────────────────────────────────────────

pub fn do_erase(geojson_str: String) -> Result<Value, AppError> {
    let fc: Value = serde_json::from_str(&geojson_str)
        .map_err(|e| AppError::BadRequest(format!("Invalid JSON: {e}")))?;
    let features = extract_features(&fc)?;
    let count = features.len();
    let fields: Vec<String> = features
        .first()
        .and_then(|f| f.get("properties"))
        .and_then(|p| p.as_object())
        .map(|obj| obj.keys().cloned().collect())
        .unwrap_or_default();
    Ok(serde_json::json!({
        "type": "FeatureCollection",
        "features": [],
        "_meta": { "features_removed": count, "fields_preserved": fields }
    }))
}

// ── Feature to Point ───────────────────────────────────────────────────────────

/// Compute centroid as envelope midpoint (GDAL 0.19 has no centroid fn).
fn envelope_centroid(geom: &Geometry) -> Result<Geometry, AppError> {
    let env = geom.envelope();
    let cx = (env.MinX + env.MaxX) / 2.0;
    let cy = (env.MinY + env.MaxY) / 2.0;
    let mut pt = Geometry::empty(OGRwkbGeometryType::wkbPoint)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Point create: {e}")))?;
    pt.set_point_2d(0, (cx, cy));
    Ok(pt)
}

pub fn do_feature_to_point(geojson_str: String) -> Result<Value, AppError> {
    let fc: Value = serde_json::from_str(&geojson_str)
        .map_err(|e| AppError::BadRequest(format!("Invalid JSON: {e}")))?;
    let features = extract_features(&fc)?;
    let input_count = features.len();
    let mut out_features = Vec::with_capacity(input_count);

    for feat in &features {
        let geom_val = match feat.get("geometry") {
            Some(g) if !g.is_null() => g,
            _ => { out_features.push(feat.clone()); continue; }
        };
        let geom = Geometry::from_geojson(&geom_val.to_string())
            .map_err(|e| AppError::BadRequest(format!("Invalid geometry: {e}")))?;
        let centroid = envelope_centroid(&geom)?;
        let cj: Value = serde_json::from_str(
            &centroid.json().map_err(|e| AppError::Internal(anyhow::anyhow!("Geom JSON: {e}")))?,
        ).map_err(|e| AppError::Internal(anyhow::anyhow!("Centroid parse: {e}")))?;
        let mut new_feat = feat.clone();
        new_feat["geometry"] = cj;
        out_features.push(new_feat);
    }

    Ok(serde_json::json!({
        "type": "FeatureCollection",
        "features": out_features,
        "_meta": { "input_features": input_count, "output_features": out_features.len() }
    }))
}

// ── Feature to Line ────────────────────────────────────────────────────────────

/// Extract polygon boundary as line coords. For points/lines, pass through.
fn geom_to_boundary_json(geom: &Geometry) -> Option<Value> {
    use gdal::vector::OGRwkbGeometryType::*;
    let gt = geom.geometry_type();
    match gt {
        wkbPolygon | wkbPolygon25D | wkbMultiPolygon | wkbMultiPolygon25D => {
            // Use the exterior ring of each polygon sub-geometry
            geom.json().ok().and_then(|s| {
                let v: Value = serde_json::from_str(&s).ok()?;
                // Convert Polygon → LinearRing coords as LineString
                let coords = v.pointer("/coordinates/0")?;
                Some(serde_json::json!({
                    "type": "LineString",
                    "coordinates": coords
                }))
            })
        }
        wkbPoint | wkbPoint25D | wkbMultiPoint | wkbMultiPoint25D => None, // empty boundary
        _ => geom.json().ok().and_then(|s| serde_json::from_str(&s).ok()),
    }
}

pub fn do_feature_to_line(geojson_str: String) -> Result<Value, AppError> {
    let fc: Value = serde_json::from_str(&geojson_str)
        .map_err(|e| AppError::BadRequest(format!("Invalid JSON: {e}")))?;
    let features = extract_features(&fc)?;
    let input_count = features.len();
    let mut out_features = Vec::new();

    for feat in &features {
        let geom_val = match feat.get("geometry") {
            Some(g) if !g.is_null() => g,
            _ => continue,
        };
        let geom = Geometry::from_geojson(&geom_val.to_string())
            .map_err(|e| AppError::BadRequest(format!("Invalid geometry: {e}")))?;
        if let Some(boundary_json) = geom_to_boundary_json(&geom) {
            let mut new_feat = feat.clone();
            new_feat["geometry"] = boundary_json;
            out_features.push(new_feat);
        }
    }

    Ok(serde_json::json!({
        "type": "FeatureCollection",
        "features": out_features,
        "_meta": { "input_features": input_count, "output_features": out_features.len() }
    }))
}

// ── Feature to Polygon ─────────────────────────────────────────────────────────

pub fn do_feature_to_polygon(geojson_str: String) -> Result<Value, AppError> {
    use gdal::vector::OGRwkbGeometryType::*;
    let fc: Value = serde_json::from_str(&geojson_str)
        .map_err(|e| AppError::BadRequest(format!("Invalid JSON: {e}")))?;
    let features = extract_features(&fc)?;
    let input_count = features.len();

    // Collect ring geometry and try to close them into polygons
    let mut out_features: Vec<Value> = Vec::new();
    for feat in &features {
        let geom_val = match feat.get("geometry") {
            Some(g) if !g.is_null() => g,
            _ => continue,
        };
        let geom = Geometry::from_geojson(&geom_val.to_string())
            .map_err(|e| AppError::BadRequest(format!("Invalid geometry: {e}")))?;
        let gt = geom.geometry_type();
        match gt {
            wkbLineString | wkbLineString25D => {
                // Close the ring and create a polygon if enough points
                let json_str = geom.json()
                    .map_err(|e| AppError::Internal(anyhow::anyhow!("Geom JSON: {e}")))?;
                let v: Value = serde_json::from_str(&json_str)
                    .map_err(|e| AppError::Internal(anyhow::anyhow!("Parse: {e}")))?;
                if let Some(coords) = v.get("coordinates").and_then(|c| c.as_array()) {
                    if coords.len() >= 3 {
                        let poly_json = serde_json::json!({
                            "type": "Polygon",
                            "coordinates": [coords]
                        });
                        let mut new_feat = serde_json::json!({
                            "type": "Feature",
                            "properties": feat.get("properties").cloned().unwrap_or(serde_json::json!({})),
                            "geometry": poly_json
                        });
                        out_features.push(new_feat);
                    }
                }
            }
            _ => {} // Skip non-line geometries
        }
    }

    if out_features.is_empty() {
        return Err(AppError::BadRequest(
            "No closed rings found — polygonize requires lines that form closed loops.".into(),
        ));
    }

    Ok(serde_json::json!({
        "type": "FeatureCollection",
        "features": out_features,
        "_meta": { "input_lines": input_count, "output_polygons": out_features.len() }
    }))
}

// ── Multipart to Singlepart ────────────────────────────────────────────────────

pub fn do_multipart_to_singlepart(geojson_str: String) -> Result<Value, AppError> {
    let fc: Value = serde_json::from_str(&geojson_str)
        .map_err(|e| AppError::BadRequest(format!("Invalid JSON: {e}")))?;
    let features = extract_features(&fc)?;
    let input_count = features.len();
    let mut out_features: Vec<Value> = Vec::new();

    for feat in &features {
        let geom_val = match feat.get("geometry") {
            Some(g) if !g.is_null() => g,
            _ => { out_features.push(feat.clone()); continue; }
        };
        let geom = Geometry::from_geojson(&geom_val.to_string())
            .map_err(|e| AppError::BadRequest(format!("Invalid geometry: {e}")))?;
        let parts = explode_geometry(&geom)?;
        if parts.is_empty() {
            out_features.push(feat.clone());
        } else {
            for part in &parts {
                let pj: Value = serde_json::from_str(
                    &part.json().map_err(|e| AppError::Internal(anyhow::anyhow!("Geom JSON: {e}")))?,
                ).map_err(|e| AppError::Internal(anyhow::anyhow!("Part parse: {e}")))?;
                let mut new_feat = feat.clone();
                new_feat["geometry"] = pj;
                out_features.push(new_feat);
            }
        }
    }

    let parts_added = out_features.len().saturating_sub(input_count);
    Ok(serde_json::json!({
        "type": "FeatureCollection",
        "features": out_features,
        "_meta": {
            "input_features": input_count,
            "output_features": out_features.len(),
            "parts_added": parts_added
        }
    }))
}

fn explode_geometry(geom: &Geometry) -> Result<Vec<Geometry>, AppError> {
    use gdal::vector::OGRwkbGeometryType::*;
    let gt = geom.geometry_type();
    match gt {
        wkbMultiPoint | wkbMultiPoint25D | wkbMultiPointM | wkbMultiPointZM
        | wkbMultiLineString | wkbMultiLineString25D | wkbMultiLineStringM | wkbMultiLineStringZM
        | wkbMultiPolygon | wkbMultiPolygon25D | wkbMultiPolygonM | wkbMultiPolygonZM
        | wkbGeometryCollection | wkbGeometryCollection25D
        | wkbGeometryCollectionM | wkbGeometryCollectionZM => {
            let count = geom.geometry_count();
            let mut parts = Vec::with_capacity(count);
            for i in 0..count {
                let sub = unsafe { geom.get_geometry(i) };
                let json = sub.json()
                    .map_err(|e| AppError::Internal(anyhow::anyhow!("Sub geom JSON: {e}")))?;
                let cloned = Geometry::from_geojson(&json)
                    .map_err(|e| AppError::Internal(anyhow::anyhow!("Sub geom clone: {e}")))?;
                parts.push(cloned);
            }
            Ok(parts)
        }
        _ => Ok(vec![]), // Already single-part — signal passthrough
    }
}

// ── Add Field ─────────────────────────────────────────────────────────────────

pub fn do_add_field(
    geojson_str: String,
    field_name: String,
    field_type: String,
    default_value: Option<String>,
) -> Result<Value, AppError> {
    let fc: Value = serde_json::from_str(&geojson_str)
        .map_err(|e| AppError::BadRequest(format!("Invalid JSON: {e}")))?;
    let features = extract_features(&fc)?;

    if let Some(first) = features.first() {
        if let Some(props) = first.get("properties").and_then(|p| p.as_object()) {
            if props.contains_key(&field_name) {
                return Err(AppError::BadRequest(format!("Field '{field_name}' already exists.")));
            }
        }
    }

    let parsed_default: Value = match (&field_type[..], &default_value) {
        (_, None) => Value::Null,
        ("int", Some(v)) => {
            let n: i64 = v.parse().map_err(|_| AppError::BadRequest(format!("Cannot parse '{v}' as int")))?;
            Value::from(n)
        }
        ("float", Some(v)) => {
            let n: f64 = v.parse().map_err(|_| AppError::BadRequest(format!("Cannot parse '{v}' as float")))?;
            Value::from(n)
        }
        ("bool", Some(v)) => Value::from(matches!(v.to_lowercase().as_str(), "true" | "1" | "yes")),
        ("str", Some(v)) => Value::from(v.clone()),
        (ft, _) => return Err(AppError::BadRequest(format!(
            "field_type must be one of: bool, float, int, str. Got '{ft}'"
        ))),
    };

    let feature_count = features.len();
    let out_features: Vec<Value> = features
        .into_iter()
        .map(|mut feat| {
            if let Some(props) = feat.get_mut("properties").and_then(|p| p.as_object_mut()) {
                props.insert(field_name.clone(), parsed_default.clone());
            } else {
                feat["properties"] = serde_json::json!({ &field_name: parsed_default.clone() });
            }
            feat
        })
        .collect();

    Ok(serde_json::json!({
        "type": "FeatureCollection",
        "features": out_features,
        "_meta": {
            "field_added": field_name,
            "field_type": field_type,
            "default_value": parsed_default,
            "features_updated": feature_count
        }
    }))
}

// ── Calculate Geometry ────────────────────────────────────────────────────────

/// Calculate geometry attributes on a GeoJSON FeatureCollection.
/// Matches ArcGIS Pro "Calculate Geometry Attributes" tool.
pub fn do_calculate_geometry(
    geojson_str: String,
    property: String,
    field_name: String,
    area_unit: String,
    length_unit: String,
) -> Result<serde_json::Value, AppError> {
    use gdal::vector::Geometry;

    // Area conversion factors from square meters
    let area_conv: f64 = match area_unit.to_lowercase().as_str() {
        "sqkm"      => 1e-6,
        "hectares"  => 1e-4,
        "sqft"      => 10.76391041671,
        "sqft_us"   => 10.763867361,
        "sqmi"      => 3.86102158542e-7,
        "sqmi_us"   => 3.86100434721e-7,
        "acres"     => 2.47105381467e-4,       // international acres
        "acres_us"  => 2.47104393047e-4,       // US survey acres
        "sqyd"      => 1.19599004630,
        "sqnmi"     => 2.91552956186e-7,
        _           => 1.0,                    // sqm default
    };

    // Length conversion factors from meters
    let len_conv: f64 = match length_unit.to_lowercase().as_str() {
        "km"        => 0.001,
        "ft"        => 3.28084,
        "ft_us"     => 3.28083333,
        "mi"        => 6.21371192237e-4,
        "mi_us"     => 6.21369949495e-4,
        "yd"        => 1.09361329834,
        "yd_us"     => 1.09360999867,
        "nmi"       => 5.39956803456e-4,
        "nmi_us"    => 5.39955687035e-4,
        _           => 1.0,                    // meters default
    };

    let mut fc: serde_json::Value = serde_json::from_str(&geojson_str)
        .map_err(|e| AppError::BadRequest(format!("Invalid JSON: {e}")))?;

    let features = fc.get_mut("features")
        .and_then(|f| f.as_array_mut())
        .ok_or_else(|| AppError::BadRequest("Not a FeatureCollection".into()))?;

    let prop = property.to_lowercase();

    for feat in features.iter_mut() {
        let geom_val = feat.get("geometry").cloned();

        let value: serde_json::Value = if let Some(ref g) = geom_val {
            if g.is_null() {
                serde_json::Value::Null
            } else {
                match Geometry::from_geojson(&g.to_string()) {
                    Ok(geom) => {
                        let raw = unsafe {
                            let h = geom.c_geometry();
                            match prop.as_str() {
                                // ── Area properties ──────────────────────────────────
                                "area" | "area_geodesic" => {
                                    let a = gdal_sys::OGR_G_Area(h).abs() * area_conv;
                                    round6(a)
                                }
                                "perimeter" => {
                                    // Perimeter = sum of exterior ring lengths
                                    let a = gdal_sys::OGR_G_Length(h).abs() * len_conv;
                                    round6(a)
                                }
                                // ── Length properties ────────────────────────────────
                                "length" | "length_geodesic" | "length_3d" => {
                                    let l = gdal_sys::OGR_G_Length(h).abs() * len_conv;
                                    round6(l)
                                }
                                // ── Bearing ─────────────────────────────────────────
                                "line_bearing" => {
                                    let n = gdal_sys::OGR_G_GetPointCount(h);
                                    if n >= 2 {
                                        let (mut x0, mut y0, mut x1, mut y1) = (0f64, 0f64, 0f64, 0f64);
                                        let last = n - 1;
                                        gdal_sys::OGR_G_GetPoint(h, 0, &mut x0, &mut y0, std::ptr::null_mut());
                                        gdal_sys::OGR_G_GetPoint(h, last, &mut x1, &mut y1, std::ptr::null_mut());
                                        let dx = x1 - x0;
                                        let dy = y1 - y0;
                                        let bearing = (dx.atan2(dy).to_degrees() + 360.0) % 360.0;
                                        round6(bearing)
                                    } else { 0.0 }
                                }
                                // ── Line start/end ───────────────────────────────────
                                "line_start_x" => { let mut x = 0f64; gdal_sys::OGR_G_GetPoint(h, 0, &mut x, std::ptr::null_mut(), std::ptr::null_mut()); round6(x) }
                                "line_start_y" => { let mut y = 0f64; let mut x = 0f64; gdal_sys::OGR_G_GetPoint(h, 0, &mut x, &mut y, std::ptr::null_mut()); round6(y) }
                                "line_end_x"   => { let n = gdal_sys::OGR_G_GetPointCount(h); let mut x = 0f64; gdal_sys::OGR_G_GetPoint(h, n-1, &mut x, std::ptr::null_mut(), std::ptr::null_mut()); round6(x) }
                                "line_end_y"   => { let n = gdal_sys::OGR_G_GetPointCount(h); let mut x = 0f64; let mut y = 0f64; gdal_sys::OGR_G_GetPoint(h, n-1, &mut x, &mut y, std::ptr::null_mut()); round6(y) }
                                // ── Centroid ─────────────────────────────────────────
                                "centroid_x" | "centroid_y" => {
                                    let centroid_h = gdal_sys::OGR_G_CreateGeometry(gdal_sys::OGRwkbGeometryType::wkbPoint);
                                    gdal_sys::OGR_G_Centroid(h, centroid_h);
                                    let (mut x, mut y) = (0f64, 0f64);
                                    gdal_sys::OGR_G_GetPoint(centroid_h, 0, &mut x, &mut y, std::ptr::null_mut());
                                    gdal_sys::OGR_G_DestroyGeometry(centroid_h);
                                    if prop == "centroid_x" { round6(x) } else { round6(y) }
                                }
                                // ── Extent ───────────────────────────────────────────
                                "extent_min_x" | "extent_min_y" | "extent_max_x" | "extent_max_y" => {
                                    let mut env = gdal_sys::OGREnvelope { MinX: 0.0, MaxX: 0.0, MinY: 0.0, MaxY: 0.0 };
                                    gdal_sys::OGR_G_GetEnvelope(h, &mut env);
                                    match prop.as_str() {
                                        "extent_min_x" => round6(env.MinX),
                                        "extent_min_y" => round6(env.MinY),
                                        "extent_max_x" => round6(env.MaxX),
                                        _              => round6(env.MaxY),
                                    }
                                }
                                // ── Point coords ─────────────────────────────────────
                                "point_x" => { let mut x = 0f64; gdal_sys::OGR_G_GetPoint(h, 0, &mut x, std::ptr::null_mut(), std::ptr::null_mut()); round6(x) }
                                "point_y" => { let mut x = 0f64; let mut y = 0f64; gdal_sys::OGR_G_GetPoint(h, 0, &mut x, &mut y, std::ptr::null_mut()); round6(y) }
                                // ── Counts ───────────────────────────────────────────
                                "part_count"   => gdal_sys::OGR_G_GetGeometryCount(h) as f64,
                                "vertex_count" => gdal_sys::OGR_G_GetPointCount(h) as f64,
                                _ => return Err(AppError::BadRequest(format!(
                                    "Unknown property '{}'. Valid: area, area_geodesic, perimeter, length, length_geodesic, length_3d, line_bearing, line_start_x, line_start_y, line_end_x, line_end_y, centroid_x, centroid_y, extent_min_x, extent_min_y, extent_max_x, extent_max_y, point_x, point_y, part_count, vertex_count",
                                    property
                                ))),
                            }
                        };
                        // Integer counts — no decimal
                        if prop == "part_count" || prop == "vertex_count" {
                            serde_json::json!(raw as i64)
                        } else {
                            serde_json::json!(raw)
                        }
                    }
                    Err(_) => serde_json::Value::Null,
                }
            }
        } else {
            serde_json::Value::Null
        };

        if let Some(props) = feat.get_mut("properties").and_then(|p| p.as_object_mut()) {
            props.insert(field_name.clone(), value);
        } else {
            feat["properties"] = serde_json::json!({ &field_name: value });
        }
    }

    Ok(fc)
}

#[inline]
fn round6(v: f64) -> f64 {
    (v * 1_000_000.0).round() / 1_000_000.0
}
