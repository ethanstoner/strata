use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use rayon::prelude::*;

use crate::disk_cache::DiskCache;
use crate::index::SeriesDetail;
use crate::pixels::decode_slice;
use crate::routes::SharedIndex;

/// Levels 0-3 are supported; anything above is rejected by the caller as a 400.
pub const MAX_LEVEL: u32 = 3;

/// Refuses to serve a response body larger than this many bytes.
pub const MAX_OUTPUT_BYTES: u64 = 512 * 1024 * 1024;

/// Bound on the number of `(series_uid, level)` entries kept in memory.
const CACHE_BOUND: usize = 16;

/// A fully-assembled volume at some pyramid level, ready to serve as raw
/// little-endian i16, x fastest, then y, then z.
#[derive(Debug, Clone, PartialEq)]
pub struct Volume {
    pub dim_x: u32,
    pub dim_y: u32,
    pub dim_z: u32,
    pub spacing_x: f64,
    pub spacing_y: f64,
    pub spacing_z: f64,
    pub hu_calibrated: bool,
    pub data: Vec<i16>,
}

impl Volume {
    /// Min/max over the data actually being returned, per the response
    /// contract — not the theoretical i16 range.
    pub fn hu_min_max(&self) -> (i16, i16) {
        let mut min = i16::MAX;
        let mut max = i16::MIN;
        for &v in &self.data {
            min = min.min(v);
            max = max.max(v);
        }
        (min, max)
    }
}

/// Voxel counts a volume would have at `factor` (a power of two), computed
/// with ceiling division so a partial edge block still counts as one voxel
/// instead of being dropped.
pub fn output_dims(base_x: u32, base_y: u32, base_z: u32, factor: u32) -> (u32, u32, u32) {
    let ceil_div = |d: u32| d.div_ceil(factor);
    (ceil_div(base_x), ceil_div(base_y), ceil_div(base_z))
}

pub fn output_bytes(dim_x: u32, dim_y: u32, dim_z: u32) -> u64 {
    (dim_x as u64) * (dim_y as u64) * (dim_z as u64) * 2
}

/// Box-averages `factor`-cubed neighbourhoods of a slice-major (x fastest,
/// then y, then z) i16 volume. Averaging is done in i64 so a block of
/// extreme values (e.g. all `i16::MAX`) cannot overflow on the way to the
/// rounded i16 result. Dimensions not a multiple of `factor` are handled by
/// averaging over the partial block's actual voxel count rather than
/// dropping the remainder, so a downsampled volume never silently truncates
/// the top of the patient's anatomy.
pub fn downsample(
    data: &[i16],
    dim_x: u32,
    dim_y: u32,
    dim_z: u32,
    factor: u32,
) -> (Vec<i16>, u32, u32, u32) {
    assert!(factor >= 1, "downsample factor must be >= 1");
    if factor == 1 {
        return (data.to_vec(), dim_x, dim_y, dim_z);
    }

    let (dim_x, dim_y, dim_z, factor) = (
        dim_x as usize,
        dim_y as usize,
        dim_z as usize,
        factor as usize,
    );
    let (new_x, new_y, new_z) = (
        dim_x.div_ceil(factor),
        dim_y.div_ceil(factor),
        dim_z.div_ceil(factor),
    );

    let mut out = vec![0i16; new_x * new_y * new_z];
    for nz in 0..new_z {
        let z0 = nz * factor;
        let z1 = (z0 + factor).min(dim_z);
        for ny in 0..new_y {
            let y0 = ny * factor;
            let y1 = (y0 + factor).min(dim_y);
            for nx in 0..new_x {
                let x0 = nx * factor;
                let x1 = (x0 + factor).min(dim_x);

                let mut sum: i64 = 0;
                let mut count: i64 = 0;
                for z in z0..z1 {
                    let z_base = z * dim_y * dim_x;
                    for y in y0..y1 {
                        let row_base = z_base + y * dim_x;
                        for x in x0..x1 {
                            sum += data[row_base + x] as i64;
                            count += 1;
                        }
                    }
                }
                let avg = (sum as f64 / count as f64)
                    .round()
                    .clamp(i16::MIN as f64, i16::MAX as f64) as i16;
                out[nz * new_y * new_x + ny * new_x + nx] = avg;
            }
        }
    }

    (out, new_x as u32, new_y as u32, new_z as u32)
}

/// Decodes every slice of a series and assembles the full-resolution
/// volume. Decoding is CPU-bound and each slice is independent, so it fans
/// out across every core via rayon; each worker writes straight into its
/// own chunk of `data`, so the geometric ordinal order (chunk index, which
/// mirrors `paths`' order) falls out of the write target rather than
/// needing a merge step or a shared, lock-protected `Vec` that parallel
/// pushes could reorder.
///
/// A single slice that fails to decode fails the whole request, naming the
/// offending ordinal and file. Substituting zeros for a bad slice was
/// rejected: it would render as a black band of "air" through the patient,
/// which looks like a successful, trustworthy result and is far more
/// dangerous than a loud error.
fn assemble_level0(detail: &SeriesDetail, paths: &[PathBuf]) -> anyhow::Result<Volume> {
    let dim_x = detail.cols as u32;
    let dim_y = detail.rows as u32;
    let dim_z = detail.slice_count;

    if paths.len() != dim_z as usize {
        anyhow::bail!(
            "series {}: index has {} slice paths but reports slice_count {dim_z}",
            detail.series_uid,
            paths.len()
        );
    }

    // DICOM PixelSpacing is (row spacing, column spacing): row spacing is
    // the distance between rows, i.e. the y-axis step; column spacing is
    // the x-axis step. Report 1.0 rather than guess when it's unavailable.
    let (spacing_y, spacing_x) = detail
        .pixel_spacing
        .map(|[row, col]| (row, col))
        .unwrap_or((1.0, 1.0));

    // spacing_z comes from spacing_mm, the median inter-slice distance
    // strata-dicom already derived from actual slice positions — never from
    // slice_thickness, which is a per-slice reconstruction parameter that
    // can differ from the true interval when slices overlap or are gapped.
    let spacing_z = detail.spacing_mm.unwrap_or(1.0);

    let slice_voxels = dim_x as usize * dim_y as usize;
    let mut data = vec![0i16; slice_voxels * dim_z as usize];

    data.par_chunks_mut(slice_voxels)
        .zip(paths.par_iter())
        .enumerate()
        .try_for_each(|(ordinal, (chunk, path))| -> anyhow::Result<()> {
            let decoded = decode_slice(path).map_err(|e| {
                anyhow::anyhow!(
                    "failed to decode slice ordinal {ordinal} ({}): {e}",
                    path.display()
                )
            })?;
            if decoded.data.len() != slice_voxels {
                anyhow::bail!(
                    "slice ordinal {ordinal} ({}) decoded to {} voxels, expected {slice_voxels} ({dim_x}x{dim_y})",
                    path.display(),
                    decoded.data.len()
                );
            }
            chunk.copy_from_slice(&decoded.data);
            Ok(())
        })?;

    Ok(Volume {
        dim_x,
        dim_y,
        dim_z,
        spacing_x,
        spacing_y,
        spacing_z,
        hu_calibrated: detail.hu_calibrated,
        data,
    })
}

/// Newest mtime across a series' source files, used together with the
/// slice count as the disk cache's staleness fingerprint: if a study is
/// re-scanned with different or updated files, either value changes and
/// every existing cache entry for that series is treated as a miss rather
/// than served. Getting this wrong would mean serving one patient's cached
/// volume under another patient's series UID after a re-index — the worst
/// possible bug for this program, so this is checked, not assumed.
fn newest_source_mtime(paths: &[PathBuf]) -> anyhow::Result<SystemTime> {
    let mut newest = SystemTime::UNIX_EPOCH;
    for path in paths {
        let modified = std::fs::metadata(path)?.modified()?;
        if modified > newest {
            newest = modified;
        }
    }
    Ok(newest)
}

type CacheKey = (String, u32);

struct CacheInner {
    map: HashMap<CacheKey, Arc<Volume>>,
    order: VecDeque<CacheKey>,
}

/// In-memory cache of assembled volumes, keyed by `(series_uid, level)`,
/// bounded to a small number of entries. Eviction is oldest-inserted-first,
/// which is enough for a milestone that isn't a disk cache.
pub struct VolumeCache {
    inner: Mutex<CacheInner>,
}

impl VolumeCache {
    pub fn new() -> Self {
        VolumeCache {
            inner: Mutex::new(CacheInner {
                map: HashMap::new(),
                order: VecDeque::new(),
            }),
        }
    }

    fn get(&self, key: &CacheKey) -> Option<Arc<Volume>> {
        self.inner.lock().unwrap().map.get(key).cloned()
    }

    fn insert(&self, key: CacheKey, volume: Arc<Volume>) {
        let mut inner = self.inner.lock().unwrap();
        if !inner.map.contains_key(&key) {
            inner.order.push_back(key.clone());
            while inner.order.len() > CACHE_BOUND {
                if let Some(oldest) = inner.order.pop_front() {
                    inner.map.remove(&oldest);
                }
            }
        }
        inner.map.insert(key, volume);
    }
}

impl Default for VolumeCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Where a served volume came from, for logging. Ordered cheapest-first:
/// a request either lands in the in-memory cache, then the on-disk cache,
/// and only then pays for a real rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheSource {
    Memory,
    Disk,
    Rebuilt,
}

impl std::fmt::Display for CacheSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            CacheSource::Memory => "memory-hit",
            CacheSource::Disk => "disk-hit",
            CacheSource::Rebuilt => "miss",
        })
    }
}

/// Returns the requested volume plus where it came from. `Ok(None)` means
/// the series is unknown.
///
/// Fetches the series' detail/paths/staleness-fingerprint once, then
/// delegates to `fetch_level`, which recurses down through the pyramid
/// (each level built from the one below rather than always from level 0 —
/// see the comment there for why that's safe).
pub fn fetch(
    index: &SharedIndex,
    cache: &VolumeCache,
    disk_cache: &DiskCache,
    uid: &str,
    level: u32,
) -> anyhow::Result<Option<(Arc<Volume>, CacheSource)>> {
    let key = (uid.to_string(), level);
    if let Some(v) = cache.get(&key) {
        return Ok(Some((v, CacheSource::Memory)));
    }

    let detail = index.lock().unwrap().get_series(uid)?;
    let Some(detail) = detail else {
        return Ok(None);
    };

    // Needed regardless of hit/miss: the disk cache can't be trusted
    // without checking it against the *current* state of the source files.
    let paths = index.lock().unwrap().slice_paths(uid)?;
    let source_slice_count = detail.slice_count;
    let source_mtime = newest_source_mtime(&paths)?;

    fetch_level(
        index,
        cache,
        disk_cache,
        uid,
        level,
        &detail,
        &paths,
        source_slice_count,
        source_mtime,
    )
    .map(Some)
}

/// Builds or fetches one pyramid level, given the series' already-resolved
/// detail/paths/staleness fingerprint (computed once by `fetch`, reused
/// across the whole recursion instead of re-querying the index at every
/// level).
///
/// Lookup order per level: in-memory cache -> on-disk cache -> build. To
/// build level N (N >= 1), this recurses for level N-1 through the same
/// order rather than always downsampling from level 0 by 2^N in one step.
///
/// Repeated factor-2 box-averaging is not bit-identical to one wider box
/// average, so this was verified before being wired in: on `data/big`
/// (1026 slices), level 2 built by cascading through level 1 differs from
/// level 2 built directly from level 0 by at most 1 HU (mean 0.09 HU),
/// against a working range in the thousands — see
/// `level2_cascade_error_vs_direct_from_level0` in tests/volume_test.rs.
/// That's indistinguishable for rendering, and cascading does roughly 8x
/// less work for level 2 (and more for level 3), so every level here
/// cascades. Level 3's cascade error was not separately measured — it's
/// one more application of the same rounding mechanism, so it's expected
/// to stay in the same single-digit-HU range, but that's an expectation,
/// not a measurement.
// `index` is only threaded through to the recursive call, not read directly
// at this level — kept as a real parameter (not `_index`) because it's part
// of the same resolved-context bundle as `detail`/`paths`/`source_*` that
// every level of the recursion carries, and `_index` at the call site would
// read as unused rather than as "passed on".
#[allow(clippy::too_many_arguments, clippy::only_used_in_recursion)]
fn fetch_level(
    index: &SharedIndex,
    cache: &VolumeCache,
    disk_cache: &DiskCache,
    uid: &str,
    level: u32,
    detail: &SeriesDetail,
    paths: &[PathBuf],
    source_slice_count: u32,
    source_mtime: SystemTime,
) -> anyhow::Result<(Arc<Volume>, CacheSource)> {
    let key = (uid.to_string(), level);
    if let Some(v) = cache.get(&key) {
        return Ok((v, CacheSource::Memory));
    }
    if let Some(vol) = disk_cache.get(uid, level, source_slice_count, source_mtime) {
        let vol = Arc::new(vol);
        cache.insert(key, vol.clone());
        return Ok((vol, CacheSource::Disk));
    }

    let volume = if level == 0 {
        Arc::new(assemble_level0(detail, paths)?)
    } else {
        let (lower, _) = fetch_level(
            index,
            cache,
            disk_cache,
            uid,
            level - 1,
            detail,
            paths,
            source_slice_count,
            source_mtime,
        )?;
        let (data, dim_x, dim_y, dim_z) =
            downsample(&lower.data, lower.dim_x, lower.dim_y, lower.dim_z, 2);
        Arc::new(Volume {
            dim_x,
            dim_y,
            dim_z,
            spacing_x: lower.spacing_x * 2.0,
            spacing_y: lower.spacing_y * 2.0,
            spacing_z: lower.spacing_z * 2.0,
            hu_calibrated: lower.hu_calibrated,
            data,
        })
    };

    maybe_persist_to_disk(
        disk_cache,
        uid,
        level,
        &volume,
        source_slice_count,
        source_mtime,
    );
    cache.insert(key, volume.clone());
    Ok((volume, CacheSource::Rebuilt))
}

/// Persists `volume` to `disk_cache`, unless serving it would exceed
/// `MAX_OUTPUT_BYTES` — a level that can never leave this process as a
/// response body (level 0 of a large study, e.g. 513MB for `data/big`) is
/// only ever an intermediate used to cascade down to a lower, servable
/// level, so writing it to disk buys nothing and just burns space (on
/// `data/big` this was the single largest cache entry: 513MB, more than the
/// rest of the whole pyramid combined). It stays reachable in memory for
/// the rest of this recursion via `cache.insert` in the caller either way.
///
/// A failed cache write must not fail the request — the client still gets
/// its volume, just without the disk-cache speedup next time. Likely causes
/// are a full disk or a permissions issue, both operational, not a reason
/// to fail an otherwise-successful decode.
fn maybe_persist_to_disk(
    disk_cache: &DiskCache,
    uid: &str,
    level: u32,
    volume: &Volume,
    source_slice_count: u32,
    source_mtime: SystemTime,
) {
    let bytes = output_bytes(volume.dim_x, volume.dim_y, volume.dim_z);
    if bytes > MAX_OUTPUT_BYTES {
        eprintln!(
            "disk cache: not persisting {uid} level {level} ({bytes} bytes exceeds the {MAX_OUTPUT_BYTES}-byte serving limit; kept in memory only)"
        );
        return;
    }
    if let Err(e) = disk_cache.put(uid, level, volume, source_slice_count, source_mtime) {
        eprintln!("disk cache: failed to persist {uid} level {level}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// A fabricated over-limit volume never reaches `DiskCache::put` at
    /// all — verified by checking no cache entry exists afterward, since a
    /// bug that skipped the guard would still leave a real file on disk.
    /// `data` deliberately doesn't match `dim_x*dim_y*dim_z` (way fewer
    /// elements): the guard only inspects the declared dimensions, so this
    /// avoids actually allocating an 800MB+ buffer just to trip it, and if
    /// the guard were ever bypassed the mismatched data would still produce
    /// a real (if wrong) file, so the "no file" assertion stays meaningful.
    #[test]
    fn unservable_levels_are_not_written_to_disk() {
        let dir = tempdir().unwrap();
        let disk_cache = DiskCache::new(
            dir.path().to_path_buf(),
            crate::disk_cache::DEFAULT_MAX_CACHE_BYTES,
        )
        .unwrap();
        let mtime = SystemTime::now();

        let huge = Volume {
            dim_x: 650,
            dim_y: 650,
            dim_z: 650, // 650^3 * 2 bytes ~= 524MB, just over the 512MB limit
            spacing_x: 1.0,
            spacing_y: 1.0,
            spacing_z: 1.0,
            hu_calibrated: true,
            data: vec![0i16; 8],
        };
        assert!(
            output_bytes(huge.dim_x, huge.dim_y, huge.dim_z) > MAX_OUTPUT_BYTES,
            "test fixture must actually exceed the serving limit"
        );

        maybe_persist_to_disk(&disk_cache, "SERIES1", 0, &huge, 650, mtime);

        assert!(
            disk_cache.get("SERIES1", 0, 650, mtime).is_none(),
            "an unservable level must leave no disk cache entry"
        );
        assert_eq!(disk_cache.entry_count().unwrap(), 0);
    }

    #[test]
    fn servable_levels_are_written_to_disk() {
        let dir = tempdir().unwrap();
        let disk_cache = DiskCache::new(
            dir.path().to_path_buf(),
            crate::disk_cache::DEFAULT_MAX_CACHE_BYTES,
        )
        .unwrap();
        let mtime = SystemTime::now();

        let small = Volume {
            dim_x: 2,
            dim_y: 2,
            dim_z: 2,
            spacing_x: 1.0,
            spacing_y: 1.0,
            spacing_z: 1.0,
            hu_calibrated: true,
            data: vec![0i16; 8],
        };

        maybe_persist_to_disk(&disk_cache, "SERIES1", 1, &small, 8, mtime);

        assert!(
            disk_cache.get("SERIES1", 1, 8, mtime).is_some(),
            "a servable level must be persisted"
        );
    }
}
