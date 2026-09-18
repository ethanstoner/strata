//! The browser UI, compiled into the binary.
//!
//! `build.rs` builds `web/` with Vite into `$OUT_DIR/web-dist`, and
//! `include_dir!` bakes every file into the executable. Nothing is read from
//! disk at runtime, which is what makes "copy one file and run it" true.

use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use include_dir::{include_dir, Dir};

static UI: Dir<'static> = include_dir!("$OUT_DIR/web-dist");

/// How the embedded UI was produced: `built` (npm ran during this build),
/// `prebuilt` (copied from `STRATA_WEB_DIST`), or `placeholder`
/// (`STRATA_SKIP_WEB_BUILD`, no real UI).
pub const UI_KIND: &str = env!("STRATA_UI_KIND");

/// True when the binary carries the real viewer rather than a placeholder.
pub fn has_real_ui() -> bool {
    UI_KIND != "placeholder"
}

/// Every embedded file path, relative to the UI root (e.g. `index.html`,
/// `assets/index-abc123.js`).
pub fn embedded_paths() -> Vec<String> {
    fn walk(dir: &Dir<'static>, out: &mut Vec<String>) {
        for f in dir.files() {
            out.push(f.path().to_string_lossy().replace('\\', "/"));
        }
        for d in dir.dirs() {
            walk(d, out);
        }
    }
    let mut out = Vec::new();
    walk(&UI, &mut out);
    out.sort();
    out
}

/// Serves the embedded UI for every path the API routes don't claim.
pub fn with_embedded_ui(router: Router) -> Router {
    router.fallback(serve_ui)
}

async fn serve_ui(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") || path == "api" {
        return StatusCode::NOT_FOUND.into_response();
    }
    let path = if path.is_empty() { "index.html" } else { path };
    if let Some(file) = UI.get_file(path) {
        return file_response(path, file.contents());
    }
    // A path with an extension is a real asset request that missed; anything
    // else is a client-side route and gets the app shell.
    if path
        .rsplit('/')
        .next()
        .is_some_and(|last| last.contains('.'))
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    match UI.get_file("index.html") {
        Some(index) => file_response("index.html", index.contents()),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn file_response(path: &str, body: &'static [u8]) -> Response {
    // Vite fingerprints everything under assets/, so those can be cached
    // forever; index.html must be revalidated so a new binary's UI shows up.
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static(content_type(path)),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static(cache)),
        ],
        body,
    )
        .into_response()
}

fn content_type(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "ico" => "image/x-icon",
        "wasm" => "application/wasm",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
