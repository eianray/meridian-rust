use axum::{extract::Query, Json};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use crate::error::AppError;

#[derive(Deserialize)]
pub struct EpsgSearchQuery {
    pub q: String,
}

#[derive(Serialize, Deserialize, ToSchema, Clone)]
pub struct EpsgEntry {
    pub code: u32,
    pub name: String,
    pub area: String,
}

/// Search EPSG registry by code or name
#[utoipa::path(
    get,
    path = "/v1/epsg/search",
    tag = "Info",
    params(
        ("q" = String, Query, description = "Search query — EPSG code or name fragment"),
    ),
    responses(
        (status = 200, description = "Matching EPSG entries", body = Vec<EpsgEntry>),
    )
)]
pub async fn epsg_search(
    Query(params): Query<EpsgSearchQuery>,
) -> Result<Json<Vec<EpsgEntry>>, AppError> {
    static ENTRIES: std::sync::OnceLock<Vec<EpsgEntry>> = std::sync::OnceLock::new();
    let entries = ENTRIES.get_or_init(|| {
        let data = include_str!("../data/epsg.json");
        serde_json::from_str(data).expect("Invalid EPSG data")
    });

    let q = params.q.to_lowercase();
    let results: Vec<EpsgEntry> = entries
        .iter()
        .filter(|e| {
            e.name.to_lowercase().contains(&q)
                || e.area.to_lowercase().contains(&q)
                || e.code.to_string().contains(&q)
        })
        .take(50)
        .cloned()
        .collect();

    Ok(Json(results))
}
