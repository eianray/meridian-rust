"""
Single-input geometry and attribute transformation operations.

POST /erase              — Delete all features, preserve empty schema
POST /dissolve           — Dissolve features by attribute field (or all)
POST /feature-to-point   — Convert polygons/lines to centroid points
POST /feature-to-line    — Convert polygon boundaries to lines
POST /feature-to-polygon — Convert closed lines to polygons
POST /multipart-to-singlepart — Explode multipart geometries to single parts
POST /add-field          — Add a new attribute field with an optional default value
"""
import os
import shutil
import tempfile
from typing import Any, Optional

import geopandas as gpd
import pandas as pd
from shapely.ops import polygonize

from app.operations.convert import (
    _unpack_upload,
    _pack_shapefile,
    detect_format,
    DRIVER_MAP,
    WRITE_FORMATS,
)


def _write_output(gdf: gpd.GeoDataFrame, fmt: str, stem: str, tmpdir: str) -> tuple[bytes, str, str]:
    driver, ext = DRIVER_MAP[fmt]

    if fmt == "shapefile":
        out_bytes = _pack_shapefile(gdf, tmpdir)
        return out_bytes, f"{stem}.zip", "application/zip"

    out_path = os.path.join(tmpdir, f"{stem}{ext}")
    gdf.to_file(out_path, driver=driver)
    with open(out_path, "rb") as f:
        out_bytes = f.read()

    media_type = {
        "geojson":    "application/geo+json",
        "kml":        "application/vnd.google-earth.kml+xml",
        "gpkg":       "application/geopackage+sqlite3",
        "geopackage": "application/geopackage+sqlite3",
    }.get(fmt, "application/octet-stream")

    return out_bytes, f"{stem}{ext}", media_type


def run_erase(
    file_bytes: bytes,
    filename: str,
    output_format: Optional[str],
) -> tuple[bytes, str, str, dict]:
    """
    Delete all features from the dataset, preserving the empty schema.
    Returns an empty file with the same fields and CRS.
    """
    fmt = (output_format or detect_format(filename) or "geojson").lower()
    if fmt not in WRITE_FORMATS:
        fmt = "geojson"

    tmpdir = tempfile.mkdtemp(prefix="meridian_erase_")
    try:
        src_path = _unpack_upload(file_bytes, filename, tmpdir)
        gdf = gpd.read_file(src_path)

        # Keep schema (columns + CRS), drop all rows
        empty = gdf.iloc[0:0].copy()

        stats = {
            "features_removed": len(gdf),
            "fields_preserved": list(empty.columns),
            "crs": str(empty.crs) if empty.crs else None,
        }

        out_b, out_fn, mime = _write_output(empty, fmt, "erased", tmpdir)
        return out_b, out_fn, mime, stats
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)


def run_dissolve(
    file_bytes: bytes,
    filename: str,
    field: Optional[str],
    output_format: Optional[str],
    aggfunc: str,
) -> tuple[bytes, str, str, dict]:
    """
    Dissolve features by a field value, or dissolve all into one if no field given.
    aggfunc controls how non-geometry fields are aggregated: first, sum, mean, count, min, max.
    """
    fmt = (output_format or detect_format(filename) or "geojson").lower()
    if fmt not in WRITE_FORMATS:
        fmt = "geojson"

    valid_aggfuncs = {"first", "sum", "mean", "count", "min", "max"}
    if aggfunc not in valid_aggfuncs:
        raise ValueError(f"aggfunc must be one of: {', '.join(sorted(valid_aggfuncs))}")

    tmpdir = tempfile.mkdtemp(prefix="meridian_dissolve_")
    try:
        src_path = _unpack_upload(file_bytes, filename, tmpdir)
        gdf = gpd.read_file(src_path)

        if field and field not in gdf.columns:
            raise ValueError(f"Field '{field}' not found. Available fields: {list(gdf.columns)}")

        if field:
            result = gdf.dissolve(by=field, aggfunc=aggfunc).reset_index()
        else:
            # Dissolve everything into a single feature
            dissolved_geom = gdf.geometry.union_all()
            result = gpd.GeoDataFrame(geometry=[dissolved_geom], crs=gdf.crs)

        stats = {
            "input_features": len(gdf),
            "output_features": len(result),
            "dissolved_by": field or "all",
        }

        out_b, out_fn, mime = _write_output(result, fmt, "dissolved", tmpdir)
        return out_b, out_fn, mime, stats
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)


def run_feature_to_point(
    file_bytes: bytes,
    filename: str,
    output_format: Optional[str],
) -> tuple[bytes, str, str, dict]:
    """
    Convert polygon or line geometries to their centroid points.
    Point geometries are passed through unchanged.
    All attributes are preserved.
    """
    fmt = (output_format or detect_format(filename) or "geojson").lower()
    if fmt not in WRITE_FORMATS:
        fmt = "geojson"

    tmpdir = tempfile.mkdtemp(prefix="meridian_ftp_")
    try:
        src_path = _unpack_upload(file_bytes, filename, tmpdir)
        gdf = gpd.read_file(src_path)

        result = gdf.copy()
        result["geometry"] = gdf.geometry.centroid

        stats = {
            "input_features": len(gdf),
            "output_features": len(result),
            "input_types": gdf.geometry.geom_type.value_counts().to_dict(),
        }

        out_b, out_fn, mime = _write_output(result, fmt, "points", tmpdir)
        return out_b, out_fn, mime, stats
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)


def run_feature_to_line(
    file_bytes: bytes,
    filename: str,
    output_format: Optional[str],
) -> tuple[bytes, str, str, dict]:
    """
    Convert polygon geometries to their boundary lines.
    Line/Point geometries are passed through unchanged.
    All attributes are preserved.
    """
    fmt = (output_format or detect_format(filename) or "geojson").lower()
    if fmt not in WRITE_FORMATS:
        fmt = "geojson"

    tmpdir = tempfile.mkdtemp(prefix="meridian_ftl_")
    try:
        src_path = _unpack_upload(file_bytes, filename, tmpdir)
        gdf = gpd.read_file(src_path)

        result = gdf.copy()
        result["geometry"] = gdf.geometry.boundary

        # Remove empty geometries (e.g. point boundaries are empty)
        result = result[~result.geometry.is_empty].copy()

        stats = {
            "input_features": len(gdf),
            "output_features": len(result),
        }

        out_b, out_fn, mime = _write_output(result, fmt, "lines", tmpdir)
        return out_b, out_fn, mime, stats
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)


def run_feature_to_polygon(
    file_bytes: bytes,
    filename: str,
    output_format: Optional[str],
) -> tuple[bytes, str, str, dict]:
    """
    Convert closed line geometries to polygons using Shapely polygonize.
    Only closed rings become polygons — open lines are discarded.
    """
    fmt = (output_format or detect_format(filename) or "geojson").lower()
    if fmt not in WRITE_FORMATS:
        fmt = "geojson"

    tmpdir = tempfile.mkdtemp(prefix="meridian_ftpoly_")
    try:
        src_path = _unpack_upload(file_bytes, filename, tmpdir)
        gdf = gpd.read_file(src_path)

        all_geoms = gdf.geometry.union_all()
        polys = list(polygonize(all_geoms))

        if not polys:
            raise ValueError(
                "No closed rings found — polygonize requires lines that form closed loops. "
                "Open lines cannot be converted to polygons."
            )

        result = gpd.GeoDataFrame(geometry=polys, crs=gdf.crs)

        stats = {
            "input_lines": len(gdf),
            "output_polygons": len(result),
        }

        out_b, out_fn, mime = _write_output(result, fmt, "polygons", tmpdir)
        return out_b, out_fn, mime, stats
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)


def run_multipart_to_singlepart(
    file_bytes: bytes,
    filename: str,
    output_format: Optional[str],
) -> tuple[bytes, str, str, dict]:
    """
    Explode multipart geometries into individual single-part features.
    All attributes are duplicated per part. CRS is preserved.
    """
    fmt = (output_format or detect_format(filename) or "geojson").lower()
    if fmt not in WRITE_FORMATS:
        fmt = "geojson"

    tmpdir = tempfile.mkdtemp(prefix="meridian_mp2sp_")
    try:
        src_path = _unpack_upload(file_bytes, filename, tmpdir)
        gdf = gpd.read_file(src_path)

        result = gdf.explode(index_parts=False).reset_index(drop=True)

        stats = {
            "input_features": len(gdf),
            "output_features": len(result),
            "parts_added": len(result) - len(gdf),
        }

        out_b, out_fn, mime = _write_output(result, fmt, "singlepart", tmpdir)
        return out_b, out_fn, mime, stats
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)


def run_add_field(
    file_bytes: bytes,
    filename: str,
    field_name: str,
    field_type: str,
    default_value: Optional[str],
    output_format: Optional[str],
) -> tuple[bytes, str, str, dict]:
    """
    Add a new attribute field to all features.
    field_type: str, int, float, bool
    default_value: string representation of the value (parsed to field_type). Null if omitted.
    """
    fmt = (output_format or detect_format(filename) or "geojson").lower()
    if fmt not in WRITE_FORMATS:
        fmt = "geojson"

    valid_types = {"str", "int", "float", "bool"}
    if field_type not in valid_types:
        raise ValueError(f"field_type must be one of: {', '.join(sorted(valid_types))}")

    tmpdir = tempfile.mkdtemp(prefix="meridian_addfield_")
    try:
        src_path = _unpack_upload(file_bytes, filename, tmpdir)
        gdf = gpd.read_file(src_path)

        if field_name in gdf.columns:
            raise ValueError(f"Field '{field_name}' already exists.")

        # Parse default value to target type
        parsed: Any = None
        if default_value is not None:
            try:
                if field_type == "int":
                    parsed = int(default_value)
                elif field_type == "float":
                    parsed = float(default_value)
                elif field_type == "bool":
                    parsed = default_value.lower() in ("true", "1", "yes")
                else:
                    parsed = str(default_value)
            except (ValueError, TypeError) as e:
                raise ValueError(f"Cannot parse default_value '{default_value}' as {field_type}: {e}")

        gdf[field_name] = parsed

        stats = {
            "field_added": field_name,
            "field_type": field_type,
            "default_value": parsed,
            "features_updated": len(gdf),
        }

        out_b, out_fn, mime = _write_output(gdf, fmt, "with_field", tmpdir)
        return out_b, out_fn, mime, stats
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)
