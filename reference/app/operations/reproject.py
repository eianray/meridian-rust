"""
POST /reproject — Project or reproject a spatial file's coordinate reference system (CRS).

Behavior is auto-detected:
- If the file has NO CRS → assigns target_epsg (Project)
- If the file HAS a CRS → reprojects to target_epsg (Reproject)

source_epsg can override/force the input CRS before reprojection.
Accepts any spatial file format. Returns in same format or requested output_format.
"""
import os
import shutil
import tempfile
import zipfile
from pathlib import Path
from typing import Optional

import geopandas as gpd
from pyproj import CRS, exceptions as pyproj_exc

from app.operations.convert import _unpack_upload, _pack_shapefile, detect_format, DRIVER_MAP, WRITE_FORMATS


def run_reproject(
    file_bytes: bytes,
    filename: str,
    target_epsg: int,
    source_epsg: Optional[int],
    output_format: Optional[str],
) -> tuple[bytes, str, str]:
    """
    Reproject spatial file to target_epsg.
    Returns (output_bytes, output_filename, media_type).
    """
    # Validate target CRS
    try:
        target_crs = CRS.from_epsg(target_epsg)
    except pyproj_exc.CRSError:
        raise ValueError(f"Invalid target EPSG: {target_epsg}")

    input_format = detect_format(filename)
    fmt = (output_format or input_format or "geojson").lower()
    if fmt not in WRITE_FORMATS:
        fmt = "geojson"

    tmpdir = tempfile.mkdtemp(prefix="meridian_reproject_")
    try:
        src_path = _unpack_upload(file_bytes, filename, tmpdir)
        gdf = gpd.read_file(src_path)

        # Override source CRS if provided
        if source_epsg:
            try:
                gdf = gdf.set_crs(CRS.from_epsg(source_epsg), allow_override=True)
            except pyproj_exc.CRSError:
                raise ValueError(f"Invalid source EPSG: {source_epsg}")

        if gdf.crs is None:
            # Project: assign CRS to unprojected file
            gdf = gdf.set_crs(target_crs)
        else:
            # Reproject: transform from existing CRS to target
            gdf = gdf.to_crs(target_crs)

        driver, ext = DRIVER_MAP[fmt]

        if fmt == "shapefile":
            out_bytes = _pack_shapefile(gdf, tmpdir)
            return out_bytes, f"reprojected_epsg{target_epsg}.zip", "application/zip"

        out_path = os.path.join(tmpdir, f"output{ext}")
        gdf.to_file(out_path, driver=driver)
        with open(out_path, "rb") as f:
            out_bytes = f.read()

        media_type = {
            "geojson":    "application/geo+json",
            "kml":        "application/vnd.google-earth.kml+xml",
            "gpkg":       "application/geopackage+sqlite3",
            "geopackage": "application/geopackage+sqlite3",
        }.get(fmt, "application/octet-stream")

        return out_bytes, f"reprojected_epsg{target_epsg}{ext}", media_type

    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)
