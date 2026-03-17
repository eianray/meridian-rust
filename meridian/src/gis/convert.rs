use gdal::DriverManager;
use gdal::vector::LayerAccess;
use serde_json::Value;
use std::path::PathBuf;

use crate::error::AppError;

#[derive(Debug, Clone, PartialEq)]
pub enum OutputFormat {
    GeoJson,
    Shapefile,
    Kml,
    Gpkg,
}

impl OutputFormat {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "geojson" | "json" => Some(OutputFormat::GeoJson),
            "shapefile" | "shp" => Some(OutputFormat::Shapefile),
            "kml" => Some(OutputFormat::Kml),
            "gpkg" | "geopackage" => Some(OutputFormat::Gpkg),
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
            _ => None,
        }
    }

    pub fn gdal_driver(&self) -> &'static str {
        match self {
            OutputFormat::GeoJson => "GeoJSON",
            OutputFormat::Shapefile => "ESRI Shapefile",
            OutputFormat::Kml => "KML",
            OutputFormat::Gpkg => "GPKG",
        }
    }
}

/// Convert a GeoJSON input to another format.
/// Returns (output_bytes, filename, mime_type).
pub fn do_convert(
    geojson_str: String,
    input_filename: String,
    output_format_str: Option<String>,
) -> Result<(Vec<u8>, String, String), AppError> {
    let out_fmt = if let Some(ref s) = output_format_str {
        OutputFormat::from_str(s).ok_or_else(|| {
            AppError::BadRequest(format!(
                "Unsupported output format '{s}'. Options: geojson, shapefile, kml, gpkg"
            ))
        })?
    } else {
        OutputFormat::from_filename(&input_filename).unwrap_or(OutputFormat::GeoJson)
    };

    let tmp_dir = tempfile::TempDir::new()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("TempDir: {e}")))?;

    let geojson_path = tmp_dir.path().join("input.geojson");
    std::fs::write(&geojson_path, geojson_str.as_bytes())
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Write temp GeoJSON: {e}")))?;

    let output_stem = input_filename
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or("converted");

    match out_fmt {
        OutputFormat::GeoJson => {
            // Already GeoJSON — re-read and re-serialize cleanly
            let fc: Value = serde_json::from_str(
                std::fs::read_to_string(&geojson_path)
                    .map_err(|e| AppError::Internal(anyhow::anyhow!("Read: {e}")))?
                    .as_str()
            ).map_err(|e| AppError::Internal(anyhow::anyhow!("JSON parse: {e}")))?;
            let out_bytes = serde_json::to_vec_pretty(&fc)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("JSON serialize: {e}")))?;
            Ok((out_bytes, format!("{output_stem}.geojson"), "application/geo+json".to_string()))
        }
        OutputFormat::Shapefile => {
            let shp_dir = tmp_dir.path().join("shp_output");
            std::fs::create_dir_all(&shp_dir)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("mkdir: {e}")))?;
            let shp_path = shp_dir.join("output.shp");
            gdal_copy_layer(&geojson_path, &shp_path, "ESRI Shapefile")?;
            let zip_bytes = zip_shapefile_dir(&shp_dir)?;
            Ok((zip_bytes, format!("{output_stem}.zip"), "application/zip".to_string()))
        }
        OutputFormat::Kml => {
            let out_path = tmp_dir.path().join("output.kml");
            gdal_copy_layer(&geojson_path, &out_path, "KML")?;
            let bytes = std::fs::read(&out_path)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Read KML: {e}")))?;
            Ok((bytes, format!("{output_stem}.kml"), "application/vnd.google-earth.kml+xml".to_string()))
        }
        OutputFormat::Gpkg => {
            let out_path = tmp_dir.path().join("output.gpkg");
            gdal_copy_layer(&geojson_path, &out_path, "GPKG")?;
            let bytes = std::fs::read(&out_path)
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Read GPKG: {e}")))?;
            Ok((bytes, format!("{output_stem}.gpkg"), "application/geopackage+sqlite3".to_string()))
        }
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
