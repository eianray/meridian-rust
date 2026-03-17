use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use thiserror::Error;
use tracing::{error, warn};
use utoipa::ToSchema;

use crate::gis::compute_price;

// ── Constants ─────────────────────────────────────────────────────────────────

pub const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
/// USDC has 6 decimal places on Solana. Used for atomic unit conversions.
pub const USDC_DECIMALS: u32 = 6;
/// $1.00 = 1_000_000 atomic units
pub const USDC_ATOMIC_PER_USD: f64 = 1_000_000.0;

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum PaymentError {
    #[error("Transaction not found or not yet confirmed")]
    NotFound,

    #[error("Insufficient payment: expected ≥{expected} atomic USDC, got {received}")]
    InsufficientAmount { expected: u64, received: u64 },

    #[error("Wrong or missing memo: expected 'meridian:{operation}'")]
    WrongMemo { operation: String },

    #[error("Signature already used")]
    AlreadyUsed,

    #[error("Solana RPC error: {0}")]
    RpcError(String),

    #[error("Database error: {0}")]
    DbError(String),
}

// ── 402 response body ─────────────────────────────────────────────────────────

/// Body returned in 402 Payment Required responses.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PaymentRequired {
    pub error: String,
    pub protocol: String,
    pub network: String,
    pub operation: String,
    pub recipient: String,
    pub amount_usd: String,
    pub amount_usdc_atomic: u64,
    pub token: String,
    pub token_mint: String,
    pub memo: String,
    pub solana_pay_url: String,
    pub instructions: String,
}

/// Build the 402 response body for a given operation and file size.
pub fn build_payment_required(
    operation: &str,
    file_size_bytes: usize,
    wallet_address: &str,
) -> PaymentRequired {
    let price_usd = compute_price(file_size_bytes);
    let amount_usdc_atomic = usd_to_atomic(price_usd);
    let amount_usd_str = format!("{price_usd:.6}");
    let memo = format!("meridian:{operation}");

    let amount_usdc_float = amount_usdc_atomic as f64 / USDC_ATOMIC_PER_USD;
    let solana_pay_url = format!(
        "solana:{wallet_address}?amount={amount_usdc_float:.6}&spl-token={USDC_MINT}&label=Meridian+GIS&memo={memo}"
    );

    PaymentRequired {
        error: "Payment required".into(),
        protocol: "solana-pay".into(),
        network: "mainnet-beta".into(),
        operation: operation.into(),
        recipient: wallet_address.into(),
        amount_usd: amount_usd_str.clone(),
        amount_usdc_atomic,
        token: "USDC".into(),
        token_mint: USDC_MINT.into(),
        memo: memo.clone(),
        solana_pay_url,
        instructions: format!(
            "Send {amount_usd_str} USDC on Solana Mainnet to {wallet_address}. \
             Include memo '{memo}'. \
             Retry request with header: X-PAYMENT: <transaction_signature>"
        ),
    }
}

/// Convert USD price to USDC atomic units (6 decimals).
pub fn usd_to_atomic(price_usd: f64) -> u64 {
    (price_usd * USDC_ATOMIC_PER_USD).round() as u64
}

// ── Transaction verification ──────────────────────────────────────────────────

/// Response shape from Solana RPC `getTransaction`.
#[derive(Debug, Deserialize)]
struct RpcResponse {
    result: Option<serde_json::Value>,
    error: Option<serde_json::Value>,
}

/// Verify a Solana USDC payment.
///
/// Checks:
/// 1. `used_signatures` table — reject duplicates
/// 2. `getTransaction` RPC — confirm existence and success
/// 3. USDC token balance delta to recipient ≥ expected_amount_atomic
/// 4. Memo instruction contains `meridian:<operation>`
/// 5. On success: insert into `used_signatures` and `operations_log`
#[allow(clippy::too_many_arguments)]
pub async fn verify_payment(
    tx_sig: &str,
    expected_recipient: &str,
    expected_amount_atomic: u64,
    operation: &str,
    request_id: &str,
    file_size_bytes: usize,
    price_usd: f64,
    rpc_url: &str,
    db: &PgPool,
) -> Result<(), PaymentError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| PaymentError::RpcError(e.to_string()))?;

    verify_payment_with_client(
        tx_sig,
        expected_recipient,
        expected_amount_atomic,
        operation,
        request_id,
        file_size_bytes,
        price_usd,
        rpc_url,
        db,
        &client,
    )
    .await
}

/// Same as `verify_payment` but takes an explicit `reqwest::Client` — useful in tests
/// to point at a wiremock server.
#[allow(clippy::too_many_arguments)]
pub async fn verify_payment_with_client(
    tx_sig: &str,
    expected_recipient: &str,
    expected_amount_atomic: u64,
    operation: &str,
    request_id: &str,
    file_size_bytes: usize,
    price_usd: f64,
    rpc_url: &str,
    db: &PgPool,
    client: &reqwest::Client,
) -> Result<(), PaymentError> {
    // 1. Idempotency check
    let already_used: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM used_signatures WHERE tx_signature = $1)"
    )
    .bind(tx_sig)
    .fetch_one(db)
    .await
    .map_err(|e| {
        error!(error = %e, "DB error checking used_signatures");
        PaymentError::DbError(e.to_string())
    })?;

    if already_used {
        return Err(PaymentError::AlreadyUsed);
    }

    // 2. Fetch transaction from RPC
    let rpc_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getTransaction",
        "params": [
            tx_sig,
            {
                "encoding": "jsonParsed",
                "commitment": "confirmed",
                "maxSupportedTransactionVersion": 0
            }
        ]
    });

    let resp = client
        .post(rpc_url)
        .json(&rpc_body)
        .send()
        .await
        .map_err(|e| PaymentError::RpcError(format!("HTTP error: {e}")))?;

    let rpc_resp: RpcResponse = resp
        .json()
        .await
        .map_err(|e| PaymentError::RpcError(format!("JSON decode error: {e}")))?;

    if let Some(err) = rpc_resp.error {
        return Err(PaymentError::RpcError(format!("RPC error: {err}")));
    }

    let tx = rpc_resp.result.ok_or(PaymentError::NotFound)?;

    // 3. Check transaction succeeded (meta.err == null)
    if !tx["meta"]["err"].is_null() {
        return Err(PaymentError::NotFound);
    }

    // 4. Check USDC token balance delta to recipient
    let received_atomic = extract_usdc_received(&tx, expected_recipient);
    if received_atomic < expected_amount_atomic {
        warn!(
            tx_sig,
            received = received_atomic,
            expected = expected_amount_atomic,
            "Insufficient USDC payment"
        );
        return Err(PaymentError::InsufficientAmount {
            expected: expected_amount_atomic,
            received: received_atomic,
        });
    }

    // 5. Verify memo contains meridian:<operation>
    let expected_memo = format!("meridian:{operation}");
    if !check_memo(&tx, &expected_memo) {
        warn!(tx_sig, operation, "Memo mismatch");
        return Err(PaymentError::WrongMemo { operation: operation.to_string() });
    }

    // 6. Record used signature + ops log (best-effort: don't fail on DB write)
    let insert_sig_result = sqlx::query(
        "INSERT INTO used_signatures (tx_signature, operation) VALUES ($1, $2) ON CONFLICT DO NOTHING"
    )
    .bind(tx_sig)
    .bind(operation)
    .execute(db)
    .await;

    if let Err(e) = insert_sig_result {
        error!(error = %e, tx_sig, "Failed to insert used_signature — proceeding anyway");
    }

    let insert_log_result = sqlx::query(
        "INSERT INTO operations_log (request_id, operation, file_size_bytes, price_usd, tx_signature, status) \
         VALUES ($1, $2, $3, $4, $5, 'ok')"
    )
    .bind(request_id)
    .bind(operation)
    .bind(file_size_bytes as i64)
    .bind(price_usd)
    .bind(tx_sig)
    .execute(db)
    .await;

    if let Err(e) = insert_log_result {
        error!(error = %e, tx_sig, "Failed to insert operations_log — proceeding anyway");
    }

    Ok(())
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

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Parse USDC token balance changes and return total atomic USDC received by `recipient`.
pub(crate) fn extract_usdc_received(tx: &serde_json::Value, recipient: &str) -> u64 {
    let pre_balances = tx["meta"]["preTokenBalances"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let post_balances = tx["meta"]["postTokenBalances"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    let mut pre_map: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
    for b in &pre_balances {
        if b["mint"].as_str() != Some(USDC_MINT) {
            continue;
        }
        if let (Some(idx), Some(amount)) = (
            b["accountIndex"].as_u64(),
            b["uiTokenAmount"]["amount"].as_str(),
        ) {
            pre_map.insert(idx, amount.parse().unwrap_or(0));
        }
    }

    let mut total: u64 = 0;
    for b in &post_balances {
        if b["mint"].as_str() != Some(USDC_MINT) {
            continue;
        }
        let owner = b["owner"].as_str().unwrap_or("");
        if owner != recipient {
            continue;
        }
        let idx = match b["accountIndex"].as_u64() {
            Some(i) => i,
            None => continue,
        };
        let post_amount: u64 = b["uiTokenAmount"]["amount"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let pre_amount = *pre_map.get(&idx).unwrap_or(&0);
        if post_amount > pre_amount {
            total += post_amount - pre_amount;
        }
    }

    total
}

/// Check if any memo instruction in the transaction contains the expected memo string.
pub(crate) fn check_memo(tx: &serde_json::Value, expected_memo: &str) -> bool {
    let instructions = tx["transaction"]["message"]["instructions"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    let inner: Vec<serde_json::Value> = tx["meta"]["innerInstructions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .flat_map(|ii| {
            ii["instructions"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
        })
        .collect();

    let all_instructions: Vec<serde_json::Value> =
        instructions.into_iter().chain(inner).collect();

    for ix in &all_instructions {
        if let Some(parsed) = ix["parsed"].as_str() {
            if parsed.contains(expected_memo) {
                return true;
            }
        }
        if let Some(data) = ix["data"].as_str() {
            if data.contains(expected_memo) {
                return true;
            }
        }
    }

    false
}

/// Verify a payment using pure in-memory state — no DB required.
/// Intended for integration tests and load harnesses.
pub async fn verify_payment_inmem(
    tx_sig: &str,
    expected_recipient: &str,
    expected_amount_atomic: u64,
    operation: &str,
    rpc_url: &str,
    client: &reqwest::Client,
    used_sigs: &std::sync::Mutex<std::collections::HashSet<String>>,
) -> Result<(), PaymentError> {
    // 1. Idempotency check (in-memory)
    {
        let sigs = used_sigs.lock().unwrap();
        if sigs.contains(tx_sig) {
            return Err(PaymentError::AlreadyUsed);
        }
    }

    // 2. Fetch transaction from RPC
    let rpc_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getTransaction",
        "params": [
            tx_sig,
            {
                "encoding": "jsonParsed",
                "commitment": "confirmed",
                "maxSupportedTransactionVersion": 0
            }
        ]
    });

    let resp = client
        .post(rpc_url)
        .json(&rpc_body)
        .send()
        .await
        .map_err(|e| PaymentError::RpcError(format!("HTTP error: {e}")))?;

    let rpc_resp: RpcResponse = resp
        .json()
        .await
        .map_err(|e| PaymentError::RpcError(format!("JSON decode error: {e}")))?;

    if let Some(err) = rpc_resp.error {
        return Err(PaymentError::RpcError(format!("RPC error: {err}")));
    }

    let tx = rpc_resp.result.ok_or(PaymentError::NotFound)?;

    if !tx["meta"]["err"].is_null() {
        return Err(PaymentError::NotFound);
    }

    let received_atomic = extract_usdc_received(&tx, expected_recipient);
    if received_atomic < expected_amount_atomic {
        return Err(PaymentError::InsufficientAmount {
            expected: expected_amount_atomic,
            received: received_atomic,
        });
    }

    let expected_memo = format!("meridian:{operation}");
    if !check_memo(&tx, &expected_memo) {
        return Err(PaymentError::WrongMemo { operation: operation.to_string() });
    }

    // 6. Record
    {
        let mut sigs = used_sigs.lock().unwrap();
        sigs.insert(tx_sig.to_string());
    }

    Ok(())
}

// ── USDC_DECIMALS usage ───────────────────────────────────────────────────────

/// Convert atomic USDC units back to USD float using USDC_DECIMALS.
pub fn atomic_to_usd(atomic: u64) -> f64 {
    atomic as f64 / 10_u64.pow(USDC_DECIMALS) as f64
}
