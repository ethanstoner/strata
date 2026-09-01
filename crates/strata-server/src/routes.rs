use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use tower_http::services::ServeDir;

use crate::index::Index;
use crate::pixels::decode_slice;

/// Shared across handlers; `Mutex` because `rusqlite::Connection` isn't `Sync`.
pub type SharedIndex = Arc<Mutex<Index>>;

/// Builds the API router.
pub fn build_router(index: SharedIndex) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/series", get(list_series))
        .route("/api/series/:uid", get(get_series))
        .route("/api/series/:uid/slices/:ordinal", get(get_slice))
        .with_state(index)
}

/// Adds static file serving from `dist_dir` at `/`, with the API routes
/// taking precedence via fallback.
pub fn with_static_files(router: Router, dist_dir: &Path) -> Router {
    router.fallback_service(ServeDir::new(dist_dir.to_path_buf()))
}

/// Any index/database failure becomes a 500; unknown-resource cases are
/// handled explicitly by each handler instead so they can return 404.
struct AppError(anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, self.0.to_string()).into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(err: E) -> Self {
        AppError(err.into())
    }
}

async fn health(State(index): State<SharedIndex>) -> Result<Json<serde_json::Value>, AppError> {
    let count = index.lock().unwrap().series_count()?;
    Ok(Json(serde_json::json!({ "status": "ok", "series": count })))
}

async fn list_series(
    State(index): State<SharedIndex>,
) -> Result<Json<Vec<crate::index::SeriesSummary>>, AppError> {
    let list = index.lock().unwrap().list_series()?;
    Ok(Json(list))
}

async fn get_series(
    State(index): State<SharedIndex>,
    AxumPath(uid): AxumPath<String>,
) -> Result<Response, AppError> {
    let detail = index.lock().unwrap().get_series(&uid)?;
    Ok(match detail {
        Some(d) => Json(d).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    })
}

async fn get_slice(
    State(index): State<SharedIndex>,
    AxumPath((uid, ordinal)): AxumPath<(String, u32)>,
) -> Result<Response, AppError> {
    let path = index.lock().unwrap().slice_path(&uid, ordinal)?;
    let Some(path) = path else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };

    let slice = decode_slice(&path)?;

    let mut bytes = Vec::with_capacity(slice.data.len() * 2);
    for v in &slice.data {
        bytes.extend_from_slice(&v.to_le_bytes());
    }

    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type",
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        "x-strata-rows",
        HeaderValue::from_str(&slice.rows.to_string()).unwrap(),
    );
    headers.insert(
        "x-strata-cols",
        HeaderValue::from_str(&slice.cols.to_string()).unwrap(),
    );
    headers.insert(
        "x-strata-hu-calibrated",
        HeaderValue::from_static(if slice.hu_calibrated { "true" } else { "false" }),
    );

    Ok((StatusCode::OK, headers, bytes).into_response())
}
