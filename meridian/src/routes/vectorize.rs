use axum::{extract::Extension, http::HeaderMap, Json};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::time::Instant;

use crate::{
    error::AppError,
    gis::{compute_price, validate_geojson_bytes, GeoJsonInput, GeoJsonOutput},
    metrics,
    middleware::request_id::RequestId,
    AppState,
};
use crate::gis::reproject::payment_gate;
use crate::gis::vectorize::do_vectorize;

pub async fn vectorize(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let mut file_input: Option<GeoJsonInput> = None;
    let mut layer_name = "data".to_string();
    let mut min_zoom: u8 = 0;
    let mut max_zoom: u8 = 14;
    let mut simplify: bool = true;
    let mut tileset_name: Option<String> = None;
    let mut description: Option<String> = None;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        match field.name() {
            Some("file") => {
                file_input = Some(GeoJsonInput::from_multipart_field(&mut field).await?);
            }
            Some("layer_name") => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("layer_name: {e}")))?;
                if !v.trim().is_empty() {
                    layer_name = v.trim().to_string();
                }
            }
            Some("min_zoom") => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("min_zoom: {e}")))?;
                if !v.trim().is_empty() {
                    min_zoom = v.trim().parse::<u8>().map_err(|_| {
                        AppError::BadRequest("min_zoom must be an integer 0–16".into())
                    })?;
                }
            }
            Some("max_zoom") => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("max_zoom: {e}")))?;
                if !v.trim().is_empty() {
                    max_zoom = v.trim().parse::<u8>().map_err(|_| {
                        AppError::BadRequest("max_zoom must be an integer 0–16".into())
                    })?;
                }
            }
            Some("simplify") => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("simplify: {e}")))?;
                simplify = !matches!(v.trim().to_lowercase().as_str(), "false" | "0" | "no");
            }
            Some("name") => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("name: {e}")))?;
                if !v.trim().is_empty() {
                    tileset_name = Some(v.trim().to_string());
                }
            }
            Some("description") => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("description: {e}")))?;
                if !v.trim().is_empty() {
                    description = Some(v.trim().to_string());
                }
            }
            _ => {}
        }
    }

    let input =
        file_input.ok_or_else(|| AppError::BadRequest("Missing 'file' field".into()))?;

    // Validate GeoJSON (UTF-8 + basic JSON structure)
    validate_geojson_bytes(&input.bytes)?;

    let price = compute_price(input.size);
    let t0 = Instant::now();
    metrics::record_request("vectorize", "received");

    payment_gate("vectorize", input.size, price, &request_id, &headers, &state).await?;

    let file_bytes = input.bytes.clone();
    let layer_name_c = layer_name.clone();
    let tileset_name_str = tileset_name.unwrap_or_default();
    let description_str = description.unwrap_or_default();

    let (mbtiles_bytes, out_filename, stats) = do_vectorize(
        &file_bytes,
        &layer_name_c,
        min_zoom,
        max_zoom,
        simplify,
        &tileset_name_str,
        &description_str,
    )
    .await
    .map_err(|e| e)?;

    metrics::record_request("vectorize", "ok");
    metrics::record_request_duration("vectorize", t0.elapsed().as_secs_f64());

    let encoded = STANDARD.encode(&mbtiles_bytes);
    let result = serde_json::json!({
        "data": encoded,
        "filename": out_filename,
        "encoding": "base64",
        "mime_type": "application/x-sqlite3",
        "stats": stats,
    });

    Ok(Json(GeoJsonOutput {
        request_id,
        price_usd: price,
        result,
    }))
}
