use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use strata_dicom::meta::SliceMeta;
use strata_dicom::series::SeriesManifest;
use strata_server::disk_cache::DiskCache;
use strata_server::index::Index;
use strata_server::routes::build_router;
use strata_server::volume::{self, downsample};

/// Built by hand rather than importing strata-dicom's fixture builder, which
/// lives under that crate's own tests/ and isn't importable from here.
fn make_slice(ordinal: i32, depth: f64) -> SliceMeta {
    SliceMeta {
        path: PathBuf::from(format!("/data/slice-{ordinal}.dcm")),
        patient_id: "PAT1".to_string(),
        study_uid: "STUDY1".to_string(),
        series_uid: "SERIES1".to_string(),
        sop_uid: format!("SOP{ordinal}"),
        modality: "CT".to_string(),
        rows: 4,
        cols: 4,
        position: [0.0, 0.0, depth],
        orientation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        rescale: Some((1.0, -1024.0)),
        pixel_spacing: Some((0.5, 0.5)),
        slice_thickness: Some(1.0),
        depth,
        series_description: None,
        study_description: None,
    }
}

fn make_manifest() -> SeriesManifest {
    let slices = vec![make_slice(0, 0.0), make_slice(1, 1.0)];
    SeriesManifest {
        series_uid: "SERIES1".to_string(),
        study_uid: "STUDY1".to_string(),
        patient_id: "PAT1".to_string(),
        modality: "CT".to_string(),
        rows: 4,
        cols: 4,
        series_description: None,
        study_description: None,
        uniform_spacing: true,
        spacing_mm: Some(1.0),
        hu_calibrated: true,
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

// ---------------------------------------------------------------------
// downsample() unit tests — no HTTP, no decoding, just the math.
// ---------------------------------------------------------------------

#[test]
fn downsample_halves_each_dimension() {
    // Each 2x2x2 octant of a 4x4x4 volume is filled with a distinct
    // constant value (ox + oy*10 + oz*100), so the box average of a whole
    // octant must equal that constant exactly.
    let (dim_x, dim_y, dim_z) = (4u32, 4u32, 4u32);
    let mut data = vec![0i16; (dim_x * dim_y * dim_z) as usize];
    for z in 0..dim_z {
        for y in 0..dim_y {
            for x in 0..dim_x {
                let (ox, oy, oz) = (x / 2, y / 2, z / 2);
                let val = (ox + oy * 10 + oz * 100) as i16;
                data[(z * dim_y * dim_x + y * dim_x + x) as usize] = val;
            }
        }
    }

    let (out, ox, oy, oz) = downsample(&data, dim_x, dim_y, dim_z, 2);
    assert_eq!((ox, oy, oz), (2, 2, 2));
    assert_eq!(out.len(), 8);

    for nz in 0..2u32 {
        for ny in 0..2u32 {
            for nx in 0..2u32 {
                let expected = (nx + ny * 10 + nz * 100) as i16;
                let actual = out[(nz * 4 + ny * 2 + nx) as usize];
                assert_eq!(
                    actual, expected,
                    "mismatch at ({nx},{ny},{nz}): got {actual}, want {expected}"
                );
            }
        }
    }
}

#[test]
fn downsample_handles_odd_dimensions() {
    // Value varies only along x, so every output voxel's average depends
    // only on which x-block it falls in — this isolates the partial edge
    // block (x=4 alone) from noise introduced by partial y/z blocks.
    let (dim_x, dim_y, dim_z) = (5u32, 5u32, 5u32);
    let mut data = vec![0i16; (dim_x * dim_y * dim_z) as usize];
    for z in 0..dim_z {
        for y in 0..dim_y {
            for x in 0..dim_x {
                data[(z * dim_y * dim_x + y * dim_x + x) as usize] = x as i16;
            }
        }
    }

    let (out, ox, oy, oz) = downsample(&data, dim_x, dim_y, dim_z, 2);
    // ceil(5/2) = 3: the partial edge block still produces a voxel.
    assert_eq!((ox, oy, oz), (3, 3, 3));
    assert_eq!(out.len(), 27);

    // Block averages along x: avg(0,1)=0.5 -> 1, avg(2,3)=2.5 -> 3,
    // and the partial edge block {4} -> 4 (averaged over its own single
    // voxel, not diluted by a phantom neighbour and not dropped).
    let expected_x = [1i16, 3, 4];
    for nz in 0..3u32 {
        for ny in 0..3u32 {
            for nx in 0..3u32 {
                let actual = out[(nz * 9 + ny * 3 + nx) as usize];
                assert_eq!(
                    actual, expected_x[nx as usize],
                    "mismatch at ({nx},{ny},{nz})"
                );
            }
        }
    }
}

#[test]
fn averaging_does_not_overflow_at_extremes() {
    let data = vec![i16::MAX; 8];
    let (out, ox, oy, oz) = downsample(&data, 2, 2, 2, 2);
    assert_eq!((ox, oy, oz), (1, 1, 1));
    assert_eq!(out, vec![i16::MAX]);
}

// ---------------------------------------------------------------------
// HTTP-level tests
// ---------------------------------------------------------------------

#[tokio::test]
async fn unknown_series_returns_404() {
    let app = app_with(&[]);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/series/does-not-exist/volume")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn level_above_3_returns_400() {
    let app = app_with(&[make_manifest()]);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/series/SERIES1/volume?level=4")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn non_integer_level_returns_400() {
    let app = app_with(&[make_manifest()]);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/series/SERIES1/volume?level=abc")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------
// Real-data risk gate. Requires data/sample/. Run with:
//   cargo test -p strata-server --test volume_test -- --ignored --nocapture
// ---------------------------------------------------------------------

fn sample_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/sample")
}

fn header_f64(headers: &axum::http::HeaderMap, name: &str) -> f64 {
    headers
        .get(name)
        .unwrap_or_else(|| panic!("missing header {name}"))
        .to_str()
        .unwrap()
        .parse()
        .unwrap()
}

fn header_u32(headers: &axum::http::HeaderMap, name: &str) -> u32 {
    headers
        .get(name)
        .unwrap_or_else(|| panic!("missing header {name}"))
        .to_str()
        .unwrap()
        .parse()
        .unwrap()
}

#[tokio::test]
#[ignore]
async fn real_volume_has_expected_shape() {
    let dir = sample_dir();
    assert!(
        dir.exists(),
        "data/sample missing; fetch a TCIA series first"
    );

    let scan = strata_dicom::scan::scan_directory(&dir).expect("scan must succeed");
    let series = scan
        .series
        .into_iter()
        .find(|s| s.is_volume)
        .expect("expected at least one multi-slice series");
    let uid = series.series_uid.clone();

    let index = Index::open_in_memory().unwrap();
    index.insert_series(&series).unwrap();
    let app = build_router(Arc::new(Mutex::new(index)));

    // Level 0: 512x512x60, spacing 0.59375 / 0.59375 / 5.0.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/series/{uid}/volume?level=0"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(header_u32(&headers, "x-strata-dim-x"), 512);
    assert_eq!(header_u32(&headers, "x-strata-dim-y"), 512);
    assert_eq!(header_u32(&headers, "x-strata-dim-z"), 60);
    assert!((header_f64(&headers, "x-strata-spacing-x") - 0.59375).abs() < 1e-6);
    assert!((header_f64(&headers, "x-strata-spacing-y") - 0.59375).abs() < 1e-6);
    assert!((header_f64(&headers, "x-strata-spacing-z") - 5.0).abs() < 1e-6);
    assert_eq!(header_u32(&headers, "x-strata-level"), 0);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.len(), 512 * 512 * 60 * 2);

    // Level 1: 256x256x30, spacing doubled.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/series/{uid}/volume?level=1"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(header_u32(&headers, "x-strata-dim-x"), 256);
    assert_eq!(header_u32(&headers, "x-strata-dim-y"), 256);
    assert_eq!(header_u32(&headers, "x-strata-dim-z"), 30);
    assert!((header_f64(&headers, "x-strata-spacing-x") - 1.1875).abs() < 1e-6);
    assert!((header_f64(&headers, "x-strata-spacing-y") - 1.1875).abs() < 1e-6);
    assert!((header_f64(&headers, "x-strata-spacing-z") - 10.0).abs() < 1e-6);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.len(), 256 * 256 * 30 * 2);
}

fn big_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/big")
}

/// Proves the parallel decode in `assemble_level0` produces byte-identical
/// output to a sequential reference decode, in order. The reference here is
/// deliberately independent of the server's own assembly path: it decodes
/// each slice one at a time via `decode_slice`, in the same ordinal order
/// `SeriesManifest::slices` is already sorted in, and concatenates by hand.
#[tokio::test]
#[ignore]
async fn parallel_decode_matches_sequential() {
    let dir = sample_dir();
    assert!(
        dir.exists(),
        "data/sample missing; fetch a TCIA series first"
    );

    let scan = strata_dicom::scan::scan_directory(&dir).expect("scan must succeed");
    let series = scan
        .series
        .into_iter()
        .find(|s| s.is_volume)
        .expect("expected at least one multi-slice series");
    let uid = series.series_uid.clone();

    let mut sequential = Vec::new();
    for slice in &series.slices {
        let decoded = strata_server::pixels::decode_slice(&slice.path).expect("slice must decode");
        sequential.extend_from_slice(&decoded.data);
    }

    let index = Index::open_in_memory().unwrap();
    index.insert_series(&series).unwrap();
    let app = build_router(Arc::new(Mutex::new(index)));

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/series/{uid}/volume?level=0"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let parallel: Vec<i16> = body
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b))
        .collect();

    assert_eq!(parallel.len(), sequential.len());
    assert_eq!(
        parallel, sequential,
        "parallel decode must match sequential decode exactly, in ordinal order"
    );
}

/// Quantifies whether level 2 could be built cheaply by downsampling level 1
/// by 2 (8x less box-average work than downsampling level 0 by 4 directly)
/// instead of always rebuilding from level 0. Two rounds of box-averaging
/// are not mathematically identical to one wider box average, so this
/// measures the real gap on `data/big` (1026 slices) rather than assuming
/// it's negligible. Not a pass/fail gate on its own — the number it prints
/// is the evidence backing the decision documented next to `volume::fetch`.
#[test]
#[ignore]
fn level2_cascade_error_vs_direct_from_level0() {
    let dir = big_dir();
    assert!(
        dir.exists(),
        "data/big missing; fetch the 1026-slice study first"
    );

    let scan = strata_dicom::scan::scan_directory(&dir).expect("scan must succeed");
    let series = scan
        .series
        .into_iter()
        .find(|s| s.is_volume)
        .expect("expected at least one multi-slice series");
    let uid = series.series_uid.clone();

    let index = Index::open_in_memory().unwrap();
    index.insert_series(&series).unwrap();
    let shared: strata_server::routes::SharedIndex = Arc::new(Mutex::new(index));
    let mem_cache = volume::VolumeCache::new();
    let cache_dir = tempfile::tempdir().unwrap();
    let disk_cache = DiskCache::new(
        cache_dir.path().to_path_buf(),
        strata_server::disk_cache::DEFAULT_MAX_CACHE_BYTES,
    )
    .unwrap();

    let (level0, _) = volume::fetch(&shared, &mem_cache, &disk_cache, &uid, 0)
        .unwrap()
        .expect("series must be found");
    let (level1, _) = volume::fetch(&shared, &mem_cache, &disk_cache, &uid, 1)
        .unwrap()
        .expect("series must be found");

    // Direct: one factor-4 box average straight from level 0.
    let (direct, dx, dy, dz) =
        downsample(&level0.data, level0.dim_x, level0.dim_y, level0.dim_z, 4);
    // Cascaded: a factor-2 box average of level 1, which is itself a
    // factor-2 box average of level 0.
    let (cascaded, cx, cy, cz) =
        downsample(&level1.data, level1.dim_x, level1.dim_y, level1.dim_z, 2);

    assert_eq!(
        (dx, dy, dz),
        (cx, cy, cz),
        "both paths must agree on output dims"
    );

    let max_abs_diff = direct
        .iter()
        .zip(cascaded.iter())
        .map(|(a, b)| (*a as i32 - *b as i32).abs())
        .max()
        .unwrap();
    let mean_abs_diff: f64 = direct
        .iter()
        .zip(cascaded.iter())
        .map(|(a, b)| (*a as i32 - *b as i32).abs() as f64)
        .sum::<f64>()
        / direct.len() as f64;

    println!(
        "level2 cascade-vs-direct on data/big: max_abs_diff={max_abs_diff} HU, mean_abs_diff={mean_abs_diff:.4} HU, n={}",
        direct.len()
    );
}
