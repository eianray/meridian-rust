//! POST /v1/pdf/rasterize — Render PDF pages to JPEG using pdftoppm (poppler-utils).
//! This is a free utility endpoint (no x402 charge) used by the DrawBridge thin client.

use axum::{extract::Extension, http::HeaderMap, Json};
use base64::Engine;
use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;
use tokio::fs;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    error::AppError,
    middleware::request_id::RequestId,
};

const MAX_PDF_BYTES: usize = 50 * 1024 * 1024; // 50 MB
const DEFAULT_DPI: u32 = 150;

#[derive(Serialize, ToSchema)]
pub struct PdfRasterizeResponse {
    pub request_id: String,
    pub pages: Vec<PageRender>,
}

#[derive(Serialize, ToSchema)]
pub struct PageRender {
    /// 1-based page number
    pub page: usize,
    /// Image width in pixels
    pub width: u32,
    /// Image height in pixels
    pub height: u32,
    /// Base64-encoded JPEG image data
    pub data: String,
}

/// POST /v1/pdf/rasterize
///
/// Accepts a PDF file upload and renders each page to a JPEG using pdftoppm.
///
/// **Input (multipart/form-data):**
/// - `file` — PDF file (max 50 MB)
/// - `dpi` — integer, default 150 (clamped 72–600)
///
/// **Output (JSON):**
/// ```json
/// {
///   "request_id": "...",
///   "pages": [
///     { "page": 1, "width": 800, "height": 600, "data": "<base64_jpeg>" }
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
    let mut dpi: u32 = DEFAULT_DPI;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        match field.name() {
            Some("file") => {
                let mut buf: Vec<u8> = Vec::new();
                while let Some(chunk) = field
                    .chunk()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("Error reading file: {e}")))?
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
                dpi = v.trim().parse().unwrap_or(DEFAULT_DPI);
                dpi = dpi.clamp(72, 600);
            }
            _ => {}
        }
    }

    let pdf_bytes = pdf_bytes.ok_or_else(|| AppError::BadRequest("Missing 'file' field".into()))?;

    let pages = render_pdf_pages(&pdf_bytes, dpi).await?;

    tracing::info!(pages = pages.len(), elapsed_ms = t0.elapsed().as_millis(), "pdf_rasterize ok");

    Ok(Json(PdfRasterizeResponse {
        request_id,
        pages,
    }))
}

async fn render_pdf_pages(pdf_bytes: &[u8], dpi: u32) -> Result<Vec<PageRender>, AppError> {
    let id = Uuid::now_v7().to_string();

    let pdf_path = PathBuf::from(format!("/tmp/{id}.pdf"));
    let prefix = format!("/tmp/{id}_page");

    // Write uploaded PDF to temp file
    fs::write(&pdf_path, pdf_bytes)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Write temp PDF: {e}")))?;

    // Run pdftoppm to render all pages as JPEGs
    // pdftoppm -r <dpi> -jpeg -jpegopt quality=85 <pdf_path> <prefix>
    // produces: <prefix>-1.jpg, <prefix>-2.jpg, ...
    let output = Command::new("pdftoppm")
        .args([
            "-r", &dpi.to_string(),
            "-jpeg",
            "-jpegopt", "quality=85",
            pdf_path.to_str().unwrap(),
            &prefix,
        ])
        .output()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("pdftoppm spawn: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Internal(anyhow::anyhow!("pdftoppm failed: {stderr}")));
    }

    // Collect rendered page files
    let mut page_files: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir("/tmp")
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Read /tmp: {e}")))?
    {
        let entry = entry.map_err(|e| AppError::Internal(anyhow::anyhow!("Read dir entry: {e}")))?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with(&format!("{id}_page-")) && name.ends_with(".jpg") {
            page_files.push(path);
        }
    }

    if page_files.is_empty() {
        return Err(AppError::Internal(anyhow::anyhow!("pdftoppm produced no output files")));
    }

    // Sort by page number (filename format: <id>_page-<N>.jpg)
    page_files.sort_by(|a, b| {
        let num_a = a.file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.split('-').nth(1))
            .and_then(|n| n.strip_suffix(".jpg"))
            .unwrap_or("0")
            .parse::<usize>()
            .unwrap_or(0);
        let num_b = b.file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.split('-').nth(1))
            .and_then(|n| n.strip_suffix(".jpg"))
            .unwrap_or("0")
            .parse::<usize>()
            .unwrap_or(0);
        num_a.cmp(&num_b)
    });

    let mut pages = Vec::with_capacity(page_files.len());

    for path in &page_files {
        let jpeg_bytes = tokio::fs::read(path)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Read JPEG: {e}")))?;

        let (width, height) = read_jpeg_dimensions(&jpeg_bytes)
            .unwrap_or((0, 0));

        let data = base64::engine::general_purpose::STANDARD.encode(&jpeg_bytes);

        let page_num = path.file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.split('-').nth(1))
            .and_then(|n| n.strip_suffix(".jpg"))
            .unwrap_or("0")
            .parse::<usize>()
            .unwrap_or(0);

        pages.push(PageRender {
            page: page_num,
            width,
            height,
            data,
        });
    }

    // Clean up temp files
    let _ = fs::remove_file(&pdf_path).await;
    for path in &page_files {
        let _ = fs::remove_file(path).await;
    }

    Ok(pages)
}

/// Read JPEG dimensions from bytes without the `image` crate.
/// Format: SOI (0xFF 0xD8) → segment(s) → SOF0 (0xFF 0xC0) → length → precision → height width
fn read_jpeg_dimensions(jpeg_bytes: &[u8]) -> Option<(u32, u32)> {
    if jpeg_bytes.len() < 10 {
        return None;
    }
    if jpeg_bytes[0] != 0xFF || jpeg_bytes[1] != 0xD8 {
        return None; // Not a JPEG
    }

    let mut i = 2;
    while i < jpeg_bytes.len() - 8 {
        if jpeg_bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = jpeg_bytes[i + 1];

        // SOF0, SOF1, SOF2 markers all have the same structure
        if matches!(marker, 0xC0 | 0xC1 | 0xC2) {
            let length = ((jpeg_bytes[i + 2] as usize) << 8) | (jpeg_bytes[i + 3] as usize);
            if length < 9 || i + 8 >= jpeg_bytes.len() {
                return None;
            }
            let height = ((jpeg_bytes[i + 5] as u32) << 8) | (jpeg_bytes[i + 6] as u32);
            let width = ((jpeg_bytes[i + 7] as u32) << 8) | (jpeg_bytes[i + 8] as u32);
            return Some((width, height));
        }

        // Skip to next marker
        if i + 3 >= jpeg_bytes.len() {
            break;
        }
        let segment_len = ((jpeg_bytes[i + 2] as usize) << 8) | (jpeg_bytes[i + 3] as usize);
        i += 2 + segment_len;
    }

    None
}
