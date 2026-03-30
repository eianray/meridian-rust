//! POST /v1/pdf/rasterize — Render PDF pages to JPEG using pdfium-render.
//! This is a free utility endpoint (no x402 charge) used by the DrawBridge thin client.

use axum::{extract::Extension, http::HeaderMap, Json};
use pdfium_render::prelude::*;
use serde::Serialize;
use std::time::Instant;
use utoipa::ToSchema;

use crate::{
    error::AppError,
    middleware::request_id::RequestId,
};

const MAX_PDF_BYTES: usize = 50 * 1024 * 1024; // 50 MB

#[derive(Serialize, ToSchema)]
pub struct PdfRasterizeResponse {
    pub request_id: String,
    pub pages: Vec<PageRender>,
}

#[derive(Serialize, ToSchema)]
pub struct PageRender {
    pub page_num: usize,
    pub width_px: u32,
    pub height_px: u32,
    pub jpeg_url: String,
}

/// POST /v1/pdf/rasterize
///
/// Accepts a PDF file upload and renders each page to a JPEG.
///
/// **Input (multipart/form-data):**
/// - `pdf` — PDF file (max 50 MB)
/// - `dpi` — integer, default 150
/// - `page` — optional integer (1-based page number). If omitted, renders all pages.
///
/// **Output (JSON):**
/// ```json
/// {
///   "request_id": "...",
///   "pages": [
///     { "page_num": 1, "width_px": 800, "height_px": 600, "jpeg_url": "/tmp/meridian_page1.jpg" }
///   ]
/// }
/// ```
#[utoipa::path(
    post,
    path = "/v1/pdf/rasterize",
    tag = "Utility",
    responses(
        (status = 200, description = "Rendered JPEG pages", body = PdfRasterizeResponse),
        (status = 400, description = "Bad request"),
        (status = 413, description = "Payload too large (>50 MB)"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn pdf_rasterize(
    Extension(RequestId(request_id)): Extension<RequestId>,
    _state: Extension<crate::AppState>,
    _headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<PdfRasterizeResponse>, AppError> {
    let t0 = Instant::now();

    let mut pdf_bytes: Option<Vec<u8>> = None;
    let mut dpi: u32 = 150;
    let mut single_page: Option<usize> = None;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        match field.name() {
            Some("pdf") => {
                let mut buf: Vec<u8> = Vec::new();
                while let Some(chunk) = field
                    .chunk()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("Error reading pdf: {e}")))?
                {
                    if buf.len() + chunk.len() > MAX_PDF_BYTES {
                        return Err(AppError::PayloadTooLarge);
                    }
                    buf.extend_from_slice(&chunk);
                }
                pdf_bytes = Some(buf);
            }
            Some("dpi") => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("dpi: {e}")))?;
                dpi = v.trim().parse().unwrap_or(150);
                dpi = dpi.clamp(72, 600);
            }
            Some("page") => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("page: {e}")))?;
                if !v.trim().is_empty() {
                    single_page = Some(
                        v.trim()
                            .parse::<usize>()
                            .map_err(|_| AppError::BadRequest("page must be a positive integer".into()))?,
                    );
                }
            }
            _ => {}
        }
    }

    let pdf_bytes = pdf_bytes.ok_or_else(|| AppError::BadRequest("Missing 'pdf' field".into()))?;

    // Render in a blocking thread
    let request_id_for_render = request_id.clone();
    let pages = tokio::task::spawn_blocking(move || {
        render_pdf_pages(&pdf_bytes, dpi, single_page, &request_id_for_render)
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Thread panic: {e}")))??;

    tracing::info!(pages = pages.len(), elapsed_ms = t0.elapsed().as_millis(), "pdf_rasterize ok");

    Ok(Json(PdfRasterizeResponse {
        request_id,
        pages,
    }))
}

fn render_pdf_pages(
    pdf_bytes: &[u8],
    dpi: u32,
    single_page: Option<usize>,
    request_id: &str,
) -> Result<Vec<PageRender>, AppError> {
    // Initialize pdfium — try system library first
    let bindings = Pdfium::bind_to_system_library()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Failed to load pdfium: {e}")))?;

    let pdfium = Pdfium::new(bindings);

    let document = pdfium
        .load_pdf_from_byte_slice(pdf_bytes, None)
        .map_err(|e| AppError::BadRequest(format!("Invalid PDF: {e}")))?;

    let page_count = document.pages().len() as usize;
    if page_count == 0 {
        return Err(AppError::BadRequest("PDF has no pages".into()));
    }

    // Determine which pages to render
    let page_indices: Vec<u16> = match single_page {
        Some(p) if p >= 1 && p <= page_count => vec![(p - 1) as u16],
        Some(p) => {
            return Err(AppError::BadRequest(format!(
                "Page {p} out of range (PDF has {page_count} pages)"
            )));
        }
        None => (0..document.pages().len()).collect(),
    };

    let scale = dpi as f32 / 72.0; // 72 DPI is the PDF default
    let mut pages = Vec::with_capacity(page_indices.len());

    for &page_idx in &page_indices {
        let page = match document.pages().get(page_idx) {
            Ok(p) => p,
            Err(e) => return Err(AppError::Internal(anyhow::anyhow!("Page {}: {}", page_idx, e))),
        };

        let width_pt = page.width().value;
        let height_pt = page.height().value;
        let width_px = (width_pt * scale).round() as u32;
        let height_px = (height_pt * scale).round() as u32;

        // Clamp to reasonable size
        let width_px = width_px.clamp(1, 8192);
        let height_px = height_px.clamp(1, 8192);

        let render_config = PdfRenderConfig::new()
            .set_fixed_size(width_px as i32, height_px as i32)
            .render_form_data(false);

        let bitmap = page
            .render_with_config(&render_config)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Render page {}: {e}", page_idx)))?;

        // Convert to JPEG
        let dynamic_image = bitmap.as_image();

        let mut jpeg_bytes: Vec<u8> = Vec::new();
        {
            let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_bytes, 85);
            encoder.encode_image(&dynamic_image)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("JPEG encode: {e}")))?;
        }

        // Write to /tmp
        let filename = format!("/tmp/meridian_{}_{}_{}x{}.jpg",
            request_id.replace("-", "_"),
            page_idx + 1,
            width_px,
            height_px,
        );
        std::fs::write(&filename, &jpeg_bytes)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Write JPEG: {e}")))?;

        pages.push(PageRender {
            page_num: page_idx as usize + 1,
            width_px,
            height_px,
            jpeg_url: filename,
        });
    }

    Ok(pages)
}
