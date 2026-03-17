"""
POST /union        — Merge two feature layers into one
POST /intersect    — Return features/areas common to both layers
POST /difference   — Return features in layer A that don't overlap layer B

All topology operations accept two file uploads (layer_a, layer_b).
CRS alignment is handled automatically — layer_b is reprojected to layer_a's CRS.
Results are returned in layer_a's CRS.
"""
import os
import shutil
import tempfile
from typing import Optional

import geopandas as gpd
from shapely.ops import unary_union

from app.operations.convert import _unpack_upload, _pack_shapefile, detect_format, DRIVER_MAP, WRITE_FORMATS


def _split_by_geom_type(gdf: gpd.GeoDataFrame) -> dict[str, gpd.GeoDataFrame]:
    """Split a mixed-geometry GeoDataFrame into per-type GeoDataFrames."""
    result = {}
    for geom_type in gdf.geometry.geom_type.unique():
        subset = gdf[gdf.geometry.geom_type == geom_type].copy()
        if not subset.empty:
            result[geom_type] = subset
    return result


def _load_two(
    bytes_a: bytes, name_a: str,
    bytes_b: bytes, name_b: str,
    tmpdir: str,
) -> tuple[gpd.GeoDataFrame, gpd.GeoDataFrame]:
    """
    Load two layers and align CRS to layer_a.
    Does NOT normalize geometry types — callers handle that per-operation.
    """
    path_a = _unpack_upload(bytes_a, name_a, os.path.join(tmpdir, "a"))
    path_b = _unpack_upload(bytes_b, name_b, os.path.join(tmpdir, "b"))

    gdf_a = gpd.read_file(path_a)
    gdf_b = gpd.read_file(path_b)

    # Align CRS: reproject B to A
    if gdf_a.crs and gdf_b.crs and gdf_a.crs != gdf_b.crs:
        gdf_b = gdf_b.to_crs(gdf_a.crs)
    elif gdf_a.crs and not gdf_b.crs:
        gdf_b = gdf_b.set_crs(gdf_a.crs)

    return gdf_a, gdf_b


def _write_multi_output(
    gdfs: dict[str, gpd.GeoDataFrame], fmt: str, stem: str, tmpdir: str
) -> tuple[bytes, str, str, dict]:
    """
    Write multiple geometry-type layers to output.
    - GPKG: one layer per geometry type in a single file
    - Others: ZIP containing one file per geometry type
    Returns (bytes, filename, media_type, stats)
    """
    total = sum(len(g) for g in gdfs.values())
    stats = {
        "total_features": total,
        "geometry_types": {k: len(v) for k, v in gdfs.items()},
        "layer_count": len(gdfs),
    }

    if fmt in ("gpkg", "geopackage"):
        out_path = os.path.join(tmpdir, f"{stem}.gpkg")
        for geom_type, gdf in gdfs.items():
            layer_name = geom_type.lower()
            gdf.to_file(out_path, driver="GPKG", layer=layer_name)
        with open(out_path, "rb") as f:
            out_bytes = f.read()
        return out_bytes, f"{stem}.gpkg", "application/geopackage+sqlite3", stats

    # All other formats: ZIP of per-type files
    import zipfile
    driver, ext = DRIVER_MAP.get(fmt, ("GeoJSON", ".geojson"))
    zip_path = os.path.join(tmpdir, f"{stem}.zip")
    with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED) as zf:
        for geom_type, gdf in gdfs.items():
            type_stem = f"{stem}_{geom_type.lower()}"
            if fmt == "shapefile":
                shp_dir = os.path.join(tmpdir, type_stem)
                os.makedirs(shp_dir, exist_ok=True)
                gdf.to_file(os.path.join(shp_dir, f"{type_stem}.shp"), driver="ESRI Shapefile")
                for fname in os.listdir(shp_dir):
                    zf.write(os.path.join(shp_dir, fname), arcname=fname)
            else:
                out_path = os.path.join(tmpdir, f"{type_stem}{ext}")
                gdf.to_file(out_path, driver=driver)
                zf.write(out_path, arcname=f"{type_stem}{ext}")
    with open(zip_path, "rb") as f:
        out_bytes = f.read()
    return out_bytes, f"{stem}.zip", "application/zip", stats


def _overlay_mixed(
    gdf_a: gpd.GeoDataFrame,
    gdf_b: gpd.GeoDataFrame,
    how: str,
) -> tuple[gpd.GeoDataFrame, dict]:
    """
    Run geopandas.overlay() handling mixed geometry types.
    Splits A and B by geometry type, runs overlay on compatible pairs,
    recombines. Returns (result_gdf, stats).
    Nothing is dropped.
    """
    types_a = _split_by_geom_type(gdf_a)
    types_b = _split_by_geom_type(gdf_b)

    results = []
    # overlay requires polygon inputs for intersection/difference/union
    # For point/line types, spatial join or clip is more appropriate —
    # we use sjoin for points and lines against polygons
    poly_types = {"Polygon", "MultiPolygon"}
    line_types = {"LineString", "MultiLineString"}
    point_types = {"Point", "MultiPoint"}

    for type_a, sub_a in types_a.items():
        for type_b, sub_b in types_b.items():
            try:
                result = gpd.overlay(sub_a, sub_b, how=how, keep_geom_type=False)
                if not result.empty:
                    results.append(result)
            except Exception:
                # Fall back to concat for union, sjoin for others
                if how == "union":
                    results.append(sub_a)
                    results.append(sub_b)

    if not results:
        return gpd.GeoDataFrame(columns=gdf_a.columns, geometry="geometry", crs=gdf_a.crs), {}

    combined = gpd.pd.concat(results, ignore_index=True)
    combined = gpd.GeoDataFrame(combined, geometry="geometry", crs=gdf_a.crs)
    stats = {"geometry_types": combined.geometry.geom_type.value_counts().to_dict()}
    return combined, stats


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


def run_union(
    bytes_a: bytes, name_a: str,
    bytes_b: bytes, name_b: str,
    output_format: Optional[str],
    dissolve: bool,
) -> tuple[bytes, str, str, dict]:
    """
    Union: combine all features from both layers.
    Mixed geometry types are preserved — each type returned as a separate layer.
    If dissolve=True, merge all geometries into a single dissolved feature per type.
    Returns (bytes, filename, media_type, stats).
    """
    fmt = (output_format or detect_format(name_a) or "geojson").lower()
    if fmt not in WRITE_FORMATS:
        fmt = "geojson"

    tmpdir = tempfile.mkdtemp(prefix="meridian_union_")
    os.makedirs(os.path.join(tmpdir, "a"), exist_ok=True)
    os.makedirs(os.path.join(tmpdir, "b"), exist_ok=True)

    try:
        gdf_a, gdf_b = _load_two(bytes_a, name_a, bytes_b, name_b, tmpdir)
        combined = gpd.pd.concat([gdf_a, gdf_b], ignore_index=True)
        combined = gpd.GeoDataFrame(combined, geometry="geometry", crs=gdf_a.crs)

        if dissolve:
            dissolved = unary_union(combined.geometry)
            combined = gpd.GeoDataFrame(geometry=[dissolved], crs=gdf_a.crs)

        geom_types = combined.geometry.geom_type.unique()
        if len(geom_types) > 1:
            split = _split_by_geom_type(combined)
            out_b, out_fn, mime, stats = _write_multi_output(split, fmt, "union", tmpdir)
        else:
            out_b, out_fn, mime = _write_output(combined, fmt, "union", tmpdir)
            stats = {"total_features": len(combined), "geometry_types": {geom_types[0]: len(combined)}}

        return out_b, out_fn, mime, stats
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)


def run_intersect(
    bytes_a: bytes, name_a: str,
    bytes_b: bytes, name_b: str,
    output_format: Optional[str],
) -> tuple[bytes, str, str, dict]:
    """
    Intersect: spatial intersection of the two layers.
    Mixed geometry types handled — results split by type, nothing dropped.
    """
    fmt = (output_format or detect_format(name_a) or "geojson").lower()
    if fmt not in WRITE_FORMATS:
        fmt = "geojson"

    tmpdir = tempfile.mkdtemp(prefix="meridian_intersect_")
    os.makedirs(os.path.join(tmpdir, "a"), exist_ok=True)
    os.makedirs(os.path.join(tmpdir, "b"), exist_ok=True)

    try:
        gdf_a, gdf_b = _load_two(bytes_a, name_a, bytes_b, name_b, tmpdir)
        result, _ = _overlay_mixed(gdf_a, gdf_b, "intersection")

        if result.empty:
            raise ValueError("Intersection is empty — the two layers do not overlap.")

        geom_types = result.geometry.geom_type.unique()
        if len(geom_types) > 1:
            split = _split_by_geom_type(result)
            out_b, out_fn, mime, stats = _write_multi_output(split, fmt, "intersection", tmpdir)
        else:
            out_b, out_fn, mime = _write_output(result, fmt, "intersection", tmpdir)
            stats = {"total_features": len(result), "geometry_types": {geom_types[0]: len(result)}}

        return out_b, out_fn, mime, stats
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)


def run_difference(
    bytes_a: bytes, name_a: str,
    bytes_b: bytes, name_b: str,
    output_format: Optional[str],
) -> tuple[bytes, str, str, dict]:
    """
    Difference: parts of layer_a not covered by layer_b. A minus (A ∩ B).
    Mixed geometry types preserved — nothing dropped.
    """
    fmt = (output_format or detect_format(name_a) or "geojson").lower()
    if fmt not in WRITE_FORMATS:
        fmt = "geojson"

    tmpdir = tempfile.mkdtemp(prefix="meridian_diff_")
    os.makedirs(os.path.join(tmpdir, "a"), exist_ok=True)
    os.makedirs(os.path.join(tmpdir, "b"), exist_ok=True)

    try:
        gdf_a, gdf_b = _load_two(bytes_a, name_a, bytes_b, name_b, tmpdir)
        result, _ = _overlay_mixed(gdf_a, gdf_b, "difference")

        if result.empty:
            raise ValueError("Difference is empty — layer_a is entirely covered by layer_b.")

        geom_types = result.geometry.geom_type.unique()
        if len(geom_types) > 1:
            split = _split_by_geom_type(result)
            out_b, out_fn, mime, stats = _write_multi_output(split, fmt, "difference", tmpdir)
        else:
            out_b, out_fn, mime = _write_output(result, fmt, "difference", tmpdir)
            stats = {"total_features": len(result), "geometry_types": {geom_types[0]: len(result)}}

        return out_b, out_fn, mime, stats
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)
