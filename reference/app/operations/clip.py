"""
POST /clip — Clip a spatial layer to a bounding box or polygon mask.

bbox: [minx, miny, maxx, maxy] in the file's native CRS (or WGS84 if specified)
mask: GeoJSON polygon (optional; if provided, clips to exact polygon shape)
"""
import json
import os
import shutil
import tempfile
from typing import Optional

import geopandas as gpd
from shapely.geometry import box, shape

from app.operations.convert import _unpack_upload, _pack_shapefile, detect_format, DRIVER_MAP, WRITE_FORMATS


def run_clip(
    file_bytes: bytes,
    filename: str,
    bbox: Optional[list[float]],
    mask_geojson: Optional[str],
    output_format: Optional[str],
) -> tuple[bytes, str, str]:
    """
    Clip features to bbox or mask polygon.
    Returns (output_bytes, output_filename, media_type).
    Raises ValueError on bad inputs.
    """
    if not bbox and not mask_geojson:
        raise ValueError("Provide either bbox [minx, miny, maxx, maxy] or a mask GeoJSON polygon.")
    if bbox and len(bbox) != 4:
        raise ValueError("bbox must be exactly 4 values: [minx, miny, maxx, maxy]")

    input_format = detect_format(filename)
    fmt = (output_format or input_format or "geojson").lower()
    if fmt not in WRITE_FORMATS:
        fmt = "geojson"

    tmpdir = tempfile.mkdtemp(prefix="meridian_clip_")
    try:
        src_path = _unpack_upload(file_bytes, filename, tmpdir)
        gdf = gpd.read_file(src_path)

        if mask_geojson:
            try:
                geom = shape(json.loads(mask_geojson))
            except Exception as e:
                raise ValueError(f"Invalid mask GeoJSON: {e}")
            mask_gdf = gpd.GeoDataFrame(geometry=[geom], crs="EPSG:4326")
            if gdf.crs and gdf.crs.to_epsg() != 4326:
                mask_gdf = mask_gdf.to_crs(gdf.crs)
            clipped = gdf.clip(mask_gdf.geometry.iloc[0])
        else:
            clip_box = box(*bbox)
            clipped = gdf.clip(clip_box)

        if clipped.empty:
            raise ValueError("Clip result is empty — no features intersect the provided bbox/mask.")

        driver, ext = DRIVER_MAP[fmt]

        if fmt == "shapefile":
            out_bytes = _pack_shapefile(clipped, tmpdir)
            return out_bytes, "clipped.zip", "application/zip"

        out_path = os.path.join(tmpdir, f"clipped{ext}")
        clipped.to_file(out_path, driver=driver)
        with open(out_path, "rb") as f:
            out_bytes = f.read()

        media_type = {
            "geojson":    "application/geo+json",
            "kml":        "application/vnd.google-earth.kml+xml",
            "gpkg":       "application/geopackage+sqlite3",
            "geopackage": "application/geopackage+sqlite3",
        }.get(fmt, "application/octet-stream")

        return out_bytes, f"clipped{ext}", media_type

    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)
