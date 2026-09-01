use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use walkdir::WalkDir;

use crate::error::DicomError;
use crate::meta::SliceMeta;
use crate::series::SeriesManifest;

const PREAMBLE_LEN: usize = 128;
const MAGIC_CODE: &[u8; 4] = b"DICM";

/// Series grouped and ordered, plus warnings that can't be attributed to any
/// single series because parsing failed before `series_uid` was readable.
pub struct ScanResult {
    pub series: Vec<SeriesManifest>,
    pub warnings: Vec<String>,
}

/// Checks for the DICOM magic code `DICM` either at its standard offset
/// (after a 128-byte preamble) or at offset 0, which non-conformant
/// exporters sometimes produce by omitting the preamble entirely. Both are
/// real DICOM; a file matching neither (plain text, unrelated binary, or
/// truncated) is skipped without a warning.
fn looks_like_dicom(path: &Path) -> bool {
    let mut header = Vec::new();
    if std::fs::File::open(path)
        .and_then(|f| f.take((PREAMBLE_LEN + MAGIC_CODE.len()) as u64).read_to_end(&mut header))
        .is_err()
    {
        return false;
    }

    header.get(PREAMBLE_LEN..PREAMBLE_LEN + MAGIC_CODE.len()) == Some(MAGIC_CODE.as_slice())
        || header.get(0..MAGIC_CODE.len()) == Some(MAGIC_CODE.as_slice())
}

pub fn scan_directory(root: &Path) -> Result<ScanResult, DicomError> {
    let mut by_series: BTreeMap<String, Vec<SliceMeta>> = BTreeMap::new();
    let mut warnings: Vec<String> = Vec::new();

    for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();

        if !looks_like_dicom(path) {
            continue;
        }

        match SliceMeta::from_file(path) {
            Ok(meta) => by_series
                .entry(meta.series_uid.clone())
                .or_default()
                .push(meta),
            // SliceMeta::from_file fails before series_uid is guaranteed to be
            // readable, so there's no series to attach this warning to. One
            // corrupt file must not abort the scan of the other 9,999.
            Err(err) => warnings.push(format!("{}: {err}", path.display())),
        }
    }

    let mut series: Vec<SeriesManifest> = by_series
        .into_values()
        .map(SeriesManifest::from_slices)
        .collect();

    series.sort_by(|a, b| a.series_uid.cmp(&b.series_uid));

    Ok(ScanResult { series, warnings })
}
