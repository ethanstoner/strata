//! Fixture builder for pixels_test.rs only. strata-dicom has its own builder
//! under its tests/ dir (crates/strata-dicom/tests/common/mod.rs) but it is
//! not importable across crates, and it doesn't write the imaging tags
//! (SamplesPerPixel, BitsStored, HighBit, PhotometricInterpretation)
//! dicom-pixeldata requires to decode — this one adds them.

use std::path::{Path, PathBuf};

use dicom::core::{DataElement, PrimitiveValue, Tag, VR};
use dicom::dictionary_std::tags;
use dicom::object::{FileMetaTableBuilder, InMemDicomObject};

use strata_dicom::error::DicomError;
use strata_server::pixels::decode_slice;

const TRANSFER_SYNTAX_EXPLICIT_VR_LE: &str = "1.2.840.10008.1.2.1";
const SOP_CLASS_CT_IMAGE_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.2";

fn pad_even(mut s: String, pad: char) -> String {
    if s.len() % 2 != 0 {
        s.push(pad);
    }
    s
}

fn uid_value(s: &str) -> PrimitiveValue {
    PrimitiveValue::Str(pad_even(s.to_string(), '\0'))
}

fn text_value(s: &str) -> PrimitiveValue {
    PrimitiveValue::Str(pad_even(s.to_string(), ' '))
}

fn ds(values: &[f64]) -> PrimitiveValue {
    let joined = values
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\\");
    PrimitiveValue::Str(pad_even(joined, ' '))
}

fn put(obj: &mut InMemDicomObject, tag: Tag, vr: VR, value: PrimitiveValue) {
    obj.put(DataElement::new(tag, vr, value));
}

/// Writes a minimal, valid, single-frame CT slice with `pixels` as the raw
/// stored (unsigned 16-bit) sample values. `rescale = None` genuinely omits
/// RescaleSlope/RescaleIntercept rather than writing an identity default.
fn write_slice(dir: &Path, rows: u16, cols: u16, pixels: &[u16], rescale: Option<(f64, f64)>) -> PathBuf {
    let mut obj = InMemDicomObject::new_empty();

    put(
        &mut obj,
        tags::SERIES_INSTANCE_UID,
        VR::UI,
        uid_value("1.2.826.0.1.3680043.8.498.1000"),
    );
    put(
        &mut obj,
        tags::SOP_INSTANCE_UID,
        VR::UI,
        uid_value("1.2.826.0.1.3680043.8.498.9.1"),
    );
    put(
        &mut obj,
        tags::SOP_CLASS_UID,
        VR::UI,
        uid_value(SOP_CLASS_CT_IMAGE_STORAGE),
    );
    put(&mut obj, tags::MODALITY, VR::CS, text_value("CT"));
    put(&mut obj, tags::ROWS, VR::US, PrimitiveValue::from(rows));
    put(&mut obj, tags::COLUMNS, VR::US, PrimitiveValue::from(cols));
    put(
        &mut obj,
        tags::IMAGE_POSITION_PATIENT,
        VR::DS,
        ds(&[0.0, 0.0, 0.0]),
    );
    put(
        &mut obj,
        tags::IMAGE_ORIENTATION_PATIENT,
        VR::DS,
        ds(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
    );
    put(&mut obj, tags::PIXEL_SPACING, VR::DS, ds(&[1.0, 1.0]));
    put(&mut obj, tags::SLICE_THICKNESS, VR::DS, ds(&[1.0]));

    put(
        &mut obj,
        tags::SAMPLES_PER_PIXEL,
        VR::US,
        PrimitiveValue::from(1u16),
    );
    put(
        &mut obj,
        tags::PHOTOMETRIC_INTERPRETATION,
        VR::CS,
        text_value("MONOCHROME2"),
    );
    put(
        &mut obj,
        tags::BITS_ALLOCATED,
        VR::US,
        PrimitiveValue::from(16u16),
    );
    put(
        &mut obj,
        tags::BITS_STORED,
        VR::US,
        PrimitiveValue::from(16u16),
    );
    put(&mut obj, tags::HIGH_BIT, VR::US, PrimitiveValue::from(15u16));
    put(
        &mut obj,
        tags::PIXEL_REPRESENTATION,
        VR::US,
        PrimitiveValue::from(0u16),
    );

    if let Some((slope, intercept)) = rescale {
        put(&mut obj, tags::RESCALE_SLOPE, VR::DS, ds(&[slope]));
        put(&mut obj, tags::RESCALE_INTERCEPT, VR::DS, ds(&[intercept]));
    }

    put(
        &mut obj,
        tags::PIXEL_DATA,
        VR::OW,
        PrimitiveValue::U16(pixels.to_vec().into()),
    );

    let meta = FileMetaTableBuilder::new()
        .media_storage_sop_class_uid(SOP_CLASS_CT_IMAGE_STORAGE)
        .media_storage_sop_instance_uid("1.2.826.0.1.3680043.8.498.9.1")
        .transfer_syntax(TRANSFER_SYNTAX_EXPLICIT_VR_LE)
        .build()
        .expect("fixture meta table is missing a required field");

    let file_obj = obj.with_exact_meta(meta);
    let path = dir.join("slice.dcm");
    file_obj
        .write_to_file(&path)
        .expect("fixture object failed to write");
    path
}

#[test]
fn applies_rescale_to_produce_hounsfield_units() {
    let dir = tempfile::tempdir().unwrap();
    // slope=1, intercept=-1024: raw 0 -> air-floor -1024 HU, raw 1024 -> water 0 HU.
    let raw = [0u16, 1024, 2024];
    let path = write_slice(dir.path(), 1, 3, &raw, Some((1.0, -1024.0)));

    let decoded = decode_slice(&path).expect("fixture must decode");

    assert!(decoded.hu_calibrated);
    assert_eq!(decoded.rows, 1);
    assert_eq!(decoded.cols, 3);
    assert_eq!(decoded.data, vec![-1024, 0, 1000]);
}

#[test]
fn missing_rescale_serves_raw_values_and_reports_uncalibrated() {
    let dir = tempfile::tempdir().unwrap();
    let raw = [0u16, 1024, 2024];
    let path = write_slice(dir.path(), 1, 3, &raw, None);

    let decoded = decode_slice(&path).expect("fixture must decode");

    assert!(!decoded.hu_calibrated);
    assert_eq!(decoded.data, vec![0, 1024, 2024]);
}

#[test]
fn nonexistent_file_is_a_dicom_error_not_a_panic() {
    let err = decode_slice(Path::new("/no/such/file.dcm")).expect_err("must fail");
    assert!(matches!(
        err,
        DicomError::MissingTag { .. } | DicomError::UnsupportedTransferSyntax { .. }
    ));
}

fn sample_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/sample")
}

/// Risk gate: proves the calibration path against a genuine chest CT rather
/// than a fixture we built to agree with ourselves. Requires `data/sample/`.
/// Run with:
///   cargo test -p strata-server --test pixels_test -- --ignored --nocapture
#[test]
#[ignore]
fn real_slice_decodes_to_plausible_hu() {
    let dir = sample_dir();
    assert!(dir.exists(), "data/sample missing; fetch a TCIA series first");

    let result = strata_dicom::scan::scan_directory(&dir).expect("scan must succeed");
    let series = result
        .series
        .into_iter()
        .find(|s| s.is_volume)
        .expect("expected at least one multi-slice series");

    let path = &series.slices[0].path;
    let decoded = decode_slice(path).expect("real slice must decode");

    assert_eq!(decoded.data.len(), decoded.rows as usize * decoded.cols as usize);

    let min = *decoded.data.iter().min().unwrap();
    let max = *decoded.data.iter().max().unwrap();
    println!("min={min} max={max} hu_calibrated={}", decoded.hu_calibrated);

    assert!(min < -900, "expected air outside the patient, got min={min}");
    assert!(max > 100, "expected bone/contrast in a chest CT, got max={max}");
}
