use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use strata_dicom::meta::SliceMeta;
use strata_dicom::series::SeriesManifest;
use strata_server::index::Index;
use strata_server::routes::build_router;

/// Built by hand rather than importing strata-dicom's fixture builder, which
/// lives under that crate's own tests/ and isn't importable from here.
fn make_slice(ordinal: i32, depth: f64, hu_calibrated: bool) -> SliceMeta {
    SliceMeta {
        path: PathBuf::from(format!("/data/slice-{ordinal}.dcm")),
        patient_id: "PAT1".to_string(),
        study_uid: "STUDY1".to_string(),
        series_uid: "SERIES1".to_string(),
        sop_uid: format!("SOP{ordinal}"),
        modality: "CT".to_string(),
        rows: 512,
        cols: 512,
        position: [0.0, 0.0, depth],
        orientation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        rescale: if hu_calibrated {
            Some((1.0, -1024.0))
        } else {
            None
        },
        pixel_spacing: Some((0.7, 0.7)),
        slice_thickness: Some(5.0),
        depth,
    }
}

/// `SeriesManifest::from_slices` is `pub(crate)` to strata-dicom, so this
/// builds the manifest directly (every field is public) with slices already
/// in depth order.
fn make_manifest(hu_calibrated: bool) -> SeriesManifest {
    let slices = vec![
        make_slice(0, -344.0, hu_calibrated),
        make_slice(1, -339.0, hu_calibrated),
        make_slice(2, -334.0, hu_calibrated),
    ];
    SeriesManifest {
        series_uid: "SERIES1".to_string(),
        study_uid: "STUDY1".to_string(),
        patient_id: "PAT1".to_string(),
        modality: "CT".to_string(),
        rows: 512,
        cols: 512,
        uniform_spacing: true,
        spacing_mm: Some(5.0),
        hu_calibrated,
        is_volume: true,
        warnings: Vec::new(),
        slices,
    }
}

fn app_with(manifests: &[SeriesManifest]) -> axum::Router {
    let index = Index::open_in_memory().unwrap();
    for m in manifests {
        index.insert_series(m).unwrap();
    }
    build_router(Arc::new(Mutex::new(index)))
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn lists_series_as_json() {
    let app = app_with(&[make_manifest(true)]);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/series")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let arr = json.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["series_uid"], "SERIES1");
    assert_eq!(arr[0]["slice_count"], 3);
}

#[tokio::test]
async fn unknown_series_returns_404_not_500() {
    let app = app_with(&[]);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/series/does-not-exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_series_slice_returns_404_not_500() {
    let app = app_with(&[]);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/series/does-not-exist/slices/0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_ordinal_returns_404_not_500() {
    let app = app_with(&[make_manifest(true)]);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/series/SERIES1/slices/999")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn health_reports_series_count() {
    let app = app_with(&[make_manifest(true)]);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["status"], "ok");
    assert_eq!(json["series"], 1);
}

#[tokio::test]
async fn hu_calibrated_false_is_preserved_through_the_api() {
    let app = app_with(&[make_manifest(false)]);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/series/SERIES1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["hu_calibrated"], false);

    // Also check the list endpoint, since it serialises a different struct.
    let response = app_with(&[make_manifest(false)])
        .oneshot(
            Request::builder()
                .uri("/api/series")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let json = body_json(response).await;
    assert_eq!(json[0]["hu_calibrated"], false);
}

#[tokio::test]
async fn series_detail_includes_depths_in_ordinal_order() {
    let app = app_with(&[make_manifest(true)]);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/series/SERIES1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["depths"], serde_json::json!([-344.0, -339.0, -334.0]));
    assert_eq!(json["spacing_mm"], 5.0);
    assert_eq!(json["pixel_spacing"], serde_json::json!([0.7, 0.7]));
    assert_eq!(json["slice_thickness"], 5.0);
}
