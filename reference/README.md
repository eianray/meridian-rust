# Node API

**Version:** 0.5.2  
**Status:** Phase 3 — Public  
**Live at:** [nodeapi.ai](https://nodeapi.ai) | [API Docs](https://nodeapi.ai/docs) | [MCP](https://nodeapi.ai/mcp/sse)

Machine-native spatial data processing for AI agents and developers. 25 endpoints. Pay $0.01 USDC per operation on Solana. No accounts, no API keys, no subscriptions.

---

## Quick Start

```bash
# 1. Send a request — get a 402 with payment details
curl -X POST https://nodeapi.ai/v1/validate \
  -F "file=@parcels.geojson"

# → 402: { "recipient": "D8m8C9amSawdqSgEdXWkMZ3M86qsVQuDJswG1wZYkezP", "amount_usd": "0.01", ... }

# 2. Send 0.01 USDC to the recipient on Solana Mainnet
#    Get the transaction signature from your wallet

# 3. Retry with the transaction signature
curl -X POST https://nodeapi.ai/v1/validate \
  -F "file=@parcels.geojson" \
  -H "X-PAYMENT: <your_solana_tx_signature>"

# → 200: { "total_features": 1204, "valid_count": 1204, ... }
```

---

## Payment

Every paid endpoint uses **Solana Pay** with USDC on Solana Mainnet.

| | |
|---|---|
| **Protocol** | Solana Pay |
| **Network** | Solana Mainnet |
| **Asset** | USDC (`EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v`) |
| **Price** | $0.01 flat per operation |
| **Recipient** | `D8m8C9amSawdqSgEdXWkMZ3M86qsVQuDJswG1wZYkezP` |

**Flow:**
1. Call any endpoint → receive `402` with Solana Pay URL + recipient
2. Send 0.01 USDC on Solana Mainnet (include memo: `nodeapi:<operation>`)
3. Retry with `X-PAYMENT: <transaction_signature>`
4. Receive processed data

---

## Endpoints

All endpoints available at `/v1/<path>` (recommended) and `/<path>` (legacy, still supported).

### Info (free)
| Method | Path | Description |
|--------|------|-------------|
| GET | `/v1/health` | Service health |
| GET | `/v1/pricing` | Full pricing list |

### Format & Inspection
| Method | Path | Description |
|--------|------|-------------|
| POST | `/v1/convert` | Convert between GeoJSON, Shapefile, GeoPackage, KML |
| POST | `/v1/reproject` | Project or reproject to any EPSG (auto-detects assign vs transform) |
| POST | `/v1/validate` | Validate geometry — returns JSON report |
| POST | `/v1/repair` | Repair invalid geometry (make_valid) |
| POST | `/v1/schema` | Extract field schema, CRS, bbox, feature count |
| POST | `/v1/dxf` | Extract geometry from DXF/CAD files |

### Geoprocessing (single input)
| Method | Path | Description |
|--------|------|-------------|
| POST | `/v1/clip` | Clip to bounding box or polygon mask |
| POST | `/v1/buffer` | Buffer features by distance in meters |
| POST | `/v1/erase` | Delete all features, preserve empty schema |
| POST | `/v1/dissolve` | Dissolve features by attribute field |
| POST | `/v1/feature-to-point` | Convert geometries to centroid points |
| POST | `/v1/feature-to-line` | Extract polygon boundaries as lines |
| POST | `/v1/feature-to-polygon` | Convert closed lines to polygons |
| POST | `/v1/multipart-to-singlepart` | Explode multipart geometries |
| POST | `/v1/add-field` | Add a new attribute column |

### Geoprocessing (two inputs)
| Method | Path | Description |
|--------|------|-------------|
| POST | `/v1/union` | Combine all features from two layers |
| POST | `/v1/intersect` | Spatial intersection of two layers |
| POST | `/v1/difference` | Layer A minus overlap with layer B |
| POST | `/v1/append` | Append features from layer B into layer A's schema |
| POST | `/v1/merge` | Merge two layers, preserving all fields |
| POST | `/v1/spatial-join` | Join attributes by spatial relationship |

### Tiles
| Method | Path | Description |
|--------|------|-------------|
| POST | `/v1/vectorize` | Generate `.mbtiles` vector tile package (tippecanoe) |

### Jobs
| Method | Path | Description |
|--------|------|-------------|
| GET | `/v1/jobs/{id}` | Poll async job status |
| GET | `/v1/jobs/{id}/download` | Download completed job result |

**Supported input formats:** GeoJSON, Shapefile (.zip), GeoPackage (.gpkg), KML, GDB (read), DXF

---

## MCP Integration

Node API exposes all 25 endpoints as MCP tools — compatible with Claude, GPT-4o, and any MCP-enabled agent.

### Remote (no install required)
```
https://nodeapi.ai/mcp/sse
```
Connect any MCP client to this SSE endpoint. No authentication needed.

### Claude Desktop (local)
Add to `~/Library/Application Support/Claude/claude_desktop_config.json`:
```json
{
  "mcpServers": {
    "node-api": {
      "command": "/path/to/venv/bin/python",
      "args": ["-m", "app.mcp_server"],
      "cwd": "/path/to/node-api"
    }
  }
}
```

**22 MCP tools:** convert, reproject, validate, repair, schema, clip, dxf, buffer, union, intersect, difference, erase, dissolve, feature-to-point, feature-to-line, feature-to-polygon, multipart-to-singlepart, add-field, append, merge, spatial-join, pricing

---

## Webhooks

For async jobs (DXF conversion, any large file), pass a `webhook_url` to receive a POST on completion instead of polling.

```bash
curl -X POST https://nodeapi.ai/v1/dxf \
  -F "file=@site-plan.dxf" \
  -F "output_format=geojson" \
  -F "webhook_url=https://your-agent.com/callback" \
  -H "X-PAYMENT: <tx_signature>"
```

Webhook payload:
```json
{
  "job_id": "abc-123",
  "operation": "dxf",
  "status": "done",
  "result_url": "https://nodeapi.ai/v1/jobs/abc-123/download",
  "result_filename": "output.geojson",
  "size_bytes": 48291
}
```

Signed with `X-Webhook-Signature: sha256=<HMAC>` for verification.

---

## Architecture

- **FastAPI** — async, OpenAPI spec at `/docs`
- **GeoPandas + GDAL** — 200+ vector format support
- **Shapely 2.x** — geometry operations
- **pyproj** — CRS reprojection
- **tippecanoe 2.79** — vector tile generation
- **PostgreSQL** — operations log + job queue
- **Solana Pay** — USDC micropayments on Solana Mainnet
- **MCP SDK** — stdio (local) + SSE (remote) transport
- **Cloudflare Tunnel** — `nodeapi.ai` → API server

---

## Dev Setup

```bash
git clone https://github.com/eianray/node-api
cd node-api
python3 -m venv venv
venv/bin/pip install -r requirements.txt
cp .env.example .env
# Leave SOLANA_WALLET_ADDRESS=YOUR_SOLANA_WALLET_ADDRESS for dev mode (bypasses payment)
venv/bin/uvicorn app.main:app --reload --port 8100
```

---

## Roadmap

- **Phase 1 ✅** — Convert, reproject, validate, repair, schema, clip
- **Phase 2 ✅** — DXF/CAD, buffer, topology (union/intersect/difference), remote MCP SSE
- **Phase 3 ✅ (Mar 4 2026)** — 11 new geoprocessing endpoints, Solana Pay, webhooks, vector tiles, API versioning, 22 MCP tools
- **Phase 4** — Hetzner migration, CDN caching, batch API, agent directory listings

---

*nodeapi.ai — spatial processing infrastructure for the agent economy.*
