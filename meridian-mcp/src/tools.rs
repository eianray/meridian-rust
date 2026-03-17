use anyhow::anyhow;
use base64::Engine as _;
use rmcp::{model::ErrorData, schemars};
use serde::Deserialize;

use crate::{client, config};

// ─── helpers ────────────────────────────────────────────────────────────────

fn map_err(e: anyhow::Error) -> ErrorData {
    ErrorData::internal_error(e.to_string(), None)
}

// ─── parameter types ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GeoJsonParams {
    /// GeoJSON string (FeatureCollection or Feature)
    pub geojson: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DissolveParams {
    /// GeoJSON string (FeatureCollection or Feature)
    pub geojson: String,
    /// Optional field name to dissolve by
    pub field_name: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReprojectParams {
    /// GeoJSON string (FeatureCollection or Feature)
    pub geojson: String,
    /// Target CRS (any GDAL CRS string, e.g. "EPSG:4326")
    pub target_crs: String,
    /// Optional source CRS override
    pub source_crs: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BufferParams {
    /// GeoJSON string (FeatureCollection or Feature)
    pub geojson: String,
    /// Buffer distance in meters
    pub distance: f64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ConvertParams {
    /// GeoJSON string (FeatureCollection or Feature)
    pub geojson: String,
    /// Output format: "geojson", "shapefile", "kml", or "gpkg"
    pub output_format: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AddFieldParams {
    /// GeoJSON string (FeatureCollection or Feature)
    pub geojson: String,
    /// Name of the new field
    pub field_name: String,
    /// Field type (e.g. "string", "integer", "float")
    pub field_type: String,
    /// Optional default value
    pub default_value: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TwoLayerParams {
    /// GeoJSON for layer A
    pub layer_a: String,
    /// GeoJSON for layer B
    pub layer_b: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ClipParams {
    /// GeoJSON layer to clip
    pub layer: String,
    /// GeoJSON mask polygon
    pub mask: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SpatialJoinParams {
    /// GeoJSON for layer A (receives attributes)
    pub layer_a: String,
    /// GeoJSON for layer B (provides attributes)
    pub layer_b: String,
    /// Spatial predicate: "intersects", "contains", or "within"
    pub predicate: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RasterParams {
    /// Base64-encoded raster file (GeoTIFF)
    pub raster_base64: String,
}

// ─── server extension ───────────────────────────────────────────────────────
// All tool methods are defined as free functions that take &MeridianServer.
// They're implemented as inherent methods on MeridianServer via the tool_router
// macro in main.rs, but we put the actual implementations here to keep main.rs
// clean. The #[tool] attribute must be on the impl in main.rs; this file just
// provides the helper impls that main.rs calls.
//
// To avoid fighting the macro system, we export plain async functions that take
// (config, client) explicitly. main.rs wires them in.

pub async fn schema(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: GeoJsonParams,
) -> Result<String, ErrorData> {
    client::call_gis(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/schema", &p.geojson, vec![])
        .await
        .map_err(map_err)
}

pub async fn validate(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: GeoJsonParams,
) -> Result<String, ErrorData> {
    client::call_gis(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/validate", &p.geojson, vec![])
        .await
        .map_err(map_err)
}

pub async fn repair(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: GeoJsonParams,
) -> Result<String, ErrorData> {
    client::call_gis(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/repair", &p.geojson, vec![])
        .await
        .map_err(map_err)
}

pub async fn dissolve(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: DissolveParams,
) -> Result<String, ErrorData> {
    let mut extra = vec![];
    if let Some(f) = p.field_name {
        extra.push(("field_name".to_string(), f));
    }
    client::call_gis(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/dissolve", &p.geojson, extra)
        .await
        .map_err(map_err)
}

pub async fn erase(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: GeoJsonParams,
) -> Result<String, ErrorData> {
    client::call_gis(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/erase", &p.geojson, vec![])
        .await
        .map_err(map_err)
}

pub async fn feature_to_point(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: GeoJsonParams,
) -> Result<String, ErrorData> {
    client::call_gis(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/feature-to-point", &p.geojson, vec![])
        .await
        .map_err(map_err)
}

pub async fn feature_to_line(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: GeoJsonParams,
) -> Result<String, ErrorData> {
    client::call_gis(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/feature-to-line", &p.geojson, vec![])
        .await
        .map_err(map_err)
}

pub async fn feature_to_polygon(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: GeoJsonParams,
) -> Result<String, ErrorData> {
    client::call_gis(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/feature-to-polygon", &p.geojson, vec![])
        .await
        .map_err(map_err)
}

pub async fn multipart_to_singlepart(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: GeoJsonParams,
) -> Result<String, ErrorData> {
    client::call_gis(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/multipart-to-singlepart", &p.geojson, vec![])
        .await
        .map_err(map_err)
}

pub async fn reproject(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: ReprojectParams,
) -> Result<String, ErrorData> {
    let mut extra = vec![("target_crs".to_string(), p.target_crs)];
    if let Some(s) = p.source_crs {
        extra.push(("source_crs".to_string(), s));
    }
    client::call_gis(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/reproject", &p.geojson, extra)
        .await
        .map_err(map_err)
}

pub async fn buffer(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: BufferParams,
) -> Result<String, ErrorData> {
    let extra = vec![("distance".to_string(), p.distance.to_string())];
    client::call_gis(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/buffer", &p.geojson, extra)
        .await
        .map_err(map_err)
}

pub async fn convert(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: ConvertParams,
) -> Result<String, ErrorData> {
    let extra = vec![("output_format".to_string(), p.output_format)];
    client::call_gis(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/convert", &p.geojson, extra)
        .await
        .map_err(map_err)
}

pub async fn add_field(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: AddFieldParams,
) -> Result<String, ErrorData> {
    let mut extra = vec![
        ("field_name".to_string(), p.field_name),
        ("field_type".to_string(), p.field_type),
    ];
    if let Some(d) = p.default_value {
        extra.push(("default_value".to_string(), d));
    }
    client::call_gis(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/add-field", &p.geojson, extra)
        .await
        .map_err(map_err)
}

pub async fn clip(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: ClipParams,
) -> Result<String, ErrorData> {
    client::call_gis_two(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/clip", &p.layer, "mask", &p.mask, vec![])
        .await
        .map_err(map_err)
}

pub async fn union(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: TwoLayerParams,
) -> Result<String, ErrorData> {
    client::call_gis_two(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/union", &p.layer_a, "layer_b", &p.layer_b, vec![])
        .await
        .map_err(map_err)
}

pub async fn intersect(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: TwoLayerParams,
) -> Result<String, ErrorData> {
    client::call_gis_two(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/intersect", &p.layer_a, "layer_b", &p.layer_b, vec![])
        .await
        .map_err(map_err)
}

pub async fn difference(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: TwoLayerParams,
) -> Result<String, ErrorData> {
    client::call_gis_two(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/difference", &p.layer_a, "layer_b", &p.layer_b, vec![])
        .await
        .map_err(map_err)
}

pub async fn append(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: TwoLayerParams,
) -> Result<String, ErrorData> {
    client::call_gis_two(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/append", &p.layer_a, "layer_b", &p.layer_b, vec![])
        .await
        .map_err(map_err)
}

pub async fn merge(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: TwoLayerParams,
) -> Result<String, ErrorData> {
    client::call_gis_two(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/merge", &p.layer_a, "layer_b", &p.layer_b, vec![])
        .await
        .map_err(map_err)
}

pub async fn spatial_join(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: SpatialJoinParams,
) -> Result<String, ErrorData> {
    let mut extra = vec![];
    if let Some(pred) = p.predicate {
        extra.push(("predicate".to_string(), pred));
    }
    client::call_gis_two(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/spatial-join", &p.layer_a, "layer_b", &p.layer_b, extra)
        .await
        .map_err(map_err)
}

pub async fn hillshade(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: RasterParams,
) -> Result<String, ErrorData> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&p.raster_base64)
        .map_err(|e| map_err(anyhow!("base64 decode: {e}")))?;
    client::call_raster(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/hillshade", bytes)
        .await
        .map_err(map_err)
}

pub async fn slope(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: RasterParams,
) -> Result<String, ErrorData> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&p.raster_base64)
        .map_err(|e| map_err(anyhow!("base64 decode: {e}")))?;
    client::call_raster(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/slope", bytes)
        .await
        .map_err(map_err)
}

pub async fn aspect(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: RasterParams,
) -> Result<String, ErrorData> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&p.raster_base64)
        .map_err(|e| map_err(anyhow!("base64 decode: {e}")))?;
    client::call_raster(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/aspect", bytes)
        .await
        .map_err(map_err)
}

pub async fn roughness(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: RasterParams,
) -> Result<String, ErrorData> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&p.raster_base64)
        .map_err(|e| map_err(anyhow!("base64 decode: {e}")))?;
    client::call_raster(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/roughness", bytes)
        .await
        .map_err(map_err)
}
