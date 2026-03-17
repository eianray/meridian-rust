"""
x402 Payment Protocol for Meridian GIS API.

HTTP 402-native micropayments in USDC on Base.
No accounts. No API keys. No credit cards. Just pay and go.

Flow:
  1. Agent sends request (no payment header)
  2. Server responds 402 with payment requirements body
  3. Agent pays USDC on Base (EIP-3009 signed transfer)
  4. Agent re-sends request with X-PAYMENT header (base64 payload)
  5. Server verifies via Coinbase facilitator → processes → returns result
     with X-PAYMENT-RESPONSE receipt header

Spec: https://x402.org
Facilitator: https://x402.org/facilitate (Coinbase-hosted, free to use)
"""

import base64
import json
from typing import Optional

import httpx
from fastapi import Header, HTTPException, Request, status

from app.config import get_settings

# USDC contract on Base mainnet
USDC_BASE_CONTRACT = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
X402_VERSION = 1

# Per-operation prices in USDC atomic units (6 decimals)
# $0.001 = 1000 units, $0.005 = 5000 units
OPERATION_PRICES: dict[str, int] = {
    # Phase 1
    "convert":                 1000,   # $0.01
    "reproject":               1000,   # $0.01
    "validate":                1000,   # $0.01
    "repair":                  1000,   # $0.01
    "schema":                  1000,   # $0.01
    "clip":                    1000,   # $0.01
    # Phase 2
    "dxf":                     1000,   # $0.01
    "buffer":                  1000,   # $0.01
    "union":                   1000,   # $0.01
    "intersect":               1000,   # $0.01
    "difference":              1000,   # $0.01
    # Phase 3
    "vectorize":               1000,   # $0.01
    "erase":                   1000,   # $0.01
    "dissolve":                1000,   # $0.01
    "feature-to-point":        1000,   # $0.01
    "feature-to-line":         1000,   # $0.01
    "feature-to-polygon":      1000,   # $0.01
    "multipart-to-singlepart": 1000,   # $0.01
    "add-field":               1000,   # $0.01
    "append":                  1000,   # $0.01
    "merge":                   1000,   # $0.01
    "spatial-join":            1000,   # $0.01
}

OPERATION_DESCRIPTIONS: dict[str, str] = {
    "convert":                 "Convert spatial data between formats",
    "reproject":               "Project or reproject spatial data to a target CRS",
    "validate":                "Validate vector geometry",
    "repair":                  "Repair invalid vector geometry",
    "schema":                  "Extract attribute schema and metadata",
    "clip":                    "Clip spatial data to bbox or polygon mask",
    "dxf":                     "Convert DXF/CAD file to spatial vector format",
    "buffer":                  "Buffer features by distance in meters",
    "union":                   "Union: combine features from two layers",
    "intersect":               "Intersect: areas common to two layers",
    "difference":              "Difference: layer_a minus overlap with layer_b",
    "vectorize":               "Vectorize: generate .mbtiles vector tile package from spatial file",
    "erase":                   "Erase: delete all features, preserve empty schema",
    "dissolve":                "Dissolve: merge features by attribute field",
    "feature-to-point":        "Feature to Point: convert geometries to centroid points",
    "feature-to-line":         "Feature to Line: extract polygon boundaries as lines",
    "feature-to-polygon":      "Feature to Polygon: convert closed lines to polygons",
    "multipart-to-singlepart": "Multipart to Single Part: explode multipart geometries",
    "add-field":               "Add Field: add a new attribute column to all features",
    "append":                  "Append: add features from layer_b into layer_a's schema",
    "merge":                   "Merge: combine two layers preserving all fields",
    "spatial-join":            "Spatial Join: join attributes by spatial relationship",
}


def build_payment_required(operation: str, resource_url: str) -> dict:
    """
    Build the 402 response body per x402 spec.
    Returned as JSON response body with status 402.
    """
    settings = get_settings()
    amount = OPERATION_PRICES.get(operation, 5000)

    return {
        "x402Version": X402_VERSION,
        "error": "Payment required",
        "accepts": [
            {
                "scheme": "exact",
                "network": "base",
                "maxAmountRequired": str(amount),
                "resource": resource_url,
                "description": OPERATION_DESCRIPTIONS.get(operation, f"Meridian: {operation}"),
                "mimeType": "application/octet-stream",
                "payTo": settings.wallet_address,
                "maxTimeoutSeconds": 300,
                "asset": USDC_BASE_CONTRACT,
                "extra": {
                    "name": "USD Coin",
                    "version": "2",
                    "decimals": 6,
                },
            }
        ],
    }


async def verify_payment(
    payment_header: str,
    operation: str,
    resource_url: str,
) -> tuple[bool, Optional[str], Optional[str]]:
    """
    Verify x402 payment via the Coinbase facilitator.
    Returns (is_valid, payer_address, tx_hash).

    In DEV_MODE (no wallet configured), bypasses verification and returns mock success.
    """
    settings = get_settings()

    # Dev mode: no wallet address configured → skip real verification
    if not settings.wallet_address or settings.wallet_address == "0x0000000000000000000000000000000000000000":
        return True, "0xDEV_MODE", "0xDEV_TX"

    # Decode the payment payload
    try:
        # base64 padding is often stripped; add it back
        padded = payment_header + "=" * (-len(payment_header) % 4)
        payload = json.loads(base64.b64decode(padded))
    except Exception:
        return False, None, None

    amount_required = str(OPERATION_PRICES.get(operation, 5000))

    async with httpx.AsyncClient(timeout=15.0) as client:
        try:
            resp = await client.post(
                settings.x402_facilitator_url,
                json={
                    "x402Version": X402_VERSION,
                    "paymentPayload": payload,
                    "paymentRequirements": {
                        "scheme": "exact",
                        "network": "base",
                        "maxAmountRequired": amount_required,
                        "resource": resource_url,
                        "payTo": settings.wallet_address,
                        "asset": USDC_BASE_CONTRACT,
                        "maxTimeoutSeconds": 300,
                    },
                },
                headers={"Content-Type": "application/json"},
            )
            if resp.status_code == 200:
                data = resp.json()
                return (
                    data.get("isValid", False),
                    data.get("payer"),
                    data.get("transaction"),
                )
        except httpx.TimeoutException:
            raise HTTPException(
                status_code=status.HTTP_503_SERVICE_UNAVAILABLE,
                detail="Payment verification timed out. Please retry.",
            )
        except Exception:
            pass

    return False, None, None


def payment_required_exception(operation: str, resource_url: str) -> HTTPException:
    """Raise a properly-formed 402 HTTPException."""
    return HTTPException(
        status_code=status.HTTP_402_PAYMENT_REQUIRED,
        detail=build_payment_required(operation, resource_url),
    )


async def require_payment(
    request: Request,
    operation: str,
    x_payment: Optional[str] = None,
) -> tuple[str, str]:
    """
    FastAPI dependency factory helper.
    Call as: payer, txhash = await require_payment(request, "convert")

    Returns (payer_address, tx_hash) on success.
    Raises 402 if no payment header present.
    Raises 400 if payment is invalid.
    Raises 429 if rate limit exceeded.
    """
    from app.ratelimit import check_rate_limit
    rate_limit_response = check_rate_limit(request)
    if rate_limit_response:
        raise HTTPException(status_code=429, detail="Rate limit exceeded. Please slow down.")

    resource_url = str(request.url)

    if not x_payment:
        raise payment_required_exception(operation, resource_url)

    is_valid, payer, txhash = await verify_payment(x_payment, operation, resource_url)

    if not is_valid:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Payment verification failed. Check your payment payload and retry.",
        )

    return payer or "unknown", txhash or "unknown"
