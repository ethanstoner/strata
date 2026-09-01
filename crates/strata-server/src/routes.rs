use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use tower_http::services::ServeDir;

use crate::index::Index;

/// Shared across handlers; `Mutex` because `rusqlite::Connection` isn't `Sync`.
pub type SharedIndex = Arc<Mutex<Index>>;

/// Builds the API router. Task 9's slice pixel-data endpoint should be added
/// here as another `.route(...)` alongside `/api/series/:uid`, e.g.
/// `/api/series/:uid/slices/:ordinal` — it can resolve a file with
/// `Index::slice_path` on the same `SharedIndex` state.
pub fn build_router(index: SharedIndex) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/series", get(list_series))
        .route("/api/series/:uid", get(get_series))
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
