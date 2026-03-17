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
    AddFieldParams, BufferParams, ClipParams, ConvertParams, DissolveParams, GeoJsonParams,
    RasterParams, ReprojectParams, SpatialJoinParams, TwoLayerParams,
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
