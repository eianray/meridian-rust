//! POST /v1/schema/infer — Inspect SHP/ZIP or GDB/ZIP and return layer schema.
//!
//! **Multipart mode (file upload):**
//!   `POST /v1/schema/infer` with `multipart/form-data` and field `file`
//!
//! **URL mode (fetch from URL):**
//!   `POST /v1/schema/infer?url=https://example.com/data.zip`
//!   (URL param takes precedence — file field ignored if URL is provided)

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

const PYTHON_SCHEMA_SCRIPT: &str = r#"
import sys, json, os, tempfile, zipfile, shutil

def infer_schema(file_path):
    import fiona
    try:
        ds = fiona.open(file_path)
    except Exception as e:
        return {"error": "cannot_open", "detail": str(e)}
    try:
        layer_name = ds.name
        schema_props = ds.schema['properties']
        count = len(ds)
        geom_type = ds.schema.get('geometry', 'Unknown')
        fields = []
        for fname, ftype in schema_props.items():
            parsed = parse_fiona_type(ftype)
            parsed['name'] = fname
            fields.append(parsed)
        return {"layer_name": layer_name, "fields": fields, "feature_count": count, "geometry_type": geom_type}
    finally:
        ds.close()

def parse_fiona_type(ftype):
    nullable = True
    low = ftype.lower()
    if low == 'str' or low.startswith('str:'):
        length = 254
        if ':' in low:
            try: length = int(low.split(':')[1].split(',')[0])
            except: pass
        return {"type": "string", "length": length, "nullable": nullable}
    if low == 'float' or low.startswith('float:'):
        precision, scale = 10, 6
        if ':' in low:
            parts = low.split(':')[1].split('.')
            try: precision = int(parts[0])
            except: pass
            try: scale = int(parts[1]) if len(parts) > 1 else 6
            except: pass
        return {"type": "float64", "precision": precision, "scale": scale, "nullable": nullable}
    if low == 'int' or low.startswith('int:'):
        return {"type": "int32", "nullable": nullable}
    if low == 'bool':
        return {"type": "boolean", "nullable": nullable}
    if low == 'date':
        return {"type": "date", "nullable": nullable}
    if low == 'datetime':
        return {"type": "datetime", "nullable": nullable}
    if low == 'time':
        return {"type": "string", "length": 32, "nullable": nullable}
    return {"type": "string", "length": 254, "nullable": nullable}

def find_layer_path(tmp_dir):
    shp_found, gdb_found = None, None
    for root, dirs, files in os.walk(tmp_dir):
        for f in files:
            if f.lower().endswith('.shp'):
                shp_found = os.path.join(root, f)
        for d in dirs:
            if d.lower().endswith('.gdb'):
                gdb_found = os.path.join(root, d)
    return shp_found, gdb_found

if __name__ == '__main__':
    mode, path = sys.argv[1], sys.argv[2]
    tmp_dir = tempfile.mkdtemp()
    try:
        actual_path = None
        if mode in ('zip', 'gdb'):
            try:
                with zipfile.ZipFile(path, 'r') as zf:
                    zf.extractall(tmp_dir)
            except Exception as e:
                print(json.dumps({"error": "invalid_zip", "detail": str(e)}))
                sys.exit(0)
            if mode == 'gdb':
                for entry in os.listdir(tmp_dir):
                    if entry.lower().endswith('.gdb'):
                        actual_path = os.path.join(tmp_dir, entry)
                        break
            else:
                shp, gdb = find_layer_path(tmp_dir)
                actual_path = shp or gdb
        else:
            actual_path = path
        if not actual_path:
            print(json.dumps({"error": "no_layers_found"}))
        else:
            result = infer_schema(actual_path)
            print(json.dumps(result))
    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)
"#;

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

    // URL query param takes precedence
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

    // Multipart file upload
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
    let ext = if detected_ext.is_empty() { "zip" } else { &detected_ext };
    let out = run_schema_inference(&request_id, file_bytes, ext).await?;

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
    run_schema_inference(request_id, bytes.to_vec(), &ext).await
}

async fn run_schema_inference(
    request_id: &str,
    file_bytes: Vec<u8>,
    detected_ext: &str,
) -> Result<SchemaInferResponse, AppError> {
    let id = Uuid::new_v4().to_string();
    let mode = if detected_ext == "shp" { "file" } else { "zip" };
    let ext_suffix = if detected_ext == "shp" { "shp" } else { "zip" };
    let tmp_path = format!("/tmp/schema_infer_{id}.{ext_suffix}");

    let tmp_path_clone = tmp_path.clone();
    fs::write(&tmp_path, &file_bytes)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Write temp: {e}")))?;

    let join_result = tokio::task::spawn_blocking(move || {
        run_python_script(&tmp_path_clone, mode)
    })
    .await;

    let _ = tokio::fs::remove_file(&tmp_path).await;

    let python_out = match join_result {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(AppError::BadRequest(format!("python3 script failed: {}", e))),
        Err(e) => return Err(AppError::Internal(anyhow::anyhow!("Join: {e}"))),
    };

    let parsed: serde_json::Value = serde_json::from_str(&python_out)
        .map_err(|e| {
            AppError::Internal(anyhow::anyhow!(
                "Python output parse error: {}; Output: {:?}",
                e, python_out
            ))
        })?;

    if let Some(err) = parsed.get("error").and_then(|e| e.as_str()) {
        let detail = parsed.get("detail").and_then(|e| e.as_str()).unwrap_or("");
        match err {
            "no_layers_found" => return Err(AppError::BadRequest("no_layers_found".into())),
            "invalid_zip" => return Err(AppError::BadRequest(format!("invalid_zip: {}", detail))),
            "cannot_open" => return Err(AppError::BadRequest(format!("cannot_open: {}", detail))),
            _ => return Err(AppError::BadRequest(format!("{}: {}", err, detail))),
        }
    }

    let layer_name = parsed.get("layer_name").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
    let feature_count = parsed.get("feature_count").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let geometry_type = parsed.get("geometry_type").and_then(|v| v.as_str()).map(String::from);

    let fields: Vec<SchemaField> = parsed.get("fields")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter().filter_map(|f| {
                let name = f.get("name")?.as_str()?.to_string();
                let field_type = f.get("type")?.as_str()?.to_string();
                let length = f.get("length").and_then(|v| v.as_u64()).map(|n| n as usize);
                let precision = f.get("precision").and_then(|v| v.as_u64()).map(|n| n as usize);
                let scale = f.get("scale").and_then(|v| v.as_u64()).map(|n| n as usize);
                let nullable = f.get("nullable").and_then(|v| v.as_bool()).unwrap_or(true);
                Some(SchemaField { name, field_type, length, precision, scale, nullable })
            }).collect()
        })
        .unwrap_or_default();

    Ok(SchemaInferResponse { request_id: request_id.to_string(), layer_name, fields, feature_count, geometry_type })
}

fn run_python_script(file_path: &str, mode: &str) -> Result<String, AppError> {
    let id = Uuid::new_v4().to_string().replace('-', "")[..8].to_string();
    let script_path = format!("/tmp/schema_script_{id}.py");
    std::fs::write(&script_path, PYTHON_SCHEMA_SCRIPT)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Write script: {e}")))?;
    let output = std::process::Command::new("python3")
        .args(["-u", &script_path, mode, file_path])
        .output()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("python3 spawn: {e}")))?;
    let _ = std::fs::remove_file(&script_path);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::BadRequest(format!("python3 script failed: {}", stderr.trim())));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.to_string())
}
