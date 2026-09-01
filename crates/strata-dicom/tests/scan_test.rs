mod common;

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
