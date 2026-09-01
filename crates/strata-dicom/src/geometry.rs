use crate::error::DicomError;

/// Cross product of the row and column direction cosines from
/// ImageOrientationPatient (0020,0037), normalised.
pub fn slice_normal(iop: &[f64]) -> Result<[f64; 3], DicomError> {
    if iop.len() != 6 {
        return Err(DicomError::BadOrientation(iop.len()));
    }
    if iop.iter().any(|v| !v.is_finite()) {
        return Err(DicomError::NonFiniteOrientation);
    }

    let row = [iop[0], iop[1], iop[2]];
    let col = [iop[3], iop[4], iop[5]];

    let cross = [
        row[1] * col[2] - row[2] * col[1],
        row[2] * col[0] - row[0] * col[2],
        row[0] * col[1] - row[1] * col[0],
    ];

    let magnitude = (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt();
    if magnitude < 1e-9 {
        return Err(DicomError::DegenerateOrientation);
    }

    Ok([
        cross[0] / magnitude,
        cross[1] / magnitude,
        cross[2] / magnitude,
    ])
}

/// Signed distance of a slice along its own normal. This is the ONLY valid
/// sort key for slice order. InstanceNumber (0020,0013) is unreliable in
/// real-world data and must never be used for this.
pub fn slice_depth(ipp: &[f64; 3], normal: &[f64; 3]) -> f64 {
    ipp[0] * normal[0] + ipp[1] * normal[1] + ipp[2] * normal[2]
}
