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
        series_description: None,
        study_description: None,
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
        series_description: None,
        study_description: None,
        uniform_spacing: true,
        spacing_mm: Some(5.0),
        hu_calibrated,
        is_volume: true,
        warnings: Vec::new(),
        slices,
    }
}

/// Same fixture with descriptions overridden, for the description-specific
/// tests below — avoids duplicating the whole slice/manifest construction
/// just to vary two optional fields.
fn make_manifest_with_descriptions(
    series_description: Option<&str>,
    study_description: Option<&str>,
) -> SeriesManifest {
    let mut m = make_manifest(true);
    m.series_description = series_description.map(str::to_string);
    m.study_description = study_description.map(str::to_string);
    m
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
async fn series_description_round_trips_through_the_index_to_detail_json() {
    let app = app_with(&[make_manifest_with_descriptions(
        Some("Chest Routine #1"),
        Some("CT Chest"),
    )]);

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
    assert_eq!(json["series_description"], "Chest Routine #1");
    assert_eq!(json["study_description"], "CT Chest");
}

#[tokio::test]
async fn missing_description_serialises_as_json_null_not_empty_string() {
    // No scanner-provided description at all — must be `null`, never `""`
    // and never a fabricated placeholder like the series UID.
    let app = app_with(&[make_manifest_with_descriptions(None, None)]);

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
    assert!(json["series_description"].is_null());
    assert!(json["study_description"].is_null());
    assert_ne!(json["series_description"], serde_json::json!(""));
    assert_ne!(json["study_description"], serde_json::json!(""));
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
