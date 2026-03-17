use base64::{engine::general_purpose::STANDARD, Engine as _};
use gdal::raster::Buffer;
use gdal::vector::{FieldDefn, LayerAccess, LayerOptions, OGRFieldType};
use gdal::{Dataset, DriverManager};
use serde::Serialize;
use std::collections::BTreeMap;
use std::f32::NAN;
use std::time::Duration;
use tempfile::TempDir;
use utoipa::ToSchema;

use crate::error::AppError;

pub const RASTER_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug)]
pub struct RasterInput {
    pub filename: String,
    pub bytes: Vec<u8>,
    pub size: usize,
}

impl RasterInput {
    pub async fn from_multipart_field(
        field: &mut axum::extract::multipart::Field<'_>,
    ) -> Result<Self, AppError> {
        use crate::gis::MAX_FILE_BYTES;

        let filename = field
            .file_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "input.tif".to_string());

        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|e| AppError::BadRequest(format!("Error reading upload: {e}")))?
        {
            if buf.len() + chunk.len() > MAX_FILE_BYTES {
                return Err(AppError::PayloadTooLarge);
            }
            buf.extend_from_slice(&chunk);
        }

        if buf.is_empty() {
            return Err(AppError::BadRequest("Empty file".into()));
        }

        Ok(Self {
            filename,
            size: buf.len(),
            bytes: buf,
        })
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RasterOpStats {
    pub tool: String,
    pub input_count: usize,
    pub input_size_bytes: usize,
    pub output_size_bytes: usize,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RasterBinaryResult {
    pub data: String,
    pub filename: String,
    pub encoding: String,
    pub mime_type: String,
    pub stats: RasterOpStats,
}

#[derive(Debug, Clone)]
pub struct RasterCommandOutput {
    pub bytes: Vec<u8>,
    pub filename: String,
    pub mime_type: String,
    pub stats: RasterOpStats,
}

impl RasterCommandOutput {
    pub fn as_json_value(&self) -> serde_json::Value {
        serde_json::to_value(RasterBinaryResult {
            data: STANDARD.encode(&self.bytes),
            filename: self.filename.clone(),
            encoding: "base64".into(),
            mime_type: self.mime_type.clone(),
            stats: self.stats.clone(),
        })
        .unwrap_or_else(|_| serde_json::json!({
            "data": STANDARD.encode(&self.bytes),
            "filename": self.filename,
            "encoding": "base64",
            "mime_type": self.mime_type,
            "stats": self.stats,
        }))
    }
}

/// Run a GDAL DEM operation (hillshade, slope, aspect, roughness) using native GDAL C API
pub async fn run_gdaldem_single(
    mode: &str,
    input: &RasterInput,
    _extra_args: &[String],
    output_ext: &str,
    mime_type: &str,
) -> Result<RasterCommandOutput, AppError> {
    let valid_modes = ["hillshade", "slope", "aspect", "roughness"];
    if !valid_modes.contains(&mode) {
        return Err(AppError::BadRequest(format!(
            "Invalid mode '{}'. Valid modes: {:?}",
            mode, valid_modes
        )));
    }

    let bytes = input.bytes.clone();
    let input_size = input.size;

    // Convert to owned strings for the blocking task
    let mode_owned = mode.to_string();
    let output_ext_owned = output_ext.to_string();
    let mime_type_owned = mime_type.to_string();

    tokio::task::spawn_blocking(move || {
        run_gdaldem_single_sync(&mode_owned, &bytes, input_size, &output_ext_owned, &mime_type_owned)
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Task join error: {}", e)))?
}

fn run_gdaldem_single_sync(
    mode: &str,
    input_bytes: &[u8],
    input_size: usize,
    output_ext: &str,
    mime_type: &str,
) -> Result<RasterCommandOutput, AppError> {
    let tmp = TempDir::new().map_err(|e| AppError::Internal(anyhow::anyhow!("TempDir: {e}")))?;

    // Write input to temp file
    let in_path = tmp.path().join("input.tif");
    std::fs::write(&in_path, input_bytes)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Write input: {e}")))?;

    // Open input dataset
    let in_ds = Dataset::open(&in_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Open input: {e}")))?;

    let out_path = tmp.path().join(format!("output.{}", output_ext));

    let mut usage_error: i32 = 0;
    let out_ds = unsafe {
        gdal_sys::GDALDEMProcessing(
            out_path.to_str().unwrap().as_ptr() as *const i8,
            in_ds.c_dataset(),
            mode.as_ptr() as *const i8,
            std::ptr::null(),  // color filename (null for DEM modes)
            std::ptr::null(),  // options
            &mut usage_error,
        )
    };

    if out_ds.is_null() {
        return Err(AppError::BadRequest(format!(
            "GDALDEMProcessing failed for mode '{}'. Usage error: {}",
            mode, usage_error
        )));
    }

    // Close output dataset to flush to disk
    unsafe { gdal_sys::GDALClose(out_ds) };

    // Read output
    if !out_path.exists() {
        return Err(AppError::Internal(anyhow::anyhow!(
            "GDALDEMProcessing completed but output not created"
        )));
    }

    let out_bytes = std::fs::read(&out_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Read output: {e}")))?;

    Ok(RasterCommandOutput {
        stats: RasterOpStats {
            tool: mode.to_string(),
            input_count: 1,
            input_size_bytes: input_size,
            output_size_bytes: out_bytes.len(),
        },
        bytes: out_bytes,
        filename: format!("{}.{}", mode, output_ext),
        mime_type: mime_type.to_string(),
    })
}

/// Run color relief using GDALDEMProcessing
pub async fn run_color_relief(
    input: &RasterInput,
    color_table_text: &str,
) -> Result<RasterCommandOutput, AppError> {
    if color_table_text.trim().is_empty() {
        return Err(AppError::BadRequest(
            "Missing color table. Provide 'color_table' text or 'color_file' upload".into(),
        ));
    }

    let bytes = input.bytes.clone();
    let input_size = input.size;
    let color_table = color_table_text.to_string();

    tokio::task::spawn_blocking(move || {
        run_color_relief_sync(&bytes, input_size, &color_table)
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Task join error: {}", e)))?
}

fn run_color_relief_sync(
    input_bytes: &[u8],
    input_size: usize,
    color_table: &str,
) -> Result<RasterCommandOutput, AppError> {
    let tmp = TempDir::new().map_err(|e| AppError::Internal(anyhow::anyhow!("TempDir: {e}")))?;

    // Write input
    let in_path = tmp.path().join("input.tif");
    std::fs::write(&in_path, input_bytes)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Write input: {e}")))?;

    // Write color table
    let color_path = tmp.path().join("color_table.txt");
    std::fs::write(&color_path, color_table)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Write color table: {e}")))?;

    // Open input
    let in_ds = Dataset::open(&in_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Open input: {e}")))?;

    let out_path = tmp.path().join("color_relief.tif");

    let mut usage_error: i32 = 0;
    let out_ds = unsafe {
        gdal_sys::GDALDEMProcessing(
            out_path.to_str().unwrap().as_ptr() as *const i8,
            in_ds.c_dataset(),
            b"color-relief\0".as_ptr() as *const i8,
            color_path.to_str().unwrap().as_ptr() as *const i8,
            std::ptr::null(),
            &mut usage_error,
        )
    };

    if out_ds.is_null() {
        return Err(AppError::BadRequest(format!(
            "GDALDEMProcessing color-relief failed. Usage error: {}",
            usage_error
        )));
    }

    unsafe { gdal_sys::GDALClose(out_ds) };

    if !out_path.exists() {
        return Err(AppError::Internal(anyhow::anyhow!(
            "color-relief completed but output not created"
        )));
    }

    let out_bytes = std::fs::read(&out_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Read output: {e}")))?;

    Ok(RasterCommandOutput {
        stats: RasterOpStats {
            tool: "color-relief".to_string(),
            input_count: 1,
            input_size_bytes: input_size,
            output_size_bytes: out_bytes.len(),
        },
        bytes: out_bytes,
        filename: "color_relief.tif".into(),
        mime_type: "image/tiff".to_string(),
    })
}

/// Run contour generation using GDALContourGenerate
pub async fn run_contours(
    input: &RasterInput,
    interval: Option<f64>,
    offset: Option<f64>,
    attribute_name: Option<String>,
) -> Result<RasterCommandOutput, AppError> {
    let bytes = input.bytes.clone();
    let input_size = input.size;
    let interval = interval.unwrap_or(100.0);
    let offset = offset.unwrap_or(0.0);
    let attr_name = attribute_name.unwrap_or_else(|| "elev".to_string());

    if interval <= 0.0 {
        return Err(AppError::BadRequest("interval must be > 0".into()));
    }

    tokio::task::spawn_blocking(move || {
        run_contours_sync(&bytes, input_size, interval, offset, &attr_name)
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Task join error: {e}")))?
}

fn run_contours_sync(
    input_bytes: &[u8],
    input_size: usize,
    interval: f64,
    offset: f64,
    attribute_name: &str,
) -> Result<RasterCommandOutput, AppError> {
    let tmp = TempDir::new().map_err(|e| AppError::Internal(anyhow::anyhow!("TempDir: {e}")))?;

    // Write input
    let in_path = tmp.path().join("input.tif");
    std::fs::write(&in_path, input_bytes)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Write input: {e}")))?;

    // Open input and get first band
    let in_ds = Dataset::open(&in_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Open input: {e}")))?;

    let band = in_ds.rasterband(1)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Get band: {e}")))?;

    // Get no-data value
    let no_data = band.no_data_value().unwrap_or(-1e10);
    let c_band = unsafe { band.c_rasterband() };

    // Get input dimensions
    let (in_xsize, in_ysize) = in_ds.raster_size();

    // Create output GeoJSON dataset
    let out_path = tmp.path().join("contours.geojson");
    let driver = DriverManager::get_driver_by_name("GeoJSON")
        .map_err(|e| AppError::Internal(anyhow::anyhow!("GeoJSON driver: {e}")))?;

    let mut out_ds = driver.create(&out_path, in_xsize, in_ysize, 0)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Create output: {e}")))?;

    // Get spatial reference from input
    let srs = in_ds.spatial_ref().ok();

    // Create layer for contours
    let mut layer_options = LayerOptions {
        name: "contours",
        srs: srs.as_ref(),
        ty: gdal_sys::OGRwkbGeometryType::wkbLineString,
        options: None,
    };

    let mut layer = out_ds.create_layer(layer_options)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Create layer: {e}")))?;

    // Add ID field (required by GDALContourGenerate for feature ID)
    let id_field = FieldDefn::new("ID", OGRFieldType::OFTInteger)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Create ID field: {e}")))?;
    id_field.add_to_layer(&layer)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Add ID field: {e}")))?;

    // Add elevation field
    let elev_field = FieldDefn::new(attribute_name, OGRFieldType::OFTReal)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Create elev field: {e}")))?;
    elev_field.add_to_layer(&layer)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Add elev field: {e}")))?;

    // Get the raw layer handle (unsafe)
    let c_layer = unsafe { layer.c_layer() };

    // Call GDALContourGenerate (unsafe)
    let result = unsafe {
        gdal_sys::GDALContourGenerate(
            c_band,
            interval,
            offset,
            0,                      // no fixed levels
            std::ptr::null_mut(),   // no fixed levels
            1,                      // use no-data
            no_data,
            c_layer,
            0,                      // ID field index
            1,                      // elevation field index
            None,                   // progress callback
            std::ptr::null_mut(),   // progress data
        )
    };

    if result != gdal_sys::CPLErr::CE_None {
        return Err(AppError::Internal(anyhow::anyhow!("GDALContourGenerate failed")));
    }

    // Drop layer before flushing dataset
    drop(layer);

    // Flush to ensure features are written
    out_ds.flush_cache()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Flush output: {e}")))?;

    drop(out_ds);

    if !out_path.exists() {
        return Err(AppError::Internal(anyhow::anyhow!(
            "Contour generation completed but output not created"
        )));
    }

    let out_bytes = std::fs::read(&out_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Read output: {e}")))?;

    Ok(RasterCommandOutput {
        stats: RasterOpStats {
            tool: "contours".to_string(),
            input_count: 1,
            input_size_bytes: input_size,
            output_size_bytes: out_bytes.len(),
        },
        bytes: out_bytes,
        filename: "contours.geojson".into(),
        mime_type: "application/geo+json".to_string(),
    })
}

/// Run raster calculation using pure Rust pixel math
pub async fn run_raster_calc(
    rasters: &BTreeMap<char, RasterInput>,
    expression: &str,
    output_format: Option<&str>,
) -> Result<RasterCommandOutput, AppError> {
    if rasters.is_empty() {
        return Err(AppError::BadRequest(
            "Missing raster inputs. Provide one or more files named A through Z".into(),
        ));
    }
    if expression.trim().is_empty() {
        return Err(AppError::BadRequest("Missing 'expression' field".into()));
    }

    let format = output_format.unwrap_or("GTiff");
    if !format.eq_ignore_ascii_case("GTiff") && !format.eq_ignore_ascii_case("GeoTIFF") {
        return Err(AppError::BadRequest(
            "Unsupported output_format. Use GTiff or GeoTIFF".into(),
        ));
    }

    // Parse expression
    let expr = parse_expression(expression.trim())
        .map_err(|e| AppError::BadRequest(format!("Expression parse error: {}", e)))?;

    // Clone data for blocking task
    let mut rasters_data: BTreeMap<char, Vec<u8>> = BTreeMap::new();
    let total_size: usize = rasters.values().map(|r| r.size).sum();

    for (letter, raster) in rasters {
        rasters_data.insert(*letter, raster.bytes.clone());
    }

    tokio::task::spawn_blocking(move || {
        run_raster_calc_sync(&rasters_data, &expr, total_size)
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Task join error: {}", e)))?
}

fn run_raster_calc_sync(
    rasters_data: &BTreeMap<char, Vec<u8>>,
    expr: &Expr,
    total_input_size: usize,
) -> Result<RasterCommandOutput, AppError> {
    let tmp = TempDir::new().map_err(|e| AppError::Internal(anyhow::anyhow!("TempDir: {e}")))?;

    // Write input files and open them
    let mut inputs: BTreeMap<char, (Dataset, usize, usize)> = BTreeMap::new();
    let mut width = 0usize;
    let mut height = 0usize;

    for (letter, bytes) in rasters_data {
        let in_path = tmp.path().join(format!("{}.tif", letter));
        std::fs::write(&in_path, bytes)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Write {}: {}", letter, e)))?;

        let ds = Dataset::open(&in_path)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Open {}: {}", letter, e)))?;

        let (w, h) = ds.raster_size();
        if width == 0 && height == 0 {
            width = w;
            height = h;
        } else if w != width || h != height {
            return Err(AppError::BadRequest(format!(
                "Raster {} has size {}x{} but expected {}x{}",
                letter, w, h, width, height
            )));
        }

        inputs.insert(*letter, (ds, w, h));
    }

    // Read all bands into memory
    let mut band_data: BTreeMap<char, Vec<f32>> = BTreeMap::new();
    for (letter, (ds, w, h)) in &inputs {
        let band = ds.rasterband(1)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Get band {}: {}", letter, e)))?;

        // Read as f32
        let buffer: Buffer<f32> = band.read_as::<f32>(
            (0, 0),
            (*w, *h),
            (*w, *h),
            None,
        ).map_err(|e| AppError::Internal(anyhow::anyhow!("Read band {}: {}", letter, e)))?;

        band_data.insert(*letter, buffer.data().to_vec());
    }

    // Evaluate expression pixel by pixel
    let mut output_data = vec![0.0f32; width * height];

    for i in 0..(width * height) {
        let mut env: BTreeMap<char, f32> = BTreeMap::new();
        for (letter, data) in &band_data {
            env.insert(*letter, data[i]);
        }
        output_data[i] = eval_expr(expr, &env);
    }

    // Get metadata from first input for geotransform/projection
    let (ref first_ds, first_w, first_h) = inputs.values().next().unwrap();
    let geo_transform = first_ds.geo_transform()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Get geo transform: {e}")))?;
    let spatial_ref = first_ds.spatial_ref()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Get spatial ref: {}", e)))?;

    // Create output dataset
    let out_path = tmp.path().join("raster_calc.tif");
    let driver = DriverManager::get_driver_by_name("GTiff")
        .map_err(|e| AppError::Internal(anyhow::anyhow!("GTiff driver: {e}")))?;

    let mut out_ds = driver.create_with_band_type::<f32, _>(
        &out_path,
        *first_w,
        *first_h,
        1,  // 1 band
    ).map_err(|e| AppError::Internal(anyhow::anyhow!("Create output: {e}")))?;

    // Set geotransform and projection
    out_ds.set_geo_transform(&geo_transform)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Set geo transform: {e}")))?;
    out_ds.set_projection(&spatial_ref.to_wkt()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Get projection WKT: {e}")))?)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Set projection: {e}")))?;

    // Write output band
    let mut out_band = out_ds.rasterband(1)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Get output band: {e}")))?;

    let mut buffer = Buffer::new(
        (*first_w, *first_h),
        output_data,
    );

    out_band.write::<f32>((0, 0), (*first_w, *first_h), &mut buffer)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Write output: {e}")))?;

    // Set nodata value for NoData
    out_band.set_no_data_value(Some(f64::from(NAN)))
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Set nodata: {e}")))?;

    drop(out_ds);

    if !out_path.exists() {
        return Err(AppError::Internal(anyhow::anyhow!(
            "Raster calc completed but output not created"
        )));
    }

    let out_bytes = std::fs::read(&out_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Read output: {e}")))?;

    let input_count = rasters_data.len();

    Ok(RasterCommandOutput {
        stats: RasterOpStats {
            tool: "raster-calc".to_string(),
            input_count,
            input_size_bytes: total_input_size,
            output_size_bytes: out_bytes.len(),
        },
        bytes: out_bytes,
        filename: "raster_calc.tif".into(),
        mime_type: "image/tiff".to_string(),
    })
}

// ============== Expression Parser ==============

#[derive(Debug, Clone)]
enum Expr {
    Num(f32),
    Var(char),
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Div(Box<Expr>, Box<Expr>),
    Neg(Box<Expr>),
}

fn parse_expression(s: &str) -> Result<Expr, String> {
    let mut chars: Vec<char> = s.chars().collect();
    parse_expr(&mut chars)
}

fn parse_expr(chars: &mut Vec<char>) -> Result<Expr, String> {
    parse_add_sub(chars)
}

fn parse_add_sub(chars: &mut Vec<char>) -> Result<Expr, String> {
    let mut left = parse_mul_div(chars)?;

    while !chars.is_empty() {
        let op = chars[0];
        if op == '+' {
            chars.remove(0);
            let right = parse_mul_div(chars)?;
            left = Expr::Add(Box::new(left), Box::new(right));
        } else if op == '-' {
            chars.remove(0);
            let right = parse_mul_div(chars)?;
            left = Expr::Sub(Box::new(left), Box::new(right));
        } else {
            break;
        }
    }

    Ok(left)
}

fn parse_mul_div(chars: &mut Vec<char>) -> Result<Expr, String> {
    let mut left = parse_unary(chars)?;

    while !chars.is_empty() {
        let op = chars[0];
        if op == '*' {
            chars.remove(0);
            let right = parse_unary(chars)?;
            left = Expr::Mul(Box::new(left), Box::new(right));
        } else if op == '/' {
            chars.remove(0);
            let right = parse_unary(chars)?;
            left = Expr::Div(Box::new(left), Box::new(right));
        } else {
            break;
        }
    }

    Ok(left)
}

fn parse_unary(chars: &mut Vec<char>) -> Result<Expr, String> {
    if !chars.is_empty() && chars[0] == '-' {
        chars.remove(0);
        let expr = parse_primary(chars)?;
        return Ok(Expr::Neg(Box::new(expr)));
    }
    parse_primary(chars)
}

fn parse_primary(chars: &mut Vec<char>) -> Result<Expr, String> {
    // Skip whitespace
    while !chars.is_empty() && chars[0].is_whitespace() {
        chars.remove(0);
    }

    if chars.is_empty() {
        return Err("Unexpected end of expression".to_string());
    }

    // Parentheses
    if chars[0] == '(' {
        chars.remove(0);
        let expr = parse_expr(chars)?;
        while !chars.is_empty() && chars[0].is_whitespace() {
            chars.remove(0);
        }
        if chars.is_empty() || chars[0] != ')' {
            return Err("Missing closing parenthesis".to_string());
        }
        chars.remove(0);
        return Ok(expr);
    }

    // Number
    if chars[0].is_ascii_digit() || chars[0] == '.' {
        let mut num_str = String::new();
        let mut has_dot = false;
        while !chars.is_empty() {
            let c = chars[0];
            if c.is_ascii_digit() {
                num_str.push(chars.remove(0));
            } else if c == '.' && !has_dot {
                has_dot = true;
                num_str.push(chars.remove(0));
            } else {
                break;
            }
        }
        let val: f32 = num_str.parse()
            .map_err(|_| format!("Invalid number: {}", num_str))?;
        return Ok(Expr::Num(val));
    }

    // Variable (uppercase letter A-Z)
    if chars[0].is_ascii_uppercase() {
        let var = chars.remove(0);
        if !var.is_ascii_uppercase() {
            return Err(format!("Invalid variable: {}", var));
        }
        return Ok(Expr::Var(var));
    }

    Err(format!("Unexpected character: {}", chars[0]))
}

fn eval_expr(expr: &Expr, env: &BTreeMap<char, f32>) -> f32 {
    match expr {
        Expr::Num(n) => *n,
        Expr::Var(c) => env.get(c).copied().unwrap_or(0.0),
        Expr::Add(a, b) => eval_expr(a, env) + eval_expr(b, env),
        Expr::Sub(a, b) => eval_expr(a, env) - eval_expr(b, env),
        Expr::Mul(a, b) => eval_expr(a, env) * eval_expr(b, env),
        Expr::Div(a, b) => {
            let b_val = eval_expr(b, env);
            if b_val == 0.0 {
                NAN
            } else {
                eval_expr(a, env) / b_val
            }
        }
        Expr::Neg(a) => -eval_expr(a, env),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expression_parser() {
        // Simple number
        assert!(matches!(parse_expression("42"), Ok(Expr::Num(42.0))));

        // Variables
        assert!(matches!(parse_expression("A"), Ok(Expr::Var('A'))));

        // Addition
        let expr = parse_expression("A+B").unwrap();
        assert!(matches!(expr, Expr::Add(_, _)));

        // Multiplication precedence
        let expr = parse_expression("A+B*C").unwrap();
        if let Expr::Add(_, rhs) = &expr {
            assert!(matches!(rhs.as_ref(), Expr::Mul(_, _)));
        } else {
            panic!("expected Expr::Add for A+B*C");
        }

        // Parentheses
        let expr = parse_expression("(A+B)*C").unwrap();
        if let Expr::Mul(lhs, _) = &expr {
            assert!(matches!(lhs.as_ref(), Expr::Add(_, _)));
        } else {
            panic!("expected Expr::Mul for (A+B)*C");
        }

        // Negative
        let expr = parse_expression("-A").unwrap();
        assert!(matches!(expr, Expr::Neg(_)));

        // Complex
        let expr = parse_expression("(A+B)/2").unwrap();
        assert!(matches!(expr, Expr::Div(_, _)));
    }

    #[test]
    fn test_expression_eval() {
        let mut env: BTreeMap<char, f32> = BTreeMap::new();
        env.insert('A', 10.0);
        env.insert('B', 5.0);

        assert_eq!(eval_expr(&parse_expression("A+B").unwrap(), &env), 15.0);
        assert_eq!(eval_expr(&parse_expression("A-B").unwrap(), &env), 5.0);
        assert_eq!(eval_expr(&parse_expression("A*B").unwrap(), &env), 50.0);
        assert_eq!(eval_expr(&parse_expression("A/B").unwrap(), &env), 2.0);
        assert_eq!(eval_expr(&parse_expression("(A+B)/3").unwrap(), &env), 5.0);
    }
}
