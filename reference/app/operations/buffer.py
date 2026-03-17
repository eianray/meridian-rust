"""
POST /buffer — Generate buffers around geometries with projection awareness.

Buffer distance is always specified in meters. The operation:
1. Reprojects to an appropriate metric CRS (UTM zone auto-selected from centroid)
2. Applies the buffer in meters
3. Reprojects back to the original CRS (or requested output CRS)

This is important: buffering in degrees (WGS84) produces elliptical, inaccurate
results. Meridian always projects to metric CRS first.
"""
import os
import shutil
import tempfile
from typing import Optional

import geopandas as gpd
from pyproj import CRS

from app.operations.convert import _unpack_upload, _pack_shapefile, detect_format, DRIVER_MAP, WRITE_FORMATS


def _best_utm_epsg(gdf: gpd.GeoDataFrame) -> int:
    """
    Pick the best UTM zone EPSG for a GeoDataFrame based on centroid.
    Works for most of the world. Falls back to Web Mercator (3857) if needed.
    """
    try:
        # Reproject centroid to WGS84 to get lon/lat
        centroid = gdf.to_crs(epsg=4326).geometry.union_all().centroid
        lon, lat = centroid.x, centroid.y

        # UTM zone number
        zone = int((lon + 180) / 6) + 1
        # North or South hemisphere
        if lat >= 0:
            epsg = 32600 + zone   # WGS84 UTM North
        else:
            epsg = 32700 + zone   # WGS84 UTM South
        return epsg
    except Exception:
        return 3857  # Web Mercator fallback


def run_buffer(
    file_bytes: bytes,
    filename: str,
    distance_meters: float,
    output_format: Optional[str],
    cap_style: str,
    join_style: str,
    resolution: int,
    source_epsg: Optional[int] = None,
) -> tuple[bytes, str, str]:
    """
    Buffer all features by distance_meters.
    Returns (out_bytes, out_filename, media_type).
    """
    if distance_meters <= 0:
        raise ValueError("distance_meters must be positive")

    cap_map  = {"round": 1, "flat": 2, "square": 3}
    join_map = {"round": 1, "mitre": 2, "bevel": 3}

    cap  = cap_map.get(cap_style.lower(), 1)
    join = join_map.get(join_style.lower(), 1)

    input_format = detect_format(filename)
    fmt = (output_format or input_format or "geojson").lower()
    if fmt not in WRITE_FORMATS:
        fmt = "geojson"

    tmpdir = tempfile.mkdtemp(prefix="meridian_buffer_")
    try:
        src_path = _unpack_upload(file_bytes, filename, tmpdir)
        gdf = gpd.read_file(src_path)

        # Assign CRS if caller provided one (e.g. for DXF-derived files)
        if source_epsg and gdf.crs is None:
            gdf = gdf.set_crs(epsg=source_epsg)

        # Require explicit CRS — guessing produces wrong buffer distances
        if gdf.crs is None:
            raise ValueError(
                "Input file has no CRS. Provide source_epsg so buffering can be done "
                "in the correct coordinate space. "
                "Example: source_epsg=32610 for UTM Zone 10N (western USA), "
                "source_epsg=4326 for WGS84 (lat/lon degrees — will be auto-projected to UTM)."
            )
        original_crs = gdf.crs

        # Reproject to metric CRS for accurate buffering
        metric_epsg = _best_utm_epsg(gdf)
        gdf_metric = gdf.to_crs(epsg=metric_epsg)

        # Apply buffer
        gdf_metric["geometry"] = gdf_metric.geometry.buffer(
            distance_meters,
            cap_style=cap,
            join_style=join,
            resolution=resolution,
        )

        # Reproject back to original CRS
        gdf_out = gdf_metric.to_crs(original_crs)

        driver, ext = DRIVER_MAP[fmt]

        if fmt == "shapefile":
            out_bytes = _pack_shapefile(gdf_out, tmpdir)
            return out_bytes, f"buffered_{distance_meters}m.zip", "application/zip"

        out_path = os.path.join(tmpdir, f"buffered{ext}")
        gdf_out.to_file(out_path, driver=driver)
        with open(out_path, "rb") as f:
            out_bytes = f.read()

        media_type = {
            "geojson":    "application/geo+json",
            "kml":        "application/vnd.google-earth.kml+xml",
            "gpkg":       "application/geopackage+sqlite3",
            "geopackage": "application/geopackage+sqlite3",
        }.get(fmt, "application/octet-stream")

        return out_bytes, f"buffered_{distance_meters}m{ext}", media_type

    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)
