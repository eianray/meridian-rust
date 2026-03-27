use base64::{engine::general_purpose::STANDARD, Engine as _};
use gdal::raster::Buffer;
use gdal::vector::{FieldDefn, LayerAccess, LayerOptions, OGRFieldType};
use gdal::{Dataset, DriverManager};
use serde::Serialize;
use std::collections::BTreeMap;
use std::f32::NAN;
use std::os::raw::c_char;
use std::time::Duration;
use tempfile::TempDir;
use utoipa::ToSchema;

use crate::error::AppError;
use crate::gis::normalize_crs_string;

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

        // If it's a zip file, extract the first .tif
        let (bytes, filename) = if filename.to_lowercase().ends_with(".zip") {
            extract_tiff_from_zip(&buf, &filename)?
        } else {
            (buf, filename)
        };

        Ok(Self {
            filename,
            size: bytes.len(),
            bytes,
        })
    }
}

/// Sanitize a zip entry path to prevent directory traversal attacks
fn sanitize_zip_path(name: &str) -> Option<std::path::PathBuf> {
    // Reject absolute paths, parent directory traversal, null bytes
    if name.starts_with('/') || name.starts_with('\\') || name.contains("..") || name.contains('\0') {
        return None;
    }
    // Strip leading ./ or .\
    let cleaned = name.trim_start_matches("./").trim_start_matches(".\\");
    if cleaned.is_empty() || cleaned.starts_with('/') || cleaned.starts_with('\\') {
        return None;
    }
    Some(std::path::PathBuf::from(cleaned))
}

/// Extract the first .tif file from a zip archive
fn extract_tiff_from_zip(zip_bytes: &[u8], original_name: &str) -> Result<(Vec<u8>, String), AppError> {
    use std::io::Read;

    let cursor = std::io::Cursor::new(zip_bytes);
    let mut zip = zip::ZipArchive::new(cursor)
        .map_err(|e| AppError::BadRequest(format!("Invalid zip file: {e}")))?;

    // Collect all .tif files in the archive
    let mut tiff_indices: Vec<usize> = Vec::new();
    for i in 0..zip.len() {
        let file = zip.by_index(i)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Zip read: {e}")))?;
        let name = file.name();
        // Skip entries with invalid paths (directory traversal protection)
        if sanitize_zip_path(name).is_none() {
            continue;
        }
        let name_lower = name.to_lowercase();
        if name_lower.ends_with(".tif") || name_lower.ends_with(".tiff") {
            tiff_indices.push(i);
        }
    }

    // Check for multiple rasters
    if tiff_indices.len() > 1 {
        return Err(AppError::BadRequest(
            "Zip contains multiple raster files. Upload a single .tif or .tiff per request.".into()
        ));
    }

    // Check for no rasters
    if tiff_indices.is_empty() {
        return Err(AppError::BadRequest("No .tif file found in zip archive".into()));
    }

    // Extract the single .tif file
    let idx = tiff_indices[0];
    let mut bytes = Vec::new();
    let mut file = zip.by_index(idx)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Zip read: {e}")))?;
    file.read_to_end(&mut bytes)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Read zip entry: {e}")))?;

    let stem = original_name
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or("input");
    Ok((bytes, format!("{}.tif", stem)))
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
            out_path.to_str().unwrap().as_ptr() as *const std::os::raw::c_char,
            in_ds.c_dataset(),
            mode.as_ptr() as *const std::os::raw::c_char,
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

/// Run slope with the -p (percent) flag using GDALDEMProcessing + GDALDEMProcessingOptionsNew
pub async fn run_gdaldem_slope_pct(input: &RasterInput) -> Result<RasterCommandOutput, AppError> {
    let bytes = input.bytes.clone();
    let input_size = input.size;

    tokio::task::spawn_blocking(move || {
        run_gdaldem_slope_pct_sync(&bytes, input_size)
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Task join error: {}", e)))?
}

fn run_gdaldem_slope_pct_sync(input_bytes: &[u8], input_size: usize) -> Result<RasterCommandOutput, AppError> {
    let tmp = TempDir::new().map_err(|e| AppError::Internal(anyhow::anyhow!("TempDir: {e}")))?;

    // Write input to temp file
    let in_path = tmp.path().join("input.tif");
    std::fs::write(&in_path, input_bytes)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Write input: {e}")))?;

    // Open input dataset
    let in_ds = Dataset::open(&in_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Open input: {e}")))?;

    let out_path = tmp.path().join("slope_pct.tif");

    // Build options: ["-p", "-s", "111120", nullptr]
    // -p: percent slope
    // -s 111120: scale factor (meters per degree) for geographic CRS inputs —
    //   required when input is in lat/lon (degrees) so GDAL can match XY to Z units.
    //   111120 = approximate meters per degree of latitude.
    let opt_p  = "-p\0".as_ptr() as *mut c_char;
    let opt_s  = "-s\0".as_ptr() as *mut c_char;
    let opt_sv = "111120\0".as_ptr() as *mut c_char;
    let opt_null: *mut c_char = std::ptr::null_mut();
    let options: [*mut c_char; 4] = [opt_p, opt_s, opt_sv, opt_null];

    let opts = unsafe {
        gdal_sys::GDALDEMProcessingOptionsNew(options.as_ptr() as *mut *mut c_char, std::ptr::null_mut())
    };
    if opts.is_null() {
        return Err(AppError::Internal(anyhow::anyhow!("GDALDEMProcessingOptionsNew returned null")));
    }

    let mut usage_error: i32 = 0;
    let out_ds = unsafe {
        gdal_sys::GDALDEMProcessing(
            out_path.to_str().unwrap().as_ptr() as *const std::os::raw::c_char,
            in_ds.c_dataset(),
            b"slope\0".as_ptr() as *const std::os::raw::c_char,
            std::ptr::null(), // color filename
            opts,
            &mut usage_error,
        )
    };

    unsafe { gdal_sys::GDALDEMProcessingOptionsFree(opts) };

    if out_ds.is_null() {
        return Err(AppError::BadRequest(format!(
            "GDALDEMProcessing slope (percent) failed. Usage error: {}",
            usage_error
        )));
    }

    // Close output dataset to flush to disk
    unsafe { gdal_sys::GDALClose(out_ds) };

    if !out_path.exists() {
        return Err(AppError::Internal(anyhow::anyhow!(
            "GDALDEMProcessing slope completed but output not created"
        )));
    }

    let out_bytes = std::fs::read(&out_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Read output: {e}")))?;

    Ok(RasterCommandOutput {
        stats: RasterOpStats {
            tool: "slope".to_string(),
            input_count: 1,
            input_size_bytes: input_size,
            output_size_bytes: out_bytes.len(),
        },
        bytes: out_bytes,
        filename: "slope_pct.tif".into(),
        mime_type: "image/tiff".to_string(),
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
            out_path.to_str().unwrap().as_ptr() as *const std::os::raw::c_char,
            in_ds.c_dataset(),
            b"color-relief\0".as_ptr() as *const std::os::raw::c_char,
            color_path.to_str().unwrap().as_ptr() as *const std::os::raw::c_char,
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
    output_type: Option<&str>,
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

    let use_int32 = output_type.map(|t| t.eq_ignore_ascii_case("int32") || t.eq_ignore_ascii_case("int")).unwrap_or(false);
    tokio::task::spawn_blocking(move || {
        run_raster_calc_sync(&rasters_data, &expr, total_size, use_int32)
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Task join error: {}", e)))?
}

fn run_raster_calc_sync(
    rasters_data: &BTreeMap<char, Vec<u8>>,
    expr: &Expr,
    total_input_size: usize,
    output_int32: bool,
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

    if output_int32 {
        let int_data: Vec<i32> = output_data.iter().map(|&v| if v.is_nan() { i32::MIN } else { v as i32 }).collect();
        let mut out_ds = driver.create_with_band_type::<i32, _>(
            &out_path, *first_w, *first_h, 1,
        ).map_err(|e| AppError::Internal(anyhow::anyhow!("Create output i32: {e}")))?;
        out_ds.set_geo_transform(&geo_transform)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Set geo transform: {e}")))?;
        out_ds.set_projection(&spatial_ref.to_wkt()
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Get projection WKT: {e}")))?)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Set projection: {e}")))?;
        let mut out_band = out_ds.rasterband(1)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Get output band: {e}")))?;
        let mut buffer = Buffer::new((*first_w, *first_h), int_data);
        out_band.write::<i32>((0, 0), (*first_w, *first_h), &mut buffer)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Write output i32: {e}")))?;
        out_band.set_no_data_value(Some(i32::MIN as f64))
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Set nodata i32: {e}")))?;
        drop(out_ds);
    } else {
        let mut out_ds = driver.create_with_band_type::<f32, _>(
            &out_path, *first_w, *first_h, 1,
        ).map_err(|e| AppError::Internal(anyhow::anyhow!("Create output: {e}")))?;
        out_ds.set_geo_transform(&geo_transform)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Set geo transform: {e}")))?;
        out_ds.set_projection(&spatial_ref.to_wkt()
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Get projection WKT: {e}")))?)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Set projection: {e}")))?;
        let mut out_band = out_ds.rasterband(1)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Get output band: {e}")))?;
        let mut buffer = Buffer::new((*first_w, *first_h), output_data);
        out_band.write::<f32>((0, 0), (*first_w, *first_h), &mut buffer)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Write output: {e}")))?;
        out_band.set_no_data_value(Some(f64::from(NAN)))
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Set nodata: {e}")))?;
        drop(out_ds);
    }

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

/// Run raster format conversion
pub async fn run_raster_convert(
    input: &RasterInput,
    output_format: &str,
) -> Result<RasterCommandOutput, AppError> {
    let format_upper = output_format.to_uppercase();
    let (driver_name, ext, mime_type) = match format_upper.as_str() {
        "GTIFF" | "GTIF" | "TIF" | "TIFF" => ("GTiff", "tif", "image/tiff"),
        "PNG" => ("PNG", "png", "image/png"),
        "JPEG" | "JPG" => ("JPEG", "jpg", "image/jpeg"),
        "AAIGRID" | "ASCII" | "ASC" => ("AAIGrid", "asc", "text/plain"),
        _ => {
            return Err(AppError::BadRequest(format!(
                "Unsupported output format '{}'. Options: GTiff, PNG, JPEG, AAIGrid",
                output_format
            )));
        }
    };

    let bytes = input.bytes.clone();
    let input_size = input.size;
    let driver = driver_name.to_string();
    let ext_owned = ext.to_string();
    let mime = mime_type.to_string();

    tokio::task::spawn_blocking(move || {
        run_raster_convert_sync(&bytes, input_size, &driver, &ext_owned, &mime)
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Task join error: {}", e)))?
}

fn run_raster_convert_sync(
    input_bytes: &[u8],
    input_size: usize,
    driver_name: &str,
    ext: &str,
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

    let out_path = tmp.path().join(format!("output.{}", ext));

    // Standard conversion using create_copy
    let driver = DriverManager::get_driver_by_name(driver_name)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Driver '{}': {e}", driver_name)))?;

    let mut options = Vec::new();
    if driver_name == "GTiff" {
        options.push("COMPRESS=DEFLATE");
    }

    let copy_options: Option<Vec<std::ffi::CString>> = if options.is_empty() {
        None
    } else {
        Some(options.iter().map(|s| std::ffi::CString::new(*s).unwrap()).collect())
    };

    let out_ds = unsafe {
        let options_ptr: *mut *mut std::os::raw::c_char = if let Some(ref opts) = copy_options {
            let mut ptrs: Vec<*mut std::os::raw::c_char> = opts
                .iter()
                .map(|s| s.as_ptr() as *mut std::os::raw::c_char)
                .collect();
            ptrs.push(std::ptr::null_mut());
            ptrs.as_mut_ptr()
        } else {
            std::ptr::null_mut()
        };

        gdal_sys::GDALCreateCopy(
            driver.c_driver(),
            out_path.to_str().unwrap().as_ptr() as *const std::os::raw::c_char,
            in_ds.c_dataset(),
            0, // strict
            options_ptr,
            None, // progress callback
            std::ptr::null_mut(), // progress data
        )
    };

    if out_ds.is_null() {
        return Err(AppError::Internal(anyhow::anyhow!("GDALCreateCopy failed")));
    }

    unsafe { gdal_sys::GDALClose(out_ds) };

    // Read output
    if !out_path.exists() {
        return Err(AppError::Internal(anyhow::anyhow!(
            "Raster conversion completed but output not created"
        )));
    }

    let out_bytes = std::fs::read(&out_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Read output: {e}")))?;

    Ok(RasterCommandOutput {
        stats: RasterOpStats {
            tool: "raster-convert".to_string(),
            input_count: 1,
            input_size_bytes: input_size,
            output_size_bytes: out_bytes.len(),
        },
        bytes: out_bytes,
        filename: format!("converted.{}", ext),
        mime_type: mime_type.to_string(),
    })
}

/// Run mosaic: merge multiple rasters into a single GeoTIFF
pub async fn run_mosaic(
    inputs: &[RasterInput],
    output_crs: Option<&str>,
    resolution: Option<f64>,
    resampling: &str,
    nodata: Option<f64>,
) -> Result<RasterCommandOutput, AppError> {
    if inputs.len() < 2 {
        return Err(AppError::BadRequest("Mosaic requires at least 2 input rasters".into()));
    }
    if inputs.len() > 10 {
        return Err(AppError::BadRequest("Mosaic supports maximum 10 input rasters".into()));
    }

    // Validate resampling method
    let resampling_lower = resampling.to_lowercase();
    if !["nearest", "bilinear", "cubic"].contains(&resampling_lower.as_str()) {
        return Err(AppError::BadRequest(
            "Invalid resampling method. Use: nearest, bilinear, or cubic".into()
        ));
    }

    // Validate output CRS if provided
    let output_crs_normalized = if let Some(crs) = output_crs {
        Some(normalize_crs_string(crs)?)
    } else {
        None
    };

    // Clone inputs for blocking task
    let inputs_data: Vec<(Vec<u8>, String)> = inputs
        .iter()
        .map(|r| (r.bytes.clone(), r.filename.clone()))
        .collect();
    let total_input_size: usize = inputs.iter().map(|r| r.size).sum();
    let resampling_owned = resampling_lower;

    tokio::task::spawn_blocking(move || {
        run_mosaic_sync(&inputs_data, total_input_size, output_crs_normalized.as_deref(), resolution, &resampling_owned, nodata)
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Task join error: {}", e)))?
}

fn run_mosaic_sync(
    inputs: &[(Vec<u8>, String)],
    total_input_size: usize,
    output_crs: Option<&str>,
    resolution: Option<f64>,
    resampling: &str,
    nodata: Option<f64>,
) -> Result<RasterCommandOutput, AppError> {
    use std::process::Command;

    let tmp = TempDir::new().map_err(|e| AppError::Internal(anyhow::anyhow!("TempDir: {e}")))?;

    // Write all input rasters to temp files
    let mut input_paths: Vec<std::path::PathBuf> = Vec::new();
    for (i, (bytes, filename)) in inputs.iter().enumerate() {
        let ext = filename.rsplit_once('.').map(|(_, e)| e).unwrap_or("tif");
        let in_path = tmp.path().join(format!("input_{}.", i)).with_extension(ext);
        std::fs::write(&in_path, bytes)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Write input {}: {}", i, e)))?;
        input_paths.push(in_path);
    }

    // Build VRT using gdalbuildvrt
    let vrt_path = tmp.path().join("mosaic.vrt");

    let mut cmd = Command::new("gdalbuildvrt");
    cmd.arg("-overwrite");

    // Add resampling option
    let resamp_opt = match resampling {
        "bilinear" => "bilinear",
        "cubic" => "cubic",
        _ => "nearest",
    };
    cmd.arg("-r").arg(resamp_opt);

    // Add nodata if specified
    if let Some(nd) = nodata {
        cmd.arg("-srcnodata").arg(nd.to_string());
        cmd.arg("-vrtnodata").arg(nd.to_string());
    }

    cmd.arg(&vrt_path);
    for path in &input_paths {
        cmd.arg(path);
    }

    let output = cmd.output()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("gdalbuildvrt failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Internal(anyhow::anyhow!("gdalbuildvrt error: {}", stderr)));
    }

    // Translate VRT to GeoTIFF using gdal_translate
    let out_path = tmp.path().join("mosaic.tif");

    let mut cmd = Command::new("gdal_translate");
    cmd.arg("-of").arg("GTiff");
    cmd.arg("-co").arg("COMPRESS=DEFLATE");

    // Add output CRS if specified
    if let Some(crs) = output_crs {
        cmd.arg("-t_srs").arg(crs);
    }

    // Add resolution if specified
    if let Some(res) = resolution {
        cmd.arg("-tr").arg(res.to_string()).arg(res.to_string());
    }

    // Add resampling for translation
    let resamp_alg = match resampling {
        "bilinear" => "bilinear",
        "cubic" => "cubic",
        _ => "nearest",
    };
    cmd.arg("-r").arg(resamp_alg);

    cmd.arg(&vrt_path);
    cmd.arg(&out_path);

    let output = cmd.output()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("gdal_translate failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Internal(anyhow::anyhow!("gdal_translate error: {}", stderr)));
    }

    // Read output
    if !out_path.exists() {
        return Err(AppError::Internal(anyhow::anyhow!(
            "Mosaic completed but output not created"
        )));
    }

    let out_bytes = std::fs::read(&out_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Read output: {e}")))?;

    Ok(RasterCommandOutput {
        stats: RasterOpStats {
            tool: "mosaic".to_string(),
            input_count: inputs.len(),
            input_size_bytes: total_input_size,
            output_size_bytes: out_bytes.len(),
        },
        bytes: out_bytes,
        filename: "mosaic.tif".into(),
        mime_type: "image/tiff".to_string(),
    })
}

/// Run raster-to-vector polygonization using GDALPolygonize
/// Converts connected regions of same-valued pixels into polygon features
pub async fn run_raster_to_vector(
    input: &RasterInput,
    band: Option<u8>,
    field_name: Option<&str>,
    no_data_value: Option<f64>,
) -> Result<RasterCommandOutput, AppError> {
    let bytes = input.bytes.clone();
    let input_size = input.size;
    let band = band.unwrap_or(1);
    let field_name = field_name.unwrap_or("DN").to_string();
    let no_data = no_data_value;

    tokio::task::spawn_blocking(move || {
        run_raster_to_vector_sync(&bytes, input_size, band, &field_name, no_data)
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Task join error: {}", e)))?
}

fn run_raster_to_vector_sync(
    input_bytes: &[u8],
    input_size: usize,
    band_num: u8,
    field_name: &str,
    no_data: Option<f64>,
) -> Result<RasterCommandOutput, AppError> {
    let tmp = TempDir::new().map_err(|e| AppError::Internal(anyhow::anyhow!("TempDir: {e}")))?;

    // Write input
    let in_path = tmp.path().join("input.tif");
    std::fs::write(&in_path, input_bytes)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Write input: {e}")))?;

    // Open input
    let in_ds = Dataset::open(&in_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Open input: {e}")))?;

    // Get the specified band
    let band = in_ds.rasterband(band_num as usize)
        .map_err(|e| AppError::BadRequest(format!("Invalid band {}: {}", band_num, e)))?;

    // Get no-data value (use provided or from raster)
    let no_data_val = no_data.or_else(|| band.no_data_value());
    let c_band = unsafe { band.c_rasterband() };

    // Create output GeoJSON dataset
    let out_path = tmp.path().join("polygons.geojson");
    let driver = DriverManager::get_driver_by_name("GeoJSON")
        .map_err(|e| AppError::Internal(anyhow::anyhow!("GeoJSON driver: {e}")))?;

    let (in_xsize, in_ysize) = in_ds.raster_size();
    let mut out_ds = driver.create(&out_path, in_xsize, in_ysize, 0)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Create output: {e}")))?;

    // Get spatial reference from input
    let srs = in_ds.spatial_ref().ok();

    // Create layer for polygons
    let mut layer_options = LayerOptions {
        name: "polygons",
        srs: srs.as_ref(),
        ty: gdal_sys::OGRwkbGeometryType::wkbPolygon,
        options: None,
    };

    let mut layer = out_ds.create_layer(layer_options)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Create layer: {e}")))?;

    // Add DN field for pixel values
    let dn_field = FieldDefn::new(field_name, OGRFieldType::OFTReal)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Create DN field: {e}")))?;
    dn_field.add_to_layer(&layer)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Add DN field: {e}")))?;

    // Get the raw layer handle
    let c_layer = unsafe { layer.c_layer() };

    // Call GDALPolygonize (unsafe)
    // Arguments: band, mask_band, layer, field_index, options, progress_callback, progress_data
    let result = unsafe {
        gdal_sys::GDALPolygonize(
            c_band,
            std::ptr::null_mut(), // no mask band
            c_layer,
            0, // field index (first field we created)
            std::ptr::null_mut(), // options
            None, // progress callback
            std::ptr::null_mut(), // progress data
        )
    };

    if result != gdal_sys::CPLErr::CE_None {
        return Err(AppError::Internal(anyhow::anyhow!("GDALPolygonize failed")));
    }

    // Drop layer before flushing dataset
    drop(layer);

    // Flush to ensure features are written
    out_ds.flush_cache()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Flush output: {e}")))?;

    drop(out_ds);

    if !out_path.exists() {
        return Err(AppError::Internal(anyhow::anyhow!(
            "Polygonize completed but output not created"
        )));
    }

    let out_bytes = std::fs::read(&out_path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Read output: {e}")))?;

    Ok(RasterCommandOutput {
        stats: RasterOpStats {
            tool: "raster-to-vector".to_string(),
            input_count: 1,
            input_size_bytes: input_size,
            output_size_bytes: out_bytes.len(),
        },
        bytes: out_bytes,
        filename: "polygons.geojson".into(),
        mime_type: "application/geo+json".to_string(),
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
    Pow(Box<Expr>, Box<Expr>),
    Neg(Box<Expr>),
    Abs(Box<Expr>),
    Floor(Box<Expr>),
    Ceil(Box<Expr>),
    Round(Box<Expr>),
    Sqrt(Box<Expr>),
    Min(Box<Expr>, Box<Expr>),
    Max(Box<Expr>, Box<Expr>),
    Sin(Box<Expr>),
    Cos(Box<Expr>),
    Tan(Box<Expr>),
    Atan2(Box<Expr>, Box<Expr>),
    // Comparison operators for conditional
    Gt(Box<Expr>, Box<Expr>),   // >
    Lt(Box<Expr>, Box<Expr>),   // <
    Gte(Box<Expr>, Box<Expr>),  // >=
    Lte(Box<Expr>, Box<Expr>),  // <=
    Eq(Box<Expr>, Box<Expr>),   // ==
    Neq(Box<Expr>, Box<Expr>),  // !=
    // Conditional: where(condition, true_val, false_val)
    Where(Box<Expr>, Box<Expr>, Box<Expr>),
}

fn parse_expression(s: &str) -> Result<Expr, String> {
    let mut chars: Vec<char> = s.chars().collect();
    parse_where(&mut chars)
}

// where(condition, true_val, false_val) - lowest precedence
fn parse_where(chars: &mut Vec<char>) -> Result<Expr, String> {
    skip_whitespace(chars);
    
    // Check for "where" keyword
    if starts_with_keyword(chars, "where") {
        consume_keyword(chars, "where")?;
        skip_whitespace(chars);
        expect_char(chars, '(')?;
        let condition = parse_comparison(chars)?;
        skip_whitespace(chars);
        expect_char(chars, ',')?;
        let true_val = parse_where(chars)?;
        skip_whitespace(chars);
        expect_char(chars, ',')?;
        let false_val = parse_where(chars)?;
        skip_whitespace(chars);
        expect_char(chars, ')')?;
        return Ok(Expr::Where(Box::new(condition), Box::new(true_val), Box::new(false_val)));
    }
    
    parse_comparison(chars)
}

// Comparison operators: >, <, >=, <=, ==, !=
fn parse_comparison(chars: &mut Vec<char>) -> Result<Expr, String> {
    let mut left = parse_add_sub(chars)?;
    skip_whitespace(chars);
    
    while !chars.is_empty() {
        if starts_with(chars, ">=") {
            chars.remove(0);
            chars.remove(0);
            let right = parse_add_sub(chars)?;
            left = Expr::Gte(Box::new(left), Box::new(right));
        } else if starts_with(chars, "<=") {
            chars.remove(0);
            chars.remove(0);
            let right = parse_add_sub(chars)?;
            left = Expr::Lte(Box::new(left), Box::new(right));
        } else if starts_with(chars, "==") {
            chars.remove(0);
            chars.remove(0);
            let right = parse_add_sub(chars)?;
            left = Expr::Eq(Box::new(left), Box::new(right));
        } else if starts_with(chars, "!=") {
            chars.remove(0);
            chars.remove(0);
            let right = parse_add_sub(chars)?;
            left = Expr::Neq(Box::new(left), Box::new(right));
        } else if chars[0] == '>' {
            chars.remove(0);
            let right = parse_add_sub(chars)?;
            left = Expr::Gt(Box::new(left), Box::new(right));
        } else if chars[0] == '<' {
            chars.remove(0);
            let right = parse_add_sub(chars)?;
            left = Expr::Lt(Box::new(left), Box::new(right));
        } else {
            break;
        }
        skip_whitespace(chars);
    }
    
    Ok(left)
}

fn parse_add_sub(chars: &mut Vec<char>) -> Result<Expr, String> {
    let mut left = parse_mul_div(chars)?;

    while !chars.is_empty() {
        skip_whitespace(chars);
        if chars.is_empty() { break; }
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
        skip_whitespace(chars);
        if chars.is_empty() { break; }
        
        if chars[0] == '*' {
            chars.remove(0);
            let right = parse_unary(chars)?;
            left = Expr::Mul(Box::new(left), Box::new(right));
        } else if chars[0] == '/' {
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
    skip_whitespace(chars);
    if !chars.is_empty() && chars[0] == '-' {
        chars.remove(0);
        let expr = parse_pow(chars)?;
        return Ok(Expr::Neg(Box::new(expr)));
    }
    parse_pow(chars)
}

fn parse_pow(chars: &mut Vec<char>) -> Result<Expr, String> {
    let left = parse_call_or_primary(chars)?;
    
    skip_whitespace(chars);
    if starts_with(chars, "**") {
        chars.remove(0);
        chars.remove(0);
        // Right-associative: recurse on the right side
        let right = parse_pow(chars)?;
        Ok(Expr::Pow(Box::new(left), Box::new(right)))
    } else {
        Ok(left)
    }
}

fn parse_call_or_primary(chars: &mut Vec<char>) -> Result<Expr, String> {
    skip_whitespace(chars);
    
    // Check for function calls: abs, sqrt, min, max, sin, cos, tan, atan2, pow
    if chars.len() >= 3 {
        let possible_fn: String = chars.iter().take(6).collect();
        let fn_name = possible_fn.chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>();
        
        match fn_name.as_str() {
            "abs" | "floor" | "ceil" | "round" | "sqrt" | "sin" | "cos" | "tan" | "min" | "max" | "atan2" | "pow" => {
                // Consume the function name
                for _ in 0..fn_name.len() {
                    chars.remove(0);
                }
                skip_whitespace(chars);
                expect_char(chars, '(')?;
                
                let expr = match fn_name.as_str() {
                    "abs" => {
                        let arg = parse_where(chars)?;
                        Expr::Abs(Box::new(arg))
                    }
                    "floor" => {
                        let arg = parse_where(chars)?;
                        Expr::Floor(Box::new(arg))
                    }
                    "ceil" => {
                        let arg = parse_where(chars)?;
                        Expr::Ceil(Box::new(arg))
                    }
                    "round" => {
                        let arg = parse_where(chars)?;
                        Expr::Round(Box::new(arg))
                    }
                    "sqrt" => {
                        let arg = parse_where(chars)?;
                        Expr::Sqrt(Box::new(arg))
                    }
                    "sin" => {
                        let arg = parse_where(chars)?;
                        Expr::Sin(Box::new(arg))
                    }
                    "cos" => {
                        let arg = parse_where(chars)?;
                        Expr::Cos(Box::new(arg))
                    }
                    "tan" => {
                        let arg = parse_where(chars)?;
                        Expr::Tan(Box::new(arg))
                    }
                    "min" | "max" | "atan2" | "pow" => {
                        let arg1 = parse_where(chars)?;
                        skip_whitespace(chars);
                        expect_char(chars, ',')?;
                        let arg2 = parse_where(chars)?;
                        match fn_name.as_str() {
                            "min" => Expr::Min(Box::new(arg1), Box::new(arg2)),
                            "max" => Expr::Max(Box::new(arg1), Box::new(arg2)),
                            "atan2" => Expr::Atan2(Box::new(arg1), Box::new(arg2)),
                            "pow" => Expr::Pow(Box::new(arg1), Box::new(arg2)),
                            _ => unreachable!()
                        }
                    }
                    _ => unreachable!()
                };
                
                skip_whitespace(chars);
                expect_char(chars, ')')?;
                return Ok(expr);
            }
            _ => {}
        }
    }
    
    parse_primary(chars)
}

fn parse_primary(chars: &mut Vec<char>) -> Result<Expr, String> {
    skip_whitespace(chars);

    if chars.is_empty() {
        return Err("Unexpected end of expression".to_string());
    }

    // Parentheses
    if chars[0] == '(' {
        chars.remove(0);
        let expr = parse_where(chars)?;
        skip_whitespace(chars);
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
        return Ok(Expr::Var(var));
    }

    Err(format!("Unexpected character: {}", chars[0]))
}

// Helper functions
fn skip_whitespace(chars: &mut Vec<char>) {
    while !chars.is_empty() && chars[0].is_whitespace() {
        chars.remove(0);
    }
}

fn starts_with(chars: &[char], prefix: &str) -> bool {
    if chars.len() < prefix.len() {
        return false;
    }
    chars.iter().zip(prefix.chars()).all(|(a, b)| *a == b)
}

fn starts_with_keyword(chars: &[char], keyword: &str) -> bool {
    if chars.len() < keyword.len() {
        return false;
    }
    let prefix: String = chars.iter().take(keyword.len()).collect();
    prefix.eq_ignore_ascii_case(keyword)
}

fn consume_keyword(chars: &mut Vec<char>, keyword: &str) -> Result<(), String> {
    if !starts_with_keyword(chars, keyword) {
        return Err(format!("Expected keyword: {}", keyword));
    }
    for _ in 0..keyword.len() {
        chars.remove(0);
    }
    Ok(())
}

fn expect_char(chars: &mut Vec<char>, expected: char) -> Result<(), String> {
    skip_whitespace(chars);
    if chars.is_empty() {
        return Err(format!("Expected '{}' but reached end of expression", expected));
    }
    if chars[0] != expected {
        return Err(format!("Expected '{}' but found '{}'", expected, chars[0]));
    }
    chars.remove(0);
    Ok(())
}

fn eval_expr(expr: &Expr, env: &BTreeMap<char, f32>) -> f32 {
    match expr {
        Expr::Num(n) => *n,
        Expr::Var(c) => env.get(c).copied().unwrap_or(f32::NAN),
        Expr::Add(a, b) => {
            let av = eval_expr(a, env);
            let bv = eval_expr(b, env);
            if av.is_nan() || bv.is_nan() { f32::NAN } else { av + bv }
        }
        Expr::Sub(a, b) => {
            let av = eval_expr(a, env);
            let bv = eval_expr(b, env);
            if av.is_nan() || bv.is_nan() { f32::NAN } else { av - bv }
        }
        Expr::Mul(a, b) => {
            let av = eval_expr(a, env);
            let bv = eval_expr(b, env);
            if av.is_nan() || bv.is_nan() { f32::NAN } else { av * bv }
        }
        Expr::Div(a, b) => {
            let av = eval_expr(a, env);
            let bv = eval_expr(b, env);
            if av.is_nan() || bv.is_nan() { f32::NAN } else if bv == 0.0 { f32::NAN } else { av / bv }
        }
        Expr::Pow(a, b) => {
            let av = eval_expr(a, env);
            let bv = eval_expr(b, env);
            if av.is_nan() || bv.is_nan() { f32::NAN } else { av.powf(bv) }
        }
        Expr::Neg(a) => {
            let av = eval_expr(a, env);
            if av.is_nan() { f32::NAN } else { -av }
        }
        Expr::Abs(a) => {
            let av = eval_expr(a, env);
            if av.is_nan() { f32::NAN } else { av.abs() }
        }
        Expr::Floor(a) => {
            let av = eval_expr(a, env);
            if av.is_nan() { f32::NAN } else { av.floor() }
        }
        Expr::Ceil(a) => {
            let av = eval_expr(a, env);
            if av.is_nan() { f32::NAN } else { av.ceil() }
        }
        Expr::Round(a) => {
            let av = eval_expr(a, env);
            if av.is_nan() { f32::NAN } else { av.round() }
        }
        Expr::Sqrt(a) => {
            let av = eval_expr(a, env);
            if av.is_nan() { f32::NAN } else { av.sqrt() }
        }
        Expr::Min(a, b) => {
            let av = eval_expr(a, env);
            let bv = eval_expr(b, env);
            if av.is_nan() || bv.is_nan() { f32::NAN } else { av.min(bv) }
        }
        Expr::Max(a, b) => {
            let av = eval_expr(a, env);
            let bv = eval_expr(b, env);
            if av.is_nan() || bv.is_nan() { f32::NAN } else { av.max(bv) }
        }
        Expr::Sin(a) => {
            let av = eval_expr(a, env);
            if av.is_nan() { f32::NAN } else { av.sin() }
        }
        Expr::Cos(a) => {
            let av = eval_expr(a, env);
            if av.is_nan() { f32::NAN } else { av.cos() }
        }
        Expr::Tan(a) => {
            let av = eval_expr(a, env);
            if av.is_nan() { f32::NAN } else { av.tan() }
        }
        Expr::Atan2(a, b) => {
            let av = eval_expr(a, env);
            let bv = eval_expr(b, env);
            if av.is_nan() || bv.is_nan() { f32::NAN } else { av.atan2(bv) }
        }
        Expr::Gt(a, b) => {
            let av = eval_expr(a, env);
            let bv = eval_expr(b, env);
            if av.is_nan() || bv.is_nan() { f32::NAN } else { if av > bv { 1.0 } else { 0.0 } }
        }
        Expr::Lt(a, b) => {
            let av = eval_expr(a, env);
            let bv = eval_expr(b, env);
            if av.is_nan() || bv.is_nan() { f32::NAN } else { if av < bv { 1.0 } else { 0.0 } }
        }
        Expr::Gte(a, b) => {
            let av = eval_expr(a, env);
            let bv = eval_expr(b, env);
            if av.is_nan() || bv.is_nan() { f32::NAN } else { if av >= bv { 1.0 } else { 0.0 } }
        }
        Expr::Lte(a, b) => {
            let av = eval_expr(a, env);
            let bv = eval_expr(b, env);
            if av.is_nan() || bv.is_nan() { f32::NAN } else { if av <= bv { 1.0 } else { 0.0 } }
        }
        Expr::Eq(a, b) => {
            let av = eval_expr(a, env);
            let bv = eval_expr(b, env);
            if av.is_nan() || bv.is_nan() { f32::NAN } else { if av == bv { 1.0 } else { 0.0 } }
        }
        Expr::Neq(a, b) => {
            let av = eval_expr(a, env);
            let bv = eval_expr(b, env);
            if av.is_nan() || bv.is_nan() { f32::NAN } else { if av != bv { 1.0 } else { 0.0 } }
        }
        Expr::Where(cond, true_val, false_val) => {
            let cond_val = eval_expr(cond, env);
            if cond_val.is_nan() { f32::NAN } else if cond_val != 0.0 { eval_expr(true_val, env) } else { eval_expr(false_val, env) }
        }
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

    #[test]
    fn test_power_operator() {
        let mut env: BTreeMap<char, f32> = BTreeMap::new();
        env.insert('A', 2.0);
        env.insert('B', 3.0);

        // A**B = 2^3 = 8
        assert!((eval_expr(&parse_expression("A**B").unwrap(), &env) - 8.0).abs() < 0.001);
        // pow(A,B) = 8
        assert!((eval_expr(&parse_expression("pow(A,B)").unwrap(), &env) - 8.0).abs() < 0.001);
    }

    #[test]
    fn test_power_precedence() {
        let mut env: BTreeMap<char, f32> = BTreeMap::new();
        env.insert('A', 2.0f32);
        // -A**2 should be -(A**2) = -4, not (-A)**2 = 4
        let expr = parse_expression("-A**2").unwrap();
        assert_eq!(eval_expr(&expr, &env), -4.0);
        // 2**3**2 should be 2**(3**2) = 2**9 = 512 (right-associative)
        let expr2 = parse_expression("2**3**2").unwrap();
        assert_eq!(eval_expr(&expr2, &env), 512.0);
    }

    #[test]
    fn test_abs_sqrt() {
        let mut env: BTreeMap<char, f32> = BTreeMap::new();
        env.insert('A', -5.0);
        env.insert('B', 16.0);

        assert_eq!(eval_expr(&parse_expression("abs(A)").unwrap(), &env), 5.0);
        assert_eq!(eval_expr(&parse_expression("sqrt(B)").unwrap(), &env), 4.0);
    }

    #[test]
    fn test_min_max() {
        let mut env: BTreeMap<char, f32> = BTreeMap::new();
        env.insert('A', 10.0);
        env.insert('B', 5.0);

        assert_eq!(eval_expr(&parse_expression("min(A,B)").unwrap(), &env), 5.0);
        assert_eq!(eval_expr(&parse_expression("max(A,B)").unwrap(), &env), 10.0);
    }

    #[test]
    fn test_trig_functions() {
        let mut env: BTreeMap<char, f32> = BTreeMap::new();
        env.insert('A', 0.0);
        env.insert('B', std::f32::consts::PI / 2.0);

        assert!((eval_expr(&parse_expression("sin(A)").unwrap(), &env) - 0.0).abs() < 0.001);
        assert!((eval_expr(&parse_expression("cos(A)").unwrap(), &env) - 1.0).abs() < 0.001);
        assert!((eval_expr(&parse_expression("sin(B)").unwrap(), &env) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_atan2() {
        let mut env: BTreeMap<char, f32> = BTreeMap::new();
        env.insert('A', 1.0);
        env.insert('B', 1.0);

        let result = eval_expr(&parse_expression("atan2(A,B)").unwrap(), &env);
        assert!((result - std::f32::consts::PI / 4.0).abs() < 0.001);
    }

    #[test]
    fn test_comparisons() {
        let mut env: BTreeMap<char, f32> = BTreeMap::new();
        env.insert('A', 10.0);
        env.insert('B', 5.0);

        assert_eq!(eval_expr(&parse_expression("A > B").unwrap(), &env), 1.0);
        assert_eq!(eval_expr(&parse_expression("A < B").unwrap(), &env), 0.0);
        assert_eq!(eval_expr(&parse_expression("A >= B").unwrap(), &env), 1.0);
        assert_eq!(eval_expr(&parse_expression("A <= B").unwrap(), &env), 0.0);
        assert_eq!(eval_expr(&parse_expression("A == B").unwrap(), &env), 0.0);
        assert_eq!(eval_expr(&parse_expression("A != B").unwrap(), &env), 1.0);
    }

    #[test]
    fn test_where_conditional() {
        let mut env: BTreeMap<char, f32> = BTreeMap::new();
        env.insert('A', 10.0);
        env.insert('B', 5.0);

        // where(A > B, 100, 0) = 100
        assert_eq!(eval_expr(&parse_expression("where(A > B, 100, 0)").unwrap(), &env), 100.0);
        // where(A < B, 100, 0) = 0
        assert_eq!(eval_expr(&parse_expression("where(A < B, 100, 0)").unwrap(), &env), 0.0);
    }

    #[test]
    fn test_complex_expression() {
        let mut env: BTreeMap<char, f32> = BTreeMap::new();
        env.insert('A', 4.0);
        env.insert('B', 2.0);

        // sqrt(A) + pow(B, 2) = 2 + 4 = 6
        let result = eval_expr(&parse_expression("sqrt(A) + pow(B, 2)").unwrap(), &env);
        assert!((result - 6.0).abs() < 0.001);
    }

    #[test]
    fn test_raster_to_vector_params() {
        // Test that band defaults are handled correctly
        let band: Option<u8> = None;
        let resolved = band.unwrap_or(1);
        assert_eq!(resolved, 1);

        let band: Option<u8> = Some(3);
        let resolved = band.unwrap_or(1);
        assert_eq!(resolved, 3);

        // Test field_name default
        let field_name: Option<&str> = None;
        let resolved = field_name.unwrap_or("DN");
        assert_eq!(resolved, "DN");

        let field_name: Option<&str> = Some("value");
        let resolved = field_name.unwrap_or("DN");
        assert_eq!(resolved, "value");
    }
}
