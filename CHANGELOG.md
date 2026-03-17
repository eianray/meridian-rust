# CHANGELOG — MeridianRust

---

## v0.5.0 — Coinbase CDP Facilitator (2026-03-17)

### Payment Infrastructure
- Replaced self-hosted `meridian-facilitator` with Coinbase CDP managed facilitator
- Facilitator URL: `https://api.cdp.coinbase.com/platform/v2/x402`
- Added Ed25519 JWT generation in `src/billing/x402.rs` — per-request signed Bearer token
- JWT claims: `iss`, `sub`, `nbf`, `exp` (+120s), `uri`
- Added `cdp_api_key_id` and `cdp_api_key_secret` to `AppConfig` (loaded from env)
- Authorization header automatically injected for CDP facilitator URLs
- All `verify_payment()` call sites updated to pass CDP credentials

### New Dependencies
- `jsonwebtoken = "9"` — JWT encoding
- `ed25519-dalek = { version = "2", features = ["pkcs8"] }` — Ed25519 signing
- `pkcs8 = { version = "0.10", features = ["alloc"] }` — PKCS#8 DER encoding

### Operational
- Removed gas management requirement (Coinbase handles settlement gas)
- `meridian-facilitator` service is now dormant / no longer required

---

## v0.4.0 — x402 / Base / Facilitator Truth Pass (2026-03-13)

### Current state clarified
- Payment model is now documented as **x402 + Base USDC + facilitator-backed settlement**
- Rust service records accepted payment / operation state, but canonical replay protection is not claimed as purely app-side
- README and env docs updated to reflect the current config surface: `HOST`, `PORT`, `LOG_LEVEL`, `DATABASE_URL`, `WALLET_ADDRESS`, `X402_FACILITATOR_URL`, and optional `DEV_MODE`
- OpenAPI metadata bumped to `v0.4.0`
- `AppConfig` now supports explicit `DEV_MODE` precedence while preserving backward-compatible wallet-derived dev mode when unset

### Known parity gaps vs Python
- DXF support not ported
- Async/background job flow not ported
- Pricing endpoint not ported
- MCP / SSE surfaces not ported
- Broader non-GeoJSON input support still narrower than Python

### Notes
- No DB schema or migration changes in this pass
- Older changelog entries below are retained as historical implementation notes; some Solana-era wording is no longer current

---

## v0.3.0 — Vectorize Endpoint (historical, 2026-03-12)

### 1 New Endpoint

**Vector Tiles**
- `POST /v1/vectorize` — Generate `.mbtiles` vector tile package via tippecanoe shell-out

**Details:**
- Multipart fields: `file` (GeoJSON), `layer_name` (default: `data`), `min_zoom` (default: 0), `max_zoom` (default: 14), `simplify` (default: true), `name`, `description`
- Validates layer_name (alphanumeric/hyphen/underscore, 1-64 chars), zoom range (0–16)
- Locates tippecanoe at PATH, `/opt/homebrew/bin/tippecanoe`, or `/usr/local/bin/tippecanoe`
- 300s timeout via `tokio::time::timeout`
- Returns base64-encoded `.mbtiles` in `result.data` with `VectorizeStats` (feature count, zoom range, layer name, size bytes)
- Output filename: `{layer_name}_z{min_zoom}-{max_zoom}.mbtiles`

### Tests
- All 25 tests passing (`cargo check` clean, `cargo test` 25/25)

### Notes
- DXF endpoint was never ported to Rust (Python-only); not included in Rust API surface
- Prior changelog wording about “full parity” should be read as historical shorthand, not a current exact parity claim

---

## v0.2.0 — Full GIS Port (historical, 2026-03-12)

### 16 New Endpoints

**Schema / Validation**
- `POST /v1/schema` — Extract field names, types, CRS, geometry type, feature count, bbox (JSON only)
- `POST /v1/validate` — Geometry validity report via GEOS IsValid (JSON only)
- `POST /v1/repair` — Fix invalid geometries via GEOS MakeValid + buffer-by-zero fallback

**Format Conversion**
- `POST /v1/convert` — Convert GeoJSON → GeoJSON / Shapefile (zipped) / KML / GeoPackage via GDAL OGR

**Geometry Transforms (single-input)**
- `POST /v1/erase` — Return empty layer preserving schema/CRS
- `POST /v1/feature-to-point` — Convert geometries to centroid points (envelope midpoint)
- `POST /v1/feature-to-line` — Extract polygon boundaries as LineStrings
- `POST /v1/feature-to-polygon` — Polygonize closed LineString geometries
- `POST /v1/multipart-to-singlepart` — Explode multipart geometries to single parts
- `POST /v1/add-field` — Add attribute column with optional typed default value

**Topology (two-input)**
- `POST /v1/union` — Combine all features from two layers; optional dissolve
- `POST /v1/intersect` — Spatial intersection of two layers via GEOS
- `POST /v1/difference` — layer_a minus (layer_a ∩ layer_b)

**Combine (two-input)**
- `POST /v1/append` — Append layer_b into layer_a schema; drop extra fields, fill missing nulls
- `POST /v1/merge` — Combine both layers preserving union of all fields
- `POST /v1/spatial-join` — Join attributes from layer_b onto layer_a by spatial predicate

### New Dependencies
- `zip = "0.6"` — Shapefile output packaging
- `tempfile = "3"` — Temp files for GDAL format conversion
- `indexmap = "2"` — Ordered field schema output
- `base64 = "0.21"` — Binary format encoding in API responses

### Implementation Notes
- All 16 endpoints follow existing payment + rate-limiting + semaphore patterns
- GDAL 0.19 API used throughout: no `centroid()` (uses envelope midpoint), predicates return `bool`
- `make_valid()` uses `CslStringList::new()` opts; buffer-by-zero fallback for older GEOS
- Binary outputs (Shapefile, KML, GPKG) returned base64-encoded in `result.data`

### Tests
- All 32 pre-existing tests pass
- New endpoint coverage: compile + integration tested (route registration, handler plumbing)

---

## Phase 5 — Hardening (historical, 2026-03-12)

### Mock RPC Payment Integration Tests (`tests/payment_integration.rs`)
- Added `wiremock = "0.6"` as dev dependency
- Extracted `verify_payment_inmem()` for DB-free testing against mock Solana RPC
- Exposed `verify_payment_with_client()` for injectable HTTP client in production
- 8 payment integration tests: valid payment, missing header shape, duplicate sig, insufficient
  amount, wrong memo, RPC null result, failed on-chain tx, RPC error response
- All tests run with no real Solana RPC or Postgres required

### Load Test Harness (`tests/load_test.rs`)
- 50 concurrent `/v1/reproject` requests — all complete without 500 or panic
- Rate limiter assertion: 70 rapid GIS requests → at least some 429s as expected

### Criterion Benchmarks (`benches/gis_bench.rs`)
- Benchmarks for `reproject`, `buffer`, `dissolve`

### Behavioral Parity Analysis (`PARITY.md`)
- Documented all differences between Rust and Python v0.7.0 implementations

### Bug Fix — Buffer negative distance
- Added `distance <= 0.0` guard in `do_buffer()` → 400 Bad Request

---

## Phase 4 — Observability & Polish (historical)

- Prometheus `/metrics` endpoint live
- Full utoipa OpenAPI annotations on all four GIS endpoints + batch
- `/docs` serves complete Swagger UI

---

## Phase 3 — Payments (historical)

- File-size based pricing ($0.01/MB, min $0.01, cap $2.00)
- 402 → pay → retry flow with `X-PAYMENT` header
- Payment idempotency via `used_signatures` PostgreSQL table
- Operations log (`operations_log` table) for all paid and dev-mode operations
- Earlier Solana USDC verification notes in this section are historical and no longer describe the current x402/Base payment path

---

## Phase 2 — Core GIS (historical)

- `/v1/reproject`, `/v1/buffer`, `/v1/clip`, `/v1/dissolve`, `/v1/batch`
- Input size limits (50 MB), file type validation, per-op 30s timeouts
- Tower rate limiting middleware (60 req/min per IP)
- GDAL semaphore cap (8 concurrent threads)

---

## Phase 1 — Skeleton (historical)

- Axum scaffold with SQLx and Tower
- `/v1/health` endpoint
- Request ID middleware, structured logging, OpenAPI scaffold
