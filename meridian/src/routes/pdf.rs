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
const MAX_PAGES: usize = 100;
const MAX_OUTPUT_BYTES: usize = 100 * 1024 * 1024; // 100 MB total base64 output

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
    let id = Uuid::new_v4().to_string();

    let pdf_path = PathBuf::from(format!("/tmp/{id}.pdf"));
    let prefix = format!("/tmp/{id}_page");

    // Write uploaded PDF to temp file
    fs::write(&pdf_path, pdf_bytes)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Write temp PDF: {e}")))?;

    // Run pdftoppm to render all pages as JPEGs
    let output = Command::new("pdftoppm")
        .args([
            "-r", &dpi.to_string(),
            "-jpeg",
            "-jpegopt", "quality=85",
            pdf_path.to_str().unwrap(),
            &prefix,
        ])
        .output()
        .map_err(|e| {
            let _ = std::fs::remove_file(&pdf_path);
            AppError::Internal(anyhow::anyhow!("pdftoppm spawn: {e}"))
        })?;

    if !output.status.success() {
        let _ = std::fs::remove_file(&pdf_path);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Internal(anyhow::anyhow!("pdftoppm failed: {stderr}")));
    }

    // Collect rendered page file paths
    let mut page_paths: Vec<PathBuf> = Vec::new();
    let entries = match std::fs::read_dir("/tmp") {
        Ok(e) => e,
        Err(e) => {
            let _ = std::fs::remove_file(&pdf_path);
            return Err(AppError::Internal(anyhow::anyhow!("Read /tmp: {e}")));
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                let _ = std::fs::remove_file(&pdf_path);
                return Err(AppError::Internal(anyhow::anyhow!("Read dir entry: {e}")));
            }
        };
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with(&format!("{id}_page-")) && name.ends_with(".jpg") {
            page_paths.push(path);
        }
    }

    if page_paths.is_empty() {
        let _ = std::fs::remove_file(&pdf_path);
        return Err(AppError::Internal(anyhow::anyhow!("pdftoppm produced no output files")));
    }

    // Sort by page number (filename format: <id>_page-<N>.jpg)
    page_paths.sort_by(|a, b| {
        let num = |p: &PathBuf| -> usize {
            p.file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.split('-').nth(1))
                .and_then(|n| n.strip_suffix(".jpg"))
                .unwrap_or("0")
                .parse()
                .unwrap_or(0)
        };
        num(a).cmp(&num(b))
    });

    // FIX 1: Page count cap — prevents huge PDFs from exhausting RAM
    if page_paths.len() > MAX_PAGES {
        let _ = std::fs::remove_file(&pdf_path);
        for p in &page_paths { let _ = std::fs::remove_file(p); }
        return Err(AppError::BadRequest(format!(
            "PDF exceeds {MAX_PAGES} page limit ({} pages)", page_paths.len()
        )));
    }

    // FIX 2: Read and encode pages — clean up all temp files on any error path
    let mut pages = Vec::with_capacity(page_paths.len());
    let mut total_bytes: usize = 0;

    for path in &page_paths {
        let jpeg_bytes = match fs::read(path).await {
            Ok(b) => b,
            Err(e) => {
                let _ = std::fs::remove_file(&pdf_path);
                for p in &page_paths { let _ = std::fs::remove_file(p); }
                return Err(AppError::Internal(anyhow::anyhow!("Read JPEG: {e}")));
            }
        };

        let (width, height) = read_jpeg_dimensions(&jpeg_bytes).unwrap_or((0, 0));
        let data = base64::engine::general_purpose::STANDARD.encode(&jpeg_bytes);

        // FIX 1: Total output byte cap — prevents RAM exhaustion from high-DPI renders
        total_bytes += data.len();
        if total_bytes > MAX_OUTPUT_BYTES {
            let _ = std::fs::remove_file(&pdf_path);
            for p in &page_paths { let _ = std::fs::remove_file(p); }
            return Err(AppError::Internal(anyhow::anyhow!("PDF output too large (exceeds 100 MB)")));
        }

        let page_num = path.file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.split('-').nth(1))
            .and_then(|n| n.strip_suffix(".jpg"))
            .unwrap_or("0")
            .parse::<usize>()
            .unwrap_or(0);

        pages.push(PageRender { page: page_num, width, height, data });
    }

    // FIX 2: Guaranteed cleanup on success path
    let _ = std::fs::remove_file(&pdf_path);
    for p in &page_paths { let _ = std::fs::remove_file(p); }

    Ok(pages)
}

/// Read JPEG dimensions from bytes without the `image` crate.
fn read_jpeg_dimensions(jpeg_bytes: &[u8]) -> Option<(u32, u32)> {
    if jpeg_bytes.len() < 10 {
        return None;
    }
    if jpeg_bytes[0] != 0xFF || jpeg_bytes[1] != 0xD8 {
        return None;
    }

    let mut i = 2;
    while i < jpeg_bytes.len() - 8 {
        if jpeg_bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = jpeg_bytes[i + 1];

        if matches!(marker, 0xC0 | 0xC1 | 0xC2) {
            let length = ((jpeg_bytes[i + 2] as usize) << 8) | (jpeg_bytes[i + 3] as usize);
            if length < 9 || i + 8 >= jpeg_bytes.len() {
                return None;
            }
            let height = ((jpeg_bytes[i + 5] as u32) << 8) | (jpeg_bytes[i + 6] as u32);
            let width = ((jpeg_bytes[i + 7] as u32) << 8) | (jpeg_bytes[i + 8] as u32);
            return Some((width, height));
        }

        if i + 3 >= jpeg_bytes.len() {
            break;
        }
        let segment_len = ((jpeg_bytes[i + 2] as usize) << 8) | (jpeg_bytes[i + 3] as usize);
        i += 2 + segment_len;
    }

    None
}
