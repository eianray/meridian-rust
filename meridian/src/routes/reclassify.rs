use axum::{extract::Extension, http::HeaderMap, Json};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use utoipa::ToSchema;

use crate::error::AppError;
use crate::gis::{compute_price, GeoJsonInput, GeoJsonOutput};
use crate::gis::reproject::payment_gate;
use crate::metrics;
use crate::middleware::request_id::RequestId;
use crate::AppState;

#[derive(Deserialize, ToSchema)]
#[allow(dead_code)]
pub struct ReclassifyParams {
    /// GeoJSON FeatureCollection file (≤200 MB)
    pub file: String,
    /// Workflow: "1" (elevation bands) or "2" (slope bands)
    pub workflow: String,
}

#[derive(Serialize, ToSchema)]
#[allow(dead_code)]
pub struct ReclassifyResult {
    #[serde(flatten)]
    pub fc: serde_json::Value,
}

/// Classify features into groups based on gridcode values and workflow.
///
/// Workflow 1 (elevation bands):
///   gridcode 0–299    → group = 1
///   gridcode 300–1000 → group = 2
///   gridcode 1001+    → group = 3
///   gridcode < 0 or -9999 → skip (exclude from output)
///
/// Workflow 2 (slope bands):
///   gridcode 0–9    → group = 1
///   gridcode 10–25 → group = 2
///   gridcode 26+   → group = 3
///   gridcode < 0 or -9999 → skip
#[utoipa::path(
    post,
    path = "/v1/reclassify",
    tag = "GIS",
    request_body(
        content_type = "multipart/form-data",
        description = "Multipart form: `file` (GeoJSON FeatureCollection), `workflow` (\"1\" or \"2\")",
        content = ReclassifyParams
    ),
    responses(
        (status = 200, description = "Reclassified GeoJSON FeatureCollection", body = GeoJsonOutput),
        (status = 400, description = "Bad request — missing file/workflow, invalid GeoJSON, invalid workflow value"),
        (status = 402, description = "Payment required", body = crate::billing::PaymentRequired),
        (status = 413, description = "Payload too large (>200 MB)"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn reclassify(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let mut file_input: Option<GeoJsonInput> = None;
    let mut workflow: Option<String> = None;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        match field.name() {
            Some("file") => {
                file_input = Some(GeoJsonInput::from_multipart_field(&mut field).await?);
            }
            Some("workflow") => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("workflow: {e}")))?;
                if !v.trim().is_empty() {
                    workflow = Some(v.trim().to_string());
                }
            }
            _ => {}
        }
    }

    let input = file_input.ok_or_else(|| AppError::BadRequest("Missing 'file' field".into()))?;
    let workflow = workflow.ok_or_else(|| AppError::BadRequest("Missing 'workflow' field".into()))?;

    let workflow_num = match workflow.as_str() {
        "1" | "2" => workflow.parse::<u8>().unwrap(),
        _ => {
            return Err(AppError::BadRequest(
                "Invalid workflow value. Must be \"1\" (elevation bands) or \"2\" (slope bands)".into(),
            ));
        }
    };

    let price = compute_price(input.size);
    let t0 = Instant::now();
    metrics::record_request("reclassify", "received");
    payment_gate("reclassify", input.size, price, &request_id, &headers, &state).await?;

    let result = do_reclassify(&input.bytes, workflow_num)?;

    metrics::record_request("reclassify", "ok");
    metrics::record_request_duration("reclassify", t0.elapsed().as_secs_f64());

    Ok(Json(GeoJsonOutput {
        request_id,
        price_usd: price,
        result,
    }))
}

fn do_reclassify(bytes: &[u8], workflow: u8) -> Result<serde_json::Value, AppError> {
    let fc: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| AppError::BadRequest(format!("Invalid GeoJSON: {e}")))?;

    let features_array = fc.get("features")
        .and_then(|v| v.as_array())
        .ok_or_else(|| AppError::BadRequest("GeoJSON is not a FeatureCollection with a features array".into()))?;

    let mut out_features: Vec<serde_json::Value> = Vec::with_capacity(features_array.len());

    for feature in features_array {
        let props = match feature.get("properties").and_then(|p| p.as_object()) {
            Some(p) => p,
            None => continue, // skip features without properties
        };

        let gridcode = props.get("gridcode")
            .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)));

        let Some(gc) = gridcode else {
            // Skip features without gridcode
            continue;
        };

        // Skip nodata values
        if gc < 0.0 || gc == -9999.0 {
            continue;
        }

        let group = match workflow {
            1 => {
                // Elevation bands
                if gc <= 299.0 {
                    1
                } else if gc <= 1000.0 {
                    2
                } else {
                    3
                }
            }
            2 => {
                // Slope bands
                if gc <= 9.0 {
                    1
                } else if gc <= 25.0 {
                    2
                } else {
                    3
                }
            }
            _ => unreachable!(),
        };

        // Clone feature and add group property
        let mut feat_value = feature.clone();
        if let Some(props_obj) = feat_value.get_mut("properties").and_then(|p| p.as_object_mut()) {
            props_obj.insert("group".to_string(), serde_json::json!(group));
        }

        out_features.push(feat_value);
    }

    Ok(serde_json::json!({
        "type": "FeatureCollection",
        "features": out_features
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_feature(gridcode: f64) -> Vec<u8> {
        let fc = serde_json::json!({
            "type": "FeatureCollection",
            "features": [{
                "type": "Feature",
                "geometry": { "type": "Point", "coordinates": [0.0, 0.0] },
                "properties": { "gridcode": gridcode }
            }]
        });
        serde_json::to_vec(&fc).unwrap()
    }

    fn make_feature_with_props(gridcode: f64, extra: serde_json::Map<String, serde_json::Value>) -> Vec<u8> {
        let mut props = serde_json::Map::new();
        props.insert("gridcode".to_string(), serde_json::json!(gridcode));
        for (k, v) in extra {
            props.insert(k, v);
        }
        let fc = serde_json::json!({
            "type": "FeatureCollection",
            "features": [{
                "type": "Feature",
                "geometry": { "type": "Point", "coordinates": [0.0, 0.0] },
                "properties": props
            }]
        });
        serde_json::to_vec(&fc).unwrap()
    }

    // Test 1: Workflow 1 — elevation bands
    #[test]
    fn test_reclassify_w1_group_1() {
        let bytes = make_feature(150.0);
        let result = do_reclassify(&bytes, 1).unwrap();
        let feat = &result["features"][0];
        assert_eq!(feat["properties"]["group"], 1);
    }

    #[test]
    fn test_reclassify_w1_group_2() {
        let bytes = make_feature(500.0);
        let result = do_reclassify(&bytes, 1).unwrap();
        let feat = &result["features"][0];
        assert_eq!(feat["properties"]["group"], 2);
    }

    #[test]
    fn test_reclassify_w1_group_3() {
        let bytes = make_feature(2000.0);
        let result = do_reclassify(&bytes, 1).unwrap();
        let feat = &result["features"][0];
        assert_eq!(feat["properties"]["group"], 3);
    }

    // Test 2: Workflow 2 — slope bands
    #[test]
    fn test_reclassify_w2_group_1() {
        let bytes = make_feature(5.0);
        let result = do_reclassify(&bytes, 2).unwrap();
        let feat = &result["features"][0];
        assert_eq!(feat["properties"]["group"], 1);
    }

    #[test]
    fn test_reclassify_w2_group_2() {
        let bytes = make_feature(15.0);
        let result = do_reclassify(&bytes, 2).unwrap();
        let feat = &result["features"][0];
        assert_eq!(feat["properties"]["group"], 2);
    }

    #[test]
    fn test_reclassify_w2_group_3() {
        let bytes = make_feature(30.0);
        let result = do_reclassify(&bytes, 2).unwrap();
        let feat = &result["features"][0];
        assert_eq!(feat["properties"]["group"], 3);
    }

    // Test 3: Nodata handling — gridcode -9999 excluded from output
    #[test]
    fn test_reclassify_nodata_excluded() {
        let bytes = make_feature(-9999.0);
        let result = do_reclassify(&bytes, 1).unwrap();
        assert_eq!(result["features"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_reclassify_negative_excluded() {
        let bytes = make_feature(-50.0);
        let result = do_reclassify(&bytes, 1).unwrap();
        assert_eq!(result["features"].as_array().unwrap().len(), 0);
    }

    // Test 4: Edge cases
    #[test]
    fn test_reclassify_w1_boundary_low() {
        // gridcode 0 → group 1
        let bytes = make_feature(0.0);
        let result = do_reclassify(&bytes, 1).unwrap();
        assert_eq!(result["features"][0]["properties"]["group"], 1);
    }

    #[test]
    fn test_reclassify_w1_boundary_mid() {
        // gridcode 299 → group 1 (0-299 inclusive)
        let bytes = make_feature(299.0);
        let result = do_reclassify(&bytes, 1).unwrap();
        assert_eq!(result["features"][0]["properties"]["group"], 1);

        // gridcode 300 → group 2
        let bytes = make_feature(300.0);
        let result = do_reclassify(&bytes, 1).unwrap();
        assert_eq!(result["features"][0]["properties"]["group"], 2);

        // gridcode 1000 → group 2
        let bytes = make_feature(1000.0);
        let result = do_reclassify(&bytes, 1).unwrap();
        assert_eq!(result["features"][0]["properties"]["group"], 2);

        // gridcode 1001 → group 3
        let bytes = make_feature(1001.0);
        let result = do_reclassify(&bytes, 1).unwrap();
        assert_eq!(result["features"][0]["properties"]["group"], 3);
    }

    #[test]
    fn test_reclassify_w2_boundary_mid() {
        // gridcode 9 → group 1
        let bytes = make_feature(9.0);
        let result = do_reclassify(&bytes, 2).unwrap();
        assert_eq!(result["features"][0]["properties"]["group"], 1);

        // gridcode 10 → group 2
        let bytes = make_feature(10.0);
        let result = do_reclassify(&bytes, 2).unwrap();
        assert_eq!(result["features"][0]["properties"]["group"], 2);

        // gridcode 25 → group 2
        let bytes = make_feature(25.0);
        let result = do_reclassify(&bytes, 2).unwrap();
        assert_eq!(result["features"][0]["properties"]["group"], 2);

        // gridcode 26 → group 3
        let bytes = make_feature(26.0);
        let result = do_reclassify(&bytes, 2).unwrap();
        assert_eq!(result["features"][0]["properties"]["group"], 3);
    }

    // Test 5: Missing gridcode property — feature skipped
    #[test]
    fn test_reclassify_missing_gridcode() {
        let fc = serde_json::json!({
            "type": "FeatureCollection",
            "features": [{
                "type": "Feature",
                "geometry": { "type": "Point", "coordinates": [0.0, 0.0] },
                "properties": { "other_field": 100 }
            }]
        });
        let bytes = serde_json::to_vec(&fc).unwrap();
        let result = do_reclassify(&bytes, 1).unwrap();
        assert_eq!(result["features"].as_array().unwrap().len(), 0);
    }

    // Test 6: Existing properties preserved alongside new group field
    #[test]
    fn test_reclassify_preserves_other_properties() {
        let mut extra = serde_json::Map::new();
        extra.insert("name".to_string(), serde_json::json!("test_feature"));
        let bytes = make_feature_with_props(500.0, extra);
        let result = do_reclassify(&bytes, 1).unwrap();
        let props = &result["features"][0]["properties"];
        assert_eq!(props["name"], "test_feature");
        assert_eq!(props["gridcode"], 500.0);
        assert_eq!(props["group"], 2);
    }

    // Test 7: Multiple features in one request
    #[test]
    fn test_reclassify_multiple_features() {
        let fc = serde_json::json!({
            "type": "FeatureCollection",
            "features": [
                {
                    "type": "Feature",
                    "geometry": { "type": "Point", "coordinates": [0.0, 0.0] },
                    "properties": { "gridcode": 150 }
                },
                {
                    "type": "Feature",
                    "geometry": { "type": "Point", "coordinates": [1.0, 1.0] },
                    "properties": { "gridcode": 500 }
                },
                {
                    "type": "Feature",
                    "geometry": { "type": "Point", "coordinates": [2.0, 2.0] },
                    "properties": { "gridcode": 2000 }
                }
            ]
        });
        let bytes = serde_json::to_vec(&fc).unwrap();
        let result = do_reclassify(&bytes, 1).unwrap();
        let features = result["features"].as_array().unwrap();
        assert_eq!(features.len(), 3);
        assert_eq!(features[0]["properties"]["group"], 1);
        assert_eq!(features[1]["properties"]["group"], 2);
        assert_eq!(features[2]["properties"]["group"], 3);
    }
}
