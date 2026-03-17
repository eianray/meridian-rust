"""
Solana Pay payment protocol for Node API.

Flow:
  1. Agent calls endpoint (no payment header)
  2. Server responds 402 with Solana Pay details + payment URL
  3. Agent sends 0.01 USDC on Solana Mainnet to our wallet
     - Include memo: "nodeapi:<operation>" in the transaction
  4. Agent retries with X-PAYMENT: <transaction_signature>
  5. Server verifies via Solana RPC → processes → returns result

Payment verification:
  - Calls getTransaction on Solana mainnet RPC
  - Confirms USDC transfer to our wallet ≥ required amount
  - Tracks used signatures in DB to prevent double-spend

USDC on Solana Mainnet:
  Mint: EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v
  Decimals: 6
  $0.01 = 10000 atomic units
"""

from typing import Optional
import httpx
from fastapi import HTTPException, Request, status

from app.config import get_settings

USDC_MINT = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
USDC_DECIMALS = 6
FLAT_PRICE_USD = 0.01  # $0.01 per operation


def build_payment_required(operation: str, resource_url: str) -> dict:
    """
    Build the 402 response body for Solana Pay.
    Returns a clean JSON structure any agent can parse.
    """
    settings = get_settings()
    recipient = settings.solana_wallet_address

    payment_url = (
        f"solana:{recipient}"
        f"?amount={FLAT_PRICE_USD}"
        f"&spl-token={USDC_MINT}"
        f"&label=Node+API"
        f"&message={operation.replace('-', '+')}"
        f"&memo=nodeapi:{operation}"
    )

    return {
        "error": "Payment required",
        "protocol": "solana-pay",
        "network": "mainnet-beta",
        "amount_usd": str(FLAT_PRICE_USD),
        "recipient": recipient,
        "token": "USDC",
        "token_mint": USDC_MINT,
        "memo": f"nodeapi:{operation}",
        "payment_url": payment_url,
        "resource": resource_url,
        "instructions": (
            f"Send {FLAT_PRICE_USD} USDC on Solana Mainnet to {recipient}. "
            f"Retry request with header: X-PAYMENT: <transaction_signature>"
        ),
    }


async def verify_payment(
    tx_signature: str,
    operation: str,
    min_amount: Optional[float] = None,
) -> tuple[bool, Optional[str]]:
    """
    Verify a Solana USDC payment via RPC.
    Returns (is_valid, payer_pubkey).

    Checks:
    - Transaction exists and is confirmed
    - USDC transferred to our wallet >= FLAT_PRICE_USD
    - Signature not already used (anti-replay)
    """
    settings = get_settings()
    recipient = settings.solana_wallet_address

    # Dev mode: no wallet configured → skip verification
    if not recipient or recipient == "YOUR_SOLANA_WALLET_ADDRESS":
        return True, "DEV_MODE"

    # Anti-replay: check if signature already used
    if _is_signature_used(tx_signature):
        return False, None

    # Fetch transaction from Solana RPC
    try:
        async with httpx.AsyncClient(timeout=20.0) as client:
            resp = await client.post(
                settings.solana_rpc_url,
                json={
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "getTransaction",
                    "params": [
                        tx_signature,
                        {
                            "encoding": "jsonParsed",
                            "commitment": "confirmed",
                            "maxSupportedTransactionVersion": 0,
                        },
                    ],
                },
            )
            data = resp.json()
    except httpx.TimeoutException:
        raise HTTPException(
            status_code=status.HTTP_503_SERVICE_UNAVAILABLE,
            detail="Solana RPC timed out during payment verification. Please retry.",
        )
    except Exception as e:
        raise HTTPException(
            status_code=status.HTTP_503_SERVICE_UNAVAILABLE,
            detail=f"Payment verification unavailable: {e}",
        )

    tx = data.get("result")
    if not tx:
        return False, None

    # Check transaction succeeded (err == null means success)
    if tx.get("meta", {}).get("err") is not None:
        return False, None

    # Find USDC transfer to our wallet in token balance changes
    pre_balances = {
        b["accountIndex"]: float(b.get("uiTokenAmount", {}).get("uiAmount") or 0)
        for b in tx.get("meta", {}).get("preTokenBalances", [])
        if b.get("mint") == USDC_MINT
    }
    post_balances = {
        b["accountIndex"]: (
            float(b.get("uiTokenAmount", {}).get("uiAmount") or 0),
            b.get("owner", ""),
        )
        for b in tx.get("meta", {}).get("postTokenBalances", [])
        if b.get("mint") == USDC_MINT
    }

    received_usdc = 0.0
    for idx, (post_amount, owner) in post_balances.items():
        if owner == recipient:
            pre_amount = pre_balances.get(idx, 0.0)
            delta = post_amount - pre_amount
            if delta > 0:
                received_usdc += delta

    required = min_amount if min_amount is not None else FLAT_PRICE_USD
    if received_usdc < required:
        return False, None

    # Extract payer: first account key in the transaction (fee payer)
    try:
        account_keys = tx["transaction"]["message"]["accountKeys"]
        payer = account_keys[0]["pubkey"] if isinstance(account_keys[0], dict) else account_keys[0]
    except (KeyError, IndexError):
        payer = "unknown"

    # Mark signature as used
    _mark_signature_used(tx_signature, operation, payer)

    return True, payer


def _is_signature_used(signature: str) -> bool:
    """Check if a transaction signature has already been used."""
    try:
        import psycopg2
        from app.db import get_conn
        with get_conn() as conn:
            with conn.cursor() as cur:
                cur.execute(
                    "SELECT 1 FROM used_tx_signatures WHERE signature = %s",
                    (signature,),
                )
                return cur.fetchone() is not None
    except Exception:
        return False  # Fail open on DB error — don't block legitimate payments


def _mark_signature_used(signature: str, operation: str, payer: str) -> None:
    """Record a used transaction signature to prevent double-spend."""
    try:
        from app.db import get_conn
        with get_conn() as conn:
            with conn.cursor() as cur:
                cur.execute(
                    """INSERT INTO used_tx_signatures (signature, operation, payer_address)
                       VALUES (%s, %s, %s)
                       ON CONFLICT (signature) DO NOTHING""",
                    (signature, operation, payer),
                )
            conn.commit()
    except Exception:
        pass


def payment_required_exception(operation: str, resource_url: str) -> HTTPException:
    """Raise a properly-formed 402 HTTPException for Solana Pay."""
    return HTTPException(
        status_code=status.HTTP_402_PAYMENT_REQUIRED,
        detail=build_payment_required(operation, resource_url),
    )


async def require_payment(
    request: Request,
    operation: str,
    x_payment: Optional[str] = None,
    price_override: Optional[float] = None,
) -> tuple[str, str]:
    """
    Solana Pay payment gate. Drop-in replacement for x402 require_payment.
    Returns (payer_address, tx_signature) on success.
    Raises 402 if no payment header.
    Raises 400 if payment invalid or already used.
    Raises 429 if rate limited (with Retry-After header).

    price_override: use a custom amount instead of FLAT_PRICE_USD (e.g. for batch).
    """
    from app.ratelimit import check_rate_limit
    # Rate limit check — passes x_payment so internal_* keys are exempt
    check_rate_limit(request, operation=operation, x_payment=x_payment)

    resource_url = str(request.url)

    if not x_payment:
        raise payment_required_exception(operation, resource_url)

    # Internal API key bypass (MCP server, trusted local callers):
    # accept any key that starts with "internal_" OR exact match of configured key
    settings = get_settings()
    if x_payment.startswith("internal_"):
        return "internal", "internal"
    if settings.internal_api_key and x_payment == settings.internal_api_key:
        return "internal", "internal"

    is_valid, payer = await verify_payment(x_payment, operation, min_amount=price_override)

    if not is_valid:
        required_str = f"≥${price_override:.2f}" if price_override else "≥0.01"
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail=(
                f"Payment verification failed. Ensure you sent {required_str} USDC on Solana Mainnet "
                "to the correct recipient, and that the signature hasn't been used before."
            ),
        )

    return payer or "unknown", x_payment
