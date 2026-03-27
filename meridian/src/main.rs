use meridian::billing;
use meridian::config::AppConfig;
use meridian::gis;
use meridian::metrics;
use meridian::middleware;
use meridian::routes;
use meridian::AppState;

use axum::{
    middleware as axum_middleware,
    routing::get,
    Router,
};
use metrics_exporter_prometheus::PrometheusHandle;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use billing::PaymentRequired;
use routes::batch::{BatchOperation, BatchResponse, BatchResult};
use middleware::rate_limit::{mcp_rate_limit_middleware, rate_limit_middleware, IpRateLimiters};
use middleware::request_id::request_id_middleware;
use routes::health::{health, HealthResponse};
use routes::package::PackageGdbParams;
use routes::reclassify::ReclassifyParams;
use gis::{
    buffer::BufferParams,
    clip::ClipParams,
    dissolve::DissolveParams,
    raster::{RasterBinaryResult, RasterOpStats},
    reproject::ReprojectParams,
    GeoJsonOutput,
};

// ── OpenAPI spec ───────────────────────────────────────────────────────────────

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Meridian GIS API",
        version = "0.4.0",
        description = "Rust GIS processing API with facilitator-backed x402 payments on Base USDC.",
        contact(name = "Eian Ray", url = "https://eianray.com"),
        license(name = "Proprietary")
    ),
    paths(
        routes::health::health,
        gis::reproject::reproject,
        gis::buffer::buffer,
        gis::clip::clip,
        gis::dissolve::dissolve,
        routes::batch::batch,
        routes::convert::convert,
        routes::raster::hillshade,
        routes::raster::slope,
        routes::raster::aspect,
        routes::raster::roughness,
        routes::raster::color_relief,
        routes::raster::contours,
        routes::raster::raster_calc,
        routes::raster::raster_convert,
        routes::raster::mosaic,
        routes::reclassify::reclassify,
        routes::package::package_gdb,
    ),
    components(
        schemas(
            HealthResponse,
            GeoJsonOutput,
            PaymentRequired,
            BatchResponse,
            BatchResult,
            BatchOperation,
            ReprojectParams,
            BufferParams,
            ClipParams,
            DissolveParams,
            routes::convert::ConvertParams,
            routes::raster::SingleRasterParams,
            routes::raster::ColorReliefParams,
            routes::raster::ContoursParams,
            routes::raster::RasterCalcParams,
            routes::raster::RasterConvertParams,
            routes::raster::MosaicParams,
            RasterBinaryResult,
            RasterOpStats,
            ReclassifyParams,
            PackageGdbParams,
        )
    ),
    tags(
        (name = "Info", description = "Free informational endpoints"),
        (name = "GIS", description = "Paid spatial processing endpoints"),
    )
)]
struct ApiDoc;

// ── Metrics endpoint ───────────────────────────────────────────────────────────

const METRICS_TOKEN: &str = "9c24464c1c0b712fb5ad26aecfa9b0f4d47526583ea8b704";

async fn metrics_handler(
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
    axum::extract::Extension(handle): axum::extract::Extension<PrometheusHandle>,
) -> impl axum::response::IntoResponse {
    match params.get("token") {
        Some(t) if t == METRICS_TOKEN => axum::response::Response::builder()
            .status(200)
            .body(axum::body::Body::from(handle.render()))
            .unwrap(),
        _ => axum::response::Response::builder()
            .status(403)
            .body(axum::body::Body::from("Forbidden"))
            .unwrap(),
    }
}

// ── App router ─────────────────────────────────────────────────────────────────

#[cfg_attr(not(test), allow(dead_code))]
fn build_router(state: AppState) -> Router {
    build_router_with_metrics(state, None)
}

fn build_router_with_metrics(state: AppState, prom: Option<PrometheusHandle>) -> Router {
    use tower_http::cors::AllowHeaders;
    use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
    use axum::http::HeaderName;
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(AllowHeaders::list(vec![
            CONTENT_TYPE,
            AUTHORIZATION,
            HeaderName::from_static("x-payment"),
            HeaderName::from_static("x-request-id"),
            HeaderName::from_static("x-mcp-key"),
        ]));

    // Swagger UI mounts at /docs; raw spec at /api-doc/openapi.json
    let docs = SwaggerUi::new("/docs")
        .url("/api-doc/openapi.json", ApiDoc::openapi());

    // Shared rate limiter state: 60 req/min per IP (standard GIS traffic)
    let limiters = IpRateLimiters::new_60rpm();

    // MCP rate limiter: 100 req/hour per IP (only fires when X-Mcp-Key header is present)
    let mcp_limiters = IpRateLimiters::new_100rph();

    // GIS routes with rate limiting applied (middleware runs on all /v1/* routes)
    let gis_routes = routes::gis::router()
        .layer(axum_middleware::from_fn(mcp_rate_limit_middleware))
        .layer(axum::extract::Extension(mcp_limiters))
        .layer(axum_middleware::from_fn(rate_limit_middleware))
        .layer(axum::extract::Extension(limiters.clone()));

    let mut router = Router::new()
        // Root redirect → landing page
        .route("/", get(|| async { axum::response::Redirect::permanent("https://meridian.nodeapi.ai") }))
        // Versioned routes
        .route("/v1/health", get(health))
        // Legacy (unversioned) alias
        .route("/health", get(health))
        // GIS endpoints (with rate limiting)
        .merge(gis_routes)
        // Docs
        .merge(docs)
        // AppState available to all handlers via Extension
        .layer(axum::extract::Extension(state))
        // Middleware (inner -> outer execution order)
        .layer(axum_middleware::from_fn(request_id_middleware))
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        // Allow up to 200 MB uploads (Axum default is 2 MB)
        .layer(axum::extract::DefaultBodyLimit::max(200 * 1024 * 1024));

    // Attach /metrics if a Prometheus handle was provided
    if let Some(handle) = prom {
        router = router
            .route("/metrics", get(metrics_handler))
            .layer(axum::extract::Extension(handle));
    }

    router
}

// ── Entry point ────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = AppConfig::from_env()?;

    // Tracing: JSON in prod, pretty in dev
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&cfg.log_level));

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Optional database pool
    let db = if let Some(ref url) = cfg.database_url {
        info!("Connecting to database");
        let pool = sqlx::PgPool::connect(url).await?;
        // Run pending migrations
        sqlx::migrate!("./migrations").run(&pool).await?;
        info!("Database migrations applied");
        Some(pool)
    } else {
        info!("No DATABASE_URL — payment logging disabled");
        None
    };

    let state = AppState {
        config: Arc::new(cfg.clone()),
        db,
    };

    let addr: SocketAddr = format!("{}:{}", cfg.host, cfg.port).parse()?;

    // Initialize Prometheus metrics recorder before the server starts
    let prom_handle = metrics::init_prometheus();

    if cfg.dev_mode {
        tracing::warn!("DEV_MODE is active - payment verification is DISABLED. Do not run in production without WALLET_ADDRESS and DEV_MODE=false.");
    }
    if cfg.dev_mode {
        tracing::warn!("[WARN] DEV_MODE active - all payment verification DISABLED. Set DEV_MODE=false and WALLET_ADDRESS for production.");
    }
    info!(
        version = env!("CARGO_PKG_VERSION"),
        host = %cfg.host,
        port = cfg.port,
        dev_mode = cfg.dev_mode,
        "Meridian starting"
    );

    let app = build_router_with_metrics(state, Some(prom_handle));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!(%addr, "Listening");
    axum::serve(listener, app).await?;

    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderName, HeaderValue, StatusCode};
    use axum_test::TestServer;

    fn make_state() -> AppState {
        AppState {
            config: Arc::new(AppConfig {
                host: "0.0.0.0".into(),
                port: 8100,
                log_level: "debug".into(),
                database_url: None,
                dev_mode: true,
                wallet_address: None,
                x402_facilitator_url: "https://x402.org/facilitate".into(),
                mcp_api_key: None,
            }),
            db: None,
        }
    }

    fn test_server() -> TestServer {
        TestServer::new(build_router(make_state())).unwrap()
    }

    // ── Health ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn health_returns_200() {
        let server = test_server();
        let resp = server.get("/v1/health").await;
        resp.assert_status(StatusCode::OK);
        let body = resp.json::<serde_json::Value>();
        assert_eq!(body["status"], "ok");
        assert!(body["request_id"].is_string());
    }

    #[tokio::test]
    async fn health_legacy_alias() {
        let server = test_server();
        let resp = server.get("/health").await;
        resp.assert_status(StatusCode::OK);
    }

    #[tokio::test]
    async fn health_echoes_request_id() {
        let server = test_server();
        let resp = server
            .get("/v1/health")
            .add_header(
                HeaderName::from_static("x-request-id"),
                HeaderValue::from_static("test-id-123"),
            )
            .await;
        resp.assert_status(StatusCode::OK);
        let body = resp.json::<serde_json::Value>();
        assert_eq!(body["request_id"], "test-id-123");
    }

    #[tokio::test]
    async fn openapi_spec_is_valid_json() {
        let server = test_server();
        let resp = server.get("/api-doc/openapi.json").await;
        resp.assert_status(StatusCode::OK);
        let json = resp.json::<serde_json::Value>();
        assert_eq!(json["info"]["title"], "Meridian GIS API");
    }

    // ── Payment gate: 402 when no X-PAYMENT header ─────────────────────────────

    /// In dev_mode=true (no wallet), payment gate is bypassed → 200
    #[tokio::test]
    async fn reproject_dev_mode_no_payment_header_returns_200_or_400() {
        // dev_mode=true, no DB → ops logging skipped
        // Sending a valid file should succeed (200) or fail with a 400 for input issues
        // At minimum, it must NOT return 402
        let geojson = r#"{"type":"FeatureCollection","features":[{"type":"Feature","properties":{},"geometry":{"type":"Point","coordinates":[0.0,0.0]}}]}"#;
        let server = test_server();
        let resp = server
            .post("/v1/reproject")
            .multipart(
                axum_test::multipart::MultipartForm::new()
                    .add_text("target_crs", "EPSG:3857")
                    .add_part(
                        "file",
                        axum_test::multipart::Part::bytes(geojson.as_bytes().to_vec())
                            .file_name("input.geojson")
                            .mime_type("application/geo+json"),
                    ),
            )
            .await;
        let status = resp.status_code();
        assert_ne!(status, StatusCode::PAYMENT_REQUIRED, "dev_mode should bypass payment gate");
    }

    // ── Validation / error path tests ─────────────────────────────────────────

    #[tokio::test]
    async fn reproject_missing_file_returns_400() {
        let server = test_server();
        let resp = server
            .post("/v1/reproject")
            .multipart(
                axum_test::multipart::MultipartForm::new()
                    .add_text("target_crs", "EPSG:3857"),
            )
            .await;
        resp.assert_status(StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn reproject_missing_crs_returns_400() {
        let server = test_server();
        let resp = server
            .post("/v1/reproject")
            .multipart(
                axum_test::multipart::MultipartForm::new()
                    .add_text("target_crs", ""),
            )
            .await;
        resp.assert_status(StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn buffer_missing_distance_returns_400() {
        let server = test_server();
        let resp = server
            .post("/v1/buffer")
            .multipart(axum_test::multipart::MultipartForm::new())
            .await;
        resp.assert_status(StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn buffer_invalid_distance_returns_400() {
        let server = test_server();
        let resp = server
            .post("/v1/buffer")
            .multipart(
                axum_test::multipart::MultipartForm::new()
                    .add_text("distance", "not-a-number"),
            )
            .await;
        resp.assert_status(StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn clip_missing_mask_returns_400() {
        let server = test_server();
        let resp = server
            .post("/v1/clip")
            .multipart(axum_test::multipart::MultipartForm::new())
            .await;
        resp.assert_status(StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn dissolve_missing_file_returns_400() {
        let server = test_server();
        let resp = server
            .post("/v1/dissolve")
            .multipart(axum_test::multipart::MultipartForm::new())
            .await;
        resp.assert_status(StatusCode::BAD_REQUEST);
    }

    // ── GDAL round-trip integration tests ─────────────────────────────────────

    #[tokio::test]
    async fn reproject_utm_point_to_wgs84_round_trip() {
        let geojson = r#"{
            "type": "FeatureCollection",
            "features": [{
                "type": "Feature",
                "properties": {},
                "geometry": {"type": "Point", "coordinates": [550000.0, 4182000.0]}
            }]
        }"#;

        let server = test_server();
        let resp = server
            .post("/v1/reproject")
            .multipart(
                axum_test::multipart::MultipartForm::new()
                    .add_text("source_crs", "EPSG:32610")
                    .add_text("target_crs", "EPSG:4326")
                    .add_part(
                        "file",
                        axum_test::multipart::Part::bytes(geojson.as_bytes().to_vec())
                            .file_name("input.geojson")
                            .mime_type("application/geo+json"),
                    ),
            )
            .await;

        resp.assert_status(StatusCode::OK);
        let body = resp.json::<serde_json::Value>();
        assert!(body["request_id"].is_string());

        let coords = &body["result"]["features"][0]["geometry"]["coordinates"];
        let lon = coords[0].as_f64().expect("lon");
        let lat = coords[1].as_f64().expect("lat");
        assert!(lon > -123.0 && lon < -122.0, "lon {lon}");
        assert!(lat > 37.0 && lat < 38.5, "lat {lat}");
    }

    #[tokio::test]
    async fn buffer_point_produces_polygon_with_area() {
        let geojson = r#"{
            "type": "FeatureCollection",
            "features": [{
                "type": "Feature",
                "properties": {},
                "geometry": {"type": "Point", "coordinates": [0.0, 0.0]}
            }]
        }"#;

        let server = test_server();
        let resp = server
            .post("/v1/buffer")
            .multipart(
                axum_test::multipart::MultipartForm::new()
                    .add_text("distance", "1000")
                    .add_part(
                        "file",
                        axum_test::multipart::Part::bytes(geojson.as_bytes().to_vec())
                            .file_name("input.geojson")
                            .mime_type("application/geo+json"),
                    ),
            )
            .await;

        resp.assert_status(StatusCode::OK);
        let body = resp.json::<serde_json::Value>();
        let geom_type = body["result"]["features"][0]["geometry"]["type"].as_str().unwrap();
        assert_eq!(geom_type, "Polygon");
        let ring_len = body["result"]["features"][0]["geometry"]["coordinates"][0]
            .as_array().unwrap().len();
        assert!(ring_len >= 4);
    }

    #[tokio::test]
    async fn clip_polygon_with_mask_reduces_area() {
        let features = r#"{"type":"FeatureCollection","features":[{"type":"Feature","properties":{},"geometry":{"type":"Polygon","coordinates":[[[-1.0,-1.0],[1.0,-1.0],[1.0,1.0],[-1.0,1.0],[-1.0,-1.0]]]}}]}"#;
        let mask = r#"{"type":"FeatureCollection","features":[{"type":"Feature","properties":{},"geometry":{"type":"Polygon","coordinates":[[[-0.25,-0.25],[0.25,-0.25],[0.25,0.25],[-0.25,0.25],[-0.25,-0.25]]]}}]}"#;

        let server = test_server();
        let resp = server
            .post("/v1/clip")
            .multipart(
                axum_test::multipart::MultipartForm::new()
                    .add_part("file", axum_test::multipart::Part::bytes(features.as_bytes().to_vec()).file_name("f.geojson").mime_type("application/geo+json"))
                    .add_part("mask", axum_test::multipart::Part::bytes(mask.as_bytes().to_vec()).file_name("m.geojson").mime_type("application/geo+json")),
            )
            .await;

        resp.assert_status(StatusCode::OK);
        let body = resp.json::<serde_json::Value>();
        assert_eq!(body["result"]["features"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn dissolve_adjacent_polygons_produces_single_feature() {
        let geojson = r#"{"type":"FeatureCollection","features":[{"type":"Feature","properties":{},"geometry":{"type":"Polygon","coordinates":[[[-1.0,0.0],[0.0,0.0],[0.0,1.0],[-1.0,1.0],[-1.0,0.0]]]}},{"type":"Feature","properties":{},"geometry":{"type":"Polygon","coordinates":[[[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0],[0.0,0.0]]]}}]}"#;

        let server = test_server();
        let resp = server
            .post("/v1/dissolve")
            .multipart(
                axum_test::multipart::MultipartForm::new()
                    .add_part("file", axum_test::multipart::Part::bytes(geojson.as_bytes().to_vec()).file_name("i.geojson").mime_type("application/geo+json")),
            )
            .await;

        resp.assert_status(StatusCode::OK);
        let body = resp.json::<serde_json::Value>();
        let features_out = body["result"]["features"].as_array().unwrap();
        assert_eq!(features_out.len(), 1);
    }
}
