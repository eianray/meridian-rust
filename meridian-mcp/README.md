# meridian-mcp

An [MCP](https://modelcontextprotocol.io/) server that exposes Meridian GIS tools to Claude and other MCP-compatible clients. Built in Rust, communicates with the Meridian API at `v2.nodeapi.ai`.

---

## What it is

`meridian-mcp` bridges Claude Desktop (or any MCP host) to Meridian's hosted GIS processing API. Give Claude a GeoJSON blob and ask it to reproject, buffer, clip, run terrain analysis — it routes the work through this server to the Meridian backend.

---

## Installation

Requires Rust + Cargo. Clone the repo, then:

```bash
cd meridian-rust
cargo build --release -p meridian-mcp
# binary is at: target/release/meridian-mcp
```

---

## Claude Desktop configuration

Edit `~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "meridian": {
      "command": "/path/to/meridian-mcp",
      "env": {
        "MCP_API_KEY": "your-key-here",
        "MERIDIAN_BASE_URL": "https://meridianapi.nodeapi.ai"
      }
    }
  }
}
```

Replace `/path/to/meridian-mcp` with the absolute path to the compiled binary (e.g. `/Users/you/meridian-rust/target/release/meridian-mcp`).

---

## SSE mode

For SSE transport (connect over HTTP instead of stdio):

```bash
SSE_PORT=8103 ./meridian-mcp
```

The server will listen at `http://localhost:8103/sse`.

---

## Available tools

### Vector / GeoJSON

| Tool | Description |
|------|-------------|
| `schema` | Inspect the schema (field names and types) of a GeoJSON layer |
| `validate` | Validate GeoJSON geometry and report errors |
| `repair` | Attempt to repair invalid GeoJSON geometries |
| `dissolve` | Dissolve features, optionally grouped by a field |
| `erase` | Erase features (delete all features from a layer) |
| `feature_to_point` | Convert polygon/line features to representative points |
| `feature_to_line` | Convert polygon features to their boundary lines |
| `feature_to_polygon` | Convert line/point features to polygons |
| `multipart_to_singlepart` | Explode multipart features into individual single-part features |
| `reproject` | Reproject a layer to any GDAL-supported CRS (e.g. `EPSG:3857`) |
| `buffer` | Buffer features by a given distance in meters |
| `convert` | Convert GeoJSON to another format: `shapefile`, `kml`, or `gpkg` |
| `add_field` | Add a new attribute field with an optional default value |
| `clip` | Clip one layer to the extent of a mask polygon |
| `union` | Union two layers |
| `intersect` | Intersect two layers |
| `difference` | Compute the geometric difference between two layers |
| `append` | Append layer B onto layer A |
| `merge` | Merge two layers into one |
| `spatial_join` | Spatial join: transfer attributes from layer B to layer A by spatial predicate |

### Raster / Terrain (GeoTIFF, base64-encoded input)

| Tool | Description |
|------|-------------|
| `hillshade` | Generate a hillshade from a DEM raster |
| `slope` | Compute slope from a DEM raster |
| `aspect` | Compute aspect (facing direction) from a DEM raster |
| `roughness` | Compute terrain roughness from a DEM raster |

---

## Getting an API key

Email **hello@eianray.com** to request a Meridian MCP API key.
