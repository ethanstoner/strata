use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

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
#[derive(Debug, Clone)]
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
    let ceil_div = |d: u32| (d + factor - 1) / factor;
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

    let (dim_x, dim_y, dim_z, factor) = (dim_x as usize, dim_y as usize, dim_z as usize, factor as usize);
    let (new_x, new_y, new_z) = (
        (dim_x + factor - 1) / factor,
        (dim_y + factor - 1) / factor,
        (dim_z + factor - 1) / factor,
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
/// volume. `Ok(None)` means the series is unknown, mirroring every other
/// handler's 404-not-500 convention.
fn assemble_level0(index: &SharedIndex, uid: &str) -> anyhow::Result<Option<Volume>> {
    let detail = index.lock().unwrap().get_series(uid)?;
    let Some(detail) = detail else {
        return Ok(None);
    };

    let dim_x = detail.cols as u32;
    let dim_y = detail.rows as u32;
    let dim_z = detail.slice_count;

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

    let mut data = Vec::with_capacity(dim_x as usize * dim_y as usize * dim_z as usize);
    for ordinal in 0..dim_z {
        let path = index.lock().unwrap().slice_path(uid, ordinal)?;
        let Some(path) = path else {
            anyhow::bail!("series {uid} is missing slice ordinal {ordinal}");
        };
        let decoded = decode_slice(&path)?;
        data.extend_from_slice(&decoded.data);
    }

    Ok(Some(Volume {
        dim_x,
        dim_y,
        dim_z,
        spacing_x,
        spacing_y,
        spacing_z,
        hu_calibrated: detail.hu_calibrated,
        data,
    }))
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

/// Returns the requested volume plus whether it was already cached.
/// `Ok(None)` means the series is unknown. Level 0 is always assembled (or
/// fetched from cache) first; higher levels downsample from it, so a
/// level-0 cache hit is shared across every level of the same series.
pub fn fetch(
    index: &SharedIndex,
    cache: &VolumeCache,
    uid: &str,
    level: u32,
) -> anyhow::Result<Option<(Arc<Volume>, bool)>> {
    let key = (uid.to_string(), level);
    if let Some(v) = cache.get(&key) {
        return Ok(Some((v, true)));
    }

    let level0_key = (uid.to_string(), 0);
    let level0 = match cache.get(&level0_key) {
        Some(v) => v,
        None => {
            let Some(v0) = assemble_level0(index, uid)? else {
                return Ok(None);
            };
            let v0 = Arc::new(v0);
            cache.insert(level0_key, v0.clone());
            v0
        }
    };

    let volume = if level == 0 {
        level0
    } else {
        let factor = 1u32 << level;
        let (data, dim_x, dim_y, dim_z) =
            downsample(&level0.data, level0.dim_x, level0.dim_y, level0.dim_z, factor);
        Arc::new(Volume {
            dim_x,
            dim_y,
            dim_z,
            spacing_x: level0.spacing_x * factor as f64,
            spacing_y: level0.spacing_y * factor as f64,
            spacing_z: level0.spacing_z * factor as f64,
            hu_calibrated: level0.hu_calibrated,
            data,
        })
    };

    cache.insert(key, volume.clone());
    Ok(Some((volume, false)))
}
