"""
POST /schema — Extract attribute schema from a spatial file (no geometry download).

Returns field names, types, CRS info, geometry type, feature count, and bbox.
No credit-burning geometry processing — this is the cheap recon operation.
"""
import os
import shutil
import tempfile
from typing import Optional

import fiona
import geopandas as gpd
from pyproj import CRS

from app.operations.convert import _unpack_upload, detect_format


def run_schema(file_bytes: bytes, filename: str) -> dict:
    """
    Extract schema without loading geometry.
    Returns dict with fields, crs, geometry_type, feature_count, bbox, layer_count.
    """
    tmpdir = tempfile.mkdtemp(prefix="meridian_schema_")
    try:
        src_path = _unpack_upload(file_bytes, filename, tmpdir)

        # List available layers
        try:
            layers = fiona.listlayers(src_path)
        except Exception:
            layers = [None]

        layer_schemas = []
        for layer_name in layers:
            try:
                kwargs = {"layer": layer_name} if layer_name else {}
                with fiona.open(src_path, **kwargs) as src:
                    schema = src.schema
                    crs_wkt = src.crs_wkt if src.crs_wkt else None
                    crs_epsg = None
                    crs_name = None

                    if crs_wkt:
                        try:
                            crs_obj = CRS.from_wkt(crs_wkt)
                            crs_epsg = crs_obj.to_epsg()
                            crs_name = crs_obj.name
                        except Exception:
                            pass

                    fields = [
                        {"name": k, "type": v}
                        for k, v in schema.get("properties", {}).items()
                    ]

                    # Fiona reports "Unknown" for mixed-geometry layers.
                    # Scan features to enumerate actual types present.
                    reported_geom = schema.get("geometry")
                    if reported_geom in (None, "Unknown", "unknown"):
                        actual_types = set()
                        for feature in src:
                            geom = feature.get("geometry")
                            if geom and geom.get("type"):
                                actual_types.add(geom["type"])
                        geometry_type = sorted(actual_types) if actual_types else "Unknown"
                    else:
                        geometry_type = reported_geom

                    layer_schemas.append({
                        "layer": layer_name or "default",
                        "geometry_type": geometry_type,
                        "feature_count": len(src),
                        "bbox": list(src.bounds) if src.bounds else None,
                        "crs_epsg": crs_epsg,
                        "crs_name": crs_name,
                        "crs_wkt": crs_wkt,
                        "fields": fields,
                    })

            except Exception as e:
                layer_schemas.append({
                    "layer": layer_name or "default",
                    "error": str(e),
                })

        return {
            "filename": filename,
            "layer_count": len(layer_schemas),
            "layers": layer_schemas,
        }

    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)
