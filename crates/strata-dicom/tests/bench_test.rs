//! Timing harness. Numbers that appear in the README come from here, not from
//! estimation. Run in release, against real data:
//!   cargo test --release -p strata-dicom --test bench_test -- --ignored --nocapture

use std::path::PathBuf;
use std::time::Instant;

use strata_dicom::scan::scan_directory;

/// Defaults to the small sample; override with STRATA_BENCH_DIR to time a
/// different study, e.g. STRATA_BENCH_DIR=data/big.
fn sample_dir() -> PathBuf {
    match std::env::var("STRATA_BENCH_DIR") {
        Ok(d) => PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(d),
        Err(_) => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/sample"),
    }
}

#[test]
#[ignore]
fn bench_index_directory() {
    let dir = sample_dir();
    assert!(dir.exists(), "data/sample missing");

    // One untimed pass so the OS file cache is warm; we are measuring parse
    // cost, not disk cold-start, and the README says which.
    let _ = scan_directory(&dir).unwrap();

    let runs = 10;
    let mut times = Vec::with_capacity(runs);
    let mut slices = 0;
    for _ in 0..runs {
        let t = Instant::now();
        let r = scan_directory(&dir).unwrap();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
        slices = r.series.iter().map(|s| s.slices.len()).sum::<usize>();
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let median = times[times.len() / 2];
    println!("INDEX slices={slices} runs={runs}");
    println!(
        "  median={:.1}ms  min={:.1}ms  max={:.1}ms",
        median,
        times[0],
        times[times.len() - 1]
    );
    println!(
        "  per_slice={:.3}ms  rate={:.0} slices/sec",
        median / slices as f64,
        slices as f64 / (median / 1000.0)
    );
}
