use std::path::Path;

use dicom::object::open_file;
use dicom::pixeldata::{ConvertOptions, ModalityLutOption, PixelDecoder};

use strata_dicom::error::DicomError;
use strata_dicom::meta::SliceMeta;

#[derive(Debug)]
pub struct DecodedSlice {
    pub rows: u16,
    pub cols: u16,
    pub hu_calibrated: bool,
    pub data: Vec<i16>,
}

/// Decodes one DICOM file's pixel data.
///
/// `hu_calibrated` and the rescale to apply come from `SliceMeta`, the same
/// tag-presence check the scanner uses — never from dicom-pixeldata's own
/// `Rescale`, whose getter silently substitutes an identity slope/intercept
/// when the tags are absent. Trusting that fallback here would report
/// synthetic HU values as calibrated.
pub fn decode_slice(path: &Path) -> Result<DecodedSlice, DicomError> {
    let meta = SliceMeta::from_file(path)?;

    let obj = open_file(path).map_err(|_| DicomError::MissingTag {
        tag: "DICOM file could not be opened for pixel decoding".to_string(),
        file: path.to_path_buf(),
    })?;
    let ts_uid = obj
        .meta()
        .transfer_syntax()
        .trim_end_matches('\0')
        .to_string();

    // Any failure past this point (unregistered transfer syntax, or a
    // codec that's registered but can't decode this data) is reported as
    // the same diagnostic: the transfer syntax UID and the file, since
    // that's the only thing an operator can act on against a real archive.
    let unsupported = || DicomError::UnsupportedTransferSyntax {
        uid: ts_uid.clone(),
        file: path.to_path_buf(),
    };
    let decoded = obj.decode_pixel_data().map_err(|_| unsupported())?;

    // Fetch raw stored samples with no Modality LUT applied: the rescale
    // (or lack of it) is applied by hand below, driven by `meta.rescale`.
    let options = ConvertOptions::new().with_modality_lut(ModalityLutOption::None);
    let raw: Vec<i32> = decoded
        .to_vec_frame_with_options(0, &options)
        .map_err(|_| unsupported())?;

    let hu_calibrated = meta.rescale.is_some();
    let data: Vec<i16> = match meta.rescale {
        Some((slope, intercept)) => raw
            .iter()
            .map(|&raw| {
                // Saturate rather than wrap: a malformed rescale producing an
                // out-of-range value must not alias to a plausible-looking
                // but wrong density.
                (raw as f64 * slope + intercept)
                    .round()
                    .clamp(i16::MIN as f64, i16::MAX as f64) as i16
            })
            .collect(),
        None => raw
            .iter()
            .map(|&raw| raw.clamp(i16::MIN as i32, i16::MAX as i32) as i16)
            .collect(),
    };

    Ok(DecodedSlice {
        rows: decoded.rows() as u16,
        cols: decoded.columns() as u16,
        hu_calibrated,
        data,
    })
}
