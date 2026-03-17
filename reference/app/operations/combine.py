"""
Multi-input combination operations.

POST /append       — Append features from layer_b onto layer_a (preserves layer_a schema)
POST /merge        — Merge two or more layers into one (union of schemas)
POST /spatial-join — Join attributes from layer_b onto layer_a based on spatial relationship
"""
import os
import shutil
import tempfile
from typing import Optional

import geopandas as gpd
import pandas as pd

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


def _load_and_align(
    bytes_a: bytes, name_a: str,
    bytes_b: bytes, name_b: str,
    tmpdir: str,
) -> tuple[gpd.GeoDataFrame, gpd.GeoDataFrame]:
    """Load two layers and align layer_b CRS to layer_a."""
    path_a = _unpack_upload(bytes_a, name_a, os.path.join(tmpdir, "a"))
    path_b = _unpack_upload(bytes_b, name_b, os.path.join(tmpdir, "b"))

    gdf_a = gpd.read_file(path_a)
    gdf_b = gpd.read_file(path_b)

    if gdf_a.crs and gdf_b.crs and gdf_a.crs != gdf_b.crs:
        gdf_b = gdf_b.to_crs(gdf_a.crs)
    elif gdf_a.crs and not gdf_b.crs:
        gdf_b = gdf_b.set_crs(gdf_a.crs)

    return gdf_a, gdf_b


def run_append(
    bytes_a: bytes, name_a: str,
    bytes_b: bytes, name_b: str,
    output_format: Optional[str],
) -> tuple[bytes, str, str, dict]:
    """
    Append features from layer_b onto layer_a.
    Uses layer_a's schema — any fields in layer_b that don't exist in layer_a are dropped.
    Fields in layer_a missing from layer_b are filled with null.
    CRS of layer_b is automatically aligned to layer_a.
    """
    fmt = (output_format or detect_format(name_a) or "geojson").lower()
    if fmt not in WRITE_FORMATS:
        fmt = "geojson"

    tmpdir = tempfile.mkdtemp(prefix="meridian_append_")
    os.makedirs(os.path.join(tmpdir, "a"), exist_ok=True)
    os.makedirs(os.path.join(tmpdir, "b"), exist_ok=True)

    try:
        gdf_a, gdf_b = _load_and_align(bytes_a, name_a, bytes_b, name_b, tmpdir)

        # Align layer_b columns to layer_a schema
        a_cols = [c for c in gdf_a.columns if c != "geometry"]
        for col in a_cols:
            if col not in gdf_b.columns:
                gdf_b[col] = None
        gdf_b_aligned = gdf_b[a_cols + ["geometry"]].copy()

        result = pd.concat([gdf_a, gdf_b_aligned], ignore_index=True)
        result = gpd.GeoDataFrame(result, geometry="geometry", crs=gdf_a.crs)

        stats = {
            "layer_a_features": len(gdf_a),
            "layer_b_features": len(gdf_b),
            "total_features": len(result),
            "schema": a_cols,
        }

        out_b, out_fn, mime = _write_output(result, fmt, "appended", tmpdir)
        return out_b, out_fn, mime, stats
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)


def run_merge(
    bytes_a: bytes, name_a: str,
    bytes_b: bytes, name_b: str,
    output_format: Optional[str],
) -> tuple[bytes, str, str, dict]:
    """
    Merge two layers into one, preserving all fields from both (union of schemas).
    Fields missing in either layer are filled with null.
    CRS of layer_b is automatically aligned to layer_a.
    """
    fmt = (output_format or detect_format(name_a) or "geojson").lower()
    if fmt not in WRITE_FORMATS:
        fmt = "geojson"

    tmpdir = tempfile.mkdtemp(prefix="meridian_merge_")
    os.makedirs(os.path.join(tmpdir, "a"), exist_ok=True)
    os.makedirs(os.path.join(tmpdir, "b"), exist_ok=True)

    try:
        gdf_a, gdf_b = _load_and_align(bytes_a, name_a, bytes_b, name_b, tmpdir)

        result = pd.concat([gdf_a, gdf_b], ignore_index=True)
        result = gpd.GeoDataFrame(result, geometry="geometry", crs=gdf_a.crs)

        all_cols = [c for c in result.columns if c != "geometry"]
        stats = {
            "layer_a_features": len(gdf_a),
            "layer_b_features": len(gdf_b),
            "total_features": len(result),
            "merged_fields": all_cols,
        }

        out_b, out_fn, mime = _write_output(result, fmt, "merged", tmpdir)
        return out_b, out_fn, mime, stats
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)


def run_spatial_join(
    bytes_a: bytes, name_a: str,
    bytes_b: bytes, name_b: str,
    how: str,
    predicate: str,
    output_format: Optional[str],
) -> tuple[bytes, str, str, dict]:
    """
    Spatial join: join attributes from layer_b onto layer_a based on spatial relationship.

    how: left (keep all of layer_a), right (keep all of layer_b), inner (only matching)
    predicate: intersects, within, contains, crosses, touches, overlaps, nearest
    """
    valid_hows = {"left", "right", "inner"}
    valid_predicates = {"intersects", "within", "contains", "crosses", "touches", "overlaps", "nearest"}

    if how not in valid_hows:
        raise ValueError(f"how must be one of: {', '.join(sorted(valid_hows))}")
    if predicate not in valid_predicates:
        raise ValueError(f"predicate must be one of: {', '.join(sorted(valid_predicates))}")

    fmt = (output_format or detect_format(name_a) or "geojson").lower()
    if fmt not in WRITE_FORMATS:
        fmt = "geojson"

    tmpdir = tempfile.mkdtemp(prefix="meridian_sjoin_")
    os.makedirs(os.path.join(tmpdir, "a"), exist_ok=True)
    os.makedirs(os.path.join(tmpdir, "b"), exist_ok=True)

    try:
        gdf_a, gdf_b = _load_and_align(bytes_a, name_a, bytes_b, name_b, tmpdir)

        # Rename conflicting non-geometry columns in layer_b
        b_cols = {c: f"{c}_right" for c in gdf_b.columns if c != "geometry" and c in gdf_a.columns}
        if b_cols:
            gdf_b = gdf_b.rename(columns=b_cols)

        result = gpd.sjoin(gdf_a, gdf_b, how=how, predicate=predicate)

        # Drop the join index column
        if "index_right" in result.columns:
            result = result.drop(columns=["index_right"])

        result = result.reset_index(drop=True)

        stats = {
            "layer_a_features": len(gdf_a),
            "layer_b_features": len(gdf_b),
            "joined_features": len(result),
            "how": how,
            "predicate": predicate,
        }

        out_b, out_fn, mime = _write_output(result, fmt, "spatial_join", tmpdir)
        return out_b, out_fn, mime, stats
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)
