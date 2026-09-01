use crate::meta::SliceMeta;

#[derive(Debug, Clone)]
pub struct SeriesManifest {
    pub series_uid: String,
    pub study_uid: String,
    pub patient_id: String,
    pub modality: String,
    pub rows: u16,
    pub cols: u16,
    pub slices: Vec<SliceMeta>,
    pub uniform_spacing: bool,
    pub spacing_mm: Option<f64>,
    pub hu_calibrated: bool,
    pub is_volume: bool,
    pub warnings: Vec<String>,
}

impl SeriesManifest {
    /// Builds a manifest from every `SliceMeta` sharing one `series_uid`.
    /// Panics if `slices` is empty — callers (scan.rs) only invoke this once
    /// per non-empty group.
    pub(crate) fn from_slices(mut slices: Vec<SliceMeta>) -> SeriesManifest {
        assert!(
            !slices.is_empty(),
            "SeriesManifest::from_slices requires at least one slice"
        );

        // depth is the only valid sort key (see geometry.rs). SliceMeta::from_file
        // already rejects non-finite positions, so partial_cmp here can never see
        // a NaN; `.expect` makes that invariant loud instead of silently
        // misplacing a slice with `unwrap_or(Ordering::Equal)`.
        slices.sort_by(|a, b| {
            a.depth.partial_cmp(&b.depth).expect(
                "SliceMeta::from_file rejects non-finite positions, so depth is always finite",
            )
        });

        let first = &slices[0];
        let series_uid = first.series_uid.clone();
        let study_uid = first.study_uid.clone();
        let patient_id = first.patient_id.clone();
        let modality = first.modality.clone();
        let rows = first.rows;
        let cols = first.cols;

        let mut warnings = Vec::new();
        if slices.iter().any(|s| s.rows != rows || s.cols != cols) {
            warnings.push(format!(
                "series {series_uid} contains slices with mismatched rows/cols; cannot form a coherent volume"
            ));
        }

        let hu_calibrated = slices.iter().all(|s| s.rescale.is_some());
        let is_volume = slices.len() > 1;
        let (spacing_mm, uniform_spacing) = spacing_stats(&slices);

        SeriesManifest {
            series_uid,
            study_uid,
            patient_id,
            modality,
            rows,
            cols,
            slices,
            uniform_spacing,
            spacing_mm,
            hu_calibrated,
            is_volume,
            warnings,
        }
    }

    /// A sentinel manifest for warnings that can't be attributed to a series
    /// because parsing failed before `series_uid` could be read. Carries no
    /// slices, so it's never a candidate for HU calibration or volume rendering.
    pub(crate) fn unattributed(warnings: Vec<String>) -> SeriesManifest {
        SeriesManifest {
            series_uid: String::new(),
            study_uid: String::new(),
            patient_id: String::new(),
            modality: String::new(),
            rows: 0,
            cols: 0,
            slices: Vec::new(),
            uniform_spacing: true,
            spacing_mm: None,
            hu_calibrated: false,
            is_volume: false,
            warnings,
        }
    }
}

/// Median of consecutive depth deltas, and whether every delta is within 1%
/// of that median. A single-slice series has no deltas and is trivially
/// uniform with no defined spacing.
fn spacing_stats(slices: &[SliceMeta]) -> (Option<f64>, bool) {
    if slices.len() < 2 {
        return (None, true);
    }

    let deltas: Vec<f64> = slices
        .windows(2)
        .map(|pair| pair[1].depth - pair[0].depth)
        .collect();

    let mut sorted = deltas.clone();
    sorted.sort_by(|a, b| {
        a.partial_cmp(b)
            .expect("deltas of finite depths are always finite")
    });
    let mid = sorted.len() / 2;
    let median = if sorted.len() % 2 == 0 {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    };

    let uniform = deltas.iter().all(|d| {
        if median.abs() < 1e-9 {
            (d - median).abs() < 1e-9
        } else {
            ((d - median).abs() / median.abs()) <= 0.01
        }
    });

    (Some(median), uniform)
}
