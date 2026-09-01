use std::path::{Path, PathBuf};

use dicom::core::Tag;
use dicom::dictionary_std::tags;
use dicom::object::{DefaultDicomObject, OpenFileOptions};

use crate::error::DicomError;
use crate::geometry::{slice_depth, slice_normal};

#[derive(Debug, Clone, PartialEq)]
pub struct SliceMeta {
    pub path: PathBuf,
    pub patient_id: String,
    pub study_uid: String,
    pub series_uid: String,
    pub sop_uid: String,
    pub modality: String,
    pub rows: u16,
    pub cols: u16,
    pub position: [f64; 3],
    pub orientation: [f64; 6],
    pub rescale: Option<(f64, f64)>,
    pub pixel_spacing: Option<(f64, f64)>,
    pub slice_thickness: Option<f64>,
    pub depth: f64,
}

/// Parse a DS/IS-style multi-valued element's raw string as backslash
/// separated numbers. `to_multi_float64` on the underlying value only
/// splits when the element was decoded as `PrimitiveValue::Strs`; our own
/// fixtures (and some real encoders) produce a single `Str` containing the
/// backslashes, which `to_multi_float64` would try to parse whole and fail.
/// Reading the raw string and splitting ourselves works for both cases.
/// Anything that fails to parse becomes NaN rather than an error here, so a
/// single downstream finiteness check (matching `slice_normal`'s own guard)
/// catches malformed and non-finite values alike.
fn parse_ds_multi(raw: &str) -> Vec<f64> {
    raw.split('\\')
        .map(|part| part.trim().parse::<f64>().unwrap_or(f64::NAN))
        .collect()
}

impl SliceMeta {
    pub fn from_file(path: &Path) -> Result<SliceMeta, DicomError> {
        let obj = OpenFileOptions::new()
            .read_until(tags::PIXEL_DATA)
            .open_file(path)
            .map_err(|_| DicomError::MissingTag {
                tag: "DICOM file could not be opened".to_string(),
                file: path.to_path_buf(),
            })?;

        let required = |tag, name: &str| -> Result<_, DicomError> {
            obj.element(tag).map_err(|_| DicomError::MissingTag {
                tag: name.to_string(),
                file: path.to_path_buf(),
            })
        };

        let series_uid = required(tags::SERIES_INSTANCE_UID, "SeriesInstanceUID")?
            .to_str()
            .unwrap_or_default()
            .trim()
            .to_string();
        let sop_uid = required(tags::SOP_INSTANCE_UID, "SOPInstanceUID")?
            .to_str()
            .unwrap_or_default()
            .trim()
            .to_string();
        let rows = required(tags::ROWS, "Rows")?
            .to_int::<u16>()
            .unwrap_or_default();
        let cols = required(tags::COLUMNS, "Columns")?
            .to_int::<u16>()
            .unwrap_or_default();

        let position_raw = required(tags::IMAGE_POSITION_PATIENT, "ImagePositionPatient")?
            .to_str()
            .unwrap_or_default()
            .to_string();
        let orientation_raw = required(tags::IMAGE_ORIENTATION_PATIENT, "ImageOrientationPatient")?
            .to_str()
            .unwrap_or_default()
            .to_string();

        // Non-finite orientation values are rejected inside slice_normal, so
        // that error propagates from here without duplicating the check.
        let orientation_values = parse_ds_multi(&orientation_raw);
        let normal = slice_normal(&orientation_values)?;
        let orientation: [f64; 6] = orientation_values
            .try_into()
            .expect("slice_normal succeeded, so exactly 6 values were present");

        let position_values = parse_ds_multi(&position_raw);
        let mut position = [f64::NAN; 3];
        for (slot, value) in position.iter_mut().zip(position_values.iter()) {
            *slot = *value;
        }
        if position.iter().any(|v| !v.is_finite()) {
            return Err(DicomError::NonFinitePosition {
                file: path.to_path_buf(),
            });
        }

        let depth = slice_depth(&position, &normal);

        let patient_id = optional_string(&obj, tags::PATIENT_ID);
        let study_uid = optional_string(&obj, tags::STUDY_INSTANCE_UID);
        let modality = optional_string(&obj, tags::MODALITY);

        // HU calibration is only meaningful when both tags are present;
        // never default a missing slope/intercept to an identity transform.
        let rescale = match (
            obj.element(tags::RESCALE_SLOPE).ok(),
            obj.element(tags::RESCALE_INTERCEPT).ok(),
        ) {
            (Some(slope), Some(intercept)) => {
                match (slope.to_float64(), intercept.to_float64()) {
                    (Ok(s), Ok(i)) => Some((s, i)),
                    _ => None,
                }
            }
            _ => None,
        };

        let pixel_spacing = obj
            .element(tags::PIXEL_SPACING)
            .ok()
            .and_then(|el| el.to_str().ok())
            .map(|raw| parse_ds_multi(&raw))
            .filter(|values| values.len() == 2 && values.iter().all(|v| v.is_finite()))
            .map(|values| (values[0], values[1]));

        let slice_thickness = obj
            .element(tags::SLICE_THICKNESS)
            .ok()
            .and_then(|el| el.to_float64().ok())
            .filter(|v| v.is_finite());

        Ok(SliceMeta {
            path: path.to_path_buf(),
            patient_id,
            study_uid,
            series_uid,
            sop_uid,
            modality,
            rows,
            cols,
            position,
            orientation,
            rescale,
            pixel_spacing,
            slice_thickness,
            depth,
        })
    }
}

/// Identifiers, not measurements: an absent tag is honestly "unknown", so
/// this defaults to an empty string rather than fabricating a value.
fn optional_string(obj: &DefaultDicomObject, tag: Tag) -> String {
    obj.element(tag)
        .ok()
        .and_then(|el| el.to_str().ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}
