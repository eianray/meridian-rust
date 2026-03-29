use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use axum::{extract::Extension, Json};
use geo::BooleanOps;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::error::AppError;
use crate::middleware::request_id::RequestId;

/// PLSS section lookup parameters.
#[derive(Deserialize, ToSchema)]
pub struct PlssLookupParams {
    /// Two-character state code (e.g. "WA", "OR").
    #[schema(example = "WA")]
    pub state: String,

    /// Principal meridian code as string (zero-padded, e.g. "26" or "03").
    #[schema(example = "26")]
    pub prinmercd: String,

    /// Township number (1–3 digits, e.g. "7" or "007").
    #[schema(example = "7")]
    pub township: String,

    /// Township direction: "N" or "S".
    #[schema(example = "N")]
    pub township_dir: String,

    /// Range number (1–3 digits, e.g. "3" or "003").
    #[schema(example = "3")]
    pub range: String,

    /// Range direction: "E" or "W".
    #[schema(example = "E")]
    pub range_dir: String,

    /// Section number (1 or 2 digits, not zero-padded, e.g. "14").
    #[schema(example = "14")]
    pub section: String,
}

/// PLSS lookup response.
#[derive(Serialize, ToSchema)]
pub struct PlssLookupResponse {
    pub request_id: String,
    pub found: bool,

    /// "sections" if found in sections file, "intersects" if found by dissolving
    /// aliquot parts from the intersects file, null if not found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<&'static str>,

    /// The matching GeoJSON Feature. Null if not found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feature: Option<Value>,
}

/// Normalize a string to uppercase, trimmed.
fn normalize(s: &str) -> String {
    s.trim().to_uppercase()
}

/// Zero-pad a numeric string to `len` characters.
fn zero_pad(s: &str, len: usize) -> String {
    s.trim()
        .parse::<usize>()
        .map_or_else(|_| s.trim().to_string(), |n| format!("{:0>width$}", n, width = len))
}

/// Compare a feature property (normalized) against an expected &str.
fn prop_matches(props: &serde_json::Map<String, Value>, key: &str, expected: &str) -> bool {
    props
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_uppercase())
        .as_deref()
        == Some(expected)
}

/// Build the PLSSID prefix for matching sections features.
/// Format: {STATE}{PRINMERCD 2}{TWNSHPNO 3}{TWNSHPFRAC=0}{TWNSHPDIR 1}{RANGENO 3}{RANGEFRAC=0}{RANGEDIR 1}{TWNSHPDPCD=0}
fn build_plssid(state: &str, prinmercd: &str, township: &str, township_dir: &str, range: &str, range_dir: &str) -> String {
    format!("{}{}{}{}{}{}{}{}{}", state, prinmercd, township, "0", township_dir, range, "0", range_dir, "0")
}

/// Stream a sections GeoJSONL file and return the first feature matching via PLSSID + section number.
fn search_geojsonl(
    path: &PathBuf,
    state: &str,
    prinmercd: &str,
    township: &str,
    township_dir: &str,
    range: &str,
    range_dir: &str,
    section: &str,
    _section_type: &str,
) -> Result<Option<Value>, AppError> {
    let file = File::open(path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Failed to open {:?}: {}", path, e)))?;
    let reader = BufReader::new(file);

    let plssid = build_plssid(state, prinmercd, township, township_dir, range, range_dir);
    let section_norm = section.trim_start_matches('0').to_string();

    for line in reader.lines() {
        let line = line
            .map_err(|e| AppError::Internal(anyhow::anyhow!("IO error reading {:?}: {}", path, e)))?;
        if line.trim().is_empty() {
            continue;
        }
        let feature: Value = match serde_json::from_str(&line) {
            Ok(f) => f,
            Err(_) => continue,
        };

        let props = match feature.get("properties") {
            Some(Value::Object(p)) => p,
            _ => continue,
        };

        // Match on PLSSID prefix + section number
        let feature_plssid = props.get("PLSSID").and_then(|v| v.as_str()).unwrap_or("").to_uppercase();
        let feature_section = props.get("FRSTDIVNO").and_then(|v| v.as_str()).unwrap_or("").trim_start_matches('0').to_string();

        if feature_plssid == plssid && feature_section == section_norm {
            return Ok(Some(feature));
        }
    }

    Ok(None)
}

/// Parse a GeoJSON coordinates array (ring) into geo::Coords.
fn parse_ring(ring: &[Value]) -> Option<Vec<geo::Coord<f64>>> {
    let coords: Vec<geo::Coord<f64>> = ring
        .iter()
        .filter_map(|c| {
            let arr = c.as_array()?;
            let x = arr.get(0)?.as_f64()?;
            let y = arr.get(1)?.as_f64()?;
            Some(geo::Coord { x, y })
        })
        .collect();
    if coords.len() >= 3 {
        Some(coords)
    } else {
        None
    }
}

/// Build a geo::Polygon from a GeoJSON exterior ring array, closing it if needed.
fn ring_to_polygon(ring: &[Value]) -> Option<geo::Polygon<f64>> {
    let mut coords = parse_ring(ring)?;
    if coords.first() != coords.last() {
        coords.push(coords[0]);
    }
    Some(geo::Polygon::new(geo::LineString::new(coords), vec![]))
}

/// Stream intersects GeoJSONL, collect all aliquot polygons for the section, dissolve.
fn dissolve_intersects(
    path: &PathBuf,
    state: &str,
    prinmercd: &str,
    township: &str,
    township_dir: &str,
    range: &str,
    range_dir: &str,
    section: &str,
) -> Result<Option<Value>, AppError> {
    let file = File::open(path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Failed to open {:?}: {}", path, e)))?;
    let reader = BufReader::new(file);

    let mut matching_polys: Vec<geo::Polygon<f64>> = Vec::new();

    for line in reader.lines() {
        let line = line
            .map_err(|e| AppError::Internal(anyhow::anyhow!("IO error reading {:?}: {}", path, e)))?;
        if line.trim().is_empty() {
            continue;
        }
        let feature: Value = match serde_json::from_str(&line) {
            Ok(f) => f,
            Err(_) => continue,
        };

        let props = match feature.get("properties") {
            Some(Value::Object(p)) => p,
            _ => continue,
        };

        let matches = prop_matches(props, "STATEABBR", state)
            && prop_matches(props, "PRINMERCD", prinmercd)
            && prop_matches(props, "TWNSHPNO", township)
            && prop_matches(props, "TWNSHPDIR", township_dir)
            && prop_matches(props, "RANGENO", range)
            && prop_matches(props, "RANGEDIR", range_dir)
            && prop_matches(props, "FRSTDIVNO", section);

        if !matches {
            continue;
        }

        let geom = match feature.get("geometry") {
            Some(g) => g,
            None => continue,
        };

        let geom_type = geom.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let coords = match geom.get("coordinates").and_then(|v| v.as_array()) {
            Some(c) => c,
            None => continue,
        };

        // Handle Polygon: coords = [ exterior_ring, ...holes ]
        // Handle MultiPolygon: coords = [ [ exterior_ring, ...holes ], ... ]
        match geom_type {
            "Polygon" => {
                if let Some(exterior) = coords.first().and_then(|r| r.as_array()) {
                    if let Some(poly) = ring_to_polygon(exterior) {
                        matching_polys.push(poly);
                    }
                }
            }
            "MultiPolygon" => {
                for polygon_rings in coords.iter().filter_map(|p| p.as_array()) {
                    if let Some(exterior) = polygon_rings.first().and_then(|r| r.as_array()) {
                        if let Some(poly) = ring_to_polygon(exterior) {
                            matching_polys.push(poly);
                        }
                    }
                }
            }
            _ => continue,
        }
    }

    if matching_polys.is_empty() {
        return Ok(None);
    }

    // Union all polygons together using BooleanOps
    let dissolved: geo::MultiPolygon<f64> = matching_polys
        .into_iter()
        .fold(geo::MultiPolygon::new(vec![]), |acc, poly| {
            acc.union(&geo::MultiPolygon::new(vec![poly]))
        });

    // Serialize dissolved geometry back to GeoJSON
    let coordinates: Value = if dissolved.0.len() == 1 {
        // Single polygon — return as Polygon
        let poly = &dissolved.0[0];
        let ring: Vec<Value> = poly
            .exterior()
            .coords()
            .map(|c| serde_json::json!([c.x, c.y]))
            .collect();
        serde_json::json!([ring])
    } else {
        // MultiPolygon
        let polys: Vec<Value> = dissolved
            .0
            .iter()
            .map(|poly| {
                let ring: Vec<Value> = poly
                    .exterior()
                    .coords()
                    .map(|c| serde_json::json!([c.x, c.y]))
                    .collect();
                serde_json::json!([ring])
            })
            .collect();
        serde_json::json!(polys)
    };

    let geom_type = if dissolved.0.len() == 1 { "Polygon" } else { "MultiPolygon" };

    let dissolved_feature = serde_json::json!({
        "type": "Feature",
        "properties": {
            "STATEABBR": state,
            "PRINMERCD": prinmercd,
            "TWNSHPNO": township,
            "TWNSHPDIR": township_dir,
            "RANGENO": range,
            "RANGEDIR": range_dir,
            "FRSTDIVNO": section,
            "FRSTDIVTYP": "SN",
            "_dissolved": true
        },
        "geometry": {
            "type": geom_type,
            "coordinates": coordinates
        }
    });

    Ok(Some(dissolved_feature))
}

/// Look up a PLSS section by legal description.
///
/// Searches `{STATE}_sections.geojsonl` first. If not found, falls back to
/// `{STATE}_intersects.geojsonl` and dissolves aliquot parts into one polygon.
///
/// Free endpoint — no payment required.
#[utoipa::path(
    post,
    path = "/v1/plss/lookup",
    tag = "Info",
    request_body = PlssLookupParams,
    responses(
        (status = 200, description = "PLSS lookup result", body = PlssLookupResponse),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Internal server error"),
    )
)]
pub async fn plss_lookup(
    Extension(RequestId(id)): Extension<RequestId>,
    Json(params): Json<PlssLookupParams>,
) -> Result<Json<PlssLookupResponse>, AppError> {
    let data_dir = std::env::var("PLSS_DATA_DIR")
        .unwrap_or_else(|_| "/opt/meridian-rust/plss-data".to_string());

    // Normalize inputs
    let state = normalize(&params.state);
    let prinmercd = normalize(&params.prinmercd);
    let township = zero_pad(&params.township, 3);
    let township_dir = normalize(&params.township_dir);
    let range = zero_pad(&params.range, 3);
    let range_dir = normalize(&params.range_dir);
    let section = normalize(&params.section);

    // 1. Try sections file
    let sections_path = PathBuf::from(&data_dir).join(format!("{}_sections.geojsonl", state));
    if sections_path.exists() {
        if let Some(feature) = search_geojsonl(
            &sections_path,
            &state,
            &prinmercd,
            &township,
            &township_dir,
            &range,
            &range_dir,
            &section,
            "SN",
        )? {
            return Ok(Json(PlssLookupResponse {
                request_id: id,
                found: true,
                source: Some("sections"),
                feature: Some(feature),
            }));
        }
    }

    // 2. Fall back to intersects
    let intersects_path =
        PathBuf::from(&data_dir).join(format!("{}_intersects.geojsonl", state));
    if intersects_path.exists() {
        if let Some(feature) = dissolve_intersects(
            &intersects_path,
            &state,
            &prinmercd,
            &township,
            &township_dir,
            &range,
            &range_dir,
            &section,
        )? {
            return Ok(Json(PlssLookupResponse {
                request_id: id,
                found: true,
                source: Some("intersects"),
                feature: Some(feature),
            }));
        }
    }

    Ok(Json(PlssLookupResponse {
        request_id: id,
        found: false,
        source: None,
        feature: None,
    }))
}
