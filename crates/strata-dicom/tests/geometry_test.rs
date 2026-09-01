use strata_dicom::geometry::{slice_normal, slice_depth};

// A standard axial acquisition: rows run +x, columns run +y.
const AXIAL: [f64; 6] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];

#[test]
fn axial_normal_points_along_z() {
    let n = slice_normal(&AXIAL).unwrap();
    assert!((n[0]).abs() < 1e-9);
    assert!((n[1]).abs() < 1e-9);
    assert!((n[2] - 1.0).abs() < 1e-9);
}

#[test]
fn depth_is_position_projected_onto_normal() {
    let n = slice_normal(&AXIAL).unwrap();
    // Only the z component should matter for an axial slice.
    assert!((slice_depth(&[10.0, -20.0, 5.5], &n) - 5.5).abs() < 1e-9);
}

#[test]
fn sagittal_normal_points_along_x() {
    // rows run +y, columns run +z
    let n = slice_normal(&[0.0, 1.0, 0.0, 0.0, 0.0, 1.0]).unwrap();
    assert!((n[0] - 1.0).abs() < 1e-9);
}

#[test]
fn rejects_wrong_length_orientation() {
    assert!(slice_normal(&[1.0, 0.0, 0.0]).is_err());
}

#[test]
fn rejects_degenerate_orientation() {
    // Parallel row and column vectors have no defined normal.
    assert!(slice_normal(&[1.0, 0.0, 0.0, 1.0, 0.0, 0.0]).is_err());
}
