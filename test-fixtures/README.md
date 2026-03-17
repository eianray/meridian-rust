# Test Fixtures

All fixtures use San Francisco area features.

## GeoJSON Files

| File | Type | CRS | Description |
|------|------|-----|-------------|
| `point_wgs84.geojson` | Points | WGS84 | 5 SF landmarks |
| `parcels_wgs84.geojson` | Polygons | WGS84 | 8 city parcels (residential/commercial/industrial/park) |
| `lines_wgs84.geojson` | LineStrings | WGS84 | 4 SF streets |
| `clip_mask.geojson` | Polygon | WGS84 | Study area mask for clip testing |
| `utm10n_points.geojson` | Points | EPSG:32610 (UTM 10N) | 3 Bay Area landmarks in projected coords |

## Shapefiles

| File | Type | CRS | Description |
|------|------|-----|-------------|
| `parcels_wgs84.shp` | Polygons | WGS84 | Same 8 parcels as GeoJSON |
| `roads_wgs84.shp` | LineStrings | WGS84 | Same 4 streets as GeoJSON |

## Suggested Test Calls

```bash
BASE=http://localhost:8100

# Health check
curl $BASE/v1/health

# Reproject: WGS84 → Web Mercator
curl -X POST $BASE/v1/reproject \
  -F "file=@point_wgs84.geojson" \
  -F "target_crs=EPSG:3857"

# Reproject: UTM 10N → WGS84 (source_crs required)
curl -X POST $BASE/v1/reproject \
  -F "file=@utm10n_points.geojson" \
  -F "source_crs=EPSG:32610" \
  -F "target_crs=EPSG:4326"

# Buffer: 500m around SF landmarks
curl -X POST $BASE/v1/buffer \
  -F "file=@point_wgs84.geojson" \
  -F "distance=500"

# Buffer: 100m around parcels
curl -X POST $BASE/v1/buffer \
  -F "file=@parcels_wgs84.geojson" \
  -F "distance=100"

# Clip: parcels within study area
curl -X POST $BASE/v1/clip \
  -F "file=@parcels_wgs84.geojson" \
  -F "mask=@clip_mask.geojson"

# Dissolve: all parcels into one feature
curl -X POST $BASE/v1/dissolve \
  -F "file=@parcels_wgs84.geojson"

# Dissolve: by zone
curl -X POST $BASE/v1/dissolve \
  -F "file=@parcels_wgs84.geojson" \
  -F "field=zone"

# Metrics
curl $BASE/metrics

# OpenAPI docs (browser)
open $BASE/docs
```
