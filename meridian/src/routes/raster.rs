use axum::{extract::Extension, http::HeaderMap, Json};
use std::collections::BTreeMap;
use std::time::Instant;
use utoipa::ToSchema;

use crate::{
    error::AppError,
    gis::{compute_price, GeoJsonOutput},
    metrics,
    middleware::request_id::RequestId,
    AppState,
};
use crate::gis::raster::{
    run_color_relief, run_contours, run_gdaldem_single, run_raster_calc, RasterInput,
};
use crate::gis::reproject::payment_gate;

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct SingleRasterParams {
    pub file: String,
}

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct ColorReliefParams {
    pub file: String,
    pub color_table: Option<String>,
    pub color_file: Option<String>,
}

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct ContoursParams {
    pub file: String,
    pub interval: Option<f64>,
    pub offset: Option<f64>,
    pub attribute_name: Option<String>,
}

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct RasterCalcParams {
    pub expression: String,
    pub output_format: Option<String>,
}

macro_rules! single_raster_endpoint {
    ($fn_name:ident, $mode:literal, $path:literal, $desc:literal) => {
        #[utoipa::path(
            post,
            path = $path,
            tag = "GIS",
            request_body(
                content_type = "multipart/form-data",
                description = "Multipart form: `file` raster upload",
                content = SingleRasterParams
            ),
            responses(
                (status = 200, description = $desc, body = GeoJsonOutput),
                (status = 400, description = "Bad request"),
                (status = 402, description = "Payment required", body = crate::billing::PaymentRequired),
                (status = 413, description = "Payload too large (>200 MB)"),
                (status = 500, description = "Internal server error")
            )
        )]
        pub async fn $fn_name(
            Extension(RequestId(request_id)): Extension<RequestId>,
            Extension(state): Extension<AppState>,
            headers: HeaderMap,
            mut multipart: axum::extract::Multipart,
        ) -> Result<Json<GeoJsonOutput>, AppError> {
            let mut file_input: Option<RasterInput> = None;
            while let Some(mut field) = multipart
                .next_field()
                .await
                .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
            {
                if matches!(field.name(), Some("file")) {
                    file_input = Some(RasterInput::from_multipart_field(&mut field).await?);
                }
            }

            let input = file_input.ok_or_else(|| AppError::BadRequest("Missing 'file' field".into()))?;
            let price = compute_price(input.size);
            let t0 = Instant::now();
            metrics::record_request($mode, "received");
            payment_gate($mode, input.size, price, &request_id, &headers, &state).await?;

            let out = run_gdaldem_single($mode, &input, &[], "tif", "image/tiff").await?;

            metrics::record_request($mode, "ok");
            metrics::record_request_duration($mode, t0.elapsed().as_secs_f64());

            Ok(Json(GeoJsonOutput {
                request_id,
                price_usd: price,
                result: out.as_json_value(),
            }))
        }
    };
}

single_raster_endpoint!(hillshade, "hillshade", "/v1/hillshade", "Hillshade raster output");
single_raster_endpoint!(slope, "slope", "/v1/slope", "Slope raster output");
single_raster_endpoint!(aspect, "aspect", "/v1/aspect", "Aspect raster output");
single_raster_endpoint!(roughness, "roughness", "/v1/roughness", "Roughness raster output");

#[utoipa::path(
    post,
    path = "/v1/color-relief",
    tag = "GIS",
    request_body(
        content_type = "multipart/form-data",
        description = "Multipart form: `file` raster upload plus `color_table` text or `color_file` upload",
        content = ColorReliefParams
    ),
    responses(
        (status = 200, description = "Color relief raster output", body = GeoJsonOutput),
        (status = 400, description = "Bad request"),
        (status = 402, description = "Payment required", body = crate::billing::PaymentRequired),
        (status = 413, description = "Payload too large (>200 MB)"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn color_relief(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let mut file_input: Option<RasterInput> = None;
    let mut color_table: Option<String> = None;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        match field.name() {
            Some("file") => file_input = Some(RasterInput::from_multipart_field(&mut field).await?),
            Some("color_table") => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("color_table: {e}")))?;
                if !v.trim().is_empty() {
                    color_table = Some(v);
                }
            }
            Some("color_file") => {
                let upload = RasterInput::from_multipart_field(&mut field).await?;
                let text = String::from_utf8(upload.bytes)
                    .map_err(|_| AppError::BadRequest("color_file must be valid UTF-8 text".into()))?;
                if !text.trim().is_empty() {
                    color_table = Some(text);
                }
            }
            _ => {}
        }
    }

    let input = file_input.ok_or_else(|| AppError::BadRequest("Missing 'file' field".into()))?;
    let color_table = color_table.ok_or_else(|| {
        AppError::BadRequest("Missing color table. Provide 'color_table' text or 'color_file' upload".into())
    })?;

    let price = compute_price(input.size);
    let t0 = Instant::now();
    metrics::record_request("color-relief", "received");
    payment_gate("color-relief", input.size, price, &request_id, &headers, &state).await?;
    let out = run_color_relief(&input, &color_table).await?;
    metrics::record_request("color-relief", "ok");
    metrics::record_request_duration("color-relief", t0.elapsed().as_secs_f64());

    Ok(Json(GeoJsonOutput {
        request_id,
        price_usd: price,
        result: out.as_json_value(),
    }))
}

#[utoipa::path(
    post,
    path = "/v1/contours",
    tag = "GIS",
    request_body(
        content_type = "multipart/form-data",
        description = "Multipart form: `file` raster upload, optional `interval`, `offset`, `attribute_name`",
        content = ContoursParams
    ),
    responses(
        (status = 200, description = "Contour GeoJSON output", body = GeoJsonOutput),
        (status = 400, description = "Bad request"),
        (status = 402, description = "Payment required", body = crate::billing::PaymentRequired),
        (status = 413, description = "Payload too large (>200 MB)"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn contours(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let mut file_input: Option<RasterInput> = None;
    let mut interval: Option<f64> = None;
    let mut offset: Option<f64> = None;
    let mut attribute_name: Option<String> = None;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        match field.name() {
            Some("file") => file_input = Some(RasterInput::from_multipart_field(&mut field).await?),
            Some("interval") => {
                let v = field.text().await.map_err(|e| AppError::BadRequest(format!("interval: {e}")))?;
                if !v.trim().is_empty() {
                    interval = Some(v.trim().parse::<f64>().map_err(|_| AppError::BadRequest("interval must be a number > 0".into()))?);
                }
            }
            Some("offset") => {
                let v = field.text().await.map_err(|e| AppError::BadRequest(format!("offset: {e}")))?;
                if !v.trim().is_empty() {
                    offset = Some(v.trim().parse::<f64>().map_err(|_| AppError::BadRequest("offset must be a number".into()))?);
                }
            }
            Some("attribute_name") => {
                let v = field.text().await.map_err(|e| AppError::BadRequest(format!("attribute_name: {e}")))?;
                if !v.trim().is_empty() {
                    attribute_name = Some(v.trim().to_string());
                }
            }
            _ => {}
        }
    }

    let input = file_input.ok_or_else(|| AppError::BadRequest("Missing 'file' field".into()))?;
    let price = compute_price(input.size);
    let t0 = Instant::now();
    metrics::record_request("contours", "received");
    payment_gate("contours", input.size, price, &request_id, &headers, &state).await?;
    let out = run_contours(&input, interval, offset, attribute_name).await?;
    metrics::record_request("contours", "ok");
    metrics::record_request_duration("contours", t0.elapsed().as_secs_f64());

    Ok(Json(GeoJsonOutput {
        request_id,
        price_usd: price,
        result: out.as_json_value(),
    }))
}

#[utoipa::path(
    post,
    path = "/v1/raster-calc",
    tag = "GIS",
    request_body(
        content_type = "multipart/form-data",
        description = "Multipart form: rasters named A-Z, required `expression`, optional `output_format`",
        content = RasterCalcParams
    ),
    responses(
        (status = 200, description = "Raster calc GeoTIFF output", body = GeoJsonOutput),
        (status = 400, description = "Bad request"),
        (status = 402, description = "Payment required", body = crate::billing::PaymentRequired),
        (status = 413, description = "Payload too large (>200 MB)"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn raster_calc(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let mut rasters: BTreeMap<char, RasterInput> = BTreeMap::new();
    let mut expression: Option<String> = None;
    let mut output_format: Option<String> = None;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        match field.name() {
            Some("expression") => {
                let v = field.text().await.map_err(|e| AppError::BadRequest(format!("expression: {e}")))?;
                if !v.trim().is_empty() {
                    expression = Some(v.trim().to_string());
                }
            }
            Some("output_format") => {
                let v = field.text().await.map_err(|e| AppError::BadRequest(format!("output_format: {e}")))?;
                if !v.trim().is_empty() {
                    output_format = Some(v.trim().to_string());
                }
            }
            Some(name) if is_raster_slot(name) => {
                let key = name.chars().next().unwrap();
                rasters.insert(key, RasterInput::from_multipart_field(&mut field).await?);
            }
            _ => {}
        }
    }

    if rasters.is_empty() {
        return Err(AppError::BadRequest(
            "Missing raster inputs. Provide one or more files named A through Z".into(),
        ));
    }
    let expression = expression.ok_or_else(|| AppError::BadRequest("Missing 'expression' field".into()))?;
    validate_expression_inputs(&expression, &rasters)?;

    let total_size: usize = rasters.values().map(|r| r.size).sum();
    let price = compute_price(total_size);
    let t0 = Instant::now();
    metrics::record_request("raster-calc", "received");
    payment_gate("raster-calc", total_size, price, &request_id, &headers, &state).await?;
    let out = run_raster_calc(&rasters, &expression, output_format.as_deref()).await?;
    metrics::record_request("raster-calc", "ok");
    metrics::record_request_duration("raster-calc", t0.elapsed().as_secs_f64());

    Ok(Json(GeoJsonOutput {
        request_id,
        price_usd: price,
        result: out.as_json_value(),
    }))
}

fn is_raster_slot(name: &str) -> bool {
    name.len() == 1 && matches!(name.chars().next(), Some('A'..='Z'))
}

fn validate_expression_inputs(
    expression: &str,
    rasters: &BTreeMap<char, RasterInput>,
) -> Result<(), AppError> {
    let used: std::collections::BTreeSet<char> = expression
        .chars()
        .filter(|c| c.is_ascii_uppercase())
        .collect();

    for letter in &used {
        if !rasters.contains_key(letter) {
            return Err(AppError::BadRequest(format!(
                "Expression references raster '{letter}' but no '{letter}' file was uploaded"
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raster_slot_detection_only_accepts_a_to_z() {
        assert!(is_raster_slot("A"));
        assert!(is_raster_slot("Z"));
        assert!(!is_raster_slot("a"));
        assert!(!is_raster_slot("AA"));
        assert!(!is_raster_slot("file"));
    }

    #[test]
    fn expression_validation_catches_missing_inputs() {
        let rasters = BTreeMap::new();
        let err = validate_expression_inputs("(A+B)/2", &rasters).unwrap_err();
        match err {
            AppError::BadRequest(msg) => assert!(msg.contains("Expression references raster 'A'")),
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
