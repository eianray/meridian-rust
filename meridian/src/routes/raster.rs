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
        run_gdaldem_single("slope", &input, &[], "tif", "image/tiff").await?
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
