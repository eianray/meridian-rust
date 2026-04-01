// PaymentRequired is used in utoipa response annotations (proc-macro context)
#[allow(unused_imports)]
use crate::billing::PaymentRequired;

use axum::{extract::Extension, http::HeaderMap, Json};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use utoipa::ToSchema;

use crate::{
    billing::{build_payment_required_with_price, log_dev_operation, verify_payment, PaymentError},
    error::AppError,
    gis::{
        buffer::do_buffer_blocking,
        clip::do_clip_blocking,
        dissolve::do_dissolve_blocking,
        reproject::{do_reproject_blocking, GDAL_SEMAPHORE},
        compute_price, validate_geojson_bytes, MAX_FILE_BYTES,
    },
    metrics,
    middleware::request_id::RequestId,
    AppState,
};

use std::time::Duration;
use tokio::time::timeout;

const BATCH_LIMIT: usize = 50;
const OP_TIMEOUT: Duration = Duration::from_secs(30);

// ── Request / Response types ──────────────────────────────────────────────────

/// A single operation in a batch request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct BatchOperation {
    /// Operation type
    pub op_type: BatchOpType,
    /// Named reference to a file field in the multipart form (e.g. "file0", "mask0")
    pub file_field: String,
    /// For clip operations: the name of the mask file field
    pub mask_field: Option<String>,
    /// For reproject: target CRS
    pub target_crs: Option<String>,
    /// Optional source CRS (default EPSG:4326)
    pub source_crs: Option<String>,
    /// For buffer: distance in meters
    pub distance: Option<f64>,
    /// For dissolve: optional attribute field to group by
    pub dissolve_field: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum BatchOpType {
    Reproject,
    Buffer,
    Clip,
    Dissolve,
}

/// Result for a single batch operation.
#[derive(Debug, Serialize, ToSchema)]
pub struct BatchResult {
    pub index: usize,
    pub op_type: String,
    pub price_usd: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Batch endpoint response
#[derive(Debug, Serialize, ToSchema)]
pub struct BatchResponse {
    pub request_id: String,
    pub total_price_usd: f64,
    pub count: usize,
    pub results: Vec<BatchResult>,
}

// ── Handler ────────────────────────────────────────────────────────────────────

/// Process up to 50 GIS operations in a single request.
///
/// Accepts `multipart/form-data` with:
/// - `operations`: JSON array of `BatchOperation` objects
/// - Named file fields (referenced by `file_field` / `mask_field` in each operation)
///
/// A single `X-PAYMENT` header must cover the **total** price (sum of all `compute_price` calls).
/// Operations are processed sequentially (GDAL semaphore respected).
#[utoipa::path(
    post,
    path = "/v1/batch",
    tag = "GIS",
    request_body(
        content_type = "multipart/form-data",
        description = "Multipart form: `operations` (JSON array of BatchOperation), plus named GeoJSON file fields",
        content = BatchOperation
    ),
    responses(
        (status = 200, description = "Batch results — one result per input operation", body = BatchResponse),
        (status = 400, description = "Bad request — invalid operations JSON, missing fields"),
        (status = 402, description = "Payment required", body = PaymentRequired),
        (status = 413, description = "Payload too large (>200 MB per file)"),
        (status = 429, description = "Rate limit exceeded — 60 requests/min per IP"),
        (status = 500, description = "Internal server error"),
    )
)]
pub async fn batch(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(state): Extension<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<BatchResponse>, AppError> {
    // Collect all multipart fields first (operations JSON + file bytes)
    let mut operations_json: Option<String> = None;
    let mut files: std::collections::HashMap<String, Vec<u8>> = std::collections::HashMap::new();

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();

        if name == "operations" {
            operations_json = Some(
                field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("Error reading 'operations' field: {e}")))?,
            );
        } else {
            // Treat everything else as a file field
            let mut buf: Vec<u8> = Vec::new();
            while let Some(chunk) = field
                .chunk()
                .await
                .map_err(|e| AppError::BadRequest(format!("Error reading file field '{name}': {e}")))?
            {
                if buf.len() + chunk.len() > MAX_FILE_BYTES {
                    return Err(AppError::PayloadTooLarge);
                }
                buf.extend_from_slice(&chunk);
            }
            if !buf.is_empty() {
                files.insert(name, buf);
            }
        }
    }

    let ops_str = operations_json
        .ok_or_else(|| AppError::BadRequest("Missing 'operations' JSON field".into()))?;
    let operations: Vec<BatchOperation> = serde_json::from_str(&ops_str)
        .map_err(|e| AppError::BadRequest(format!("Invalid 'operations' JSON: {e}")))?;

    if operations.is_empty() {
        return Err(AppError::BadRequest("'operations' array is empty".into()));
    }
    if operations.len() > BATCH_LIMIT {
        return Err(AppError::BadRequest(format!(
            "Too many operations: {}/{BATCH_LIMIT}",
            operations.len()
        )));
    }

    let request_start = Instant::now();
    metrics::record_request("batch", "received");

    // Compute total price (sum over all operations)
    let total_price: f64 = operations.iter().map(|op| {
        let size = match &op.op_type {
            BatchOpType::Clip => {
                let f = files.get(&op.file_field).map(|b| b.len()).unwrap_or(0);
                let m = op.mask_field.as_ref().and_then(|mf| files.get(mf)).map(|b| b.len()).unwrap_or(0);
                f + m
            }
            _ => files.get(&op.file_field).map(|b| b.len()).unwrap_or(0),
        };
        compute_price(size)
    }).sum();

    // Payment gate (single header covers total)
    let payment_result = batch_payment_gate(
        total_price,
        operations.len(),
        &request_id,
        &headers,
        &state,
    ).await;
    match &payment_result {
        Ok(_) => metrics::record_payment("batch", if state.config.dev_mode { "dev" } else { "success" }),
        Err(_) => metrics::record_payment("batch", "failed"),
    }
    payment_result?;

    // Process operations sequentially
    let mut results: Vec<BatchResult> = Vec::with_capacity(operations.len());

    for (idx, op) in operations.iter().enumerate() {
        let file_bytes = files.get(&op.file_field).cloned().unwrap_or_default();
        let file_size = file_bytes.len();
        let price = compute_price(file_size);

        let op_result = process_single_op(op, file_bytes, &files).await;

        results.push(match op_result {
            Ok(value) => BatchResult {
                index: idx,
                op_type: op_type_str(&op.op_type).into(),
                price_usd: price,
                result: Some(value),
                error: None,
            },
            Err(e) => BatchResult {
                index: idx,
                op_type: op_type_str(&op.op_type).into(),
                price_usd: price,
                result: None,
                error: Some(e.to_string()),
            },
        });
    }

    // Log to ops table if DB available
    if let Some(db) = &state.db {
        let x_payment = headers.get("x-payment").and_then(|v| v.to_str().ok()).map(|s| s.to_string());
        let status = if state.config.dev_mode { "dev" } else { "ok" };
        if let Err(e) = sqlx::query(
            "INSERT INTO operations_log (request_id, operation, file_size_bytes, price_usd, tx_signature, status) \
             VALUES ($1, 'batch', $2, $3, $4, $5)"
        )
        .bind(&request_id)
        .bind(0_i64) // aggregate
        .bind(total_price)
        .bind(x_payment.as_deref())
        .bind(status)
        .execute(db)
        .await {
            tracing::warn!("ops_log insert failed: {e}");
        }
    }

    metrics::record_request("batch", "ok");
    metrics::record_request_duration("batch", request_start.elapsed().as_secs_f64());

    Ok(Json(BatchResponse {
        request_id,
        total_price_usd: total_price,
        count: results.len(),
        results,
    }))
}

// ── Payment gate for batch ────────────────────────────────────────────────────

async fn batch_payment_gate(
    total_price: f64,
    op_count: usize,
    request_id: &str,
    headers: &HeaderMap,
    state: &AppState,
) -> Result<(), AppError> {
    let x_payment = headers
        .get("x-payment")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    if state.config.dev_mode {
        if let Some(db) = &state.db {
            log_dev_operation(request_id, "batch", op_count, total_price, db).await;
        }
        return Ok(());
    }

    let wallet = state
        .config
        .wallet_address
        .as_deref()
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!(
            "WALLET_ADDRESS not set - cannot accept payments in production mode"
        )))?;

    let resource_url = "https://api.meridian.tools/v1/batch".to_string();

    let payment_header = match x_payment {
        None => {
            let body = build_payment_required_with_price("batch", total_price, wallet, &resource_url);
            return Err(AppError::PaymentRequired { body });
        }
        Some(h) => h,
    };

    let db = state.db.as_ref().ok_or_else(|| {
        AppError::Internal(anyhow::anyhow!("DATABASE_URL required for payment verification"))
    })?;

    verify_payment(
        &payment_header,
        "batch",
        &resource_url,
        wallet,
        &state.config.x402_facilitator_url,
        op_count,
        total_price,
        request_id,
        db,
    )
    .await
    .map(|_payer| ())
    .map_err(|e| match e {
        PaymentError::AlreadyUsed => AppError::BadRequest("Payment already used".into()),
        PaymentError::NotFound => AppError::BadRequest("Transaction not found or not yet confirmed".into()),
        PaymentError::InsufficientAmount { expected, received } => AppError::BadRequest(
            format!("Insufficient payment for batch: expected {expected} USDC atomic, received {received}"),
        ),
        PaymentError::InvalidPayload => AppError::BadRequest("Invalid X-PAYMENT payload".into()),
        PaymentError::FacilitatorError(msg) => AppError::Internal(anyhow::anyhow!("Facilitator error: {msg}")),
        PaymentError::DbError(msg) => AppError::Internal(anyhow::anyhow!("DB error: {msg}")),
    })
}

// ── Per-operation dispatch ────────────────────────────────────────────────────

async fn process_single_op(
    op: &BatchOperation,
    file_bytes: Vec<u8>,
    files: &std::collections::HashMap<String, Vec<u8>>,
) -> Result<serde_json::Value, AppError> {
    let geojson_str = validate_geojson_bytes(&file_bytes)?;
    let src_crs = op.source_crs.clone().unwrap_or_else(|| "EPSG:4326".to_string());

    let _permit: tokio::sync::SemaphorePermit<'_> = GDAL_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Semaphore error: {e}")))?;

    let op_name = op_type_str(&op.op_type);
    let gdal_start = Instant::now();

    let result = match &op.op_type {
        BatchOpType::Reproject => {
            let target_crs = op
                .target_crs
                .clone()
                .ok_or_else(|| AppError::BadRequest("Reproject requires 'target_crs'".into()))?;
            if target_crs.trim().is_empty() {
                return Err(AppError::BadRequest("'target_crs' cannot be empty".into()));
            }
            timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
                do_reproject_blocking(geojson_str, src_crs, target_crs)
            }))
            .await
            .map_err(|_| AppError::Timeout)?
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Thread panic: {e}")))?
        }
        BatchOpType::Buffer => {
            let distance = op
                .distance
                .ok_or_else(|| AppError::BadRequest("Buffer requires 'distance'".into()))?;
            timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
                do_buffer_blocking(geojson_str, distance, src_crs)
            }))
            .await
            .map_err(|_| AppError::Timeout)?
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Thread panic: {e}")))?
        }
        BatchOpType::Clip => {
            let mask_field = op
                .mask_field
                .as_ref()
                .ok_or_else(|| AppError::BadRequest("Clip requires 'mask_field'".into()))?;
            let mask_bytes = files
                .get(mask_field)
                .cloned()
                .ok_or_else(|| AppError::BadRequest(format!("Mask field '{mask_field}' not found in upload")))?;
            let mask_str = validate_geojson_bytes(&mask_bytes)?;
            timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
                do_clip_blocking(geojson_str, mask_str, src_crs)
            }))
            .await
            .map_err(|_| AppError::Timeout)?
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Thread panic: {e}")))?
        }
        BatchOpType::Dissolve => {
            let dissolve_field = op.dissolve_field.as_ref().filter(|f| !f.is_empty()).cloned();
            timeout(OP_TIMEOUT, tokio::task::spawn_blocking(move || {
                do_dissolve_blocking(geojson_str, dissolve_field, src_crs)
            }))
            .await
            .map_err(|_| AppError::Timeout)?
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Thread panic: {e}")))?
        }
    };

    metrics::record_gdal_duration(op_name, gdal_start.elapsed().as_secs_f64());
    result
}

fn op_type_str(t: &BatchOpType) -> &'static str {
    match t {
        BatchOpType::Reproject => "reproject",
        BatchOpType::Buffer => "buffer",
        BatchOpType::Clip => "clip",
        BatchOpType::Dissolve => "dissolve",
    }
}
