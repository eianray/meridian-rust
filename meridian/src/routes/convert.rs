use axum::{extract::Extension, http::HeaderMap, Json};
use std::time::{Duration, Instant};
use tokio::time::timeout;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use utoipa::ToSchema;

use crate::{
    error::AppError,
    gis::{
        compute_price,
        convert::do_convert,
        GeoJsonOutput,
    },
    metrics,
    middleware::request_id::RequestId,
    AppState,
};
use crate::gis::reproject::{payment_gate, GDAL_SEMAPHORE};

const OP_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(ToSchema)]
#[allow(dead_code)]
pub struct ConvertParams {
    pub file: String,
    pub output_format: Option<String>,
}

/// Input file for conversion - accepts any vector format
pub struct ConvertInput {
    pub bytes: Vec<u8>,
    pub size: usize,
    pub filename: String,
    pub mime_type: Option<String>,
}

impl ConvertInput {
    pub async fn from_multipart_field(
        field: &mut axum::extract::multipart::Field<'_>,
    ) -> Result<Self, AppError> {
        use crate::gis::MAX_FILE_BYTES;

        let filename = field
            .file_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "input.geojson".to_string());
        
        let mime_type = field.content_type().map(|s| s.to_string());

        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|e| AppError::BadRequest(format!("Error reading upload: {e}")))?
        {
            if buf.len() + chunk.len() > MAX_FILE_BYTES {
                return Err(AppError::PayloadTooLarge);
            }
            buf.extend_from_slice(&chunk);
        }

        if buf.is_empty() {
            return Err(AppError::BadRequest("Empty file".into()));
        }

        let size = buf.len();
        Ok(Self { bytes: buf, size, filename, mime_type })
    }
}

#[utoipa::path(
    post,
    path = "/v1/convert",
    tag = "GIS",
    request_body(
        content_type = "multipart/form-data",
        description = "Multipart form: `file` upload, optional `output_format`",
        content = ConvertParams
    ),
    responses(
        (status = 200, description = "Converted file output", body = GeoJsonOutput),
        (status = 400, description = "Bad request"),
        (status = 402, description = "Payment required", body = crate::billing::PaymentRequired),
        (status = 413, description = "Payload too large (>200 MB)"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn convert(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<GeoJsonOutput>, AppError> {
    let mut file_input: Option<ConvertInput> = None;
    let mut output_format: Option<String> = None;

    while let Some(mut field) = multipart.next_field().await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        match field.name() {
            Some("file") => {
                file_input = Some(ConvertInput::from_multipart_field(&mut field).await?);
            }
            Some("output_format") => {
                let v = field.text().await
                    .map_err(|e| AppError::BadRequest(format!("output_format: {e}")))?;
                if !v.trim().is_empty() {
                    output_format = Some(v.trim().to_string());
                }
            }
            _ => {}
        }
    }

    let input = file_input.ok_or_else(|| AppError::BadRequest("Missing 'file' field".into()))?;
    let price = compute_price(input.size);
    let t0 = Instant::now();
    metrics::record_request("convert", "received");
    payment_gate("convert", input.size, price, &request_id, &headers, &state).await?;

    let _permit = GDAL_SEMAPHORE.acquire().await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Semaphore: {e}")))?;

    let (out_bytes, out_filename, mime_type) = timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
        do_convert(input.bytes, input.filename, input.mime_type, output_format)
    })).await
        .map_err(|_| AppError::BadRequest("Timed out".into()))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?
        .map_err(|e: AppError| e)?;

    metrics::record_request("convert", "ok");
    metrics::record_request_duration("convert", t0.elapsed().as_secs_f64());

    if mime_type == "application/geo+json" {
        let result: serde_json::Value = serde_json::from_slice(&out_bytes)
            .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(&out_bytes).to_string()));
        return Ok(Json(GeoJsonOutput { request_id, price_usd: price, result }));
    }

    // Binary: base64 encode
    let encoded = STANDARD.encode(&out_bytes);
    let result = serde_json::json!({
        "filename": out_filename,
        "mime_type": mime_type,
        "encoding": "base64",
        "data": encoded
    });

    Ok(Json(GeoJsonOutput { request_id, price_usd: price, result }))
}
