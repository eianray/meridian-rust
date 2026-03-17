# MeridianRust

Production-focused Rust rewrite of the Meridian GIS API, with x402/Base USDC payment enforcement through a facilitator-backed settlement flow.

**Status:** v0.5.0 — Rust is the canonical Meridian implementation; core GIS + vector tiles + DEM/raster shipped; x402/Base payment flow live via Coinbase CDP facilitator  
**Reference:** [Meridian v0.7.0](../meridian-api/) — legacy Python/FastAPI service kept for compatibility

---

## Overview

MeridianRust ports the Meridian GIS API from Python/FastAPI to Rust/Axum. It now covers the main synchronous GIS surface plus vector tiles and DEM/raster tools, with payment negotiation handled via x402 on Base.

The Rust service and the Python service are close in shape, but not at full behavioral parity yet. Current remaining gaps vs Python include DXF handling, async/background job flows, the pricing endpoint, MCP/SSE surfaces, and broader non-GeoJSON input support.

---

## Stack

| Component | Implementation |
|-----------|---------------|
| HTTP framework | Axum 0.7 |
| Database | SQLx + PostgreSQL |
| Rate limiting | Tower middleware |
| Payments | x402 + Base USDC + facilitator settlement |
| GIS / geometry | `gdal` crate 0.19 (GDAL/OGR FFI) |
| DEM / raster tools | `gdaldem`, `gdal_contour`, `gdal_calc.py` (shell-out from Rust) |
| API docs | `utoipa` (OpenAPI auto-generated) |
| Logging | `tracing` (structured, request IDs) |
| Metrics | Prometheus (`metrics-exporter-prometheus`) |
| Vector tiles | tippecanoe (shell-out) |

---

## Endpoints

> Note: Not all endpoints are currently described in the OpenAPI/Swagger surface. The table below is the source
> of truth for the Rust API surface; the OpenAPI spec will be expanded over time to match it.

### Info
| Endpoint | Description |
|----------|-------------|
| `GET /v1/health` | Service health check |

### Schema / Validation
| Endpoint | Description |
|----------|-------------|
| `POST /v1/schema` | Extract field names, types, CRS, geometry type, feature count, bbox |
| `POST /v1/validate` | Geometry validity report via GEOS IsValid |
| `POST /v1/repair` | Fix invalid geometries via GEOS MakeValid + buffer-by-zero fallback |

### Format Conversion
| Endpoint | Description |
|----------|-------------|
| `POST /v1/convert` | Convert GeoJSON → GeoJSON / Shapefile / KML / GeoPackage |

### Core GIS
| Endpoint | Description |
|----------|-------------|
| `POST /v1/reproject` | Reproject to any GDAL CRS string |
| `POST /v1/buffer` | Buffer features by distance in meters (auto-UTM) |
| `POST /v1/clip` | Clip to polygon mask |
| `POST /v1/dissolve` | Merge features by attribute field |

### Geometry Transforms
| Endpoint | Description |
|----------|-------------|
| `POST /v1/erase` | Delete all features, preserve empty schema |
| `POST /v1/feature-to-point` | Convert geometries to centroid points |
| `POST /v1/feature-to-line` | Extract polygon boundaries as LineStrings |
| `POST /v1/feature-to-polygon` | Polygonize closed LineString geometries |
| `POST /v1/multipart-to-singlepart` | Explode multipart geometries to single parts |
| `POST /v1/add-field` | Add attribute column with optional typed default |

### Topology (two-input)
| Endpoint | Description |
|----------|-------------|
| `POST /v1/union` | Combine all features from two layers |
| `POST /v1/intersect` | Spatial intersection of two layers |
| `POST /v1/difference` | layer_a minus (layer_a ∩ layer_b) |

### Combine (two-input)
| Endpoint | Description |
|----------|-------------|
| `POST /v1/append` | Add features from layer_b into layer_a's schema |
| `POST /v1/merge` | Combine two layers preserving all fields from both |
| `POST /v1/spatial-join` | Join attributes from layer_b onto layer_a by spatial predicate |

### Vector Tiles
| Endpoint | Description |
|----------|-------------|
| `POST /v1/vectorize` | Generate `.mbtiles` vector tile package via tippecanoe |

### DEM / Raster
| Endpoint | Description |
|----------|-------------|
| `POST /v1/hillshade` | Shaded relief via `gdaldem hillshade` |
| `POST /v1/slope` | Terrain slope via `gdaldem slope` |
| `POST /v1/aspect` | Terrain aspect via `gdaldem aspect` |
| `POST /v1/roughness` | Terrain roughness via `gdaldem roughness` |
| `POST /v1/color-relief` | Hypsometric tint / color relief via `gdaldem color-relief` |
| `POST /v1/contours` | Contour extraction via `gdal_contour` (GeoJSON output) |
| `POST /v1/raster-calc` | Pixel math across raster inputs via `gdal_calc.py` |

### Batch
| Endpoint | Description |
|----------|-------------|
| `POST /v1/batch` | Run up to 50 operations in one request with a single x402 payment |

---

## Pricing

Current Rust pricing is file-size based: **$0.01 per MB, minimum $0.01, cap $2.00**.  
The legacy Python service also exposes a separate pricing endpoint; that endpoint is not yet ported here.

---

## Payment Flow

1. POST to any paid endpoint → `402 Payment Required` with x402 payment requirements
2. Client signs an EIP-3009 USDC authorization on Base
3. Retry with `X-PAYMENT: <base64-encoded-payment-payload>`
4. Facilitator validates and settles `transferWithAuthorization()` on Base mainnet
5. Meridian trusts the facilitator result, records operation/payment audit metadata, and returns the processed result

### Facilitator boundary

Meridian does **not** broadcast Base transactions itself. It prices the operation, requests x402 payment, and defers canonical verification / replay protection / settlement to the facilitator. Meridian adds server-side idempotency and operation logging around accepted payments.

Payment idempotency today is backed by PostgreSQL records for accepted attempts and operation logs. That helps reject duplicate replays at the application boundary, but the canonical payment replay guarantees come from the underlying x402 + USDC authorization model and facilitator settlement path rather than Meridian alone.

---

## Current Differences vs Python

See [PARITY.md](./PARITY.md) for the fuller behavioral diff vs Python v0.7.0.

Notable current differences:
- **Input support:** Rust is still narrower than Python in practice; GeoJSON is the safest supported vector input path today
- **DXF:** Not ported
- **Async/background jobs:** Not ported
- **Pricing endpoint:** Not ported
- **MCP / SSE surfaces:** Not ported
- **Binary outputs:** Returned in JSON wrappers, with non-JSON payloads base64-encoded in `result.data`
- **Error key:** `error` (Rust) vs `detail` (Python FastAPI)
- **Missing field HTTP status:** `400` (Rust) vs `422` (Python Pydantic)

---

## Cutover Plan (Python → Rust)

Meridian v0.7.0 (Python/FastAPI) remains deployed at `nodeapi.ai` as a legacy service. MeridianRust is the
production target. The recommended migration flow is:

1. Configure MeridianRust with production env vars: `DEV_MODE=false`, `WALLET_ADDRESS`,
   `X402_FACILITATOR_URL`, correct `DATABASE_URL` and `PORT`.
2. Run the full black-box acceptance suite against MeridianRust (see
   `ACCEPTANCE-REPORT-2026-03-12.md`).
3. During a small downtime window, stop Python Meridian and point `nodeapi.ai` to the Rust
   service (currently `v2.nodeapi.ai` in production).
4. Run a short smoke test against `nodeapi.ai` (health + one vector op + one DEM op).
5. Keep the Python service available for a short fallback window, then retire it once Rust has
   proven stable under real traffic.

## Tests

Automated + live validation:
- Unit + integration tests for core GIS and raster helpers
- Payment integration tests for x402 flow
- Load test: 50 concurrent `/v1/reproject` requests, no 500s
- Rate limiter: 70 rapid requests → expected 429s
- Production black-box acceptance run on 2026-03-12: vector + raster flows exercised against public endpoints with live x402 negotiation and Base USDC settlement

---

## Infrastructure

- System GDAL required (`apt install libgdal-dev gdal-bin` or `brew install gdal`)
- tippecanoe required for `/v1/vectorize` (`brew install tippecanoe`)
- PostgreSQL for payment/idempotency state and operations log
- No facilitator gas management required — Coinbase CDP handles settlement gas
- See `docker-compose.yml` for local dev setup

---

## Status

| Phase | Description | Status |
|-------|-------------|--------|
| 1 | Skeleton (Axum, SQLx, health, logging) | ✅ |
| 2 | Core GIS (reproject, buffer, clip, dissolve, batch) | ✅ |
| 3 | Payments (x402 / Base USDC, idempotency, ops log) | ✅ |
| 4 | Observability (Prometheus, OpenAPI/Swagger) | ✅ |
| 5 | Hardening (load test, parity analysis, benchmarks) | ✅ |
| 6 | Expanded GIS surface + vectorize | ✅ |
| 7 | Facilitator-backed x402 settlement flow | ✅ |
| 8 | DEM / raster endpoints live (`hillshade` → `raster-calc`) | ✅ |
| 9 | Truth / parity cleanup pass | ✅ |

**Live:** v2.nodeapi.ai  
**Acceptance report:** [`ACCEPTANCE-REPORT-2026-03-12.md`](./ACCEPTANCE-REPORT-2026-03-12.md)

---

## Roadmap

### Near-term
- [x] Coinbase CDP facilitator live — `https://api.cdp.coinbase.com/platform/v2/x402`
- [ ] Multi-wallet facilitator pool — scale tx throughput beyond a single settlement wallet
- [ ] Pricing endpoint parity
- [ ] MCP server / SSE endpoint for agent tool discovery
- [ ] Broader input-format support review

### DEM / Terrain Analysis + Raster Math
Now live via Rust shell-outs to `gdaldem`, `gdal_contour`, and `gdal_calc.py`:
- [x] `/v1/hillshade` — shaded relief from sun angle/azimuth (`gdaldem hillshade`)
- [x] `/v1/slope` — degrees or percent rise (`gdaldem slope`)
- [x] `/v1/aspect` — direction of maximum slope (`gdaldem aspect`)
- [x] `/v1/roughness` — terrain ruggedness index (`gdaldem roughness`)
- [x] `/v1/color-relief` — hypsometric tinting from elevation ramp (`gdaldem color-relief`)
- [x] `/v1/contours` — contour lines/polygons at specified interval (`gdal_contour`)
- [x] `/v1/raster-calc` — pixel-level math on up to 26 raster inputs (`gdal_calc.py`)

### Hydrology (backlog — GRASS GIS shell-out)
- [ ] `/v1/watershed` — pour point → catchment polygon
- [ ] `/v1/flow-direction` — D8/D-infinity
- [ ] `/v1/stream-network` — threshold accumulation → polylines

### Viewshed (backlog)
- [ ] `/v1/viewshed` — visible area from observer point(s)
