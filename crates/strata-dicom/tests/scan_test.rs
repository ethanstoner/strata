mod common;

use strata_dicom::error::DicomError;
use strata_dicom::meta::SliceMeta;

#[test]
fn fixture_builder_produces_a_readable_file() {
    let dir = tempfile::tempdir().unwrap();
    let p = common::write_slice(dir.path(), &common::FixtureSlice::default());
    let obj = dicom::object::open_file(&p).expect("fixture must be valid DICOM");
    assert_eq!(
        obj.element_by_name("Rows").unwrap().to_int::<u16>().unwrap(),
        64
    );
}

#[test]
fn omitting_rescale_really_omits_the_tags() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::FixtureSlice {
        rescale: None,
        ..Default::default()
    };
    let p = common::write_slice(dir.path(), &f);
    let obj = dicom::object::open_file(&p).unwrap();
    assert!(
        obj.element_by_name("RescaleSlope").is_err(),
        "fixture must omit the tag, not write a default"
    );
}

#[test]
fn two_slices_get_distinct_paths() {
    let dir = tempfile::tempdir().unwrap();
    let a = common::write_slice(dir.path(), &common::FixtureSlice::default());
    let b = common::write_slice(dir.path(), &common::FixtureSlice::default());
    assert_ne!(a, b);
}

#[test]
fn extracts_geometry_and_identity_tags() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::FixtureSlice {
        position: [0.0, 0.0, 7.5],
        ..Default::default()
    };
    let p = common::write_slice(dir.path(), &f);

    let meta = SliceMeta::from_file(&p).expect("valid fixture must parse");

    assert_eq!(meta.position, [0.0, 0.0, 7.5]);
    assert_eq!(meta.rows, 64);
    assert_eq!(meta.cols, 64);
    assert_eq!(meta.rescale, Some((1.0, -1024.0)));
    assert_eq!(meta.series_uid, f.series_uid);
    assert_eq!(meta.study_uid, f.study_uid);
    assert_eq!(meta.modality, "CT");
    assert!(!meta.sop_uid.is_empty());
}

#[test]
fn absent_rescale_tags_yield_none_not_a_default() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::FixtureSlice {
        rescale: None,
        ..Default::default()
    };
    let p = common::write_slice(dir.path(), &f);

    let meta = SliceMeta::from_file(&p).expect("valid fixture must parse");

    assert_eq!(meta.rescale, None);
}

#[test]
fn missing_required_tag_names_the_file_in_the_error() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::FixtureSlice {
        omit_tag: Some("ImageOrientationPatient"),
        ..Default::default()
    };
    let p = common::write_slice(dir.path(), &f);

    let err = SliceMeta::from_file(&p).expect_err("must fail without orientation");
    let msg = err.to_string();

    assert!(
        msg.contains("ImageOrientationPatient"),
        "error message was: {msg}"
    );
    assert!(
        msg.contains(p.file_name().unwrap().to_str().unwrap()),
        "error message was: {msg}"
    );
}

#[test]
fn rejects_non_finite_position() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::FixtureSlice {
        position: [0.0, 0.0, f64::NAN],
        ..Default::default()
    };
    let p = common::write_slice(dir.path(), &f);

    let err = SliceMeta::from_file(&p).expect_err("non-finite position must be rejected");

    assert!(matches!(err, DicomError::NonFinitePosition { .. }));
}

#[test]
fn computes_depth_from_geometry() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::FixtureSlice {
        position: [0.0, 0.0, 7.5],
        orientation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        ..Default::default()
    };
    let p = common::write_slice(dir.path(), &f);

    let meta = SliceMeta::from_file(&p).expect("valid fixture must parse");

    assert!((meta.depth - 7.5).abs() < 1e-9);
}
