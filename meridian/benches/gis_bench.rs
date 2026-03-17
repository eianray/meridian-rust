/// Criterion benchmarks for GIS operations.
/// Measures p50/p95/p99 latency for reproject, buffer, and dissolve
/// using a small 10-feature GeoJSON fixture in dev_mode (no payment overhead).
///
/// Run with: cargo bench --bench gis_bench
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use meridian::gis::{
    buffer::do_buffer_blocking,
    dissolve::do_dissolve_blocking,
    reproject::do_reproject_blocking,
};

// ── Fixture ───────────────────────────────────────────────────────────────────

/// 10-feature GeoJSON FeatureCollection with points near the equator.
fn fixture_10_points() -> String {
    let features: Vec<serde_json::Value> = (0..10)
        .map(|i| {
            serde_json::json!({
                "type": "Feature",
                "properties": { "id": i, "category": if i % 2 == 0 { "even" } else { "odd" } },
                "geometry": {
                    "type": "Point",
                    "coordinates": [i as f64 * 0.5, i as f64 * 0.3]
                }
            })
        })
        .collect();

    serde_json::json!({
        "type": "FeatureCollection",
        "features": features
    })
    .to_string()
}

/// 10-feature polygon GeoJSON for dissolve benchmarks.
fn fixture_10_polygons() -> String {
    let features: Vec<serde_json::Value> = (0..10)
        .map(|i| {
            let x = i as f64 * 0.5;
            let y = 0.0;
            serde_json::json!({
                "type": "Feature",
                "properties": { "id": i, "group": if i < 5 { "a" } else { "b" } },
                "geometry": {
                    "type": "Polygon",
                    "coordinates": [[
                        [x, y],
                        [x + 0.4, y],
                        [x + 0.4, y + 0.4],
                        [x, y + 0.4],
                        [x, y]
                    ]]
                }
            })
        })
        .collect();

    serde_json::json!({
        "type": "FeatureCollection",
        "features": features
    })
    .to_string()
}

// ── Benchmarks ─────────────────────────────────────────────────────────────────

fn bench_reproject(c: &mut Criterion) {
    let geojson = fixture_10_points();
    let mut group = c.benchmark_group("reproject");
    group.sample_size(50);

    group.bench_with_input(
        BenchmarkId::new("wgs84_to_web_mercator", "10_points"),
        &geojson,
        |b, input| {
            b.iter(|| {
                do_reproject_blocking(
                    input.clone(),
                    "EPSG:4326".to_string(),
                    "EPSG:3857".to_string(),
                )
                .expect("reproject failed")
            });
        },
    );

    group.bench_with_input(
        BenchmarkId::new("wgs84_to_utm10n", "10_points"),
        &geojson,
        |b, input| {
            b.iter(|| {
                do_reproject_blocking(
                    input.clone(),
                    "EPSG:4326".to_string(),
                    "EPSG:32610".to_string(),
                )
                .expect("reproject failed")
            });
        },
    );

    group.finish();
}

fn bench_buffer(c: &mut Criterion) {
    let geojson = fixture_10_points();
    let mut group = c.benchmark_group("buffer");
    group.sample_size(50);

    for distance in [100.0_f64, 1000.0, 10000.0] {
        group.bench_with_input(
            BenchmarkId::new("meters", distance as u64),
            &(geojson.clone(), distance),
            |b, (input, dist)| {
                b.iter(|| {
                    do_buffer_blocking(input.clone(), *dist, "EPSG:4326".to_string())
                        .expect("buffer failed")
                });
            },
        );
    }

    group.finish();
}

fn bench_dissolve(c: &mut Criterion) {
    let geojson = fixture_10_polygons();
    let mut group = c.benchmark_group("dissolve");
    group.sample_size(50);

    group.bench_with_input(
        BenchmarkId::new("dissolve_all", "10_polygons"),
        &geojson,
        |b, input| {
            b.iter(|| {
                do_dissolve_blocking(input.clone(), None, "EPSG:4326".to_string())
                    .expect("dissolve failed")
            });
        },
    );

    group.bench_with_input(
        BenchmarkId::new("dissolve_by_group", "10_polygons"),
        &geojson,
        |b, input| {
            b.iter(|| {
                do_dissolve_blocking(
                    input.clone(),
                    Some("group".to_string()),
                    "EPSG:4326".to_string(),
                )
                .expect("dissolve by group failed")
            });
        },
    );

    group.finish();
}

criterion_group!(gis_benchmarks, bench_reproject, bench_buffer, bench_dissolve);
criterion_main!(gis_benchmarks);
