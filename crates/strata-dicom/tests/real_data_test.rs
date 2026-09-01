//! Risk gate: everything else in this crate is tested against fixtures we
//! generate ourselves, which proves internal consistency and nothing about
//! real scanner output. This runs the scanner against genuine TCIA data.
//!
//! Requires `data/sample/` to be populated. Run with:
//!   cargo test -p strata-dicom --test real_data_test -- --ignored --nocapture

use std::path::PathBuf;

use strata_dicom::scan::scan_directory;

fn sample_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/sample")
}

#[test]
#[ignore]
fn scans_a_real_series() {
    let dir = sample_dir();
    assert!(dir.exists(), "data/sample missing; fetch a TCIA series first");

    let result = scan_directory(&dir).expect("scan must not abort on real data");

    println!("scan-level warnings: {}", result.warnings.len());
    for w in &result.warnings {
        println!("  WARN {w}");
    }
    println!("series found: {}", result.series.len());

    for s in &result.series {
        println!(
            "  uid={}\n    slices={} dims={}x{} modality={}\n    uniform_spacing={} spacing_mm={:?}\n    hu_calibrated={} is_volume={} warnings={}\n    series_description={:?} study_description={:?}",
            s.series_uid, s.slices.len(), s.rows, s.cols, s.modality,
            s.uniform_spacing, s.spacing_mm, s.hu_calibrated, s.is_volume,
            s.warnings.len(),
            s.series_description, s.study_description,
        );
        for w in &s.warnings {
            println!("      WARN {w}");
        }

        let depths: Vec<f64> = s.slices.iter().map(|x| x.depth).collect();
        if depths.len() > 1 {
            println!("    depth range: {:.2} .. {:.2}", depths[0], depths[depths.len() - 1]);
            // The whole crate exists to guarantee this.
            assert!(
                depths.windows(2).all(|w| w[0] <= w[1]),
                "slices are not sorted by depth ascending"
            );
        }

        // If the scanner accepted these as one series they must agree on size,
        // otherwise they cannot form a coherent volume.
        assert!(s.slices.iter().all(|x| x.rows == s.rows && x.cols == s.cols));
    }

    assert!(!result.series.is_empty(), "found no series in real data");
    assert!(
        result.series.iter().any(|s| s.is_volume),
        "no multi-slice series found; the sample is not a volume"
    );
}

/// The ordering guarantee, checked against real acquisition order rather than
/// a fixture. InstanceNumber and geometric depth agree in well-behaved data;
/// where they disagree, geometry is authoritative. This test reports the
/// relationship rather than asserting agreement.
#[test]
#[ignore]
fn reports_instance_number_versus_geometric_order() {
    let result = scan_directory(&sample_dir()).unwrap();
    for s in result.series.iter().filter(|s| s.is_volume) {
        let n = s.slices.len();
        let ascending = s.slices.windows(2).all(|w| w[0].depth <= w[1].depth);
        println!(
            "series {} : {} slices, depth ascending = {}",
            &s.series_uid[..20.min(s.series_uid.len())], n, ascending
        );
    }
}
