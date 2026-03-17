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
