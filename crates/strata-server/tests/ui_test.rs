//! The web UI is compiled into the binary; these check it is served from
//! memory with the right types, and never shadows the API.

use std::sync::{Arc, Mutex};

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use tower::ServiceExt;

use strata_server::index::Index;
use strata_server::routes::build_router;
use strata_server::ui;

fn app() -> axum::Router {
    let index = Arc::new(Mutex::new(Index::open_in_memory().unwrap()));
    ui::with_embedded_ui(build_router(index))
}

async fn get(path: &str) -> (StatusCode, Option<String>, Vec<u8>) {
    let res = app()
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let ct = res
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_string());
    let body = to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, ct, body)
}

#[tokio::test]
async fn root_serves_embedded_index_html() {
    let (status, ct, body) = get("/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.unwrap().starts_with("text/html"));
    assert!(String::from_utf8_lossy(&body).contains("<html") || !ui::has_real_ui());
}

#[tokio::test]
async fn every_embedded_asset_is_served_with_a_real_content_type() {
    let paths = ui::embedded_paths();
    assert!(paths.iter().any(|p| p == "index.html"), "{paths:?}");
    if ui::has_real_ui() {
        assert!(
            paths
                .iter()
                .any(|p| p.starts_with("assets/") && p.ends_with(".js")),
            "{paths:?}"
        );
    }
    for p in paths {
        let (status, ct, body) = get(&format!("/{p}")).await;
        assert_eq!(status, StatusCode::OK, "{p}");
        assert!(!body.is_empty(), "{p}");
        if p.ends_with(".js") {
            assert!(ct.unwrap().starts_with("text/javascript"), "{p}");
        }
    }
}

#[tokio::test]
async fn api_routes_win_and_unknown_api_paths_404_instead_of_html() {
    let (status, ct, _) = get("/api/health").await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.unwrap().starts_with("application/json"));
    let (status, _, _) = get("/api/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = get("/assets/missing.js").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
