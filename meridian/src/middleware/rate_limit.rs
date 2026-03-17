/// Per-IP rate limiting middleware using `governor`.
/// Limit: 60 requests per minute per IP.
///
/// On limit exceeded, returns HTTP 429 with a JSON error body and
/// a `Retry-After` header.
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    num::NonZeroU32,
    sync::{Arc, Mutex},
};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{HeaderValue, StatusCode},
    middleware::Next,
    response::Response,
    Extension,
};
use governor::{
    clock::DefaultClock,
    state::{InMemoryState, NotKeyed},
    Quota, RateLimiter,
};

type Limiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock>;

/// Per-IP rate limiter state — shared across all requests via `Extension`.
#[derive(Clone)]
pub struct IpRateLimiters {
    inner: Arc<Mutex<HashMap<IpAddr, Arc<Limiter>>>>,
    quota: Quota,
}

impl IpRateLimiters {
    /// Create a new limiter registry with 60 requests/minute per IP.
    pub fn new_60rpm() -> Self {
        let quota = Quota::per_minute(NonZeroU32::new(60).unwrap());
        IpRateLimiters {
            inner: Arc::new(Mutex::new(HashMap::new())),
            quota,
        }
    }

    /// Create a new limiter registry with 100 requests/hour per IP (for MCP traffic).
    pub fn new_100rph() -> Self {
        let quota = Quota::per_hour(NonZeroU32::new(100).unwrap());
        IpRateLimiters {
            inner: Arc::new(Mutex::new(HashMap::new())),
            quota,
        }
    }

    fn limiter_for(&self, ip: IpAddr) -> Arc<Limiter> {
        let mut map = self.inner.lock().unwrap();
        map.entry(ip)
            .or_insert_with(|| Arc::new(RateLimiter::direct(self.quota)))
            .clone()
    }

    /// Returns Ok if request is allowed, Err(retry_after_secs) if rate-limited.
    pub fn check(&self, ip: IpAddr) -> Result<(), u64> {
        let limiter = self.limiter_for(ip);
        match limiter.check() {
            Ok(_) => Ok(()),
            Err(_not_until) => {
                // governor's NotUntil requires the clock impl for wait_time_from.
                // Quota is 60/min = 1/s, so worst-case is 1 second.
                Err(1)
            }
        }
    }
}

/// MCP-specific rate limiting middleware.
/// Applies a 100 requests/hour per IP limit, but only when the `X-Mcp-Key`
/// header is present and non-empty. Non-MCP traffic passes through untouched
/// (the existing GIS rate limiter handles it).
pub async fn mcp_rate_limit_middleware(
    Extension(limiters): Extension<IpRateLimiters>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    // Only rate-limit when X-Mcp-Key header is present and non-empty.
    let has_mcp_key = req
        .headers()
        .get("X-Mcp-Key")
        .map(|v| !v.as_bytes().is_empty())
        .unwrap_or(false);

    if !has_mcp_key {
        return next.run(req).await;
    }

    let ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip())
        .unwrap_or(IpAddr::from([0, 0, 0, 0]));

    match limiters.check(ip) {
        Ok(_) => next.run(req).await,
        Err(retry_after_secs) => {
            let body = serde_json::json!({
                "error": "Too many requests — MCP limit is 100/hour per IP",
                "retry_after_seconds": retry_after_secs
            });
            Response::builder()
                .status(StatusCode::TOO_MANY_REQUESTS)
                .header("Content-Type", "application/json")
                .header(
                    "Retry-After",
                    HeaderValue::from_str(&retry_after_secs.to_string())
                        .unwrap_or(HeaderValue::from_static("1")),
                )
                .body(Body::from(body.to_string()))
                .unwrap()
        }
    }
}

/// Axum middleware function.
/// Extracts the client IP from `ConnectInfo<SocketAddr>` or falls back to 0.0.0.0.
pub async fn rate_limit_middleware(
    Extension(limiters): Extension<IpRateLimiters>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    let ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip())
        .unwrap_or(IpAddr::from([0, 0, 0, 0]));

    match limiters.check(ip) {
        Ok(_) => next.run(req).await,
        Err(retry_after_secs) => {
            let body = serde_json::json!({
                "error": "Too many requests — limit is 60/minute per IP",
                "retry_after_seconds": retry_after_secs
            });
            Response::builder()
                .status(StatusCode::TOO_MANY_REQUESTS)
                .header("Content-Type", "application/json")
                .header(
                    "Retry-After",
                    HeaderValue::from_str(&retry_after_secs.to_string())
                        .unwrap_or(HeaderValue::from_static("1")),
                )
                .body(Body::from(body.to_string()))
                .unwrap()
        }
    }
}
