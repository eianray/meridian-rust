/// Concurrency stress test for GIS endpoints.
/// Sends 50 concurrent requests to /v1/reproject and verifies:
///   - All complete without panic or 500
///   - Rate limiter returns 429 for requests beyond 60/min per IP
///
/// Run with: cargo test --test load_test
use std::sync::Arc;

use axum::http::StatusCode;
use axum_test::TestServer;
use meridian::{AppState, config::AppConfig};

fn make_dev_state() -> AppState {
    AppState {
        config: Arc::new(AppConfig {
            host: "0.0.0.0".into(),
            port: 8100,
            log_level: "error".into(),
            database_url: None,
            dev_mode: true,
            wallet_address: None,
            x402_facilitator_url: "https://x402.org/facilitate".into(),
            mcp_api_key: None,
        }),
        db: None,
    }
}

fn build_test_app() -> axum::Router {
    use axum::{middleware as axum_middleware, routing::get, Router};
    use meridian::middleware::rate_limit::{rate_limit_middleware, IpRateLimiters};
    use meridian::middleware::request_id::request_id_middleware;
    use meridian::routes;
    use tower_http::trace::TraceLayer;

    let state = make_dev_state();
    let limiters = IpRateLimiters::new_60rpm();

    let gis_routes = routes::gis::router()
        .layer(axum_middleware::from_fn(rate_limit_middleware))
        .layer(axum::extract::Extension(limiters));

    Router::new()
        .route("/v1/health", get(meridian::routes::health::health))
        .merge(gis_routes)
        .layer(axum::extract::Extension(state))
        .layer(axum_middleware::from_fn(request_id_middleware))
        .layer(TraceLayer::new_for_http())
}

/// Valid 10-feature GeoJSON fixture.
fn make_10_feature_geojson() -> String {
    let features: Vec<serde_json::Value> = (0..10)
        .map(|i| {
            serde_json::json!({
                "type": "Feature",
                "properties": { "id": i },
                "geometry": {
                    "type": "Point",
                    "coordinates": [i as f64 * 0.1, i as f64 * 0.1]
                }
            })
        })
        .collect();

    serde_json::json!({
        "type": "FeatureCollection",
        "features": features
    })
    .to_string()
}

/// 50 concurrent reproject requests — all should complete, none should 500 or panic.
/// Uses tokio::task::LocalSet + spawn_local since axum_test::TestServer is !Send.
#[tokio::test]
async fn test_50_concurrent_reproject_no_panic_or_500() {
    let geojson = make_10_feature_geojson();
    let geojson_bytes = geojson.into_bytes();

    let local = tokio::task::LocalSet::new();

    let results: Arc<tokio::sync::Mutex<Vec<StatusCode>>> =
        Arc::new(tokio::sync::Mutex::new(Vec::with_capacity(50)));

    let results_clone = Arc::clone(&results);
    local
        .run_until(async move {
            let mut handles = Vec::with_capacity(50);
            for _ in 0..50 {
                let bytes = geojson_bytes.clone();
                let results_ref = Arc::clone(&results_clone);
                let handle = tokio::task::spawn_local(async move {
                    let server = TestServer::new(build_test_app()).unwrap();
                    let resp = server
                        .post("/v1/reproject")
                        .multipart(
                            axum_test::multipart::MultipartForm::new()
                                .add_text("target_crs", "EPSG:3857")
                                .add_part(
                                    "file",
                                    axum_test::multipart::Part::bytes(bytes)
                                        .file_name("input.geojson")
                                        .mime_type("application/geo+json"),
                                ),
                        )
                        .await;
                    let status = resp.status_code();
                    results_ref.lock().await.push(status);
                });
                handles.push(handle);
            }
            for h in handles {
                h.await.expect("task panicked");
            }
        })
        .await;

    let results = results.lock().await;

    let ok_count = results.iter().filter(|&&s| s == StatusCode::OK).count();
    let too_many = results
        .iter()
        .filter(|&&s| s == StatusCode::TOO_MANY_REQUESTS)
        .count();
    let other: Vec<_> = results
        .iter()
        .filter(|&&s| s != StatusCode::OK && s != StatusCode::TOO_MANY_REQUESTS)
        .copied()
        .collect();

    assert!(
        other.is_empty(),
        "Unexpected status codes (expected only 200/429): {other:?}"
    );
    assert!(ok_count > 0, "Expected at least some 200 responses");

    eprintln!(
        "load_test: 50 concurrent reproject → {ok_count} OK, {too_many} rate-limited"
    );
}

/// Verify rate limiter engages: 70 rapid requests to a GIS endpoint should produce 429s.
/// The rate limiter is applied to GIS routes (/v1/reproject etc.), not /v1/health.
/// We send a minimal but valid GeoJSON body to exercise the rate-limiter path.
#[tokio::test]
async fn test_rate_limiter_returns_429_beyond_threshold() {
    let local = tokio::task::LocalSet::new();
    let rate_limited_count: Arc<tokio::sync::Mutex<usize>> =
        Arc::new(tokio::sync::Mutex::new(0));
    let geojson = make_10_feature_geojson();

    let count_clone = Arc::clone(&rate_limited_count);
    local
        .run_until(async move {
            // Build one server so all requests share the same rate limiter
            let server = TestServer::new(build_test_app()).unwrap();
            for _ in 0..70 {
                let count_ref = Arc::clone(&count_clone);
                // Sequential requests to a GIS route — rate limiter fires after 60
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
                if resp.status_code() == StatusCode::TOO_MANY_REQUESTS {
                    *count_ref.lock().await += 1;
                }
            }
        })
        .await;

    let fired = *rate_limited_count.lock().await;
    assert!(
        fired > 0,
        "Expected some 429s from 70 rapid GIS requests against 60/min limiter, got 0"
    );
    eprintln!("load_test: rate limiter fired {fired}/70 times");
}
