use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use thiserror::Error;
use tracing::error;
use utoipa::ToSchema;

use crate::gis::compute_price;

// ── Constants ─────────────────────────────────────────────────────────────────

pub const USDC_BASE_CONTRACT: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
pub const X402_VERSION: u32 = 1;

/// USDC has 6 decimal places. $1.00 = 1_000_000 atomic units.
const USDC_ATOMIC_PER_USD: f64 = 1_000_000.0;

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum PaymentError {
    #[error("Transaction not found or not yet confirmed")]
    NotFound,

    #[error("Insufficient payment: expected ≥{expected} atomic USDC, got {received}")]
    InsufficientAmount { expected: u64, received: u64 },

    #[error("Signature already used")]
    AlreadyUsed,

    #[error("Database error: {0}")]
    DbError(String),

    #[error("Facilitator error: {0}")]
    FacilitatorError(String),

    #[error("Invalid payment payload")]
    InvalidPayload,
}

// ── 402 response body ─────────────────────────────────────────────────────────

/// Body returned in 402 Payment Required responses (x402 spec).
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PaymentRequired {
    pub x402_version: u32,
    pub error: String,
    pub accepts: Vec<PaymentAccept>,
}

/// A single payment option in the x402 `accepts` array.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PaymentAccept {
    pub scheme: String,
    pub network: String,
    pub max_amount_required: String,
    pub resource: String,
    pub description: String,
    pub mime_type: String,
    pub pay_to: String,
    pub max_timeout_seconds: u32,
    pub asset: String,
}

// ── Facilitator response ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct FacilitatorResponse {
    #[serde(rename = "isValid")]
    is_valid: bool,
    payer: Option<String>,
    #[allow(dead_code)]
    transaction: Option<String>,
    error: Option<String>,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Build a 402 response body with an explicit USD price (for batch where total != single-file price).
pub fn build_payment_required_with_price(
    operation: &str,
    price_usd: f64,
    wallet_address: &str,
    resource_url: &str,
) -> PaymentRequired {
    let atomic = usd_to_atomic(price_usd);
    PaymentRequired {
        x402_version: X402_VERSION,
        error: "Payment required".into(),
        accepts: vec![PaymentAccept {
            scheme: "exact".into(),
            network: "base".into(),
            max_amount_required: atomic.to_string(),
            resource: resource_url.to_string(),
            description: format!("Meridian: {operation}"),
            mime_type: "application/octet-stream".into(),
            pay_to: wallet_address.to_string(),
            max_timeout_seconds: 300,
            asset: USDC_BASE_CONTRACT.to_string(),
        }],
    }
}

/// Build the 402 response body for a given operation and file size.
pub fn build_payment_required(
    operation: &str,
    file_size_bytes: usize,
    wallet_address: &str,
    resource_url: &str,
) -> PaymentRequired {
    let price_usd = compute_price(file_size_bytes);
    let atomic = usd_to_atomic(price_usd);

    PaymentRequired {
        x402_version: X402_VERSION,
        error: "Payment required".into(),
        accepts: vec![PaymentAccept {
            scheme: "exact".into(),
            network: "base".into(),
            max_amount_required: atomic.to_string(),
            resource: resource_url.to_string(),
            description: format!("Meridian: {operation}"),
            mime_type: "application/octet-stream".into(),
            pay_to: wallet_address.to_string(),
            max_timeout_seconds: 300,
            asset: USDC_BASE_CONTRACT.to_string(),
        }],
    }
}

/// Convert USD price to USDC atomic units (6 decimals).
pub fn usd_to_atomic(price_usd: f64) -> u64 {
    (price_usd * USDC_ATOMIC_PER_USD).round() as u64
}

/// Verify an x402 payment via the configured facilitator.
///
/// The facilitator is the canonical verifier, replay protector, and settlement
/// boundary for x402/Base USDC payments. Meridian trusts the facilitator verdict
/// and records payer/audit metadata for its own operation log.
/// Returns the payer's EVM address on success.
#[allow(clippy::too_many_arguments)]
pub async fn verify_payment(
    payment_header: &str,
    operation: &str,
    resource_url: &str,
    wallet_address: &str,
    facilitator_url: &str,
    file_size_bytes: usize,
    price_usd: f64,
    request_id: &str,
    db: &PgPool,
) -> Result<String, PaymentError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| PaymentError::FacilitatorError(e.to_string()))?;

    verify_payment_with_client(
        payment_header,
        operation,
        resource_url,
        wallet_address,
        file_size_bytes,
        price_usd,
        request_id,
        db,
        &client,
        facilitator_url,
    )
    .await
}

/// Same as `verify_payment` but takes an explicit `reqwest::Client` and facilitator URL
/// — useful in tests to point at a mock server.
#[allow(clippy::too_many_arguments)]
pub async fn verify_payment_with_client(
    payment_header: &str,
    operation: &str,
    resource_url: &str,
    wallet_address: &str,
    file_size_bytes: usize,
    price_usd: f64,
    request_id: &str,
    db: &PgPool,
    client: &reqwest::Client,
    facilitator_url: &str,
) -> Result<String, PaymentError> {
    let atomic = usd_to_atomic(price_usd);

    // Decode the base64 payment payload into JSON
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
    let padded = {
        let rem = payment_header.len() % 4;
        if rem == 0 {
            payment_header.to_string()
        } else {
            format!("{}{}", payment_header, "=".repeat(4 - rem))
        }
    };
    let decoded_bytes = B64.decode(&padded).map_err(|_| PaymentError::InvalidPayload)?;
    let payload: serde_json::Value =
        serde_json::from_slice(&decoded_bytes).map_err(|_| PaymentError::InvalidPayload)?;

    let body = serde_json::json!({
        "x402Version": X402_VERSION,
        "paymentPayload": payload,
        "paymentRequirements": {
            "scheme": "exact",
            "network": "base",
            "maxAmountRequired": atomic.to_string(),
            "resource": resource_url,
            "payTo": wallet_address,
            "asset": USDC_BASE_CONTRACT,
            "maxTimeoutSeconds": 300
        }
    });

    // Meridian does not perform canonical on-chain verification here; it sends
    // the x402 payload to the configured facilitator and trusts that verdict.

    let resp = client
        .post(facilitator_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| PaymentError::FacilitatorError(format!("HTTP error: {e}")))?;

    let status = resp.status();
    let fac_resp: FacilitatorResponse = resp
        .json()
        .await
        .map_err(|e| PaymentError::FacilitatorError(format!("JSON decode error: {e}")))?;

    if !status.is_success() {
        return Err(PaymentError::FacilitatorError(format!(
            "Facilitator returned {status}: {}",
            fac_resp.error.unwrap_or_default()
        )));
    }

    if !fac_resp.is_valid {
        let msg = fac_resp.error.unwrap_or_else(|| "unknown".into());
        // Map known error strings to typed variants
        let msg_lower = msg.to_lowercase();
        if msg_lower.contains("already used") || msg_lower.contains("duplicate") || msg_lower.contains("replay") {
            return Err(PaymentError::AlreadyUsed);
        }
        if msg_lower.starts_with("insufficient") {
            return Err(PaymentError::InsufficientAmount {
                expected: atomic,
                received: 0,
            });
        }
        return Err(PaymentError::FacilitatorError(msg));
    }

    let payer = fac_resp.payer.unwrap_or_else(|| "unknown".into());

    // Log facilitator-confirmed payer/audit metadata to operations_log.
    // Legacy note: `tx_signature` currently stores payment reference / payer
    // metadata, not necessarily an on-chain transaction hash.
    let insert_result = sqlx::query(
        "INSERT INTO operations_log (request_id, operation, file_size_bytes, price_usd, tx_signature, status) \
         VALUES ($1, $2, $3, $4, $5, 'ok')"
    )
    .bind(request_id)
    .bind(operation)
    .bind(file_size_bytes as i64)
    .bind(price_usd)
    .bind(&payer)
    .execute(db)
    .await;

    if let Err(e) = insert_result {
        error!(error = %e, request_id, "Failed to insert operations_log — proceeding anyway");
        tracing::warn!("payment_log_failure: audit record lost for request {}", request_id);
    }

    Ok(payer)
}

/// Log a dev-mode (free) operation to the ops log.
pub async fn log_dev_operation(
    request_id: &str,
    operation: &str,
    file_size_bytes: usize,
    price_usd: f64,
    db: &PgPool,
) {
    let res = sqlx::query(
        "INSERT INTO operations_log (request_id, operation, file_size_bytes, price_usd, tx_signature, status) \
         VALUES ($1, $2, $3, $4, NULL, 'dev')"
    )
    .bind(request_id)
    .bind(operation)
    .bind(file_size_bytes as i64)
    .bind(price_usd)
    .execute(db)
    .await;

    if let Err(e) = res {
        error!(error = %e, request_id, "Failed to insert dev operations_log");
    }
}
