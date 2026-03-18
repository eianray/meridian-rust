use axum::{routing::post, Router};

use crate::gis::{buffer::buffer, clip::clip, dissolve::dissolve, reproject::reproject};
use crate::routes::{
    batch::batch,
    combine::{append, merge, spatial_join},
    convert::convert,
    raster::{aspect, color_relief, contours, hillshade, mosaic, raster_calc, raster_convert, raster_to_vector, roughness, slope},
    schema::{repair, schema, validate},
    topology::{difference, intersect, union},
    transform::{
        add_field, erase, feature_to_line, feature_to_point, feature_to_polygon,
        multipart_to_singlepart,
    },
    vectorize::vectorize,
};

pub fn router() -> Router {
    Router::new()
        // Existing
        .route("/v1/reproject", post(reproject))
        .route("/v1/buffer", post(buffer))
        .route("/v1/clip", post(clip))
        .route("/v1/dissolve", post(dissolve))
        .route("/v1/batch", post(batch))
        // Schema / validation
        .route("/v1/schema", post(schema))
        .route("/v1/validate", post(validate))
        .route("/v1/repair", post(repair))
        // Format conversion
        .route("/v1/convert", post(convert))
        // Geometry transforms (single-input)
        .route("/v1/erase", post(erase))
        .route("/v1/feature-to-point", post(feature_to_point))
        .route("/v1/feature-to-line", post(feature_to_line))
        .route("/v1/feature-to-polygon", post(feature_to_polygon))
        .route("/v1/multipart-to-singlepart", post(multipart_to_singlepart))
        .route("/v1/add-field", post(add_field))
        // Topology (two-input)
        .route("/v1/union", post(union))
        .route("/v1/intersect", post(intersect))
        .route("/v1/difference", post(difference))
        // Combine (two-input)
        .route("/v1/append", post(append))
        .route("/v1/merge", post(merge))
        .route("/v1/spatial-join", post(spatial_join))
        // Vector tiles
        .route("/v1/vectorize", post(vectorize))
        // Raster / DEM
        .route("/v1/hillshade", post(hillshade))
        .route("/v1/slope", post(slope))
        .route("/v1/aspect", post(aspect))
        .route("/v1/roughness", post(roughness))
        .route("/v1/color-relief", post(color_relief))
        .route("/v1/contours", post(contours))
        .route("/v1/raster-calc", post(raster_calc))
        // Raster conversion
        .route("/v1/convert/raster", post(raster_convert))
        // Mosaic
        .route("/v1/mosaic", post(mosaic))
        // Raster-to-vector polygonization
        .route("/v1/raster-to-vector", post(raster_to_vector))
}
