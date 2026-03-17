# Meridian White Paper Addendum — 2026-03-12

## Update — 2026-03-17: Coinbase CDP Facilitator

The self-hosted `meridian-facilitator` microservice has been replaced with the Coinbase CDP managed facilitator (`https://api.cdp.coinbase.com/platform/v2/x402`).

**What changed:**
- Settlement is now handled by Coinbase's infrastructure (no self-hosted facilitator required)
- Ed25519 JWT auth added to MeridianRust billing layer — generated per-request, signed with CDP API key
- `.env` now requires `CDP_API_KEY_ID` and `CDP_API_KEY_SECRET` in addition to `X402_FACILITATOR_URL`
- Gas management and nonce handling are now Coinbase's responsibility

**What didn't change:**
- Payment protocol: x402 / Base USDC / EIP-3009
- Client-facing API: identical
- Receiving wallet: unchanged

**Operational note:**
The previous recommendation to monitor facilitator gas balance no longer applies. Coinbase manages gas for settlement.

---

This addendum captures changes after the earlier Meridian white paper / collateral was produced.

## What changed

Meridian now exists in two meaningful forms:
1. **Original Python/FastAPI service** at `nodeapi.ai` (legacy/compatibility layer)
2. **Rust/Axum production rewrite** at `v2.nodeapi.ai` (canonical implementation)

The Rust version is no longer a partial port. It is now a production-validated system with:
- x402/Base USDC payments
- self-hosted facilitator for settlement
- vector GIS operations
- vector tile generation
- DEM/raster processing
- production black-box acceptance validation

## Production validation

On 2026-03-12, MeridianRust completed a full external-style black-box validation using:
- public production endpoints
- real x402 payment negotiation
- real EIP-3009 signing
- real Base mainnet settlement
- valid and invalid request paths

**Final result: 22/22 PASS**

See:
- `projects/meridian-rust/ACCEPTANCE-REPORT-2026-03-12.md`

## New endpoint surface in Rust

DEM / raster tools now live:
- `/v1/hillshade`
- `/v1/slope`
- `/v1/aspect`
- `/v1/roughness`
- `/v1/color-relief`
- `/v1/contours`
- `/v1/raster-calc`

These are implemented as Rust-controlled shell-outs to GDAL tools (`gdaldem`, `gdal_contour`, `gdal_calc.py`) while keeping the application codebase Rust-native.

## Product implication

Meridian is still fundamentally **agent-native**. That remains the core thesis.

But the product now has a second highly practical use case:
- a **human operator interface** over the same API

Planned next step:
- embed an **Operator Panel** at the bottom of the Meridian landing page
- allow manual upload → quote → pay → run → download
- use the same production endpoints and payment path as agent clients

This is not a pivot away from the agent-native thesis. It is a thin manual client over an agent-first backend.

## Why this matters

This gives Meridian a stronger real-world position:
- **For agents:** machine-native geospatial processing with direct payment rails
- **For humans:** a zero-account manual interface for ad hoc jobs, testing, and production dogfooding

That dual use is a strategic advantage, not a contradiction, so long as the product is framed correctly:
- API first
- manual client second

## Operational lesson worth preserving

The 2026-03-12 acceptance run surfaced an important operational requirement for self-hosted facilitators: the facilitator wallet must maintain sufficient **Base ETH** for gas.

As of 2026-03-17, this is no longer applicable — Coinbase CDP manages gas for settlement.
