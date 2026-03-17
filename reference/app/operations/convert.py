"""
POST /convert — Convert between spatial vector formats.

Supported formats: GeoJSON, Shapefile (.zip), KML, GeoPackage (.gpkg), GDB (read only)
"""
import io
import os
import shutil
import tempfile
import zipfile
from pathlib import Path
from typing import Optional

import fiona
import geopandas as gpd

# Map friendly names to fiona driver strings
DRIVER_MAP = {
    "geojson":    ("GeoJSON",      ".geojson"),
    "shapefile":  ("ESRI Shapefile", ".zip"),
    "kml":        ("KML",          ".kml"),
    "gpkg":       ("GPKG",         ".gpkg"),
    "geopackage": ("GPKG",         ".gpkg"),
    "gdb":        ("OpenFileGDB",  ".gdb"),  # read only
}

READ_FORMATS  = set(DRIVER_MAP.keys())
WRITE_FORMATS = {"geojson", "shapefile", "kml", "gpkg", "geopackage"}


def detect_format(filename: str) -> Optional[str]:
    """Guess format from filename extension."""
    ext = Path(filename).suffix.lower()
    return {
        ".geojson": "geojson",
        ".json":    "geojson",
        ".zip":     "shapefile",
        ".kml":     "kml",
        ".kmz":     "kml",
        ".gpkg":    "gpkg",
        ".gdb":     "gdb",
    }.get(ext)


def _unpack_upload(data: bytes, filename: str, tmpdir: str) -> str:
    """
    Write uploaded bytes to tmpdir. If it's a .zip, extract and return path.
    Returns the path to the primary spatial file.
    """
    # Sanitize filename to prevent path traversal attacks
    safe_filename = Path(filename).name
    ext = Path(safe_filename).suffix.lower()
    raw_path = os.path.join(tmpdir, safe_filename)
    with open(raw_path, "wb") as f:
        f.write(data)

    if ext == ".zip":
        extract_dir = os.path.join(tmpdir, "extracted")
        os.makedirs(extract_dir, exist_ok=True)
        with zipfile.ZipFile(raw_path) as zf:
            zf.extractall(extract_dir)
        # Find the .shp inside
        for root, dirs, files in os.walk(extract_dir):
            for fname in files:
                if fname.endswith(".shp"):
                    return os.path.join(root, fname)
        # No .shp — maybe a .gdb folder (GDB zipped)
        for root, dirs, _ in os.walk(extract_dir):
            for d in dirs:
                if d.endswith(".gdb"):
                    return os.path.join(root, d)
        raise ValueError("ZIP file does not contain a .shp or .gdb")

    return raw_path


def _pack_shapefile(gdf: gpd.GeoDataFrame, tmpdir: str) -> bytes:
    """Write shapefile to tmpdir and zip it up, return zip bytes."""
    shp_dir = os.path.join(tmpdir, "output_shp")
    os.makedirs(shp_dir, exist_ok=True)
    gdf.to_file(os.path.join(shp_dir, "output.shp"), driver="ESRI Shapefile")
    zip_path = os.path.join(tmpdir, "output.zip")
    with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED) as zf:
        for f in os.listdir(shp_dir):
            zf.write(os.path.join(shp_dir, f), arcname=f)
    with open(zip_path, "rb") as f:
        return f.read()


def run_convert(
    file_bytes: bytes,
    filename: str,
    input_format: Optional[str],
    output_format: str,
) -> tuple[bytes, str, str]:
    """
    Convert spatial file to target format.
    Returns (output_bytes, output_filename, media_type).
    Raises ValueError on unsupported format or read error.
    """
    output_format = output_format.lower()
    if output_format not in WRITE_FORMATS:
        raise ValueError(f"Unsupported output format '{output_format}'. Options: {sorted(WRITE_FORMATS)}")

    if not input_format:
        input_format = detect_format(filename)
    if not input_format or input_format.lower() not in READ_FORMATS:
        raise ValueError(f"Cannot detect or unsupported input format. Specify input_format explicitly.")

    if input_format.lower() == "gdb" and output_format == "gdb":
        raise ValueError("GDB is read-only; cannot output to GDB format.")

    tmpdir = tempfile.mkdtemp(prefix="meridian_convert_")
    try:
        src_path = _unpack_upload(file_bytes, filename, tmpdir)
        gdf = gpd.read_file(src_path)

        driver, ext = DRIVER_MAP[output_format]

        if output_format == "shapefile":
            out_bytes = _pack_shapefile(gdf, tmpdir)
            return out_bytes, "converted.zip", "application/zip"

        out_path = os.path.join(tmpdir, f"output{ext}")
        gdf.to_file(out_path, driver=driver)
        with open(out_path, "rb") as f:
            out_bytes = f.read()

        media_type = {
            "geojson":    "application/geo+json",
            "kml":        "application/vnd.google-earth.kml+xml",
            "gpkg":       "application/geopackage+sqlite3",
            "geopackage": "application/geopackage+sqlite3",
        }.get(output_format, "application/octet-stream")

        return out_bytes, f"converted{ext}", media_type

    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)
