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
    time::{Duration, Instant},
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
    /// Map of IP → (Limiter, last_seen Instant)
    inner: Arc<Mutex<HashMap<IpAddr, (Arc<Limiter>, Instant)>>>,
    quota: Quota,
}

const MAX_ENTRIES: usize = 10_000;
const EVICTION_CAP: usize = MAX_ENTRIES;
const TTL: Duration = Duration::from_secs(3600); // 1 hour

impl IpRateLimiters {
    /// Create a new limiter registry with 60 requests/minute per IP.
    pub fn new_60rpm() -> Self {
        let quota = Quota::per_minute(NonZeroU32::new(60).unwrap());
        IpRateLimiters {
            inner: Arc::new(Mutex::new(HashMap::new())),
            quota,
        }
    }

    fn evict_stale(&self, map: &mut HashMap<IpAddr, (Arc<Limiter>, Instant)>) {
        let now = Instant::now();
        map.retain(|_, (_, last_seen)| {
            now.duration_since(*last_seen) < TTL
        });
    }

    fn evict_oldest_pct(&self, map: &mut HashMap<IpAddr, (Arc<Limiter>, Instant)>, pct: f64) {
        let count = ((map.len() as f64) * pct).ceil() as usize;
        if count == 0 {
            return;
        }
        // Sort by last_seen ascending and retain only the newest entries
        let mut entries: Vec<_> = map.iter().collect();
        entries.sort_by_key(|(_, (_, last_seen))| *last_seen);
        let threshold_idx = entries.len().saturating_sub(count);
        let threshold = entries.get(threshold_idx).map(|(_, (_, t))| *t).unwrap_or_else(Instant::now);
        map.retain(|_, (_, last_seen)| *last_seen >= threshold);
    }

    fn limiter_for(&self, ip: IpAddr) -> Arc<Limiter> {
        let mut map = self.inner.lock().unwrap();
        self.evict_stale(&mut map);
        if map.len() > EVICTION_CAP {
            self.evict_oldest_pct(&mut map, 0.20);
        }
        let now = Instant::now();
        match map.entry(ip) {
            std::collections::hash_map::Entry::Occupied(e) => {
                let (limiter, last_seen) = e.into_mut();
                *last_seen = now;
                limiter.clone()
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                let limiter = Arc::new(RateLimiter::direct(self.quota));
                e.insert((limiter.clone(), now));
                limiter
            }
        }
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
