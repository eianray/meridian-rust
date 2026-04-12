use axum::{extract::Extension, http::HeaderMap, response::IntoResponse, Json};
use std::collections::BTreeMap;
use std::time::Instant;
use utoipa::ToSchema;
use std::path::PathBuf;

use crate::{
    error::AppError,
    gis::{compute_price, GeoJsonOutput},
    metrics,
    middleware::request_id::RequestId,
    AppState,
};
use crate::gis::raster::{
    run_color_relief, run_contours, run_gdaldem_single, run_gdaldem_slope_pct,
    run_mosaic, run_raster_calc, run_raster_convert, run_raster_to_vector, RasterInput,
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
single_raster_endpoint!(aspect, "aspect", "/v1/aspect", "Aspect raster output");
single_raster_endpoint!(roughness, "roughness", "/v1/roughness", "Roughness raster output");

/// Slope endpoint with optional `percent` flag.
/// If `percent = "true"`, slope is returned as percent instead of degrees.
#[utoipa::path(
    post,
    path = "/v1/slope",
    tag = "GIS",
    request_body(
        content_type = "multipart/form-data",
        description = "Multipart form: `file` raster upload, optional `percent` (\"true\" for percent output)",
        content = SingleRasterParams
    ),
    responses(
        (status = 200, description = "Slope raster output", body = GeoJsonOutput),
        (status = 400, description = "Bad request"),
        (status = 402, description = "Payment required", body = crate::billing::PaymentRequired),
        (status = 413, description = "Payload too large (>200 MB)"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn slope(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let mut file_input: Option<RasterInput> = None;
    let mut percent_flag = false;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        match field.name() {
            Some("file") => {
                file_input = Some(RasterInput::from_multipart_field(&mut field).await?);
            }
            Some("percent") => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("percent: {e}")))?;
                percent_flag = v.trim().eq_ignore_ascii_case("true");
            }
            _ => {}
        }
    }

    let input = file_input.ok_or_else(|| AppError::BadRequest("Missing 'file' field".into()))?;
    let price = compute_price(input.size);
    let t0 = Instant::now();
    metrics::record_request("slope", "received");
    payment_gate("slope", input.size, price, &request_id, &headers, &state).await?;

    let out = if percent_flag {
        run_gdaldem_slope_pct(&input).await?
    } else {
        run_gdaldem_single("slope", &input, &["-compute_edges".to_string()], "tif", "image/tiff").await?
    };

    metrics::record_request("slope", "ok");
    metrics::record_request_duration("slope", t0.elapsed().as_secs_f64());

    Ok(Json(GeoJsonOutput {
        request_id,
        price_usd: price,
        result: out.as_json_value(),
    }))
}

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
    let mut output_type: Option<String> = None;

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
            Some("output_type") => {
                let v = field.text().await.map_err(|e| AppError::BadRequest(format!("output_type: {e}")))?;
                if !v.trim().is_empty() {
                    output_type = Some(v.trim().to_string());
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
    let out = run_raster_calc(&rasters, &expression, output_format.as_deref(), output_type.as_deref()).await?;
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

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct RasterConvertParams {
    pub file: String,
    pub output_format: String,
}

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct MosaicParams {
    pub file_1: String,
    pub file_2: String,
    pub output_crs: Option<String>,
    pub resolution: Option<f64>,
    pub resampling: Option<String>,
    pub nodata: Option<f64>,
}

/// Parse mosaic field name like "file_1" and return the numeric index.
/// Only accepts file_1 through file_10 (no leading zeros, no file_0).
fn parse_mosaic_field_index(name: &str) -> Option<usize> {
    let suffix = name.strip_prefix("file_")?;
    // Reject leading zeros and zero index
    if suffix.starts_with('0') {
        return None;
    }
    let n: usize = suffix.parse().ok()?;
    if n >= 1 && n <= 10 {
        Some(n)
    } else {
        None
    }
}

#[utoipa::path(
    post,
    path = "/v1/convert/raster",
    tag = "GIS",
    request_body(
        content_type = "multipart/form-data",
        description = "Multipart form: `file` raster upload, `output_format` (GTiff, PNG, JPEG, AAIGrid)",
        content = RasterConvertParams
    ),
    responses(
        (status = 200, description = "Converted raster output", body = GeoJsonOutput),
        (status = 400, description = "Bad request"),
        (status = 402, description = "Payment required", body = crate::billing::PaymentRequired),
        (status = 413, description = "Payload too large (>200 MB)"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn raster_convert(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let mut file_input: Option<RasterInput> = None;
    let mut output_format: Option<String> = None;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        match field.name() {
            Some("file") => file_input = Some(RasterInput::from_multipart_field(&mut field).await?),
            Some("output_format") => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("output_format: {e}")))?;
                if !v.trim().is_empty() {
                    output_format = Some(v.trim().to_string());
                }
            }
            _ => {}
        }
    }

    let input = file_input.ok_or_else(|| AppError::BadRequest("Missing 'file' field".into()))?;
    let output_format = output_format.ok_or_else(|| AppError::BadRequest("Missing 'output_format' field".into()))?;

    let price = compute_price(input.size);
    let t0 = Instant::now();
    metrics::record_request("raster-convert", "received");
    payment_gate("raster-convert", input.size, price, &request_id, &headers, &state).await?;
    let out = run_raster_convert(&input, &output_format).await?;
    metrics::record_request("raster-convert", "ok");
    metrics::record_request_duration("raster-convert", t0.elapsed().as_secs_f64());

    Ok(Json(GeoJsonOutput {
        request_id,
        price_usd: price,
        result: out.as_json_value(),
    }))
}

#[utoipa::path(
    post,
    path = "/v1/mosaic",
    tag = "GIS",
    request_body(
        content_type = "multipart/form-data",
        description = "Multipart form: file_1 through file_N (2-10 rasters), optional output_crs, resolution, resampling (nearest/bilinear/cubic), nodata",
        content = MosaicParams
    ),
    responses(
        (status = 200, description = "Mosaicked GeoTIFF output", body = GeoJsonOutput),
        (status = 400, description = "Bad request"),
        (status = 402, description = "Payment required", body = crate::billing::PaymentRequired),
        (status = 413, description = "Payload too large (>200 MB)"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn mosaic(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let mut inputs: Vec<(usize, RasterInput)> = Vec::new();
    let mut output_crs: Option<String> = None;
    let mut resolution: Option<f64> = None;
    let mut resampling: String = "nearest".to_string();
    let mut nodata: Option<f64> = None;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        match field.name() {
            Some(name) if name.starts_with("file_") => {
                if let Some(idx) = parse_mosaic_field_index(name) {
                    inputs.push((idx, RasterInput::from_multipart_field(&mut field).await?));
                } else if name == "file_0" {
                    return Err(AppError::BadRequest(
                        "Invalid field name 'file_0'. Use file_1, file_2, ... file_10".into(),
                    ));
                }
                // Silently ignore other invalid file_* names
            }
            Some("output_crs") => {
                let v = field.text().await.map_err(|e| AppError::BadRequest(format!("output_crs: {e}")))?;
                if !v.trim().is_empty() {
                    output_crs = Some(v.trim().to_string());
                }
            }
            Some("resolution") => {
                let v = field.text().await.map_err(|e| AppError::BadRequest(format!("resolution: {e}")))?;
                if !v.trim().is_empty() {
                    resolution = Some(v.trim().parse::<f64>().map_err(|_| {
                        AppError::BadRequest("resolution must be a positive number".into())
                    })?);
                }
            }
            Some("resampling") => {
                let v = field.text().await.map_err(|e| AppError::BadRequest(format!("resampling: {e}")))?;
                if !v.trim().is_empty() {
                    resampling = v.trim().to_string();
                }
            }
            Some("nodata") => {
                let v = field.text().await.map_err(|e| AppError::BadRequest(format!("nodata: {e}")))?;
                if !v.trim().is_empty() {
                    nodata = Some(v.trim().parse::<f64>().map_err(|_| {
                        AppError::BadRequest("nodata must be a number".into())
                    })?);
                }
            }
            _ => {}
        }
    }

    if inputs.len() < 2 {
        return Err(AppError::BadRequest("Mosaic requires at least 2 input rasters (file_1, file_2, ...)".into()));
    }

    // Sort by numeric suffix so order of arrival doesn't matter
    inputs.sort_by_key(|(idx, _)| *idx);
    let inputs: Vec<RasterInput> = inputs.into_iter().map(|(_, r)| r).collect();

    let total_size: usize = inputs.iter().map(|r| r.size).sum();
    let price = compute_price(total_size);
    let t0 = Instant::now();
    metrics::record_request("mosaic", "received");
    payment_gate("mosaic", total_size, price, &request_id, &headers, &state).await?;
    let out = run_mosaic(&inputs, output_crs.as_deref(), resolution, &resampling, nodata).await?;
    metrics::record_request("mosaic", "ok");
    metrics::record_request_duration("mosaic", t0.elapsed().as_secs_f64());

    Ok(Json(GeoJsonOutput {
        request_id,
        price_usd: price,
        result: out.as_json_value(),
    }))
}

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct RasterToVectorParams {
    pub file: String,
    pub band: Option<u8>,
    pub field_name: Option<String>,
    pub no_data: Option<f64>,
}

#[utoipa::path(
    post,
    path = "/v1/raster-to-vector",
    tag = "GIS",
    request_body(
        content_type = "multipart/form-data",
        description = "Multipart form: `file` raster upload, optional `band` (1-based, default 1), `field_name` (default DN), `no_data` value to exclude",
        content = RasterToVectorParams
    ),
    responses(
        (status = 200, description = "Polygonized GeoJSON output", body = GeoJsonOutput),
        (status = 400, description = "Bad request"),
        (status = 402, description = "Payment required", body = crate::billing::PaymentRequired),
        (status = 413, description = "Payload too large (>200 MB)"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn raster_to_vector(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let mut file_input: Option<RasterInput> = None;
    let mut band: Option<u8> = None;
    let mut field_name: Option<String> = None;
    let mut no_data: Option<f64> = None;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        match field.name() {
            Some("file") => file_input = Some(RasterInput::from_multipart_field(&mut field).await?),
            Some("band") => {
                let v = field.text().await.map_err(|e| AppError::BadRequest(format!("band: {e}")))?;
                if !v.trim().is_empty() {
                    band = Some(v.trim().parse::<u8>().map_err(|_| {
                        AppError::BadRequest("band must be a positive integer".into())
                    })?);
                }
            }
            Some("field_name") => {
                let v = field.text().await.map_err(|e| AppError::BadRequest(format!("field_name: {e}")))?;
                if !v.trim().is_empty() {
                    field_name = Some(v.trim().to_string());
                }
            }
            Some("no_data") => {
                let v = field.text().await.map_err(|e| AppError::BadRequest(format!("no_data: {e}")))?;
                if !v.trim().is_empty() {
                    no_data = Some(v.trim().parse::<f64>().map_err(|_| {
                        AppError::BadRequest("no_data must be a number".into())
                    })?);
                }
            }
            _ => {}
        }
    }

    let input = file_input.ok_or_else(|| AppError::BadRequest("Missing 'file' field".into()))?;
    let price = compute_price(input.size);
    let t0 = Instant::now();
    metrics::record_request("raster-to-vector", "received");
    payment_gate("raster-to-vector", input.size, price, &request_id, &headers, &state).await?;
    let out = run_raster_to_vector(&input, band, field_name.as_deref(), no_data).await?;
    metrics::record_request("raster-to-vector", "ok");
    metrics::record_request_duration("raster-to-vector", t0.elapsed().as_secs_f64());

    Ok(Json(GeoJsonOutput {
        request_id,
        price_usd: price,
        result: out.as_json_value(),
    }))
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

// ── Raster Warp (reproject) ──────────────────────────────────────────────────

#[derive(serde::Deserialize, ToSchema)]
#[allow(dead_code)]
pub struct RasterWarpParams {
    /// Input raster GeoTIFF
    pub file: String,
    /// Target CRS (e.g. "EPSG:3338")
    pub target_crs: String,
}

/// Reproject a raster GeoTIFF to a target CRS using gdalwarp.
#[utoipa::path(
    post,
    path = "/v1/raster-warp",
    tag = "GIS",
    request_body(
        content_type = "multipart/form-data",
        description = "Multipart form: `file` (GeoTIFF), `target_crs` (e.g. \"EPSG:3338\")",
        content = RasterWarpParams
    ),
    responses(
        (status = 200, description = "Warped GeoTIFF", body = GeoJsonOutput),
        (status = 400, description = "Bad request"),
        (status = 402, description = "Payment required", body = crate::billing::PaymentRequired),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn raster_warp(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let mut file_input: Option<RasterInput> = None;
    let mut target_crs: Option<String> = None;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        match field.name() {
            Some("file") => {
                file_input = Some(RasterInput::from_multipart_field(&mut field).await?);
            }
            Some("target_crs") => {
                let v = field.text().await.map_err(|e| AppError::BadRequest(format!("target_crs: {e}")))?;
                if !v.trim().is_empty() { target_crs = Some(v.trim().to_string()); }
            }
            _ => {}
        }
    }

    let input = file_input.ok_or_else(|| AppError::BadRequest("Missing 'file' field".into()))?;
    let crs = target_crs.ok_or_else(|| AppError::BadRequest("Missing 'target_crs' field".into()))?;

    let price = crate::gis::compute_price(input.size);
    let t0 = Instant::now();
    metrics::record_request("raster-warp", "received");
    payment_gate("raster-warp", input.size, price, &request_id, &headers, &state).await?;

    let bytes = input.bytes.clone();
    let out = tokio::task::spawn_blocking(move || run_raster_warp_sync(&bytes, &crs))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Thread panic: {e}")))?
        .map_err(|e| e)?;

    metrics::record_request("raster-warp", "ok");
    metrics::record_request_duration("raster-warp", t0.elapsed().as_secs_f64());

    Ok(Json(GeoJsonOutput {
        request_id,
        price_usd: price,
        result: out.as_json_value(),
    }))
}

fn run_raster_warp_sync(input_bytes: &[u8], target_crs: &str) -> Result<crate::gis::raster::RasterCommandOutput, AppError> {
    use tempfile::TempDir;
    use std::process::Command;

    let tmp = TempDir::new().map_err(|e| AppError::Internal(anyhow::anyhow!("TempDir: {e}")))?;
    let in_path  = tmp.path().join("input.tif");
    let out_path = tmp.path().join("warped.tif");

    std::fs::write(&in_path, input_bytes)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Write input: {e}")))?;

    let status = Command::new("gdalwarp")
        .args([
            "-t_srs", target_crs,
            "-r", "bilinear",
            "-of", "GTiff",
            in_path.to_str().unwrap(),
            out_path.to_str().unwrap(),
        ])
        .status()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("gdalwarp exec: {e}")))?;

    if !status.success() {
        return Err(AppError::Internal(anyhow::anyhow!("gdalwarp failed with status: {}", status)));
    }

    if !out_path.exists() {
        return Err(AppError::Internal(anyhow::anyhow!("gdalwarp completed but output not created")));
    }

    let out_bytes = std::fs::read(&out_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Read output: {e}")))?;

    Ok(crate::gis::raster::RasterCommandOutput {
        stats: crate::gis::raster::RasterOpStats {
            tool: "raster-warp".to_string(),
            input_count: 1,
            input_size_bytes: input_bytes.len(),
            output_size_bytes: out_bytes.len(),
        },
        bytes: out_bytes,
        filename: "warped.tif".into(),
        mime_type: "image/tiff".to_string(),
    })
}

// ═══════════════════════════════════════════════════════════════════════════
//  POST /v1/elevation/fetch-dggs
//  Fetches raw IfSAR DTM from Alaska DGGS elevation portal (elevation.alaska.gov)
//  Multipart params:
//    geojson: string  (GeoJSON Polygon in WGS84 — the AOI)
// ═══════════════════════════════════════════════════════════════════════════

pub async fn fetch_elevation_dggs(
    Extension(RequestId(_request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    _headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let mut geojson_str: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?    
    {
        let name = field.name().unwrap_or("").to_string();
        let text = field.text().await.map_err(|e| AppError::BadRequest(format!("Field read: {e}")))?
;
        match name.as_str() {
            "geojson" => geojson_str = Some(text),
            _ => {}
        }
    }

    let geojson_str = geojson_str
        .ok_or_else(|| AppError::BadRequest("Missing geojson field".into()))?;

    // Validate it parses as JSON
    let _geojson_val: serde_json::Value = serde_json::from_str(&geojson_str)
        .map_err(|e| AppError::BadRequest(format!("Invalid GeoJSON: {e}")))?;

    // Create async job
    let job_id = state.job_store.create();
    let job_id_clone = job_id.clone();
    let semaphore = state.dggs_semaphore.clone();
    let job_store = state.job_store.clone();

    // Spawn background task
    tokio::spawn(async move {
        let _permit = semaphore.acquire_owned().await.unwrap();
        job_store.set_running(&job_id_clone);
        match run_fetch_dggs_sync(&geojson_str).await {
            Ok(bytes) => job_store.complete(&job_id_clone, bytes),
            Err(e) => job_store.fail(&job_id_clone, e.to_string()),
        }
    });

    Ok((axum::http::StatusCode::ACCEPTED, axum::Json(serde_json::json!({"job_id": job_id}))))
}

// ── GET /v1/jobs/:id/status ───────────────────────────────────────────────────

/// Returns the current status of a job (pending / running / complete / failed / not_found).
/// Always returns HTTP 200.
pub async fn job_status(
    Extension(state): Extension<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> axum::Json<serde_json::Value> {
    let (status, error) = state.job_store.get_status(&id);
    match error {
        Some(e) => axum::Json(serde_json::json!({"status": status, "error": e})),
        None => axum::Json(serde_json::json!({"status": status})),
    }
}

// ── GET /v1/jobs/:id/result ───────────────────────────────────────────────────

/// Returns the GeoTIFF result if the job is complete.
/// Consumes the result (one-shot retrieval).
pub async fn job_result(
    Extension(state): Extension<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl axum::response::IntoResponse {
    let (status, _) = state.job_store.get_status(&id);
    match status.as_str() {
        "complete" => {
            match state.job_store.take_result(&id) {
                Some(bytes) => (
                    axum::http::StatusCode::OK,
                    [(axum::http::header::CONTENT_TYPE, "image/tiff")],
                    bytes,
                ).into_response(),
                None => (
                    axum::http::StatusCode::NOT_FOUND,
                    axum::Json(serde_json::json!({"error": "job not found or already retrieved"}))
                ).into_response(),
            }
        }
        "pending" | "running" => (
            axum::http::StatusCode::from_u16(425).unwrap(),
            axum::Json(serde_json::json!({"error": "not ready"}))
        ).into_response(),
        _ => (
            axum::http::StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({"error": "job not found or already retrieved"}))
        ).into_response(),
    }
}

async fn run_fetch_dggs_sync(geojson_str: &str) -> Result<Vec<u8>, AppError> {
    use std::process::Command;
    use tempfile::TempDir;
    use futures_util::StreamExt;

    let dggs_base = "https://elevation.alaska.gov";

    // ── 1. Query DGGS portal for intersecting datasets ──────────────────────
    // Short timeout for the metadata query only
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("reqwest build: {e}")))?;
    // No timeout for the download client — tiles can be 50–200 MB
    let dl_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("reqwest dl_client build: {e}")))?;

    let encoded_geojson = percent_encode(geojson_str.as_bytes());
    let body = format!("geojson={}", encoded_geojson);

    let query_resp = client
        .post(format!("{dggs_base}/query.json"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Origin", dggs_base)
        .header("Referer", format!("{dggs_base}/"))
        .header("User-Agent", "Mozilla/5.0 (compatible; Meridian/1.0)")
        .body(body)
        .send()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("DGGS query failed: {e}")))?;

    if !query_resp.status().is_success() {
        return Err(AppError::Internal(anyhow::anyhow!(
            "DGGS query.json HTTP {}", query_resp.status()
        )));
    }

    #[derive(serde::Deserialize)]
    struct DggsDataset {
        dataset_id: u64,
        dataset_name: String,
        project_name: String,
        files: u64,
    }

    let datasets: Vec<DggsDataset> = query_resp.json().await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("DGGS query parse: {e}")))?;

    // Find IfSAR DTM dataset (project_name="IFSAR", dataset_name contains "DTM")
    let dtm = datasets.iter()
        .find(|d| d.project_name == "IFSAR" && d.dataset_name.to_uppercase().contains("DTM"))
        .ok_or_else(|| AppError::BadRequest(
            "No IfSAR DTM coverage available for this area. Only areas covered by the Alaska DGGS IfSAR survey are supported.".into()
        ))?;

    let dataset_id = dtm.dataset_id;
    let files_count = dtm.files;
    tracing::info!("DGGS: found IfSAR DTM dataset_id={dataset_id}, {files_count} tile(s)");

    /// Extract a bounding-box polygon from any valid GeoJSON geometry.
    /// Walks coordinate arrays to find min/max lon/lat across all rings,
    /// regardless of nesting depth (Polygon, MultiPolygon, Feature, FeatureCollection).
    fn extract_bbox_polygon(geojson_str: &str) -> Result<String, AppError> {
        use serde_json::Value;

        let v: Value = serde_json::from_str(geojson_str)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("bbox parse: {e}")))?;

        let mut min_lon = f64::INFINITY;
        let mut max_lon = f64::NEG_INFINITY;
        let mut min_lat = f64::INFINITY;
        let mut max_lat = f64::NEG_INFINITY;

        fn walk_value(v: &Value, min_lon: &mut f64, max_lon: &mut f64, min_lat: &mut f64, max_lat: &mut f64) {
            match v {
                Value::Array(arr) => {
                    if arr.len() == 2 {
                        if let (Some(&Value::Number(ref lon)), Some(&Value::Number(ref lat))) = (arr.get(0), arr.get(1)) {
                            if let (Some(lon_f), Some(lat_f)) = (lon.as_f64(), lat.as_f64()) {
                                if lon_f < *min_lon { *min_lon = lon_f; }
                                if lon_f > *max_lon { *max_lon = lon_f; }
                                if lat_f < *min_lat { *min_lat = lat_f; }
                                if lat_f > *max_lat { *max_lat = lat_f; }
                                return;
                            }
                        }
                    }
                    for item in arr { walk_value(item, min_lon, max_lon, min_lat, max_lat); }
                }
                Value::Object(obj) => { for val in obj.values() { walk_value(val, min_lon, max_lon, min_lat, max_lat); } }
                _ => {}
            }
        }

        walk_value(&v, &mut min_lon, &mut max_lon, &mut min_lat, &mut max_lat);

        if min_lon == f64::INFINITY {
            return Err(AppError::Internal(anyhow::anyhow!("No coordinates found in GeoJSON")));
        }

        let bbox = format!(
            "{{\"type\":\"Polygon\",\"coordinates\":[[[{},{}],[{},{}],[{},{}],[{},{}],[{},{}]]]}}",
            min_lon, min_lat, max_lon, min_lat, max_lon, max_lat, min_lon, max_lat, min_lon, min_lat
        );
        Ok(bbox)
    }


    // ── 2. Download tiles from DGGS portal ──────────────────────────────────
    // POST with bbox (4-point envelope) to avoid HTTP 414 on complex polygons.
    // The download endpoint only needs a simple envelope to select the right tile(s).
    // (Full polygon in query.json above is correct — only the download needs bbox.)
    let bbox_geojson = extract_bbox_polygon(geojson_str)?;
    let encoded_bbox = percent_encode(bbox_geojson.as_bytes());
    let download_body = format!("geojson={}&ids={}", encoded_bbox, dataset_id);
    tracing::info!("DGGS: POSTing download with bbox for dataset_id={}", dataset_id);

    let dl_resp = dl_client
        .post(format!("{dggs_base}/download"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Origin", dggs_base)
        .header("Referer", format!("{dggs_base}/"))
        .header("User-Agent", "Mozilla/5.0 (compatible; Meridian/1.0)")
        .body(download_body)
        .send()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("DGGS download failed: {e}")))?;

    if !dl_resp.status().is_success() {
        return Err(AppError::Internal(anyhow::anyhow!(
            "DGGS download HTTP {}", dl_resp.status()
        )));
    }

    // ── 3. Unpack outer zip, then inner per-tile zips ────────────────────────
    // Stream download to disk instead of buffering in memory — tiles can be 50-200 MB
    let tmp = TempDir::new()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("TempDir: {e}")))?;

    let zip_path = tmp.path().join("download.zip");
    {
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::File::create(&zip_path).await
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Create zip file: {e}")))?;
        let mut stream = dl_resp.bytes_stream();
        let mut downloaded: u64 = 0;
        loop {
            match stream.next().await {
                Some(Ok(chunk)) => {
                    downloaded += chunk.len() as u64;
                    file.write_all(&chunk).await
                        .map_err(|e| AppError::Internal(anyhow::anyhow!("Write zip chunk: {e}")))?;
                }
                Some(Err(e)) => {
                    return Err(AppError::Internal(anyhow::anyhow!("DGGS download stream: {e}")));
                }
                None => break,
            }
        }
        tracing::info!("DGGS: download complete ({} MB)", downloaded / 1_048_576);
    }
    let zip_bytes = std::fs::read(&zip_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Read zip file: {e}")))?;

    let mut tif_paths: Vec<std::path::PathBuf> = Vec::new();

    {
        use std::io::Cursor;
        let cursor = Cursor::new(&zip_bytes[..]);
        let mut outer = zip::ZipArchive::new(cursor)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Outer zip open: {e}")))?;

        for i in 0..outer.len() {
            let mut entry = outer.by_index(i)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Outer zip entry {i}: {e}")))?;
            let name = entry.name().to_string();
            if !name.ends_with(".zip") { continue; }

            // Read inner zip bytes
            let mut inner_bytes = Vec::new();
            use std::io::Read;
            entry.read_to_end(&mut inner_bytes)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Read inner zip {name}: {e}")))?;

            // Unpack inner zip, extract only .tif files
            let inner_cursor = Cursor::new(inner_bytes);
            let mut inner = zip::ZipArchive::new(inner_cursor)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Inner zip open {name}: {e}")))?;

            for j in 0..inner.len() {
                let mut f = inner.by_index(j)
                    .map_err(|e| AppError::Internal(anyhow::anyhow!("Inner zip entry {j}: {e}")))?;
                let fname = f.name().to_string();
                if !fname.to_lowercase().ends_with(".tif") { continue; }

                let stem = std::path::Path::new(&fname)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("tile.tif")
                    .to_string();
                let tif_path = tmp.path().join(&stem);
                let mut out_file = std::fs::File::create(&tif_path)
                    .map_err(|e| AppError::Internal(anyhow::anyhow!("Create tif {stem}: {e}")))?;
                std::io::copy(&mut f, &mut out_file)
                    .map_err(|e| AppError::Internal(anyhow::anyhow!("Write tif {stem}: {e}")))?;
                tif_paths.push(tif_path);
                tracing::info!("DGGS: extracted {stem}");
            }
        }
    }

    if tif_paths.is_empty() {
        return Err(AppError::Internal(anyhow::anyhow!("DGGS: no TIF files found in download")));
    }

    // ── 4. Merge multiple tiles if needed ───────────────────────────────────
    let merged_path = tmp.path().join("merged.tif");
    if tif_paths.len() == 1 {
        std::fs::copy(&tif_paths[0], &merged_path)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Copy single tile: {e}")))?;
    } else {
        // gdal_merge.py -o merged.tif tile1.tif tile2.tif ...
        let mut args: Vec<&str> = vec!["-o", merged_path.to_str().unwrap()];
        let tile_strs: Vec<String> = tif_paths.iter()
            .map(|p| p.to_str().unwrap().to_string())
            .collect();
        for t in &tile_strs { args.push(t.as_str()); }
        let status = Command::new("gdal_merge.py")
            .args(&args)
            .status()
            .map_err(|e| AppError::Internal(anyhow::anyhow!("gdal_merge exec: {e}")))?;
        if !status.success() {
            return Err(AppError::Internal(anyhow::anyhow!("gdal_merge failed: {}", status)));
        }
        tracing::info!("DGGS: merged {} tiles", tif_paths.len());
    }

    // ── 5. Clip merged raster to AOI bbox using gdalwarp -te + -te_srs ─────────
    // Use bbox clip with -te_srs EPSG:4326 (same approach as run_clip_s3_tile_sync).
    // The cutline approach requires GDAL to reproject a polygon cutline CRS which
    // is unreliable here; -te_srs handles the WGS84→EPSG:3338 conversion correctly.
    let bbox_str = extract_bbox_polygon(geojson_str)?;
    let bbox_v: serde_json::Value = serde_json::from_str(&bbox_str)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Parse clip bbox: {e}")))?;
    let coords = bbox_v["coordinates"][0].as_array()
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("Clip bbox coords missing")))?;
    let min_lon = coords[0][0].as_f64().unwrap_or(0.0);
    let min_lat = coords[0][1].as_f64().unwrap_or(0.0);
    let max_lon = coords[2][0].as_f64().unwrap_or(0.0);
    let max_lat = coords[2][1].as_f64().unwrap_or(0.0);

    let clipped_path = tmp.path().join("clipped.tif");
    let status = Command::new("gdalwarp")
        .args([
            "-te", &min_lon.to_string(), &min_lat.to_string(),
                    &max_lon.to_string(), &max_lat.to_string(),
            "-te_srs", "EPSG:4326",
            "-dstnodata", "-9999",
            "-of", "GTiff",
            "-co", "COMPRESS=LZW",
            merged_path.to_str().unwrap(),
            clipped_path.to_str().unwrap(),
        ])
        .status()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("gdalwarp exec: {e}")))?;

    if !status.success() {
        return Err(AppError::Internal(anyhow::anyhow!("gdalwarp clip failed: {}", status)));
    }

    let bytes = std::fs::read(&clipped_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Read clipped: {e}")))?;
    Ok(bytes)
}

/// URL-encode a byte slice (percent-encoding, form-safe)
fn percent_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    for &b in input {
        match b {
            b'A'..=b'Z' | b'0'..=b'9' | b'a'..=b'z'
            | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

// ═══════════════════════════════════════════════════════════════════════════

