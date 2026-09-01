use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::{FromRef, Path as AxumPath, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use tower_http::services::ServeDir;

use crate::disk_cache::DiskCache;
use crate::index::Index;
use crate::pixels::decode_slice;
use crate::volume::{self, VolumeCache};

/// Shared across handlers; `Mutex` because `rusqlite::Connection` isn't `Sync`.
pub type SharedIndex = Arc<Mutex<Index>>;

/// Router state. Handlers extract the piece they need (`State<SharedIndex>`,
/// `State<Arc<VolumeCache>>`, `State<Arc<DiskCache>>`) via the `FromRef`
/// impls below rather than this struct directly, so existing handlers
/// didn't need to change.
#[derive(Clone)]
struct AppState {
    index: SharedIndex,
    volume_cache: Arc<VolumeCache>,
    disk_cache: Arc<DiskCache>,
}

impl FromRef<AppState> for SharedIndex {
    fn from_ref(state: &AppState) -> Self {
        state.index.clone()
    }
}

impl FromRef<AppState> for Arc<VolumeCache> {
    fn from_ref(state: &AppState) -> Self {
        state.volume_cache.clone()
    }
}

impl FromRef<AppState> for Arc<DiskCache> {
    fn from_ref(state: &AppState) -> Self {
        state.disk_cache.clone()
    }
}

static ANON_CACHE_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Builds the API router with a disk-backed volume cache under `cache_dir`.
/// This is what `main.rs` uses in production, with `cache_dir` derived from
/// `--cache-dir` or defaulting next to the SQLite index.
pub fn build_router_with_cache_dir(index: SharedIndex, cache_dir: PathBuf) -> Router {
    let disk_cache =
        Arc::new(DiskCache::new(cache_dir).expect("failed to create volume disk cache directory"));
    let state = AppState {
        index,
        volume_cache: Arc::new(VolumeCache::new()),
        disk_cache,
    };
    Router::new()
        .route("/api/health", get(health))
        .route("/api/series", get(list_series))
        .route("/api/series/:uid", get(get_series))
        .route("/api/series/:uid/slices/:ordinal", get(get_slice))
        .route("/api/series/:uid/volume", get(get_volume))
        .with_state(state)
}

/// Builds the API router with a private, process-unique temp directory as
/// its disk cache. Existing callers (tests, mainly) that don't care about
/// cache placement keep working unchanged; each call gets its own
/// directory so parallel test runs can't collide on the same cache files.
pub fn build_router(index: SharedIndex) -> Router {
    let n = ANON_CACHE_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("strata-cache-{}-{n}", std::process::id()));
    build_router_with_cache_dir(index, dir)
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

#[derive(Debug, Deserialize)]
struct VolumeQuery {
    level: Option<u32>,
}

async fn get_volume(
    State(index): State<SharedIndex>,
    State(cache): State<Arc<VolumeCache>>,
    State(disk_cache): State<Arc<DiskCache>>,
    AxumPath(uid): AxumPath<String>,
    Query(query): Query<VolumeQuery>,
) -> Result<Response, AppError> {
    let level = query.level.unwrap_or(0);
    if level > volume::MAX_LEVEL {
        return Ok((
            StatusCode::BAD_REQUEST,
            format!("level must be an integer 0-{}", volume::MAX_LEVEL),
        )
            .into_response());
    }

    let detail = index.lock().unwrap().get_series(&uid)?;
    let Some(detail) = detail else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };

    // Reject before touching disk: an absurd level on a large series
    // shouldn't cost a single decode.
    let factor = 1u32 << level;
    let (out_x, out_y, out_z) =
        volume::output_dims(detail.cols as u32, detail.rows as u32, detail.slice_count, factor);
    if volume::output_bytes(out_x, out_y, out_z) > volume::MAX_OUTPUT_BYTES {
        return Ok((
            StatusCode::BAD_REQUEST,
            "requested volume level would exceed the 512MB response limit",
        )
            .into_response());
    }

    let Some((vol, source)) = volume::fetch(&index, &cache, &disk_cache, &uid, level)? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    println!("volume {source} for series {uid} level {level}");

    let mut bytes = Vec::with_capacity(vol.data.len() * 2);
    for v in &vol.data {
        bytes.extend_from_slice(&v.to_le_bytes());
    }

    let (hu_min, hu_max) = vol.hu_min_max();

    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type",
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        "x-strata-dim-x",
        HeaderValue::from_str(&vol.dim_x.to_string()).unwrap(),
    );
    headers.insert(
        "x-strata-dim-y",
        HeaderValue::from_str(&vol.dim_y.to_string()).unwrap(),
    );
    headers.insert(
        "x-strata-dim-z",
        HeaderValue::from_str(&vol.dim_z.to_string()).unwrap(),
    );
    headers.insert(
        "x-strata-spacing-x",
        HeaderValue::from_str(&vol.spacing_x.to_string()).unwrap(),
    );
    headers.insert(
        "x-strata-spacing-y",
        HeaderValue::from_str(&vol.spacing_y.to_string()).unwrap(),
    );
    headers.insert(
        "x-strata-spacing-z",
        HeaderValue::from_str(&vol.spacing_z.to_string()).unwrap(),
    );
    headers.insert(
        "x-strata-hu-calibrated",
        HeaderValue::from_static(if vol.hu_calibrated { "true" } else { "false" }),
    );
    headers.insert(
        "x-strata-level",
        HeaderValue::from_str(&level.to_string()).unwrap(),
    );
    headers.insert(
        "x-strata-hu-min",
        HeaderValue::from_str(&hu_min.to_string()).unwrap(),
    );
    headers.insert(
        "x-strata-hu-max",
        HeaderValue::from_str(&hu_max.to_string()).unwrap(),
    );

    Ok((StatusCode::OK, headers, bytes).into_response())
}
