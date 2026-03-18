use gdal::DriverManager;
use gdal::vector::{FieldValue, LayerAccess};
use serde_json::Value;
use std::path::PathBuf;

use crate::error::AppError;

#[derive(Debug, Clone, PartialEq)]
pub enum InputFormat {
    GeoJson,
    Shapefile,
    Kml,
    Gpkg,
}

impl InputFormat {
    pub fn from_filename(filename: &str) -> Option<Self> {
        let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
        match ext.as_str() {
            "geojson" | "json" => Some(InputFormat::GeoJson),
            "zip" | "shp" => Some(InputFormat::Shapefile),
            "kml" | "kmz" => Some(InputFormat::Kml),
            "gpkg" => Some(InputFormat::Gpkg),
            _ => None,
        }
    }

    pub fn from_mime_type(mime: &str) -> Option<Self> {
        match mime.to_lowercase().as_str() {
            "application/geo+json" | "application/json" => Some(InputFormat::GeoJson),
            "application/zip" => Some(InputFormat::Shapefile),
            "application/vnd.google-earth.kml+xml" => Some(InputFormat::Kml),
            "application/geopackage+sqlite3" => Some(InputFormat::Gpkg),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum OutputFormat {
    GeoJson,
    Shapefile,
    Kml,
    Gpkg,
    Csv,
}

impl OutputFormat {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "geojson" | "json" => Some(OutputFormat::GeoJson),
            "shapefile" | "shp" => Some(OutputFormat::Shapefile),
            "kml" => Some(OutputFormat::Kml),
            "gpkg" | "geopackage" => Some(OutputFormat::Gpkg),
            "csv" => Some(OutputFormat::Csv),
            _ => None,
        }
    }

    pub fn from_filename(filename: &str) -> Option<Self> {
        let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
        match ext.as_str() {
            "geojson" | "json" => Some(OutputFormat::GeoJson),
            "zip" | "shp" => Some(OutputFormat::Shapefile),
            "kml" | "kmz" => Some(OutputFormat::Kml),
            "gpkg" => Some(OutputFormat::Gpkg),
            "csv" => Some(OutputFormat::Csv),
            _ => None,
        }
    }

    pub fn gdal_driver(&self) -> Option<&'static str> {
        match self {
            OutputFormat::GeoJson => Some("GeoJSON"),
            OutputFormat::Shapefile => Some("ESRI Shapefile"),
            OutputFormat::Kml => Some("KML"),
            OutputFormat::Gpkg => Some("GPKG"),
            OutputFormat::Csv => None, // CSV is handled separately
        }
    }
}

/// Convert vector data between formats.
/// Returns (output_bytes, filename, mime_type).
pub fn do_convert(
    input_bytes: Vec<u8>,
    input_filename: String,
    input_mime: Option<String>,
    output_format_str: Option<String>,
) -> Result<(Vec<u8>, String, String), AppError> {
    // Detect input format
    let input_fmt = InputFormat::from_filename(&input_filename)
        .or_else(|| input_mime.as_ref().and_then(|m| InputFormat::from_mime_type(m)))
        .ok_or_else(|| {
            AppError::BadRequest(format!(
                "Cannot detect input format from filename '{}' or mime type {:?}",
                input_filename, input_mime
            ))
        })?;

    let out_fmt = if let Some(ref s) = output_format_str {
        OutputFormat::from_str(s).ok_or_else(|| {
            AppError::BadRequest(format!(
                "Unsupported output format '{s}'. Options: geojson, shapefile, kml, gpkg, csv"
            ))
        })?
    } else {
        OutputFormat::from_filename(&input_filename).unwrap_or(OutputFormat::GeoJson)
    };

    let tmp_dir = tempfile::TempDir::new()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("TempDir: {e}")))?;

    let output_stem = input_filename
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or("converted");

    // Write input to temp file based on format
    let input_path = match input_fmt {
        InputFormat::GeoJson => {
            let path = tmp_dir.path().join("input.geojson");
            std::fs::write(&path, &input_bytes)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Write input: {e}")))?;
            path
        }
        InputFormat::Shapefile => {
            // Unzip the shapefile
            let shp_dir = tmp_dir.path().join("shp_input");
            std::fs::create_dir_all(&shp_dir)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("mkdir: {e}")))?;
            unzip_to_dir(&input_bytes, &shp_dir)?;
            // Find the .shp file
            find_shp_file(&shp_dir)?
        }
        InputFormat::Kml => {
            let path = tmp_dir.path().join("input.kml");
            std::fs::write(&path, &input_bytes)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Write input: {e}")))?;
            path
        }
        InputFormat::Gpkg => {
            let path = tmp_dir.path().join("input.gpkg");
            std::fs::write(&path, &input_bytes)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Write input: {e}")))?;
            path
        }
    };

    // Handle CSV output separately (attribute table export)
    if out_fmt == OutputFormat::Csv {
        let csv_bytes = export_to_csv(&input_path)?;
        return Ok((csv_bytes, format!("{output_stem}.csv"), "text/csv".to_string()));
    }

    // GDAL-based conversions
    let driver_name = out_fmt.gdal_driver().unwrap();

    match out_fmt {
        OutputFormat::GeoJson => {
            let out_path = tmp_dir.path().join("output.geojson");
            gdal_copy_layer(&input_path, &out_path, driver_name)?;
            let out_bytes = std::fs::read(&out_path)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Read output: {e}")))?;
            Ok((out_bytes, format!("{output_stem}.geojson"), "application/geo+json".to_string()))
        }
        OutputFormat::Shapefile => {
            let shp_dir = tmp_dir.path().join("shp_output");
            std::fs::create_dir_all(&shp_dir)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("mkdir: {e}")))?;
            let shp_path = shp_dir.join("output.shp");
            gdal_copy_layer(&input_path, &shp_path, driver_name)?;
            let zip_bytes = zip_shapefile_dir(&shp_dir)?;
            Ok((zip_bytes, format!("{output_stem}.zip"), "application/zip".to_string()))
        }
        OutputFormat::Kml => {
            let out_path = tmp_dir.path().join("output.kml");
            gdal_copy_layer(&input_path, &out_path, driver_name)?;
            let bytes = std::fs::read(&out_path)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Read KML: {e}")))?;
            Ok((bytes, format!("{output_stem}.kml"), "application/vnd.google-earth.kml+xml".to_string()))
        }
        OutputFormat::Gpkg => {
            let out_path = tmp_dir.path().join("output.gpkg");
            gdal_copy_layer(&input_path, &out_path, driver_name)?;
            let bytes = std::fs::read(&out_path)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Read GPKG: {e}")))?;
            Ok((bytes, format!("{output_stem}.gpkg"), "application/geopackage+sqlite3".to_string()))
        }
        OutputFormat::Csv => unreachable!(),
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

fn unzip_to_dir(zip_bytes: &[u8], dest_dir: &std::path::Path) -> Result<(), AppError> {
    use std::io::Read;
    let cursor = std::io::Cursor::new(zip_bytes);
    let mut zip = zip::ZipArchive::new(cursor)
        .map_err(|e| AppError::BadRequest(format!("Invalid zip file: {e}")))?;

    for i in 0..zip.len() {
        let mut file = zip.by_index(i)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Zip read: {e}")))?;
        let name = file.name();
        // Validate path to prevent directory traversal
        let sanitized = sanitize_zip_path(name)
            .ok_or_else(|| AppError::BadRequest("Zip contains invalid file path".into()))?;
        let out_path = dest_dir.join(sanitized);
        if file.name().ends_with('/') {
            std::fs::create_dir_all(&out_path)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("mkdir: {e}")))?;
        } else {
            let mut out_file = std::fs::File::create(&out_path)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Create file: {e}")))?;
            std::io::copy(&mut file, &mut out_file)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Copy: {e}")))?;
        }
    }
    Ok(())
}

fn find_shp_file(dir: &std::path::Path) -> Result<PathBuf, AppError> {
    for entry in std::fs::read_dir(dir)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Read dir: {e}")))? {
        let entry = entry.map_err(|e| AppError::Internal(anyhow::anyhow!("Dir entry: {e}")))?;
        let path = entry.path();
        if path.extension().map(|e| e == "shp").unwrap_or(false) {
            return Ok(path);
        }
    }
    Err(AppError::BadRequest("No .shp file found in zip archive".into()))
}

fn export_to_csv(input_path: &PathBuf) -> Result<Vec<u8>, AppError> {
    use std::io::Write;

    let ds = gdal::Dataset::open(input_path)
        .map_err(|e| AppError::BadRequest(format!("Cannot open input: {e}")))?;

    let mut layer = ds.layer(0)
        .map_err(|e| AppError::BadRequest(format!("No layers in input: {e}")))?;

    // Collect field info first
    let field_info: Vec<(String, usize)> = layer.defn()
        .fields()
        .enumerate()
        .map(|(i, f)| (f.name().to_string(), i))
        .collect();

    let mut csv = Vec::new();

    // Write header
    let mut header: Vec<String> = field_info.iter().map(|(name, _)| name.clone()).collect();
    header.push("geometry".to_string());
    writeln!(csv, "{}", header.join(","))
        .map_err(|e| AppError::Internal(anyhow::anyhow!("CSV write: {e}")))?;

    // Write rows
    for feature in layer.features() {
        let mut row = Vec::new();
        for (_, field_idx) in &field_info {
            let value = feature.field(*field_idx)
                .unwrap_or(None)
                .map(|v| field_value_to_string(&v))
                .unwrap_or_default();
            row.push(escape_csv_field(&value));
        }
        // Add geometry as WKT, output "NULL" for null geometries
        let geom_wkt = feature.geometry()
            .and_then(|g| g.wkt().ok())
            .unwrap_or_else(|| "NULL".to_string());
        row.push(escape_csv_field(&geom_wkt));

        writeln!(csv, "{}", row.join(","))
            .map_err(|e| AppError::Internal(anyhow::anyhow!("CSV write: {e}")))?;
    }

    Ok(csv)
}

fn field_value_to_string(value: &FieldValue) -> String {
    match value {
        FieldValue::StringValue(s) => s.clone(),
        FieldValue::RealValue(r) => r.to_string(),
        FieldValue::IntegerValue(i) => i.to_string(),
        FieldValue::Integer64Value(i) => i.to_string(),
        FieldValue::DateValue(d) => d.to_string(),
        FieldValue::DateTimeValue(dt) => dt.to_string(),
        _ => String::new(),
    }
}

fn escape_csv_field(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') || field.contains('\r') {
        let escaped = field.replace("\"", "\"\"");
        format!("\"{}\"", escaped)
    } else {
        field.to_string()
    }
}

fn gdal_copy_layer(src: &PathBuf, dst: &PathBuf, driver_name: &str) -> Result<(), AppError> {
    let src_ds = gdal::Dataset::open(src)
        .map_err(|e| AppError::BadRequest(format!("Cannot open source: {e}")))?;

    let driver = DriverManager::get_driver_by_name(driver_name)
        .map_err(|e| AppError::BadRequest(format!("Driver '{driver_name}' not available: {e}")))?;

    let mut dst_ds = driver
        .create_vector_only(dst.to_str().unwrap_or("output"))
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Create output dataset: {e}")))?;

    let src_layer = src_ds
        .layer(0)
        .map_err(|e| AppError::BadRequest(format!("No layers in source: {e}")))?;

    // Iterate features and copy manually
    let defn = src_layer.defn();
    let srs = src_layer.spatial_ref();
    let geom_type = defn.geometry_type();

    use gdal::vector::LayerOptions;
    let layer_opts = LayerOptions {
        name: "output",
        srs: srs.as_ref(),
        ty: geom_type,
        options: None,
    };
    let mut dst_layer = dst_ds.create_layer(layer_opts)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Create layer: {e}")))?;

    // Copy field definitions
    for field in defn.fields() {
        let field_defn = gdal::vector::FieldDefn::new(field.name().as_str(), field.field_type())
            .map_err(|e| AppError::Internal(anyhow::anyhow!("FieldDefn: {e}")))?;
        field_defn.add_to_layer(&dst_layer)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Add field: {e}")))?;
    }

    // Copy features
    let mut src_layer2 = src_ds.layer(0)
        .map_err(|e| AppError::BadRequest(format!("Re-open source layer: {e}")))?;
    for feature in src_layer2.features() {
        feature.create(&dst_layer)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("Create feature: {e}")))?;
    }

    Ok(())
}

fn zip_shapefile_dir(dir: &std::path::Path) -> Result<Vec<u8>, AppError> {
    use std::io::Write;
    let buf = Vec::new();
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(buf));
    let options = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for entry in std::fs::read_dir(dir)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Read shp dir: {e}")))?
    {
        let entry = entry.map_err(|e| AppError::Internal(anyhow::anyhow!("Dir entry: {e}")))?;
        let path = entry.path();
        if path.is_file() {
            let fname = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            zip.start_file(&fname, options)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Zip start file: {e}")))?;
            let data = std::fs::read(&path)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Read file: {e}")))?;
            zip.write_all(&data)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Zip write: {e}")))?;
        }
    }
    let cursor = zip.finish()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Zip finish: {e}")))?;
    Ok(cursor.into_inner())
}
