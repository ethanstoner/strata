#![allow(dead_code)]

//! Synthetic DICOM fixture builder used by strata-dicom's integration tests.
//!
//! `dicom-rs` does not pad or validate string values on the way in (unlike
//! `FileMetaTableBuilder`, which pads meta-group UIDs itself), and its
//! `PrimitiveValue::Strs` byte-length calculation does not agree with what it
//! actually writes when the joined value has odd length. To sidestep both
//! issues, every string-valued element here is built as a single
//! backslash-joined `Str`, pre-padded to even length by hand.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use dicom::core::{DataElement, PrimitiveValue, Tag, VR};
use dicom::dictionary_std::tags;
use dicom::object::{FileMetaTableBuilder, InMemDicomObject};

const TRANSFER_SYNTAX_EXPLICIT_VR_LE: &str = "1.2.840.10008.1.2.1";
const SOP_CLASS_CT_IMAGE_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.2";

pub struct FixtureSlice {
    pub series_uid: String,
    pub study_uid: String,
    pub patient_id: String,
    /// Deliberately settable to a value that disagrees with true slice order,
    /// so tests can prove callers don't sort by it.
    pub instance_number: i32,
    pub position: [f64; 3],
    pub orientation: [f64; 6],
    /// (slope, intercept). `None` genuinely omits both tags rather than
    /// writing an identity default.
    pub rescale: Option<(f64, f64)>,
    pub rows: u16,
    pub cols: u16,
    /// Element keyword (e.g. "ImageOrientationPatient") to leave off the
    /// written file entirely, so tests can exercise MissingTag paths.
    pub omit_tag: Option<&'static str>,
}

impl Default for FixtureSlice {
    fn default() -> Self {
        FixtureSlice {
            series_uid: "1.2.826.0.1.3680043.8.498.1000".to_string(),
            study_uid: "1.2.826.0.1.3680043.8.498.2000".to_string(),
            patient_id: "FIXTURE-PATIENT-1".to_string(),
            instance_number: 1,
            position: [0.0, 0.0, 0.0],
            orientation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            rescale: Some((1.0, -1024.0)),
            rows: 64,
            cols: 64,
            omit_tag: None,
        }
    }
}

static SOP_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Pad a string to even byte length, as DICOM VRs require. `\0` is the
/// standard pad for UI; other string VRs pad with a space.
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

/// Build one DS/IS-style element from pre-formatted numbers, joined with
/// `\` and padded as a single `Str` so the declared and written lengths
/// can never disagree (see module docs).
fn numeric_string_value(parts: &[String]) -> PrimitiveValue {
    let joined = parts.join("\\");
    PrimitiveValue::Str(pad_even(joined, ' '))
}

fn ds(values: &[f64]) -> PrimitiveValue {
    numeric_string_value(&values.iter().map(|v| v.to_string()).collect::<Vec<_>>())
}

fn is(value: i32) -> PrimitiveValue {
    numeric_string_value(&[value.to_string()])
}

fn put(obj: &mut InMemDicomObject, tag: Tag, vr: VR, value: PrimitiveValue) {
    obj.put(DataElement::new(tag, vr, value));
}

/// Writes a valid DICOM file into `dir`. Returns the path written.
pub fn write_slice(dir: &Path, s: &FixtureSlice) -> PathBuf {
    let n = SOP_COUNTER.fetch_add(1, Ordering::SeqCst);
    let sop_instance_uid = format!("1.2.826.0.1.3680043.8.498.9.{n}");

    let mut obj = InMemDicomObject::new_empty();
    let omit = |name: &str| s.omit_tag == Some(name);

    if !omit("PatientID") {
        put(&mut obj, tags::PATIENT_ID, VR::LO, text_value(&s.patient_id));
    }
    if !omit("StudyInstanceUID") {
        put(
            &mut obj,
            tags::STUDY_INSTANCE_UID,
            VR::UI,
            uid_value(&s.study_uid),
        );
    }
    if !omit("SeriesInstanceUID") {
        put(
            &mut obj,
            tags::SERIES_INSTANCE_UID,
            VR::UI,
            uid_value(&s.series_uid),
        );
    }
    if !omit("SOPInstanceUID") {
        put(
            &mut obj,
            tags::SOP_INSTANCE_UID,
            VR::UI,
            uid_value(&sop_instance_uid),
        );
    }
    put(
        &mut obj,
        tags::SOP_CLASS_UID,
        VR::UI,
        uid_value(SOP_CLASS_CT_IMAGE_STORAGE),
    );
    if !omit("Modality") {
        put(&mut obj, tags::MODALITY, VR::CS, text_value("CT"));
    }
    put(&mut obj, tags::INSTANCE_NUMBER, VR::IS, is(s.instance_number));
    if !omit("Rows") {
        put(&mut obj, tags::ROWS, VR::US, PrimitiveValue::from(s.rows));
    }
    if !omit("Columns") {
        put(&mut obj, tags::COLUMNS, VR::US, PrimitiveValue::from(s.cols));
    }
    if !omit("ImagePositionPatient") {
        put(
            &mut obj,
            tags::IMAGE_POSITION_PATIENT,
            VR::DS,
            ds(&s.position),
        );
    }
    if !omit("ImageOrientationPatient") {
        put(
            &mut obj,
            tags::IMAGE_ORIENTATION_PATIENT,
            VR::DS,
            ds(&s.orientation),
        );
    }
    put(&mut obj, tags::PIXEL_SPACING, VR::DS, ds(&[1.0, 1.0]));
    put(&mut obj, tags::SLICE_THICKNESS, VR::DS, ds(&[1.0]));
    put(
        &mut obj,
        tags::BITS_ALLOCATED,
        VR::US,
        PrimitiveValue::from(16u16),
    );
    put(
        &mut obj,
        tags::PIXEL_REPRESENTATION,
        VR::US,
        PrimitiveValue::from(0u16),
    );

    if let Some((slope, intercept)) = s.rescale {
        put(&mut obj, tags::RESCALE_SLOPE, VR::DS, ds(&[slope]));
        put(&mut obj, tags::RESCALE_INTERCEPT, VR::DS, ds(&[intercept]));
    }

    let rows = s.rows as usize;
    let cols = s.cols as usize;
    let pixels: Vec<u16> = (0..rows * cols).map(|i| i as u16).collect();
    put(
        &mut obj,
        tags::PIXEL_DATA,
        VR::OW,
        PrimitiveValue::U16(pixels.into()),
    );

    let meta = FileMetaTableBuilder::new()
        .media_storage_sop_class_uid(SOP_CLASS_CT_IMAGE_STORAGE)
        .media_storage_sop_instance_uid(sop_instance_uid.as_str())
        .transfer_syntax(TRANSFER_SYNTAX_EXPLICIT_VR_LE)
        .build()
        .expect("fixture meta table is missing a required field");

    let file_obj = obj.with_exact_meta(meta);

    let path = dir.join(format!("{n}.dcm"));
    file_obj
        .write_to_file(&path)
        .expect("fixture object failed to write");

    path
}
