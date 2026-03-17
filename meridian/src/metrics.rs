//! Prometheus metrics initialization and helpers.
//!
//! Metrics defined:
//! - `meridian_requests_total`           counter   labels: operation, status
//! - `meridian_request_duration_seconds` histogram labels: operation
//! - `meridian_payment_total`            counter   labels: operation, result (success|failed|dev)
//! - `meridian_gdal_duration_seconds`    histogram labels: operation

use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

/// Initialize the Prometheus exporter and return the handle used to render `/metrics`.
pub fn init_prometheus() -> PrometheusHandle {
    PrometheusBuilder::new()
        .install_recorder()
        .expect("Failed to install Prometheus metrics recorder")
}

// ── Thin recording helpers ────────────────────────────────────────────────────

/// Record one completed request.
pub fn record_request(operation: &str, status: &str) {
    let labels = [
        ("operation", operation.to_owned()),
        ("status", status.to_owned()),
    ];
    metrics::counter!("meridian_requests_total", 1, &labels[..]);
}

/// Record end-to-end request duration in seconds.
pub fn record_request_duration(operation: &str, secs: f64) {
    let labels = [("operation", operation.to_owned())];
    metrics::histogram!("meridian_request_duration_seconds", secs, &labels[..]);
}

/// Record payment gate outcome: `"success"`, `"failed"`, or `"dev"`.
pub fn record_payment(operation: &str, result: &str) {
    let labels = [
        ("operation", operation.to_owned()),
        ("result", result.to_owned()),
    ];
    metrics::counter!("meridian_payment_total", 1, &labels[..]);
}

/// Record time spent inside the GDAL blocking call in seconds.
pub fn record_gdal_duration(operation: &str, secs: f64) {
    let labels = [("operation", operation.to_owned())];
    metrics::histogram!("meridian_gdal_duration_seconds", secs, &labels[..]);
}
