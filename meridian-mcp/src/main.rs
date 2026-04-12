mod client;
mod config;
mod tools;

use anyhow::Result;
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ErrorData, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::io::stdio,
};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

use tools::{
    AddFieldParams, BatchParams, BufferParams, CalculateGeometryParams, ClipParams,
    ColorReliefParams, ContoursParams, ConvertParams, DissolveParams, ElevationFetchParams,
    ExportGisParams, ExportJgwParams, GeoJsonParams, MosaicParams, PackageGdbParams,
    PdfRasterizeParams, RasterCalcParams, RasterConvertParams, RasterGeoreferenceParams,
    RasterParams, RasterToVectorParams, RasterWarpParams, ReclassifyParams, ReprojectParams,
    SpatialJoinParams, TwoLayerParams, VectorizeParams,
};

/// Empty parameters for the health check tool
#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct HealthParams {}

#[derive(Debug, Clone)]
pub struct MeridianServer {
    tool_router: ToolRouter<Self>,
    config: std::sync::Arc<config::Config>,
    client: reqwest::Client,
}

#[tool_router]
impl MeridianServer {
    pub fn new(config: config::Config) -> Self {
        Self {
            tool_router: Self::tool_router(),
            config: std::sync::Arc::new(config),
            client: client::build_client(),
        }
    }

    // ── health ────────────────────────────────────────────────────────────

    #[tool(description = "Check the health of the Meridian API")]
    async fn meridian_health(
        &self,
        Parameters(HealthParams {}): Parameters<HealthParams>,
    ) -> Result<String, ErrorData> {
        let url = format!("{}/v1/health", self.config.base_url);
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ErrorData::internal_error(format!("HTTP error: {e}"), None))?;
        let json: Value = response
            .json::<Value>()
            .await
            .map_err(|e| ErrorData::internal_error(format!("JSON parse error: {e}"), None))?;
        Ok(serde_json::to_string_pretty(&json).unwrap_or_else(|_| json.to_string()))
    }

    // ── schema / validation ───────────────────────────────────────────────

    #[tool(description = "Extract field names, types, CRS, geometry type, feature count, and bbox from a GeoJSON layer")]
    async fn meridian_schema(
        &self,
        Parameters(p): Parameters<GeoJsonParams>,
    ) -> Result<String, ErrorData> {
        tools::schema(&self.config, &self.client, p).await
    }

    #[tool(description = "Run a geometry validity report via GEOS IsValid on a GeoJSON layer")]
    async fn meridian_validate(
        &self,
        Parameters(p): Parameters<GeoJsonParams>,
    ) -> Result<String, ErrorData> {
        tools::validate(&self.config, &self.client, p).await
    }

    #[tool(description = "Fix invalid geometries in a GeoJSON layer via GEOS MakeValid")]
    async fn meridian_repair(
        &self,
        Parameters(p): Parameters<GeoJsonParams>,
    ) -> Result<String, ErrorData> {
        tools::repair(&self.config, &self.client, p).await
    }

    // ── core GIS ──────────────────────────────────────────────────────────

    #[tool(description = "Merge features by attribute field in a GeoJSON layer (dissolve)")]
    async fn meridian_dissolve(
        &self,
        Parameters(p): Parameters<DissolveParams>,
    ) -> Result<String, ErrorData> {
        tools::dissolve(&self.config, &self.client, p).await
    }

    #[tool(description = "Reproject a GeoJSON layer to any GDAL CRS string (e.g. EPSG:3857)")]
    async fn meridian_reproject(
        &self,
        Parameters(p): Parameters<ReprojectParams>,
    ) -> Result<String, ErrorData> {
        tools::reproject(&self.config, &self.client, p).await
    }

    #[tool(description = "Buffer GeoJSON features by a distance in meters (auto-UTM projection)")]
    async fn meridian_buffer(
        &self,
        Parameters(p): Parameters<BufferParams>,
    ) -> Result<String, ErrorData> {
        tools::buffer(&self.config, &self.client, p).await
    }

    // ── format conversion ─────────────────────────────────────────────────

    #[tool(description = "Convert GeoJSON to another format: geojson, shapefile, kml, or gpkg")]
    async fn meridian_convert(
        &self,
        Parameters(p): Parameters<ConvertParams>,
    ) -> Result<String, ErrorData> {
        tools::convert(&self.config, &self.client, p).await
    }

    // ── geometry transforms ───────────────────────────────────────────────

    #[tool(description = "Delete all features from a GeoJSON layer, preserving the empty schema")]
    async fn meridian_erase(
        &self,
        Parameters(p): Parameters<GeoJsonParams>,
    ) -> Result<String, ErrorData> {
        tools::erase(&self.config, &self.client, p).await
    }

    #[tool(description = "Convert GeoJSON geometries to centroid points")]
    async fn meridian_feature_to_point(
        &self,
        Parameters(p): Parameters<GeoJsonParams>,
    ) -> Result<String, ErrorData> {
        tools::feature_to_point(&self.config, &self.client, p).await
    }

    #[tool(description = "Extract polygon boundaries as LineStrings from a GeoJSON layer")]
    async fn meridian_feature_to_line(
        &self,
        Parameters(p): Parameters<GeoJsonParams>,
    ) -> Result<String, ErrorData> {
        tools::feature_to_line(&self.config, &self.client, p).await
    }

    #[tool(description = "Polygonize closed LineString geometries in a GeoJSON layer")]
    async fn meridian_feature_to_polygon(
        &self,
        Parameters(p): Parameters<GeoJsonParams>,
    ) -> Result<String, ErrorData> {
        tools::feature_to_polygon(&self.config, &self.client, p).await
    }

    #[tool(description = "Explode multipart geometries to single parts in a GeoJSON layer")]
    async fn meridian_multipart_to_singlepart(
        &self,
        Parameters(p): Parameters<GeoJsonParams>,
    ) -> Result<String, ErrorData> {
        tools::multipart_to_singlepart(&self.config, &self.client, p).await
    }

    #[tool(description = "Add an attribute column with an optional typed default to a GeoJSON layer")]
    async fn meridian_add_field(
        &self,
        Parameters(p): Parameters<AddFieldParams>,
    ) -> Result<String, ErrorData> {
        tools::add_field(&self.config, &self.client, p).await
    }

    // ── topology / two-input ──────────────────────────────────────────────

    #[tool(description = "Clip a GeoJSON layer to a polygon mask")]
    async fn meridian_clip(
        &self,
        Parameters(p): Parameters<ClipParams>,
    ) -> Result<String, ErrorData> {
        tools::clip(&self.config, &self.client, p).await
    }

    #[tool(description = "Combine all features from two GeoJSON layers (union)")]
    async fn meridian_union(
        &self,
        Parameters(p): Parameters<TwoLayerParams>,
    ) -> Result<String, ErrorData> {
        tools::union(&self.config, &self.client, p).await
    }

    #[tool(description = "Spatial intersection of two GeoJSON layers")]
    async fn meridian_intersect(
        &self,
        Parameters(p): Parameters<TwoLayerParams>,
    ) -> Result<String, ErrorData> {
        tools::intersect(&self.config, &self.client, p).await
    }

    #[tool(description = "Subtract the intersection of layer_b from layer_a (difference)")]
    async fn meridian_difference(
        &self,
        Parameters(p): Parameters<TwoLayerParams>,
    ) -> Result<String, ErrorData> {
        tools::difference(&self.config, &self.client, p).await
    }

    #[tool(description = "Add features from layer_b into layer_a's schema (append)")]
    async fn meridian_append(
        &self,
        Parameters(p): Parameters<TwoLayerParams>,
    ) -> Result<String, ErrorData> {
        tools::append(&self.config, &self.client, p).await
    }

    #[tool(description = "Combine two GeoJSON layers preserving all fields from both (merge)")]
    async fn meridian_merge(
        &self,
        Parameters(p): Parameters<TwoLayerParams>,
    ) -> Result<String, ErrorData> {
        tools::merge(&self.config, &self.client, p).await
    }

    #[tool(description = "Join attributes from layer_b onto layer_a by spatial predicate (intersects/contains/within)")]
    async fn meridian_spatial_join(
        &self,
        Parameters(p): Parameters<SpatialJoinParams>,
    ) -> Result<String, ErrorData> {
        tools::spatial_join(&self.config, &self.client, p).await
    }

    // ── raster / DEM ──────────────────────────────────────────────────────

    #[tool(description = "Generate a shaded relief image from a base64-encoded GeoTIFF DEM")]
    async fn meridian_hillshade(
        &self,
        Parameters(p): Parameters<RasterParams>,
    ) -> Result<String, ErrorData> {
        tools::hillshade(&self.config, &self.client, p).await
    }

    #[tool(description = "Compute terrain slope from a base64-encoded GeoTIFF DEM")]
    async fn meridian_slope(
        &self,
        Parameters(p): Parameters<RasterParams>,
    ) -> Result<String, ErrorData> {
        tools::slope(&self.config, &self.client, p).await
    }

    #[tool(description = "Compute terrain aspect (direction of max slope) from a base64-encoded GeoTIFF DEM")]
    async fn meridian_aspect(
        &self,
        Parameters(p): Parameters<RasterParams>,
    ) -> Result<String, ErrorData> {
        tools::aspect(&self.config, &self.client, p).await
    }

    #[tool(description = "Compute terrain roughness index from a base64-encoded GeoTIFF DEM")]
    async fn meridian_roughness(
        &self,
        Parameters(p): Parameters<RasterParams>,
    ) -> Result<String, ErrorData> {
        tools::roughness(&self.config, &self.client, p).await
    }

    #[tool(description = "Apply color ramp to a base64-encoded GeoTIFF DEM to produce a colored relief image")]
    async fn meridian_color_relief(
        &self,
        Parameters(p): Parameters<ColorReliefParams>,
    ) -> Result<String, ErrorData> {
        tools::color_relief(&self.config, &self.client, p).await
    }

    #[tool(description = "Generate contour lines as GeoJSON from a base64-encoded GeoTIFF DEM")]
    async fn meridian_contours(
        &self,
        Parameters(p): Parameters<ContoursParams>,
    ) -> Result<String, ErrorData> {
        tools::contours(&self.config, &self.client, p).await
    }

    #[tool(description = "Evaluate a raster math expression on one or two base64-encoded GeoTIFF inputs (e.g. slope classification, unit conversion)")]
    async fn meridian_raster_calc(
        &self,
        Parameters(p): Parameters<RasterCalcParams>,
    ) -> Result<String, ErrorData> {
        tools::raster_calc(&self.config, &self.client, p).await
    }

    #[tool(description = "Convert a base64-encoded GeoTIFF to another raster format (tif, png, jpg)")]
    async fn meridian_raster_convert(
        &self,
        Parameters(p): Parameters<RasterConvertParams>,
    ) -> Result<String, ErrorData> {
        tools::raster_convert(&self.config, &self.client, p).await
    }

    #[tool(description = "Reproject a base64-encoded GeoTIFF to a target CRS using gdalwarp")]
    async fn meridian_raster_warp(
        &self,
        Parameters(p): Parameters<RasterWarpParams>,
    ) -> Result<String, ErrorData> {
        tools::raster_warp(&self.config, &self.client, p).await
    }

    #[tool(description = "Polygonize a base64-encoded GeoTIFF raster to a GeoJSON vector layer")]
    async fn meridian_raster_to_vector(
        &self,
        Parameters(p): Parameters<RasterToVectorParams>,
    ) -> Result<String, ErrorData> {
        tools::raster_to_vector(&self.config, &self.client, p).await
    }

    #[tool(description = "Convert a GeoJSON layer to MBTiles vector tiles (Mapbox Vector Tiles)")]
    async fn meridian_vectorize(
        &self,
        Parameters(p): Parameters<VectorizeParams>,
    ) -> Result<String, ErrorData> {
        tools::vectorize(&self.config, &self.client, p).await
    }

    #[tool(description = "Reclassify a polygonized raster GeoJSON layer by gridcode into elevation or slope groups (workflow 1=elevation, 2=slope)")]
    async fn meridian_reclassify(
        &self,
        Parameters(p): Parameters<ReclassifyParams>,
    ) -> Result<String, ErrorData> {
        tools::reclassify(&self.config, &self.client, p).await
    }

    #[tool(description = "Calculate geometry properties (area, perimeter, length, x, y) and store as a new attribute field")]
    async fn meridian_calculate_geometry(
        &self,
        Parameters(p): Parameters<CalculateGeometryParams>,
    ) -> Result<String, ErrorData> {
        tools::calculate_geometry(&self.config, &self.client, p).await
    }

    #[tool(description = "Fetch a clipped IfSAR DTM GeoTIFF for a WGS84 AOI polygon from the Alaska DGGS elevation portal (Alaska only, EPSG:3338 output)")]
    async fn meridian_elevation_fetch(
        &self,
        Parameters(p): Parameters<ElevationFetchParams>,
    ) -> Result<String, ErrorData> {
        tools::elevation_fetch(&self.config, &self.client, p).await
    }

    #[tool(description = "Run multiple GIS operations in one request. Pass a JSON array of operations and up to two base64-encoded input files.")]
    async fn meridian_batch(
        &self,
        Parameters(p): Parameters<BatchParams>,
    ) -> Result<String, ErrorData> {
        tools::batch(&self.config, &self.client, p).await
    }

    #[tool(description = "Merge 2–4 base64-encoded GeoTIFF rasters into a single mosaic, optionally reprojecting and resampling")]
    async fn meridian_mosaic(
        &self,
        Parameters(p): Parameters<MosaicParams>,
    ) -> Result<String, ErrorData> {
        tools::mosaic(&self.config, &self.client, p).await
    }

    #[tool(description = "Package 1–3 GeoJSON layers into a File Geodatabase (GDB) zip archive")]
    async fn meridian_package_gdb(
        &self,
        Parameters(p): Parameters<PackageGdbParams>,
    ) -> Result<String, ErrorData> {
        tools::package_gdb(&self.config, &self.client, p).await
    }

    #[tool(description = "Rasterize a base64-encoded PDF to PNG images (one per page), optionally at a custom DPI")]
    async fn meridian_pdf_rasterize(
        &self,
        Parameters(p): Parameters<PdfRasterizeParams>,
    ) -> Result<String, ErrorData> {
        tools::pdf_rasterize(&self.config, &self.client, p).await
    }

    #[tool(description = "Georeference a base64-encoded raster image using ground control points (GCPs), returning a Cloud Optimized GeoTIFF")]
    async fn meridian_raster_georeference(
        &self,
        Parameters(p): Parameters<RasterGeoreferenceParams>,
    ) -> Result<String, ErrorData> {
        tools::raster_georeference(&self.config, &self.client, p).await
    }

    #[tool(description = "Export a base64-encoded georeferenced raster as a JPEG + ESRI world file (.jgw) using ground control points")]
    async fn meridian_export_jgw(
        &self,
        Parameters(p): Parameters<ExportJgwParams>,
    ) -> Result<String, ErrorData> {
        tools::export_jgw(&self.config, &self.client, p).await
    }

    #[tool(description = "Export a GeoJSON layer as DXF (AutoCAD) format, optionally reprojecting from a source CRS")]
    async fn meridian_export_dxf(
        &self,
        Parameters(p): Parameters<ExportGisParams>,
    ) -> Result<String, ErrorData> {
        tools::export_dxf(&self.config, &self.client, p).await
    }

    #[tool(description = "Export a GeoJSON layer as KML (Google Earth) format, optionally reprojecting from a source CRS")]
    async fn meridian_export_kml(
        &self,
        Parameters(p): Parameters<ExportGisParams>,
    ) -> Result<String, ErrorData> {
        tools::export_kml(&self.config, &self.client, p).await
    }

    #[tool(description = "Export a GeoJSON layer as a zipped Shapefile, optionally reprojecting from a source CRS")]
    async fn meridian_export_shapefile(
        &self,
        Parameters(p): Parameters<ExportGisParams>,
    ) -> Result<String, ErrorData> {
        tools::export_shapefile(&self.config, &self.client, p).await
    }
}

#[tool_handler]
impl ServerHandler for MeridianServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Meridian MCP server — GIS tools for the Meridian API (meridianapi.nodeapi.ai)".to_string())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cfg = config::Config::from_env();
    tracing::info!("meridian-mcp starting (base_url={})", cfg.base_url);

    // If SSE_PORT is set, run an HTTP server with the MCP Streamable HTTP transport.
    // Otherwise, fall back to stdio (used by Claude Desktop and local tooling).
    if let Ok(port_str) = std::env::var("SSE_PORT") {
        let port: u16 = port_str
            .parse()
            .map_err(|_| anyhow::anyhow!("SSE_PORT must be a valid port number, got: {port_str}"))?;

        use rmcp::transport::streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService,
            session::local::LocalSessionManager,
        };
        use tokio_util::sync::CancellationToken;

        let ct = CancellationToken::new();

        let service: StreamableHttpService<MeridianServer, LocalSessionManager> =
            StreamableHttpService::new(
                {
                    let cfg = cfg;
                    move || Ok(MeridianServer::new(cfg.clone()))
                },
                Arc::new(LocalSessionManager::default()),
                StreamableHttpServerConfig {
                    cancellation_token: ct.child_token(),
                    ..Default::default()
                },
            );

        let router = axum::Router::new().nest_service("/sse", service);
        let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
        let listener = tokio::net::TcpListener::bind(addr).await?;

        tracing::info!(%addr, "meridian-mcp SSE transport listening at http://{addr}/sse");

        // Graceful shutdown on Ctrl-C
        let shutdown = async move {
            let _ = tokio::signal::ctrl_c().await;
            ct.cancel();
        };

        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown)
            .await?;
    } else {
        let server = MeridianServer::new(cfg);
        let transport = stdio();
        rmcp::serve_server(server, transport).await?;
    }

    Ok(())
}
