use anyhow::Result;
use reqwest::multipart;

/// Build a shared reqwest client. Call once at startup.
pub fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .build()
        .expect("failed to build reqwest client")
}

/// Single-file GIS POST — sends GeoJSON as multipart `file` field.
/// `extra_params` are additional text form fields (key, value).
pub async fn call_gis(
    client: &reqwest::Client,
    base_url: &str,
    mcp_key: &str,
    endpoint: &str,
    geojson_str: &str,
    extra_params: Vec<(String, String)>,
) -> Result<String> {
    let url = format!("{}{}", base_url, endpoint);

    let file_part = multipart::Part::text(geojson_str.to_string())
        .file_name("data.geojson")
        .mime_str("application/geo+json")?;

    let mut form = multipart::Form::new().part("file", file_part);
    for (k, v) in extra_params {
        form = form.text(k, v);
    }

    let resp = client
        .post(&url)
        .header("X-Mcp-Key", mcp_key)
        .multipart(form)
        .send()
        .await?;

    Ok(resp.text().await?)
}

/// Two-file GIS POST — sends layer_a as `file`, layer_b under `second_field`.
/// For clip: second_field = "mask". For union/intersect/etc.: second_field = "layer_b".
pub async fn call_gis_two(
    client: &reqwest::Client,
    base_url: &str,
    mcp_key: &str,
    endpoint: &str,
    layer_a: &str,
    second_field: &str,
    layer_b: &str,
    extra_params: Vec<(String, String)>,
) -> Result<String> {
    let url = format!("{}{}", base_url, endpoint);

    let part_a = multipart::Part::text(layer_a.to_string())
        .file_name("layer_a.geojson")
        .mime_str("application/geo+json")?;

    let part_b = multipart::Part::text(layer_b.to_string())
        .file_name("layer_b.geojson")
        .mime_str("application/geo+json")?;

    let mut form = multipart::Form::new()
        .part("file", part_a)
        .part(second_field.to_string(), part_b);

    for (k, v) in extra_params {
        form = form.text(k, v);
    }

    let resp = client
        .post(&url)
        .header("X-Mcp-Key", mcp_key)
        .multipart(form)
        .send()
        .await?;

    Ok(resp.text().await?)
}

/// Raster POST — sends raw bytes as multipart `file` (GeoTIFF).
pub async fn call_raster(
    client: &reqwest::Client,
    base_url: &str,
    mcp_key: &str,
    endpoint: &str,
    raster_bytes: Vec<u8>,
) -> Result<String> {
    let url = format!("{}{}", base_url, endpoint);

    let file_part = multipart::Part::bytes(raster_bytes)
        .file_name("input.tif")
        .mime_str("image/tiff")?;

    let form = multipart::Form::new().part("file", file_part);

    let resp = client
        .post(&url)
        .header("X-Mcp-Key", mcp_key)
        .multipart(form)
        .send()
        .await?;

    Ok(resp.text().await?)
}

/// Raster POST with extra text fields.
pub async fn call_raster_with_extra(
    client: &reqwest::Client,
    base_url: &str,
    mcp_key: &str,
    endpoint: &str,
    raster_bytes: Vec<u8>,
    extra_params: Vec<(String, String)>,
) -> Result<String> {
    let url = format!("{}{}", base_url, endpoint);

    let file_part = multipart::Part::bytes(raster_bytes)
        .file_name("input.tif")
        .mime_str("image/tiff")?;

    let mut form = multipart::Form::new().part("file", file_part);
    for (k, v) in extra_params {
        form = form.text(k, v);
    }

    let resp = client
        .post(&url)
        .header("X-Mcp-Key", mcp_key)
        .multipart(form)
        .send()
        .await?;

    Ok(resp.text().await?)
}

/// Raster-calc POST — sends raster under slot name (A, B, etc.) with expression.
pub async fn call_raster_with_slot(
    client: &reqwest::Client,
    base_url: &str,
    mcp_key: &str,
    endpoint: &str,
    slot: &str,
    raster_bytes: Vec<u8>,
    extra_params: Vec<(String, String)>,
) -> Result<String> {
    let url = format!("{}{}", base_url, endpoint);

    let file_part = multipart::Part::bytes(raster_bytes)
        .file_name("input.tif")
        .mime_str("image/tiff")?;

    let mut form = multipart::Form::new().part(slot.to_string(), file_part);
    for (k, v) in extra_params {
        form = form.text(k, v);
    }

    let resp = client
        .post(&url)
        .header("X-Mcp-Key", mcp_key)
        .multipart(form)
        .send()
        .await?;

    Ok(resp.text().await?)
}

/// Raster-calc POST with two rasters (A and B).
pub async fn call_raster_calc_two(
    client: &reqwest::Client,
    base_url: &str,
    mcp_key: &str,
    bytes_a: Vec<u8>,
    bytes_b: Vec<u8>,
    extra_params: Vec<(String, String)>,
) -> Result<String> {
    let url = format!("{}/v1/raster-calc", base_url);

    let part_a = multipart::Part::bytes(bytes_a)
        .file_name("a.tif")
        .mime_str("image/tiff")?;
    let part_b = multipart::Part::bytes(bytes_b)
        .file_name("b.tif")
        .mime_str("image/tiff")?;

    let mut form = multipart::Form::new()
        .part("A", part_a)
        .part("B", part_b);
    for (k, v) in extra_params {
        form = form.text(k, v);
    }

    let resp = client
        .post(&url)
        .header("X-Mcp-Key", mcp_key)
        .multipart(form)
        .send()
        .await?;

    Ok(resp.text().await?)
}

/// Elevation DGGS POST — sends AOI GeoJSON as multipart `geojson` field.
pub async fn call_elevation_dggs(
    client: &reqwest::Client,
    base_url: &str,
    mcp_key: &str,
    aoi_geojson: &str,
) -> Result<String> {
    let url = format!("{}/v1/elevation/fetch-dggs", base_url);

    let part = multipart::Part::text(aoi_geojson.to_string())
        .file_name("aoi.geojson")
        .mime_str("application/geo+json")?;

    let form = multipart::Form::new().part("geojson", part);

    let resp = client
        .post(&url)
        .header("X-Mcp-Key", mcp_key)
        .multipart(form)
        .send()
        .await?;

    Ok(resp.text().await?)
}
