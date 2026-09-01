mod common;

use strata_dicom::error::DicomError;
use strata_dicom::meta::SliceMeta;
use strata_dicom::scan::scan_directory;

#[test]
fn fixture_builder_produces_a_readable_file() {
    let dir = tempfile::tempdir().unwrap();
    let p = common::write_slice(dir.path(), &common::FixtureSlice::default());
    let obj = dicom::object::open_file(&p).expect("fixture must be valid DICOM");
    assert_eq!(
        obj.element_by_name("Rows")
            .unwrap()
            .to_int::<u16>()
            .unwrap(),
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

#[test]
fn orders_by_geometry_when_instance_numbers_are_shuffled() {
    let dir = tempfile::tempdir().unwrap();

    // Both filename order and write order run opposite to depth, so no
    // directory-enumeration order can produce the expected result by accident.
    // Only a geometric sort can.
    for (name, z, instance_number) in [("a.dcm", 10.0, 2), ("b.dcm", 5.0, 1), ("c.dcm", 0.0, 3)] {
        common::write_slice_as(
            dir.path(),
            &common::FixtureSlice {
                position: [0.0, 0.0, z],
                instance_number,
                ..Default::default()
            },
            name,
        );
    }

    let result = scan_directory(dir.path()).expect("scan must succeed");
    assert_eq!(result.series.len(), 1);

    let depths: Vec<f64> = result.series[0].slices.iter().map(|s| s.depth).collect();
    assert_eq!(
        depths,
        vec![0.0, 5.0, 10.0],
        "slices must be ordered by geometric depth, not InstanceNumber"
    );
}

#[test]
fn separates_two_interleaved_series_in_one_directory() {
    let dir = tempfile::tempdir().unwrap();
    let series_a = "1.2.826.0.1.3680043.8.498.1111";
    let series_b = "1.2.826.0.1.3680043.8.498.2222";

    for i in 0..3 {
        let z = i as f64 * 5.0;
        common::write_slice(
            dir.path(),
            &common::FixtureSlice {
                series_uid: series_a.to_string(),
                position: [0.0, 0.0, z],
                ..Default::default()
            },
        );
        common::write_slice(
            dir.path(),
            &common::FixtureSlice {
                series_uid: series_b.to_string(),
                position: [0.0, 0.0, z],
                ..Default::default()
            },
        );
    }

    let result = scan_directory(dir.path()).expect("scan must succeed");
    assert_eq!(
        result.series.len(),
        2,
        "expected two distinct series manifests"
    );
    assert!(result.series.iter().all(|m| m.slices.len() == 3));
}

#[test]
fn flags_non_uniform_slice_spacing() {
    let dir = tempfile::tempdir().unwrap();
    for z in [0.0, 5.0, 40.0] {
        common::write_slice(
            dir.path(),
            &common::FixtureSlice {
                position: [0.0, 0.0, z],
                ..Default::default()
            },
        );
    }

    let result = scan_directory(dir.path()).expect("scan must succeed");
    assert_eq!(result.series.len(), 1);
    assert!(!result.series[0].uniform_spacing);
}

#[test]
fn uniform_spacing_is_true_for_even_slices() {
    let dir = tempfile::tempdir().unwrap();
    for z in [0.0, 5.0, 10.0] {
        common::write_slice(
            dir.path(),
            &common::FixtureSlice {
                position: [0.0, 0.0, z],
                ..Default::default()
            },
        );
    }

    let result = scan_directory(dir.path()).expect("scan must succeed");
    assert_eq!(result.series.len(), 1);
    assert!(result.series[0].uniform_spacing);
    let spacing = result.series[0]
        .spacing_mm
        .expect("multi-slice series must report a spacing");
    assert!((spacing - 5.0).abs() < 1e-9);
}

#[test]
fn series_is_uncalibrated_if_any_slice_lacks_rescale() {
    let dir = tempfile::tempdir().unwrap();
    common::write_slice(
        dir.path(),
        &common::FixtureSlice {
            position: [0.0, 0.0, 0.0],
            rescale: Some((1.0, -1024.0)),
            ..Default::default()
        },
    );
    common::write_slice(
        dir.path(),
        &common::FixtureSlice {
            position: [0.0, 0.0, 5.0],
            rescale: Some((1.0, -1024.0)),
            ..Default::default()
        },
    );
    common::write_slice(
        dir.path(),
        &common::FixtureSlice {
            position: [0.0, 0.0, 10.0],
            rescale: None,
            ..Default::default()
        },
    );

    let result = scan_directory(dir.path()).expect("scan must succeed");
    assert_eq!(result.series.len(), 1);
    assert!(
        !result.series[0].hu_calibrated,
        "a partially calibrated series must not be reported as calibrated"
    );
}

#[test]
fn single_slice_series_is_not_a_volume() {
    let dir = tempfile::tempdir().unwrap();
    common::write_slice(dir.path(), &common::FixtureSlice::default());

    let result = scan_directory(dir.path()).expect("scan must succeed");
    assert_eq!(result.series.len(), 1);
    assert!(!result.series[0].is_volume);
}

#[test]
fn unparseable_file_becomes_a_warning_not_a_scan_failure() {
    let dir = tempfile::tempdir().unwrap();
    common::write_slice(
        dir.path(),
        &common::FixtureSlice {
            position: [0.0, 0.0, 0.0],
            ..Default::default()
        },
    );
    common::write_slice(
        dir.path(),
        &common::FixtureSlice {
            position: [0.0, 0.0, 5.0],
            ..Default::default()
        },
    );
    let bad_path = common::write_slice(
        dir.path(),
        &common::FixtureSlice {
            omit_tag: Some("ImageOrientationPatient"),
            ..Default::default()
        },
    );

    let result = scan_directory(dir.path()).expect("a corrupt file must not fail the scan");

    let total_slices: usize = result.series.iter().map(|m| m.slices.len()).sum();
    assert_eq!(total_slices, 2, "the two good slices must still be present");

    let bad_name = bad_path.file_name().unwrap().to_str().unwrap();
    assert!(
        result.warnings.iter().any(|w| w.contains(bad_name)),
        "expected a scan-level warning naming {bad_name}, got {:?}",
        result.warnings
    );
    assert!(
        result.series.iter().all(|m| m.warnings.is_empty()),
        "a file whose series is unrecoverable must not be attached to a real series"
    );
}

#[test]
fn non_dicom_files_are_skipped_silently() {
    let dir = tempfile::tempdir().unwrap();
    common::write_slice(
        dir.path(),
        &common::FixtureSlice {
            position: [0.0, 0.0, 0.0],
            ..Default::default()
        },
    );
    common::write_slice(
        dir.path(),
        &common::FixtureSlice {
            position: [0.0, 0.0, 5.0],
            ..Default::default()
        },
    );
    std::fs::write(dir.path().join("notes.txt"), b"hello").unwrap();

    let result = scan_directory(dir.path()).expect("scan must succeed");
    assert_eq!(result.series.len(), 1);
    assert_eq!(result.series[0].slices.len(), 2);
    assert!(
        result.series[0].warnings.is_empty(),
        "a non-DICOM file must not generate a per-series warning"
    );
    assert!(
        result.warnings.is_empty(),
        "a non-DICOM file must not generate a scan-level warning either"
    );
}

#[test]
fn parses_dicom_without_a_128_byte_preamble() {
    let dir = tempfile::tempdir().unwrap();
    let normal_path = common::write_slice(
        dir.path(),
        &common::FixtureSlice {
            position: [0.0, 0.0, 0.0],
            ..Default::default()
        },
    );

    let bytes = std::fs::read(&normal_path).expect("fixture file must be readable");
    let dicm_offset = bytes
        .windows(4)
        .position(|w| w == b"DICM")
        .expect("a valid fixture must contain the DICM magic code");
    let stripped = &bytes[dicm_offset..];
    assert_eq!(
        &stripped[0..4],
        b"DICM",
        "stripped file must begin with the magic code, with no preamble"
    );

    let no_preamble_path = dir.path().join("no_preamble.dcm");
    std::fs::write(&no_preamble_path, stripped).expect("must write stripped fixture");

    // If dicom-rs itself can't open a preamble-less file, that's a real
    // finding worth surfacing rather than a test to force green: the
    // detector must still recognise it as DICOM and report a warning
    // naming the file, instead of silently dropping it.
    match SliceMeta::from_file(&no_preamble_path) {
        Ok(_) => {
            let result = scan_directory(dir.path()).expect("scan must succeed");
            let total_slices: usize = result.series.iter().map(|m| m.slices.len()).sum();
            assert_eq!(
                total_slices, 2,
                "both the normal and the preamble-less slice must be found"
            );
            assert!(
                result.warnings.is_empty() && result.series.iter().all(|m| m.warnings.is_empty()),
                "a readable preamble-less file must not produce a warning"
            );
        }
        Err(err) => {
            let result = scan_directory(dir.path()).expect("scan must succeed");
            let name = no_preamble_path.file_name().unwrap().to_str().unwrap();
            assert!(
                result.warnings.iter().any(|w| w.contains(name))
                    || result
                        .series
                        .iter()
                        .flat_map(|m| &m.warnings)
                        .any(|w| w.contains(name)),
                "dicom-rs could not open a preamble-less file ({err}); the scanner must still \
                 report it as a warning, not drop it silently"
            );
        }
    }
}

#[test]
fn extracts_series_and_study_description() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::FixtureSlice {
        series_description: "Chest Routine #1".to_string(),
        study_description: "CT Chest".to_string(),
        ..Default::default()
    };
    let p = common::write_slice(dir.path(), &f);

    let meta = SliceMeta::from_file(&p).expect("valid fixture must parse");

    assert_eq!(
        meta.series_description,
        Some("Chest Routine #1".to_string())
    );
    assert_eq!(meta.study_description, Some("CT Chest".to_string()));
}

#[test]
fn absent_description_is_none_not_empty_string() {
    let dir = tempfile::tempdir().unwrap();

    let no_series = common::write_slice(
        dir.path(),
        &common::FixtureSlice {
            omit_tag: Some("SeriesDescription"),
            ..Default::default()
        },
    );
    let meta = SliceMeta::from_file(&no_series).expect("valid fixture must parse");
    assert_eq!(
        meta.series_description, None,
        "an absent tag must be None, not an empty-string placeholder"
    );

    let no_study = common::write_slice(
        dir.path(),
        &common::FixtureSlice {
            omit_tag: Some("StudyDescription"),
            ..Default::default()
        },
    );
    let meta = SliceMeta::from_file(&no_study).expect("valid fixture must parse");
    assert_eq!(
        meta.study_description, None,
        "an absent tag must be None, not an empty-string placeholder"
    );
}

#[test]
fn whitespace_only_description_normalises_to_none() {
    let dir = tempfile::tempdir().unwrap();
    let f = common::FixtureSlice {
        series_description: "   ".to_string(),
        study_description: "   ".to_string(),
        ..Default::default()
    };
    let p = common::write_slice(dir.path(), &f);

    let meta = SliceMeta::from_file(&p).expect("valid fixture must parse");

    assert_eq!(meta.series_description, None);
    assert_eq!(meta.study_description, None);
}

#[test]
fn series_with_disagreeing_descriptions_warns_and_takes_first() {
    let dir = tempfile::tempdir().unwrap();
    let series_uid = "1.2.826.0.1.3680043.8.498.5555".to_string();

    common::write_slice(
        dir.path(),
        &common::FixtureSlice {
            series_uid: series_uid.clone(),
            position: [0.0, 0.0, 0.0],
            series_description: "Faza tetnicza 5.0 miekkie".to_string(),
            ..Default::default()
        },
    );
    common::write_slice(
        dir.path(),
        &common::FixtureSlice {
            series_uid: series_uid.clone(),
            position: [0.0, 0.0, 5.0],
            series_description: "Faza zylna 5.0 miekkie".to_string(),
            ..Default::default()
        },
    );

    let result = scan_directory(dir.path()).expect("scan must succeed");
    assert_eq!(result.series.len(), 1);
    let manifest = &result.series[0];

    // Slices are sorted by depth before the first is picked, so the z=0.0
    // slice's description wins, matching the rows/cols mismatch convention.
    assert_eq!(
        manifest.series_description,
        Some("Faza tetnicza 5.0 miekkie".to_string()),
        "must take the first (lowest-depth) slice's description"
    );
    assert!(
        manifest
            .warnings
            .iter()
            .any(|w| w.contains(&series_uid) && w.contains("description")),
        "expected a warning naming the series about disagreeing descriptions, got {:?}",
        manifest.warnings
    );
}

#[test]
fn two_series_same_patient_are_distinguishable_by_description() {
    let dir = tempfile::tempdir().unwrap();
    let series_a = "1.2.826.0.1.3680043.8.498.6001".to_string();
    let series_b = "1.2.826.0.1.3680043.8.498.6002".to_string();
    let patient = "FIXTURE-PATIENT-SHARED".to_string();

    for i in 0..2 {
        let z = i as f64 * 5.0;
        common::write_slice(
            dir.path(),
            &common::FixtureSlice {
                series_uid: series_a.clone(),
                patient_id: patient.clone(),
                position: [0.0, 0.0, z],
                series_description: "Chest Routine #1".to_string(),
                ..Default::default()
            },
        );
        common::write_slice(
            dir.path(),
            &common::FixtureSlice {
                series_uid: series_b.clone(),
                patient_id: patient.clone(),
                position: [0.0, 0.0, z],
                series_description: "Chest Routine #2".to_string(),
                ..Default::default()
            },
        );
    }

    let result = scan_directory(dir.path()).expect("scan must succeed");
    assert_eq!(
        result.series.len(),
        2,
        "expected two distinct series manifests"
    );

    let manifest_a = result
        .series
        .iter()
        .find(|m| m.series_uid == series_a)
        .expect("series_a must be present");
    let manifest_b = result
        .series
        .iter()
        .find(|m| m.series_uid == series_b)
        .expect("series_b must be present");

    assert_eq!(manifest_a.patient_id, patient);
    assert_eq!(manifest_b.patient_id, patient);
    assert_eq!(manifest_a.modality, manifest_b.modality);
    assert_eq!(manifest_a.slices.len(), manifest_b.slices.len());

    assert_eq!(
        manifest_a.series_description,
        Some("Chest Routine #1".to_string())
    );
    assert_eq!(
        manifest_b.series_description,
        Some("Chest Routine #2".to_string())
    );
}
