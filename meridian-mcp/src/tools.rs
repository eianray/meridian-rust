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
pub struct ReclassifyParams {
    /// GeoJSON FeatureCollection string
    pub geojson: String,
    /// Workflow: "1" (elevation bands: 0-299/300-1000/>1000) or "2" (slope bands: 0-9/10-25/>25)
    pub workflow: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CalculateGeometryParams {
    /// GeoJSON string (FeatureCollection or Feature)
    pub geojson: String,
    /// Property to calculate: "area", "perimeter", "length", "x", "y" (default: "area")
    pub property: Option<String>,
    /// Output field name (defaults to property name)
    pub field_name: Option<String>,
    /// Area unit: "sqm", "sqkm", "sqft", "acres", "hectares" (default: "sqm")
    pub area_unit: Option<String>,
    /// Length unit: "m", "km", "ft", "mi" (default: "m")
    pub length_unit: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct VectorizeParams {
    /// GeoJSON string (FeatureCollection or Feature)
    pub geojson: String,
    /// Layer name in the MBTiles (default: "data")
    pub layer_name: Option<String>,
    /// Minimum zoom level 0-16 (default: 0)
    pub min_zoom: Option<u8>,
    /// Maximum zoom level 0-16 (default: 14)
    pub max_zoom: Option<u8>,
    /// Apply simplification (default: true)
    pub simplify: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ColorReliefParams {
    /// Base64-encoded raster GeoTIFF (DEM)
    pub raster_base64: String,
    /// Color table as text (e.g. "0 0 0 255\n100 255 255 0\n200 255 0 0")
    pub color_table: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ContoursParams {
    /// Base64-encoded raster GeoTIFF (DEM)
    pub raster_base64: String,
    /// Contour interval in raster units (default: 10.0)
    pub interval: Option<f64>,
    /// Offset from zero for first contour (default: 0.0)
    pub offset: Option<f64>,
    /// Attribute name for elevation values (default: "elevation")
    pub attribute_name: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RasterCalcParams {
    /// Base64-encoded raster GeoTIFF for variable A
    pub raster_a_base64: String,
    /// Base64-encoded raster GeoTIFF for variable B (optional)
    pub raster_b_base64: Option<String>,
    /// Expression using raster variable names A-Z (e.g. "A * 3.28084" or "where(A<10,1,2)")
    pub expression: String,
    /// Output type: "float32", "int32", etc. (optional)
    pub output_type: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RasterConvertParams {
    /// Base64-encoded raster GeoTIFF
    pub raster_base64: String,
    /// Output format: "tif", "png", "jpg" (default: "tif")
    pub output_format: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RasterWarpParams {
    /// Base64-encoded raster GeoTIFF
    pub raster_base64: String,
    /// Target CRS (e.g. "EPSG:3338")
    pub target_crs: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RasterToVectorParams {
    /// Base64-encoded raster GeoTIFF
    pub raster_base64: String,
    /// Band number to polygonize (default: 1)
    pub band: Option<u8>,
    /// Output field name for pixel values (default: "value")
    pub field_name: Option<String>,
    /// No-data value to exclude from output (optional)
    pub no_data: Option<f64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BatchParams {
    /// JSON array of operations: [{"op": "clip", "file_field": "f1", "mask_field": "f2"}, ...]
    pub operations: String,
    /// Base64-encoded file for field A (used as file_field in operations)
    pub file_a_base64: String,
    /// Base64-encoded file for field B (optional second file)
    pub file_b_base64: Option<String>,
    /// Whether files are rasters (true) or GeoJSON (false, default)
    pub raster_mode: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ElevationFetchParams {
    /// WGS84 GeoJSON Polygon string defining the AOI
    pub aoi_geojson: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MosaicParams {
    /// Base64-encoded raster 1 (required)
    pub raster_1_base64: String,
    /// Base64-encoded raster 2 (required)
    pub raster_2_base64: String,
    /// Base64-encoded raster 3 (optional)
    pub raster_3_base64: Option<String>,
    /// Base64-encoded raster 4 (optional)
    pub raster_4_base64: Option<String>,
    /// Target CRS for output (e.g. "EPSG:3338", default: CRS of first raster)
    pub output_crs: Option<String>,
    /// Output resolution in target CRS units (optional)
    pub resolution: Option<f64>,
    /// Resampling algorithm: "nearest", "bilinear", "cubic" (default: "nearest")
    pub resampling: Option<String>,
    /// NoData value to use (optional)
    pub nodata: Option<f64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PackageGdbParams {
    /// GeoJSON string for layer 1 (FeatureCollection)
    pub layer_1_geojson: String,
    /// Name for layer 1
    pub layer_1_name: String,
    /// GeoJSON string for layer 2 (optional)
    pub layer_2_geojson: Option<String>,
    /// Name for layer 2 (required if layer_2_geojson provided)
    pub layer_2_name: Option<String>,
    /// GeoJSON string for layer 3 (optional)
    pub layer_3_geojson: Option<String>,
    /// Name for layer 3 (required if layer_3_geojson provided)
    pub layer_3_name: Option<String>,
    /// Source CRS of all layers (default: "EPSG:4326")
    pub source_crs: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PdfRasterizeParams {
    /// Base64-encoded PDF bytes
    pub pdf_base64: String,
    /// Output DPI (default: 150)
    pub dpi: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RasterGeoreferenceParams {
    /// Base64-encoded raster image (TIFF or any GDAL-readable format, max 50 MB)
    pub raster_base64: String,
    /// JSON array of ground control points: [{"pixel_x":N,"pixel_y":N,"geo_x":N,"geo_y":N},...] (min 3)
    pub gcps_json: String,
    /// Output CRS (default: "EPSG:4326")
    pub output_crs: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ExportJgwParams {
    /// Base64-encoded raster image (GeoTIFF or any GDAL-readable format)
    pub raster_base64: String,
    /// JSON array of ground control points: [{"pixel_x":N,"pixel_y":N,"geo_x":N,"geo_y":N},...]
    pub gcps_json: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ExportGisParams {
    /// GeoJSON string (FeatureCollection or Feature)
    pub geojson: String,
    /// Input CRS (default: "EPSG:4326")
    pub input_crs: Option<String>,
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

// ─── new tools ───────────────────────────────────────────────────────────────

pub async fn reclassify(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: ReclassifyParams,
) -> Result<String, ErrorData> {
    let extra = vec![("workflow".to_string(), p.workflow)];
    client::call_gis(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/reclassify", &p.geojson, extra)
        .await
        .map_err(map_err)
}

pub async fn calculate_geometry(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: CalculateGeometryParams,
) -> Result<String, ErrorData> {
    let mut extra = vec![];
    if let Some(prop) = p.property { extra.push(("property".to_string(), prop)); }
    if let Some(f) = p.field_name { extra.push(("field_name".to_string(), f)); }
    if let Some(u) = p.area_unit { extra.push(("area_unit".to_string(), u)); }
    if let Some(u) = p.length_unit { extra.push(("length_unit".to_string(), u)); }
    client::call_gis(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/calculate-geometry", &p.geojson, extra)
        .await
        .map_err(map_err)
}

pub async fn vectorize(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: VectorizeParams,
) -> Result<String, ErrorData> {
    let mut extra = vec![];
    if let Some(n) = p.layer_name { extra.push(("layer_name".to_string(), n)); }
    if let Some(z) = p.min_zoom { extra.push(("min_zoom".to_string(), z.to_string())); }
    if let Some(z) = p.max_zoom { extra.push(("max_zoom".to_string(), z.to_string())); }
    if let Some(s) = p.simplify { extra.push(("simplify".to_string(), s.to_string())); }
    client::call_gis(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/vectorize", &p.geojson, extra)
        .await
        .map_err(map_err)
}

pub async fn color_relief(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: ColorReliefParams,
) -> Result<String, ErrorData> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&p.raster_base64)
        .map_err(|e| map_err(anyhow!("base64 decode: {e}")))?;
    client::call_raster_with_extra(
        client, &cfg.base_url, &cfg.mcp_api_key, "/v1/color-relief", bytes,
        vec![("color_table".to_string(), p.color_table)],
    ).await.map_err(map_err)
}

pub async fn contours(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: ContoursParams,
) -> Result<String, ErrorData> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&p.raster_base64)
        .map_err(|e| map_err(anyhow!("base64 decode: {e}")))?;
    let mut extra = vec![];
    if let Some(i) = p.interval { extra.push(("interval".to_string(), i.to_string())); }
    if let Some(o) = p.offset { extra.push(("offset".to_string(), o.to_string())); }
    if let Some(n) = p.attribute_name { extra.push(("attribute_name".to_string(), n)); }
    client::call_raster_with_extra(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/contours", bytes, extra)
        .await.map_err(map_err)
}

pub async fn raster_calc(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: RasterCalcParams,
) -> Result<String, ErrorData> {
    let bytes_a = base64::engine::general_purpose::STANDARD
        .decode(&p.raster_a_base64)
        .map_err(|e| map_err(anyhow!("base64 decode A: {e}")))?;
    let mut extra = vec![("expression".to_string(), p.expression)];
    if let Some(t) = p.output_type { extra.push(("output_type".to_string(), t)); }

    if let Some(b64_b) = p.raster_b_base64 {
        let bytes_b = base64::engine::general_purpose::STANDARD
            .decode(&b64_b)
            .map_err(|e| map_err(anyhow!("base64 decode B: {e}")))?;
        client::call_raster_calc_two(
            client, &cfg.base_url, &cfg.mcp_api_key, bytes_a, bytes_b, extra,
        ).await.map_err(map_err)
    } else {
        client::call_raster_with_slot(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/raster-calc", "A", bytes_a, extra)
            .await.map_err(map_err)
    }
}

pub async fn raster_convert(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: RasterConvertParams,
) -> Result<String, ErrorData> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&p.raster_base64)
        .map_err(|e| map_err(anyhow!("base64 decode: {e}")))?;
    let mut extra = vec![];
    if let Some(f) = p.output_format { extra.push(("output_format".to_string(), f)); }
    client::call_raster_with_extra(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/convert/raster", bytes, extra)
        .await.map_err(map_err)
}

pub async fn raster_warp(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: RasterWarpParams,
) -> Result<String, ErrorData> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&p.raster_base64)
        .map_err(|e| map_err(anyhow!("base64 decode: {e}")))?;
    let extra = vec![("target_crs".to_string(), p.target_crs)];
    client::call_raster_with_extra(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/raster-warp", bytes, extra)
        .await.map_err(map_err)
}

pub async fn raster_to_vector(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: RasterToVectorParams,
) -> Result<String, ErrorData> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&p.raster_base64)
        .map_err(|e| map_err(anyhow!("base64 decode: {e}")))?;
    let mut extra = vec![];
    if let Some(b) = p.band { extra.push(("band".to_string(), b.to_string())); }
    if let Some(f) = p.field_name { extra.push(("field_name".to_string(), f)); }
    if let Some(n) = p.no_data { extra.push(("no_data".to_string(), n.to_string())); }
    client::call_raster_with_extra(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/raster-to-vector", bytes, extra)
        .await.map_err(map_err)
}

pub async fn elevation_fetch(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: ElevationFetchParams,
) -> Result<String, ErrorData> {
    client::call_elevation_dggs(client, &cfg.base_url, &cfg.mcp_api_key, &p.aoi_geojson)
        .await
        .map_err(map_err)
}

pub async fn mosaic(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: MosaicParams,
) -> Result<String, ErrorData> {
    let url = format!("{}/v1/mosaic", cfg.base_url);

    let decode = |b64: &str, slot: &str| -> Result<Vec<u8>, ErrorData> {
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| map_err(anyhow!("base64 decode {slot}: {e}")))
    };

    let b1 = decode(&p.raster_1_base64, "raster_1")?;
    let b2 = decode(&p.raster_2_base64, "raster_2")?;

    let mk_part = |bytes: Vec<u8>| -> Result<reqwest::multipart::Part, ErrorData> {
        reqwest::multipart::Part::bytes(bytes)
            .file_name("input.tif")
            .mime_str("image/tiff")
            .map_err(|e| map_err(anyhow!("mime: {e}")))
    };

    let mut form = reqwest::multipart::Form::new()
        .part("file_1", mk_part(b1)?)
        .part("file_2", mk_part(b2)?);

    if let Some(b3) = p.raster_3_base64 { form = form.part("file_3", mk_part(decode(&b3, "raster_3")?)?); }
    if let Some(b4) = p.raster_4_base64 { form = form.part("file_4", mk_part(decode(&b4, "raster_4")?)?); }
    if let Some(crs) = p.output_crs { form = form.text("output_crs", crs); }
    if let Some(r) = p.resolution { form = form.text("resolution", r.to_string()); }
    if let Some(rs) = p.resampling { form = form.text("resampling", rs); }
    if let Some(n) = p.nodata { form = form.text("nodata", n.to_string()); }

    let resp = client.post(&url).header("X-Mcp-Key", &cfg.mcp_api_key).multipart(form).send()
        .await.map_err(|e| map_err(anyhow!("HTTP: {e}")))?;
    resp.text().await.map_err(|e| map_err(anyhow!("response: {e}")))
}

pub async fn package_gdb(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: PackageGdbParams,
) -> Result<String, ErrorData> {
    let url = format!("{}/v1/package/gdb", cfg.base_url);

    let mut form = reqwest::multipart::Form::new()
        .text("layer_1", p.layer_1_geojson)
        .text("name_1", p.layer_1_name);

    if let Some(g) = p.layer_2_geojson { form = form.text("layer_2", g); }
    if let Some(n) = p.layer_2_name { form = form.text("name_2", n); }
    if let Some(g) = p.layer_3_geojson { form = form.text("layer_3", g); }
    if let Some(n) = p.layer_3_name { form = form.text("name_3", n); }
    if let Some(c) = p.source_crs { form = form.text("source_crs", c); }

    let resp = client.post(&url).header("X-Mcp-Key", &cfg.mcp_api_key).multipart(form).send()
        .await.map_err(|e| map_err(anyhow!("HTTP: {e}")))?;
    resp.text().await.map_err(|e| map_err(anyhow!("response: {e}")))
}

pub async fn pdf_rasterize(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: PdfRasterizeParams,
) -> Result<String, ErrorData> {
    let url = format!("{}/v1/pdf/rasterize", cfg.base_url);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&p.pdf_base64)
        .map_err(|e| map_err(anyhow!("base64 decode: {e}")))?;

    let file_part = reqwest::multipart::Part::bytes(bytes)
        .file_name("input.pdf")
        .mime_str("application/pdf")
        .map_err(|e| map_err(anyhow!("mime: {e}")))?;

    let mut form = reqwest::multipart::Form::new().part("file", file_part);
    if let Some(dpi) = p.dpi { form = form.text("dpi", dpi.to_string()); }

    let resp = client.post(&url).header("X-Mcp-Key", &cfg.mcp_api_key).multipart(form).send()
        .await.map_err(|e| map_err(anyhow!("HTTP: {e}")))?;
    resp.text().await.map_err(|e| map_err(anyhow!("response: {e}")))
}

pub async fn raster_georeference(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: RasterGeoreferenceParams,
) -> Result<String, ErrorData> {
    let url = format!("{}/v1/raster-georeference", cfg.base_url);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&p.raster_base64)
        .map_err(|e| map_err(anyhow!("base64 decode: {e}")))?;

    let file_part = reqwest::multipart::Part::bytes(bytes)
        .file_name("input.tif")
        .mime_str("image/tiff")
        .map_err(|e| map_err(anyhow!("mime: {e}")))?;

    let mut form = reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("gcps", p.gcps_json);
    if let Some(crs) = p.output_crs { form = form.text("output_crs", crs); }

    let resp = client.post(&url).header("X-Mcp-Key", &cfg.mcp_api_key).multipart(form).send()
        .await.map_err(|e| map_err(anyhow!("HTTP: {e}")))?;
    resp.text().await.map_err(|e| map_err(anyhow!("response: {e}")))
}

pub async fn export_jgw(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: ExportJgwParams,
) -> Result<String, ErrorData> {
    let url = format!("{}/v1/export/jgw", cfg.base_url);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&p.raster_base64)
        .map_err(|e| map_err(anyhow!("base64 decode: {e}")))?;

    let file_part = reqwest::multipart::Part::bytes(bytes)
        .file_name("input.tif")
        .mime_str("image/tiff")
        .map_err(|e| map_err(anyhow!("mime: {e}")))?;

    let form = reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("gcps", p.gcps_json);

    let resp = client.post(&url).header("X-Mcp-Key", &cfg.mcp_api_key).multipart(form).send()
        .await.map_err(|e| map_err(anyhow!("HTTP: {e}")))?;
    resp.text().await.map_err(|e| map_err(anyhow!("response: {e}")))
}

pub async fn export_dxf(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: ExportGisParams,
) -> Result<String, ErrorData> {
    let mut extra = vec![];
    if let Some(crs) = p.input_crs { extra.push(("input_crs".to_string(), crs)); }
    client::call_gis(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/export/dxf", &p.geojson, extra)
        .await.map_err(map_err)
}

pub async fn export_kml(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: ExportGisParams,
) -> Result<String, ErrorData> {
    let mut extra = vec![];
    if let Some(crs) = p.input_crs { extra.push(("input_crs".to_string(), crs)); }
    client::call_gis(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/export/kml", &p.geojson, extra)
        .await.map_err(map_err)
}

pub async fn export_shapefile(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: ExportGisParams,
) -> Result<String, ErrorData> {
    let mut extra = vec![];
    if let Some(crs) = p.input_crs { extra.push(("input_crs".to_string(), crs)); }
    client::call_gis(client, &cfg.base_url, &cfg.mcp_api_key, "/v1/export/shapefile", &p.geojson, extra)
        .await.map_err(map_err)
}

pub async fn batch(
    cfg: &config::Config,
    client: &reqwest::Client,
    p: BatchParams,
) -> Result<String, ErrorData> {
    let bytes_a = base64::engine::general_purpose::STANDARD
        .decode(&p.file_a_base64)
        .map_err(|e| map_err(anyhow!("base64 decode A: {e}")))?;

    let is_raster = p.raster_mode.unwrap_or(false);
    let mime = if is_raster { "image/tiff" } else { "application/geo+json" };
    let filename_a = if is_raster { "a.tif" } else { "a.geojson" };

    let part_a = reqwest::multipart::Part::bytes(bytes_a)
        .file_name(filename_a)
        .mime_str(mime)
        .map_err(|e| map_err(anyhow!("mime: {e}")))?;

    let url = format!("{}/v1/batch", cfg.base_url);
    let mut form = reqwest::multipart::Form::new()
        .text("operations", p.operations)
        .part("file_a", part_a);

    if let Some(b64_b) = p.file_b_base64 {
        let bytes_b = base64::engine::general_purpose::STANDARD
            .decode(&b64_b)
            .map_err(|e| map_err(anyhow!("base64 decode B: {e}")))?;
        let filename_b = if is_raster { "b.tif" } else { "b.geojson" };
        let part_b = reqwest::multipart::Part::bytes(bytes_b)
            .file_name(filename_b)
            .mime_str(mime)
            .map_err(|e| map_err(anyhow!("mime: {e}")))?;
        form = form.part("file_b", part_b);
    }

    let resp = client
        .post(&url)
        .header("X-Mcp-Key", &cfg.mcp_api_key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| map_err(anyhow!("HTTP: {e}")))?;

    resp.text().await.map_err(|e| map_err(anyhow!("response: {e}")))
}
