use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use walkdir::WalkDir;

use crate::error::DicomError;
use crate::meta::SliceMeta;
use crate::series::SeriesManifest;

const PREAMBLE_LEN: usize = 128;
const MAGIC_CODE: &[u8; 4] = b"DICM";

/// Checks the standard DICOM Part 10 header (128-byte preamble followed by
/// the `DICM` magic code) without parsing the rest of the file. Files that
/// fail this check (plain text, truncated, unrelated binaries) are not
/// DICOM at all and are skipped without a warning, so a real archive full
/// of README.txt or .DS_Store files doesn't drown the warnings that matter.
fn looks_like_dicom(path: &Path) -> bool {
    let mut header = [0u8; PREAMBLE_LEN + MAGIC_CODE.len()];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut header))
        .map(|()| &header[PREAMBLE_LEN..] == MAGIC_CODE)
        .unwrap_or(false)
}

pub fn scan_directory(root: &Path) -> Result<Vec<SeriesManifest>, DicomError> {
    let mut by_series: BTreeMap<String, Vec<SliceMeta>> = BTreeMap::new();
    let mut unattributed_warnings: Vec<String> = Vec::new();

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
            Err(err) => unattributed_warnings.push(format!("{}: {err}", path.display())),
        }
    }

    let mut manifests: Vec<SeriesManifest> = by_series
        .into_values()
        .map(SeriesManifest::from_slices)
        .collect();

    if !unattributed_warnings.is_empty() {
        manifests.push(SeriesManifest::unattributed(unattributed_warnings));
    }

    manifests.sort_by(|a, b| a.series_uid.cmp(&b.series_uid));

    Ok(manifests)
}
