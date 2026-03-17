"""
POST /dxf — Convert DXF/CAD files to vector spatial formats.

DXF is the lingua franca of AEC (Architecture, Engineering, Construction).
This is the Phase 2 marquee operation — it unlocks the construction/BIM market.

Supported DXF entities:
  LINE, LWPOLYLINE, POLYLINE, CIRCLE, ARC, POINT, INSERT (blocks), HATCH

Important: DXF files carry no CRS. Coordinates are in whatever units the
CAD operator was working in (often local/surveyed or state plane).
Users must specify source_epsg if they want the output georeferenced.
Without it, raw DXF coordinates are preserved as-is with no CRS assigned.
"""
import os
import shutil
import tempfile
from pathlib import Path
from typing import Optional

import ezdxf
from ezdxf import recover
from ezdxf.math import Vec3
import geopandas as gpd
import numpy as np
from shapely.geometry import (
    LineString, MultiLineString, MultiPoint, MultiPolygon,
    Point, Polygon, mapping
)
from shapely.ops import unary_union

from app.operations.convert import _pack_shapefile, DRIVER_MAP, WRITE_FORMATS


# DXF entity types we handle
SUPPORTED_ENTITIES = {
    "LINE", "LWPOLYLINE", "POLYLINE", "CIRCLE", "ARC", "POINT",
    "INSERT", "HATCH", "SPLINE"
}


def _arc_to_points(center: Vec3, radius: float, start_angle: float,
                   end_angle: float, n: int = 32) -> list[tuple]:
    """Approximate an arc as n line segments."""
    import math
    # Normalize angles
    if end_angle < start_angle:
        end_angle += 360.0
    angles = np.linspace(math.radians(start_angle), math.radians(end_angle), n)
    return [
        (center.x + radius * math.cos(a), center.y + radius * math.sin(a))
        for a in angles
    ]


def _circle_to_polygon(center: Vec3, radius: float, n: int = 64) -> Polygon:
    import math
    angles = np.linspace(0, 2 * math.pi, n, endpoint=False)
    coords = [(center.x + radius * math.cos(a), center.y + radius * math.sin(a))
              for a in angles]
    return Polygon(coords)


def _extract_entity(entity, features: list, layer_filter: Optional[list[str]]):
    """Extract one entity into the features list. Called recursively for INSERT blocks."""
    dxftype = entity.dxftype()
    layer = entity.dxf.layer if entity.dxf.hasattr("layer") else "0"

    if layer_filter and layer not in layer_filter:
        return

    props = {"layer": layer, "entity_type": dxftype}

    try:
        if dxftype == "POINT":
            pt = entity.dxf.location
            features.append({"geometry": Point(pt.x, pt.y), **{"layer": layer, "entity_type": dxftype}, "properties": props})

        elif dxftype == "LINE":
            s, e = entity.dxf.start, entity.dxf.end
            features.append({"geometry": LineString([(s.x, s.y), (e.x, e.y)]), "layer": layer, "entity_type": dxftype, "properties": props})

        elif dxftype == "LWPOLYLINE":
            pts = [(p[0], p[1]) for p in entity.get_points()]
            if len(pts) < 2:
                return
            if entity.is_closed and len(pts) >= 3:
                if pts[0] != pts[-1]:
                    pts.append(pts[0])
                features.append({"geometry": Polygon(pts), "layer": layer, "entity_type": dxftype, "properties": props})
            else:
                features.append({"geometry": LineString(pts), "layer": layer, "entity_type": dxftype, "properties": props})

        elif dxftype == "POLYLINE":
            pts = [(v.dxf.location.x, v.dxf.location.y) for v in entity.vertices]
            if len(pts) < 2:
                return
            if entity.is_closed and len(pts) >= 3:
                if pts[0] != pts[-1]:
                    pts.append(pts[0])
                features.append({"geometry": Polygon(pts), "layer": layer, "entity_type": dxftype, "properties": props})
            else:
                features.append({"geometry": LineString(pts), "layer": layer, "entity_type": dxftype, "properties": props})

        elif dxftype == "ARC":
            c = entity.dxf.center
            pts = _arc_to_points(c, entity.dxf.radius, entity.dxf.start_angle, entity.dxf.end_angle)
            features.append({"geometry": LineString(pts), "layer": layer, "entity_type": dxftype, "properties": props})

        elif dxftype == "CIRCLE":
            features.append({"geometry": _circle_to_polygon(entity.dxf.center, entity.dxf.radius), "layer": layer, "entity_type": dxftype, "properties": props})

        elif dxftype == "SPLINE":
            pts = [(p.x, p.y) for p in entity.flattening(0.01)]
            if len(pts) >= 2:
                features.append({"geometry": LineString(pts), "layer": layer, "entity_type": dxftype, "properties": props})

        elif dxftype == "HATCH":
            polys = []
            for path in entity.paths:
                try:
                    pts = [(v.x, v.y) if hasattr(v, 'x') else (v[0], v[1]) for v in path.vertices]
                    if len(pts) >= 3:
                        polys.append(Polygon(pts))
                except Exception:
                    continue
            if polys:
                features.append({"geometry": unary_union(polys), "layer": layer, "entity_type": dxftype, "properties": props})

        elif dxftype == "INSERT":
            # Block reference: resolve via virtual_entities() which applies
            # the INSERT's scale/rotation/translation transform and yields
            # the actual geometry as if it were native modelspace.
            try:
                for virtual_entity in entity.virtual_entities():
                    _extract_entity(virtual_entity, features, layer_filter)
            except Exception:
                pass  # Some INSERT entities reference undefined blocks

    except Exception:
        pass


def _extract_entities(msp, layer_filter: Optional[list[str]] = None) -> list[dict]:
    """
    Walk modelspace and extract all geometry.
    INSERT block references are recursively resolved via virtual_entities().
    """
    features = []
    for entity in msp:
        _extract_entity(entity, features, layer_filter)
    return features


def run_dxf_convert(
    file_bytes: bytes,
    filename: str,
    output_format: str,
    source_epsg: Optional[int],
    layer_filter: Optional[list[str]],
    entity_types: Optional[list[str]],
) -> tuple[bytes, str, str, dict]:
    """
    Convert DXF file to spatial format.
    Returns (out_bytes, out_filename, media_type, stats).

    stats = {feature_count, layer_count, entity_types_found, warnings}
    """
    fmt = output_format.lower()
    if fmt not in WRITE_FORMATS:
        raise ValueError(f"Unsupported output format '{fmt}'. Options: {sorted(WRITE_FORMATS)}")

    tmpdir = tempfile.mkdtemp(prefix="meridian_dxf_")
    try:
        # Sanitize filename to prevent path traversal attacks
        safe_filename = Path(filename).name
        dxf_path = os.path.join(tmpdir, safe_filename)
        with open(dxf_path, "wb") as f:
            f.write(file_bytes)

        # Use recover mode — DXF files from the wild are often slightly malformed
        warnings_list = []
        try:
            doc, auditor = recover.readfile(dxf_path)
            if auditor.has_errors:
                warnings_list.append(f"DXF audit: {len(auditor.errors)} issues auto-corrected")
        except Exception:
            doc = ezdxf.readfile(dxf_path)

        msp = doc.modelspace()

        # Filter by entity type if requested
        type_filter = [t.upper() for t in entity_types] if entity_types else None

        all_features = _extract_entities(msp, layer_filter)

        if type_filter:
            all_features = [f for f in all_features if f["entity_type"] in type_filter]

        if not all_features:
            raise ValueError(
                "No geometry extracted from DXF. "
                "Check layer names or entity_types filter. "
                f"Layers in file: {[layer.dxf.name for layer in doc.layers]}"
            )

        # Build GeoDataFrame
        geometries = [f["geometry"] for f in all_features]
        properties = [f["properties"] for f in all_features]

        gdf = gpd.GeoDataFrame(properties, geometry=geometries)

        # Assign CRS if provided
        if source_epsg:
            try:
                gdf = gdf.set_crs(epsg=source_epsg)
            except Exception as e:
                warnings_list.append(f"Could not set CRS EPSG:{source_epsg} — {e}")

        # Collect stats
        layers_found = list(gdf["layer"].unique()) if "layer" in gdf.columns else []
        etypes_found = list(gdf["entity_type"].unique()) if "entity_type" in gdf.columns else []
        stats = {
            "feature_count": len(gdf),
            "layer_count": len(layers_found),
            "layers": layers_found,
            "entity_types_found": etypes_found,
            "crs_assigned": f"EPSG:{source_epsg}" if source_epsg else None,
            "warnings": warnings_list,
        }

        # Write output
        driver, ext = DRIVER_MAP[fmt]

        if fmt == "shapefile":
            out_bytes = _pack_shapefile(gdf, tmpdir)
            return out_bytes, "converted_dxf.zip", "application/zip", stats

        out_path = os.path.join(tmpdir, f"converted{ext}")
        gdf.to_file(out_path, driver=driver)
        with open(out_path, "rb") as f:
            out_bytes = f.read()

        media_type = {
            "geojson":    "application/geo+json",
            "kml":        "application/vnd.google-earth.kml+xml",
            "gpkg":       "application/geopackage+sqlite3",
            "geopackage": "application/geopackage+sqlite3",
        }.get(fmt, "application/octet-stream")

        return out_bytes, f"converted{ext}", media_type, stats

    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)
