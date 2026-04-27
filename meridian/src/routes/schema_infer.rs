//! POST /v1/schema/infer — Inspect a SHP file and return layer schema.
//!
//! Pure Rust using `shapefile` + `dbase` crates. No Python/Fiona required.
//!
//! **Multipart mode (file upload):**
//!   `POST /v1/schema/infer` with `multipart/form-data` and field `file`
//!
//! **URL mode (fetch from URL):**
//!   `POST /v1/schema/infer?url=https://example.com/data.zip`
//!   (URL param takes precedence — file field ignored if URL is provided)
//!
//! **Supported inputs:**
//!   - `.shp` file (multipart upload)
//!   - `.zip` containing one or more `.shp` files (first .shp in archive is used)
//!   - GDB files: route through `/v1/convert/gdb-to-shp` first, then here

use axum::{
    extract::{Extension, Query},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tokio::fs;
use uuid::Uuid;

use crate::{
    error::AppError,
    middleware::request_id::RequestId,
    AppState,
};

const MAX_FILE_BYTES: usize = 50 * 1024 * 1024;

#[derive(Serialize)]
pub struct SchemaInferResponse {
    pub request_id: String,
    pub layer_name: String,
    pub fields: Vec<SchemaField>,
    pub feature_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub geometry_type: Option<String>,
}

#[derive(Serialize)]
pub struct SchemaField {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub precision: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<usize>,
    pub nullable: bool,
}

#[derive(Deserialize)]
pub struct UrlQuery {
    pub url: Option<String>,
}

pub fn router() -> Router {
    Router::new().route("/v1/schema/infer", post(schema_infer_handler))
}

pub async fn schema_infer_handler(
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(_state): Extension<AppState>,
    Query(params): Query<UrlQuery>,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<SchemaInferResponse>, AppError> {
    let t0 = Instant::now();

    if let Some(url) = params.url {
        let out = handle_url_mode(&request_id, &url).await?;
        tracing::info!(
            request_id = %request_id,
            layer = %out.layer_name,
            fields = out.fields.len(),
            count = out.feature_count,
            elapsed_ms = t0.elapsed().as_millis(),
            "schema_infer url ok"
        );
        return Ok(Json(out));
    }

    let mut file_bytes: Option<Vec<u8>> = None;
    let mut detected_ext = String::new();

    while let Some(mut field) = multipart.next_field().await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        if field.name() == Some("file") {
            if let Some(fn_) = field.file_name().map(|s| s.to_string()) {
                detected_ext = fn_.rsplit('.').next().unwrap_or("").to_lowercase();
            }
            let mut buf = Vec::new();
            while let Some(chunk) = field.chunk().await
                .map_err(|e| AppError::BadRequest(format!("Read chunk: {e}")))?
            {
                if buf.len() + chunk.len() > MAX_FILE_BYTES {
                    return Err(AppError::PayloadTooLarge);
                }
                buf.extend_from_slice(&chunk);
            }
            file_bytes = Some(buf);
        }
    }

    let file_bytes = file_bytes.ok_or_else(|| AppError::BadRequest("Missing 'file' field".into()))?;
    let out = run_schema_inference(file_bytes, &detected_ext)?;
    let out = SchemaInferResponse {
        request_id: request_id.clone(),
        ..out
    };

    tracing::info!(
        request_id = %request_id,
        layer = %out.layer_name,
        fields = out.fields.len(),
        count = out.feature_count,
        elapsed_ms = t0.elapsed().as_millis(),
        "schema_infer ok"
    );

    Ok(Json(out))
}

async fn handle_url_mode(request_id: &str, url: &str) -> Result<SchemaInferResponse, AppError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("reqwest: {e}")))?;

    let resp = client.get(url).send().await
        .map_err(|_| AppError::BadGateway("fetch_failed".into()))?;

    if !resp.status().is_success() {
        return Err(AppError::BadGateway(format!("fetch_failed: HTTP {}", resp.status())));
    }

    let bytes = resp.bytes().await
        .map_err(|_| AppError::BadGateway("fetch_failed".into()))?;

    if bytes.len() > MAX_FILE_BYTES {
        return Err(AppError::PayloadTooLarge);
    }

    let ext = url.rsplit('.').next().unwrap_or("").to_lowercase();
    let mut out = run_schema_inference(bytes.to_vec(), &ext)?;
    out.request_id = request_id.to_string();
    Ok(out)
}

fn run_schema_inference(
    file_bytes: Vec<u8>,
    detected_ext: &str,
) -> Result<SchemaInferResponse, AppError> {
    let id = Uuid::new_v4().to_string();
    let tmp_dir = format!("/tmp/schema_infer_{id}");
    let tmp_path = if detected_ext == "shp" {
        format!("{tmp_dir}.shp")
    } else {
        format!("{tmp_dir}.zip")
    };

    std::fs::create_dir_all(&tmp_dir)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Create tmp dir: {e}")))?;
    std::fs::write(&tmp_path, &file_bytes)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Write temp: {e}")))?;

    let result = infer_schema_from_path(&tmp_path, &tmp_dir, detected_ext);

    let _ = std::fs::remove_file(&tmp_path);
    let _ = std::fs::remove_dir_all(&tmp_dir);

    result
}

fn infer_schema_from_path(
    tmp_path: &str,
    tmp_dir: &str,
    detected_ext: &str,
) -> Result<SchemaInferResponse, AppError> {
    let shp_path = if detected_ext == "shp" {
        tmp_path.to_string()
    } else {
        let file = std::fs::File::open(tmp_path)
            .map_err(|e| AppError::BadRequest(format!("invalid_zip: {}", e)))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| AppError::BadRequest(format!("invalid_zip: {}", e)))?;

        for i in 0..archive.len() {
            let mut file = archive.by_index(i)
                .map_err(|e| AppError::BadRequest(format!("invalid_zip: {}", e)))?;
            let outpath = std::path::Path::new(tmp_dir).join(file.name());
            if file.is_dir() { continue; }
            if let Some(p) = outpath.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            let mut outfile = std::fs::File::create(&outpath)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Extract write: {e}")))?;
            std::io::copy(&mut file, &mut outfile)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Extract copy: {e}")))?;
        }

        find_first_shp(tmp_dir)
            .ok_or_else(|| AppError::BadRequest("no_layers_found".into()))?
    };

    // Use shapefile::Reader to open and read schema
    let mut reader = shapefile::Reader::from_path(&shp_path)
        .map_err(|e| AppError::BadRequest(format!("cannot_open: {}", e)))?;

    let geometry_type = shape_type_name(reader.header().shape_type);

    let layer_name = std::path::Path::new(&shp_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    // Read fields from the DBF file directly using dbase crate
    let dbf_path = std::path::Path::new(&shp_path).with_extension("dbf");
    let mut dbf_reader = dbase::Reader::from_path(&dbf_path)
        .map_err(|e| AppError::BadRequest(format!("cannot_open: {} (dbf)", e)))?;
    let mut fields = Vec::new();
    for field_info in dbf_reader.fields() {
        let (ft, _, precision, scale) = dbase_field_type(field_info.field_type());
        fields.push(SchemaField {
            name: field_info.name().to_string(),
            field_type: ft,
            length: Some(field_info.length() as usize),
            precision,
            scale,
            nullable: true,
        });
    }

    let feature_count = reader.shape_count()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("shape_count: {}", e)))?;

    Ok(SchemaInferResponse {
        request_id: String::new(),
        layer_name,
        fields,
        feature_count,
        geometry_type: Some(geometry_type),
    })
}

fn find_first_shp(dir: &str) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_first_shp(path.to_str()?) {
                return Some(found);
            }
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if ext.eq_ignore_ascii_case("shp") {
                return path.to_str().map(String::from);
            }
        }
    }
    None
}

fn shape_type_name(st: shapefile::ShapeType) -> String {
    match st {
        shapefile::ShapeType::NullShape => "Null".into(),
        shapefile::ShapeType::Point => "Point".into(),
        shapefile::ShapeType::PointZ => "PointZ".into(),
        shapefile::ShapeType::PointM => "PointM".into(),
        shapefile::ShapeType::Polyline => "PolyLine".into(),
        shapefile::ShapeType::PolylineZ => "PolyLineZ".into(),
        shapefile::ShapeType::PolylineM => "PolyLineM".into(),
        shapefile::ShapeType::Polygon => "Polygon".into(),
        shapefile::ShapeType::PolygonZ => "PolygonZ".into(),
        shapefile::ShapeType::PolygonM => "PolygonM".into(),
        shapefile::ShapeType::Multipoint => "MultiPoint".into(),
        shapefile::ShapeType::MultipointZ => "MultiPointZ".into(),
        shapefile::ShapeType::MultipointM => "MultiPointM".into(),
        shapefile::ShapeType::Multipatch => "MultiPatch".into(),
    }
}

fn dbase_field_type(ft: dbase::FieldType) -> (String, Option<usize>, Option<usize>, Option<usize>) {
    use dbase::FieldType::*;
    match ft {
        Character => ("string".into(), None, None, None),
        Date => ("date".into(), None, None, None),
        Float | Double => ("float64".into(), None, Some(20), Some(6)),
        Numeric => ("float64".into(), None, Some(20), Some(6)),
        Integer => ("int32".into(), None, None, None),
        Logical => ("boolean".into(), None, None, None),
        Memo => ("string".into(), None, None, None),
        Currency => ("float64".into(), None, Some(20), Some(4)),
        DateTime => ("datetime".into(), None, None, None),
    }
}