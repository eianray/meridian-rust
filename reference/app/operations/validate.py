"""
POST /validate — Validate vector geometry. Returns JSON report only.
POST /repair   — Validate + repair. Returns fixed spatial file.

Clean separation: one endpoint tells you what's wrong, the other fixes it.
"""
import os
import shutil
import tempfile
from typing import Optional

import geopandas as gpd
from shapely.validation import explain_validity, make_valid

from app.operations.convert import _unpack_upload, _pack_shapefile, detect_format, DRIVER_MAP, WRITE_FORMATS


def run_validate(file_bytes: bytes, filename: str) -> dict:
    """
    Validate geometry. Returns a JSON-serializable report dict.

    {
        "total_features": int,
        "valid_count":    int,
        "invalid_count":  int,
        "all_valid":      bool,
        "issues": [{"feature_index": int, "reason": str}, ...]
    }
    """
    tmpdir = tempfile.mkdtemp(prefix="meridian_validate_")
    try:
        src_path = _unpack_upload(file_bytes, filename, tmpdir)
        gdf = gpd.read_file(src_path)

        issues = []
        for i, geom in enumerate(gdf.geometry):
            if geom is None:
                issues.append({"feature_index": i, "reason": "null geometry"})
            elif not geom.is_valid:
                issues.append({"feature_index": i, "reason": explain_validity(geom)})

        return {
            "total_features": len(gdf),
            "valid_count":    len(gdf) - len(issues),
            "invalid_count":  len(issues),
            "all_valid":      len(issues) == 0,
            "issues":         issues,
        }
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)


def run_repair(
    file_bytes: bytes,
    filename: str,
    output_format: Optional[str],
) -> tuple[bytes, str, str, dict]:
    """
    Repair invalid geometry using shapely.make_valid().
    Returns (out_bytes, out_filename, media_type, stats).

    stats = {
        "total_features": int,
        "fixed_count":    int,   # features that were invalid and were repaired
    }
    """
    input_format = detect_format(filename)
    fmt = (output_format or input_format or "geojson").lower()
    if fmt not in WRITE_FORMATS:
        fmt = "geojson"

    tmpdir = tempfile.mkdtemp(prefix="meridian_repair_")
    try:
        src_path = _unpack_upload(file_bytes, filename, tmpdir)
        gdf = gpd.read_file(src_path)

        fixed_count = 0
        def _repair(geom):
            nonlocal fixed_count
            if geom is not None and not geom.is_valid:
                fixed_count += 1
                return make_valid(geom)
            return geom

        gdf["geometry"] = gdf["geometry"].apply(_repair)

        driver, ext = DRIVER_MAP[fmt]

        if fmt == "shapefile":
            out_bytes = _pack_shapefile(gdf, tmpdir)
            return out_bytes, "repaired.zip", "application/zip", {
                "total_features": len(gdf), "fixed_count": fixed_count
            }

        out_path = os.path.join(tmpdir, f"repaired{ext}")
        gdf.to_file(out_path, driver=driver)
        with open(out_path, "rb") as f:
            out_bytes = f.read()

        media_type = {
            "geojson":    "application/geo+json",
            "kml":        "application/vnd.google-earth.kml+xml",
            "gpkg":       "application/geopackage+sqlite3",
            "geopackage": "application/geopackage+sqlite3",
        }.get(fmt, "application/octet-stream")

        return out_bytes, f"repaired{ext}", media_type, {
            "total_features": len(gdf), "fixed_count": fixed_count
        }

    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)
