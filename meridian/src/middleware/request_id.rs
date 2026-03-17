/// Request ID middleware — injects X-Request-ID header into every response.
/// Uses the incoming X-Request-ID if present, otherwise generates a new UUID v4.
use axum::{
    extract::Request,
    http::{HeaderName, HeaderValue},
    middleware::Next,
    response::Response,
};
use uuid::Uuid;

pub static X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

pub async fn request_id_middleware(mut req: Request, next: Next) -> Response {
    // Use provided request ID or generate one
    let id = req
        .headers()
        .get(&X_REQUEST_ID)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_owned())
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    // Inject into request extensions so handlers can access it
    req.extensions_mut().insert(RequestId(id.clone()));

    // Attach to request headers so tracing middleware sees it
    req.headers_mut().insert(
        X_REQUEST_ID.clone(),
        HeaderValue::from_str(&id).unwrap_or_else(|_| HeaderValue::from_static("invalid")),
    );

    let mut response = next.run(req).await;

    // Echo back on the response
    if let Ok(val) = HeaderValue::from_str(&id) {
        response.headers_mut().insert(X_REQUEST_ID.clone(), val);
    }

    response
}

/// Extractor — handlers can do `Extension(RequestId(id)): Extension<RequestId>`
#[derive(Clone, Debug)]
pub struct RequestId(pub String);
