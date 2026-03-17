"""
In-memory rate limiter (token bucket per IP).
No Redis needed at Phase 2 scale.

Limits:
  - 60 requests/minute per IP (global, burst up to 15)
  - 10 requests/minute per IP for expensive operations (burst up to 3):
      dxf, vectorize, spatial-join
  - Internal API key requests (X-PAYMENT: internal_*) are exempt
  - Returns 429 with Retry-After header when exceeded

slowapi is listed in requirements.txt for future middleware integration;
this module provides a lightweight equivalent that slots into the existing
require_payment() gate without adding ASGI middleware complexity.
"""
import time
from collections import defaultdict
from threading import Lock
from typing import Optional

from fastapi import HTTPException, Request

# ── Rate configs ──────────────────────────────────────────────────────────────
RATE_GLOBAL_RPS    = 1.0   # 60 req/min = 1.0 req/sec
RATE_GLOBAL_BURST  = 15    # allow short bursts up to 15

RATE_EXPENSIVE_RPS   = 10 / 60   # 10 req/min ≈ 0.167 req/sec
RATE_EXPENSIVE_BURST = 3

CLEANUP_INTERVAL_SECS = 300  # purge idle buckets every 5 min

# Expensive endpoints that get the tighter per-op limit
EXPENSIVE_OPS = {"dxf", "vectorize", "spatial-join", "spatial_join"}

# ── Bucket store ──────────────────────────────────────────────────────────────
# Key: "{ip}:{bucket_type}" where bucket_type is "" (global) or the op name
_buckets: dict[str, dict] = {}
_lock = Lock()
_last_cleanup = time.monotonic()


def _get_ip(request: Request) -> str:
    """Extract real client IP, respecting Cloudflare and standard proxy headers."""
    cf_ip = request.headers.get("CF-Connecting-IP")
    if cf_ip:
        return cf_ip
    xff = request.headers.get("X-Forwarded-For")
    if xff:
        return xff.split(",")[0].strip()
    return request.client.host if request.client else "unknown"


def _consume(bucket_key: str, rps: float, burst: int) -> int | None:
    """
    Try to consume one token from the named bucket.
    Returns None if allowed, or retry_after_seconds (int) if limited.
    Must be called under _lock.
    """
    global _last_cleanup
    now = time.monotonic()

    # Periodic cleanup
    if now - _last_cleanup > CLEANUP_INTERVAL_SECS:
        idle_cutoff = now - 600
        stale = [k for k, v in _buckets.items() if v["last"] < idle_cutoff]
        for k in stale:
            del _buckets[k]
        _last_cleanup = now

    if bucket_key not in _buckets:
        _buckets[bucket_key] = {"tokens": float(burst), "last": now}

    bucket = _buckets[bucket_key]
    elapsed = now - bucket["last"]
    bucket["tokens"] = min(float(burst), bucket["tokens"] + elapsed * rps)
    bucket["last"] = now

    if bucket["tokens"] >= 1.0:
        bucket["tokens"] -= 1.0
        return None  # allowed

    # How long until next token is available
    deficit = 1.0 - bucket["tokens"]
    return max(1, int(deficit / rps) + 1)


def check_rate_limit(
    request: Request,
    operation: str = "",
    x_payment: Optional[str] = None,
) -> None:
    """
    Rate-limit gate. Raises HTTPException(429) with Retry-After header if limited.
    Returns None silently if allowed.

    - Exempt: x_payment starting with "internal_"
    - Expensive ops (dxf, vectorize, spatial-join): 10 req/min per IP
    - All others: 60 req/min per IP
    """
    # Internal key bypass — no rate limiting for trusted callers
    if x_payment and x_payment.startswith("internal_"):
        return

    ip = _get_ip(request)
    op_normalized = operation.replace("_", "-").lower()

    with _lock:
        # 1. Check global bucket (always applies)
        retry_global = _consume(f"{ip}:global", RATE_GLOBAL_RPS, RATE_GLOBAL_BURST)

        # 2. Check per-op bucket for expensive operations
        retry_op = None
        if op_normalized in EXPENSIVE_OPS:
            retry_op = _consume(f"{ip}:{op_normalized}", RATE_EXPENSIVE_RPS, RATE_EXPENSIVE_BURST)

    retry_after = None
    if retry_global is not None:
        retry_after = retry_global
    if retry_op is not None:
        retry_after = max(retry_after or 0, retry_op)

    if retry_after is not None:
        raise HTTPException(
            status_code=429,
            detail=f"Rate limit exceeded. Retry after {retry_after}s.",
            headers={"Retry-After": str(retry_after)},
        )
