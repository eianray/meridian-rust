// Library crate root — exposes internal modules for integration tests
// and benchmark harnesses without duplicating the binary's main.rs.

pub mod billing;
pub mod config;
pub mod error;
pub mod gis;
pub mod metrics;
pub mod middleware;
pub mod routes;

use std::sync::Arc;

/// Shared application state (mirrors main.rs AppState).
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<config::AppConfig>,
    pub db: Option<sqlx::PgPool>,
}
