# MeridianRust Acceptance Report

**Date:** 2026-03-12  
**Environment:** Production (`https://v2.nodeapi.ai`)  
**Version validated:** `0.4.0`  
**Validation style:** External black-box simulation of a real client/agent using public endpoints, real x402 payment flow, and real Base mainnet USDC settlement.

---

## Executive Summary

MeridianRust passed a full end-to-end production acceptance run after one code fix and one operational fix:

1. **Code fix:** MeridianRust payment verification was still using a hardcoded Coinbase facilitator URL instead of the configured self-hosted facilitator URL from `X402_FACILITATOR_URL`.
2. **Operational fix:** The facilitator wallet ran too low on Base ETH during the long acceptance sweep, causing settlement instability until the wallet was topped up.

After those two fixes, the final acceptance run completed **22/22 PASS**.

This validation included:
- public endpoint access only
- unauthenticated probes
- 402 payment negotiation
- EIP-3009 signing by an external-style client harness
- retry with `X-PAYMENT`
- successful processing and output validation
- invalid-input failure validation
- raster/DEM endpoints
- vector/GIS endpoints

---

## Scope of Validation

### Included in scope
- Existing paid vector/GIS endpoints
- Newly added DEM/raster endpoints
- Shared x402/Base USDC payment path
- Self-hosted facilitator on `localhost:8102`
- Public production host `v2.nodeapi.ai`
- Valid-input and invalid-input behavior
- End-to-end file handling and output return path

### Not in scope
- Load/stress testing beyond the acceptance sweep
- Batch endpoint in this final sweep
- Multi-wallet facilitator scaling
- Coinbase public facilitator cutover
- Hydrology/viewshed backlog endpoints (not implemented yet)

---

## Test Fixtures Used

Prepared local black-box fixtures under `/tmp/meridian-blackbox/`:

- `sample.geojson` — small polygon FeatureCollection with properties
- `dem.tif` — small grayscale GeoTIFF-like raster fixture for DEM operations
- `dem_b.tif` — second raster fixture for raster math
- `color.txt` — color-relief lookup table for `gdaldem color-relief`

These fixtures were intentionally small to reduce transaction cost and make repeated production validation practical while still exercising the full stack.

---

## Stage-by-Stage Validation History

### Stage 1 — Fixture prep
**Status:** PASS

Created vector, raster, and color-table fixtures for production testing.

### Stage 2 — Unauthenticated probe
**Status:** PASS

Confirmed that public paid endpoints returned **402 Payment Required** when called without `X-PAYMENT`.

Verified 402 behavior on:
- `reproject`
- `schema`
- `convert`
- `buffer`
- `feature-to-point`
- `clip`
- `union`
- `merge`
- `vectorize`
- `hillshade`
- `slope`
- `aspect`
- `roughness`
- `color-relief`
- `contours`
- `raster-calc`

### Stage 3 — Payment path
**Initial result:** FAIL  
**Final result:** PASS

#### Failure discovered
Public retry with signed `X-PAYMENT` returned:
- `500`
- `Facilitator error: JSON decode error: expected value at line 1 column 1`

#### Root cause
MeridianRust had a hardcoded facilitator URL in the shared x402 verification path:
- `https://x402.org/facilitate`

That bypassed the configured production value:
- `X402_FACILITATOR_URL=http://localhost:8102/facilitate`

#### Fix applied
Patched MeridianRust to read `X402_FACILITATOR_URL` from environment in shared payment verification.

#### Additional deployment/config issue discovered and fixed
The production `.env` had also been overwritten with stale dev-style values during deploy, including:
- wrong port (`8100`)
- `DEV_MODE=true`
- missing production database/payment settings

Production `.env` was restored to:
- `PORT=8101`
- `DEV_MODE=false`
- correct `DATABASE_URL`
- correct `WALLET_ADDRESS`
- correct `X402_FACILITATOR_URL`

#### Re-test
After patch + config restoration:
- `reproject` external payment flow: PASS
- `hillshade` external payment flow: PASS

### Stage 4 — Success-path execution
**Status:** PASS

Validated successful full paid execution for all tested success-path endpoints:
- `reproject`
- `schema`
- `convert`
- `buffer`
- `feature-to-point`
- `clip`
- `union`
- `merge`
- `vectorize`
- `hillshade`
- `slope`
- `aspect`
- `roughness`
- `color-relief`
- `contours`
- `raster-calc`

### Stage 5 — Failure-path execution
**Status:** PASS

Validated clean `400` behavior for:
- `color-relief` missing color table
- `raster-calc` missing expression
- `raster-calc` missing referenced raster slot
- `raster-calc` invalid output format
- `contours` invalid interval
- `hillshade` missing file

Important observation: some invalid-input requests are rejected **before payment**, which is preferable because malformed requests do not force unnecessary settlement attempts.

### Stage 6 — Final uninterrupted acceptance sweep
**Initial result:** FAIL  
**Final result:** PASS

#### Initial failure pattern
The first uninterrupted full sweep produced:
- early success on several endpoints
- one hard failure on `clip`
- subsequent settlement degradation and misleading payment failures

#### Root cause
The facilitator wallet was effectively out of Base ETH for gas during the long sweep.

Balance observed during investigation:
- ETH: `0.00000083217254101`
- USDC: `0.007056`

This caused settlement instability during the acceptance run.

#### Operational fix
Facilitator wallet was reloaded with Base ETH.

Observed balance after funding:
- ETH: `0.005000804701823349`
- USDC: `0.007056`

#### Final rerun result
The full acceptance sweep then passed **22/22** in **34 seconds**.

---

## Final Acceptance Matrix

### Success-path cases
| Endpoint | Result | Notes |
|---|---:|---|
| `/v1/reproject` | PASS | JSON result returned |
| `/v1/schema` | PASS | schema payload returned |
| `/v1/convert` | PASS | `sample.kml` returned |
| `/v1/buffer` | PASS | JSON result returned |
| `/v1/feature-to-point` | PASS | JSON result returned |
| `/v1/clip` | PASS | JSON result returned |
| `/v1/union` | PASS | JSON result returned |
| `/v1/merge` | PASS | JSON result returned |
| `/v1/vectorize` | PASS | `data_z0-14.mbtiles` returned |
| `/v1/hillshade` | PASS | `hillshade.tif` returned |
| `/v1/slope` | PASS | `slope.tif` returned |
| `/v1/aspect` | PASS | `aspect.tif` returned |
| `/v1/roughness` | PASS | `roughness.tif` returned |
| `/v1/color-relief` | PASS | `color-relief.tif` returned |
| `/v1/contours` | PASS | `contours.geojson` returned |
| `/v1/raster-calc` | PASS | `raster-calc.tif` returned |

### Failure-path cases
| Case | Expected behavior | Result |
|---|---|---:|
| `color-relief` missing color table | `400` | PASS |
| `raster-calc` missing expression | `400` | PASS |
| `raster-calc` missing raster slot | `400` | PASS |
| `raster-calc` bad output format | `400` | PASS |
| `contours` invalid interval | `400` | PASS |
| `hillshade` missing file | `400` | PASS |

---

## Output Behavior Confirmed

### Vector / JSON-style outputs
These returned structured JSON results or GeoJSON-like content:
- `reproject`
- `schema`
- `buffer`
- `feature-to-point`
- `clip`
- `union`
- `merge`

### Binary/base64-style outputs
These returned file-oriented result objects with filename, mime type, base64 encoding, and stats:
- `convert` → KML
- `vectorize` → MBTiles
- `hillshade` → GeoTIFF
- `slope` → GeoTIFF
- `aspect` → GeoTIFF
- `roughness` → GeoTIFF
- `color-relief` → GeoTIFF
- `contours` → GeoJSON file payload
- `raster-calc` → GeoTIFF

---

## Defects Found and Resolved

### 1. Hardcoded facilitator URL
**Severity:** High  
**Symptom:** public payment retries failed with 500  
**Resolution:** MeridianRust now reads facilitator URL from environment.

### 2. Production `.env` overwritten with stale dev config
**Severity:** High  
**Symptom:** wrong port, dev mode, missing DB/payment settings  
**Resolution:** restored correct production `.env`.

### 3. Facilitator wallet underfunded for long acceptance run
**Severity:** Medium / operational  
**Symptom:** settlement instability during long uninterrupted run  
**Resolution:** funded wallet with Base ETH.

---

## Readiness Assessment

### Current judgment
**MeridianRust v0.4.0 is production-viable for the currently implemented endpoint surface, including DEM/raster operations, provided the facilitator wallet is funded with sufficient Base ETH.**

### Confidence level
High for:
- public endpoint availability
- x402 payment negotiation
- self-hosted facilitator integration
- current GIS endpoint set tested here
- DEM/raster endpoint set implemented in v0.4.0
- malformed input handling for tested cases

Moderate for:
- long sustained production usage without more gas monitoring/alerting
- broader concurrency beyond the current validation sweep
- untested endpoint surface outside this acceptance matrix

---

## Operational Recommendations

1. **Keep Base ETH buffer on facilitator wallet**
   - below ~`0.005 ETH` should be considered warning territory
   - set an alert or heartbeat check

2. **Document funding requirement explicitly**
   - x402 settlement depends on facilitator gas, not only user USDC

3. **Add gas-balance monitoring**
   - even a simple periodic balance check would prevent misleading acceptance failures

4. **Preserve production `.env` during deploys**
   - do not overwrite with stale local config
   - consider excluding `.env` from rsync if not already handled carefully

5. **Add final acceptance harness to repo or scripts directory**
   - today’s black-box harness is worth preserving and cleaning up

---

## Recommended Next Documentation Updates

- Update README to reflect:
  - x402/Base instead of Solana Pay
  - v0.4.0
  - DEM/raster endpoints now live
  - full 22/22 production acceptance pass
- Update landing page to mention:
  - DEM processing is now live
  - raster math is now live
  - validated production acceptance

---

## Final Statement

As of 2026-03-12, MeridianRust passed a full black-box production acceptance run as an external agent-style client using real x402 payments and real public endpoints.

**Final result: 22/22 PASS.**
