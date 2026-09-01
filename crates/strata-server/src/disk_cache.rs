//! On-disk pyramid cache: persists assembled `Volume`s so the cost of
//! decoding a whole series is paid once per study, not once per process.
//!
//! Format is a fixed-size hand-rolled header followed by raw little-endian
//! `i16` samples. No serde_cbor/bincode: the shape is simple and stable
//! enough that a hand-rolled reader/writer is less risk than a new
//! dependency plus its own versioning story.

use std::fs::{self, File};
use std::io::{self, BufWriter, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::volume::Volume;

const MAGIC: [u8; 4] = *b"SVC1";

/// Default `--max-cache-bytes`. 2 GiB comfortably fits several large
/// studies' *servable* pyramids (level 0 is never persisted, so a study
/// like `data/big`, 1026 slices, only costs ~65 MB: levels 1-3) while still
/// bounding disk use for a self-hosted tool pointed at a folder of many
/// studies, rather than growing without limit.
pub const DEFAULT_MAX_CACHE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// The on-disk filename suffix for a cache entry, used both to build paths
/// and to filter directory listings down to entries (as opposed to the
/// `.tmp-<pid>` files `put` briefly creates mid-write).
const ENTRY_SUFFIX: &str = ".svc";

/// Fixed byte layout, in field order: magic, dims, spacing, hu_calibrated,
/// hu_min/max, source_slice_count, source_mtime (secs+nanos), data_len.
/// Bumping the format means changing `MAGIC`, which makes every existing
/// cache file fail the magic check and get rebuilt rather than misread.
const HEADER_LEN: usize = 4 // magic
    + 4 + 4 + 4 // dim_x, dim_y, dim_z
    + 8 + 8 + 8 // spacing_x, spacing_y, spacing_z
    + 1 // hu_calibrated
    + 2 + 2 // hu_min, hu_max
    + 4 // source_slice_count
    + 8 + 4 // source_mtime_secs, source_mtime_nanos
    + 8; // data_len (i16 sample count)

struct Header {
    dim_x: u32,
    dim_y: u32,
    dim_z: u32,
    spacing_x: f64,
    spacing_y: f64,
    spacing_z: f64,
    hu_calibrated: bool,
    source_slice_count: u32,
    source_mtime: (u64, u32),
    data_len: u64,
}

fn mtime_to_parts(t: SystemTime) -> (u64, u32) {
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => (d.as_secs(), d.subsec_nanos()),
        Err(_) => (0, 0),
    }
}

/// Reads a `Header` out of exactly `HEADER_LEN` bytes. Returns `None` on a
/// bad magic number rather than an `Err`, since a bad magic is a normal
/// "not our cache format" outcome (stale format, foreign file, garbage) and
/// callers treat it as a plain cache miss, not a hard failure.
fn decode_header(buf: &[u8]) -> Option<Header> {
    if buf.len() != HEADER_LEN || buf[0..4] != MAGIC {
        return None;
    }
    let mut off = 4;
    let mut take = |n: usize| {
        let s = &buf[off..off + n];
        off += n;
        s
    };
    let dim_x = u32::from_le_bytes(take(4).try_into().unwrap());
    let dim_y = u32::from_le_bytes(take(4).try_into().unwrap());
    let dim_z = u32::from_le_bytes(take(4).try_into().unwrap());
    let spacing_x = f64::from_le_bytes(take(8).try_into().unwrap());
    let spacing_y = f64::from_le_bytes(take(8).try_into().unwrap());
    let spacing_z = f64::from_le_bytes(take(8).try_into().unwrap());
    let hu_calibrated = take(1)[0] != 0;
    let _hu_min = i16::from_le_bytes(take(2).try_into().unwrap());
    let _hu_max = i16::from_le_bytes(take(2).try_into().unwrap());
    let source_slice_count = u32::from_le_bytes(take(4).try_into().unwrap());
    let mtime_secs = u64::from_le_bytes(take(8).try_into().unwrap());
    let mtime_nanos = u32::from_le_bytes(take(4).try_into().unwrap());
    let data_len = u64::from_le_bytes(take(8).try_into().unwrap());
    Some(Header {
        dim_x,
        dim_y,
        dim_z,
        spacing_x,
        spacing_y,
        spacing_z,
        hu_calibrated,
        source_slice_count,
        source_mtime: (mtime_secs, mtime_nanos),
        data_len,
    })
}

/// Persists assembled pyramid levels next to the SQLite index (or wherever
/// `--cache-dir` points), keyed on `(series_uid, level)`, bounded to
/// `max_bytes` total by evicting least-recently-written entries.
pub struct DiskCache {
    dir: PathBuf,
    max_bytes: u64,
}

/// One `.svc` file's path, size, and mtime — everything eviction needs to
/// pick a victim without a separate index.
struct Entry {
    path: PathBuf,
    bytes: u64,
    mtime: SystemTime,
}

impl DiskCache {
    pub fn new(dir: PathBuf, max_bytes: u64) -> io::Result<Self> {
        fs::create_dir_all(&dir)?;
        Ok(DiskCache { dir, max_bytes })
    }

    /// Lists cache entries by scanning the directory fresh each time rather
    /// than maintaining an index — this cache is checked at most once per
    /// write, not on the hot read path, so the scan cost is fine.
    fn entries(&self) -> io::Result<Vec<Entry>> {
        let mut out = Vec::new();
        for dirent in fs::read_dir(&self.dir)? {
            let dirent = dirent?;
            let path = dirent.path();
            let is_entry = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(ENTRY_SUFFIX));
            if !is_entry {
                continue; // skip .tmp-<pid> write-in-progress files and anything foreign
            }
            let meta = dirent.metadata()?;
            if !meta.is_file() {
                continue;
            }
            out.push(Entry {
                path,
                bytes: meta.len(),
                mtime: meta.modified()?,
            });
        }
        Ok(out)
    }

    /// Total bytes currently occupied by cache entries, for `/api/health`.
    pub fn total_bytes(&self) -> io::Result<u64> {
        Ok(self.entries()?.iter().map(|e| e.bytes).sum())
    }

    /// Number of cache entries currently on disk, for `/api/health`.
    pub fn entry_count(&self) -> io::Result<usize> {
        Ok(self.entries()?.len())
    }

    /// The configured `--max-cache-bytes` budget, for `/api/health`.
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    /// Evicts least-recently-written entries (oldest mtime first) until
    /// `incoming_bytes` more would fit under `max_bytes`, or there is
    /// nothing left to evict. `keep` is excluded from eviction candidates:
    /// it is the entry about to be written and, on a replace, its old file
    /// is already accounted for by being overwritten via rename rather than
    /// needing to be evicted first.
    ///
    /// A file that fails to delete (e.g. still open for reading — on
    /// Windows an open handle blocks `DeleteFile` outright, which is the
    /// conservative behaviour we want) is logged and left in place rather
    /// than retried or treated as an error: a reader must never have its
    /// file pulled out from under it, so staying briefly over budget is
    /// preferred over forcing the delete.
    fn make_room_for(&self, incoming_bytes: u64, keep: &Path) -> io::Result<()> {
        let mut entries = self.entries()?;
        entries.retain(|e| e.path != keep);
        entries.sort_by_key(|e| e.mtime);

        let mut total: u64 = entries.iter().map(|e| e.bytes).sum();
        for entry in &entries {
            if total + incoming_bytes <= self.max_bytes {
                break;
            }
            match fs::remove_file(&entry.path) {
                Ok(()) => total -= entry.bytes,
                Err(e) => eprintln!(
                    "disk cache: could not evict {} to make room ({e}); leaving it in place",
                    entry.path.display()
                ),
            }
        }
        Ok(())
    }

    fn path_for(&self, series_uid: &str, level: u32) -> PathBuf {
        // Series UIDs are DICOM UIDs (digits and dots), always filename-safe,
        // but sanitise defensively rather than trust an external format.
        let safe: String = series_uid
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.dir.join(format!("{safe}.L{level}.svc"))
    }

    /// Returns a cached volume only if it exists, isn't truncated/corrupt,
    /// and matches the caller's expected source fingerprint. Every failure
    /// mode collapses to `None` (a plain miss) — the caller always has a
    /// working fallback (rebuild), so a corrupt or stale cache file must
    /// never become a hard error or, worse, get served as if valid.
    pub fn get(
        &self,
        series_uid: &str,
        level: u32,
        expected_slice_count: u32,
        expected_mtime: SystemTime,
    ) -> Option<Volume> {
        match self.try_get(series_uid, level, expected_slice_count, expected_mtime) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("disk cache: treating {series_uid} level {level} as a miss ({e})");
                None
            }
        }
    }

    fn try_get(
        &self,
        series_uid: &str,
        level: u32,
        expected_slice_count: u32,
        expected_mtime: SystemTime,
    ) -> io::Result<Option<Volume>> {
        let path = self.path_for(series_uid, level);
        let mut file = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };

        let file_len = file.metadata()?.len();
        if file_len < HEADER_LEN as u64 {
            return Ok(None); // truncated before even a full header
        }

        let mut header_buf = [0u8; HEADER_LEN];
        file.read_exact(&mut header_buf)?;
        let Some(header) = decode_header(&header_buf) else {
            return Ok(None); // bad magic
        };

        let expected_total = HEADER_LEN as u64 + header.data_len * 2;
        if file_len != expected_total {
            return Ok(None); // truncated or padded — never trust a length mismatch
        }

        let expected_mtime_parts = mtime_to_parts(expected_mtime);
        if header.source_slice_count != expected_slice_count
            || header.source_mtime != expected_mtime_parts
        {
            return Ok(None); // source changed since this entry was written — stale
        }

        // No zero-copy path here: `Volume::data` is `Vec<i16>` and the file
        // is little-endian bytes, so converting means one pass either way.
        // We read the whole payload into memory in one `read_exact` (rather
        // than looping) and decode it host-endian in a second pass; the
        // tradeoff is a transient extra `data_len * 2` byte buffer, which is
        // the same order of magnitude as the `Vec<i16>` we're about to hand
        // back anyway.
        let mut raw = vec![0u8; (header.data_len * 2) as usize];
        file.read_exact(&mut raw)?;
        let data: Vec<i16> = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b))
            .collect();

        Ok(Some(Volume {
            dim_x: header.dim_x,
            dim_y: header.dim_y,
            dim_z: header.dim_z,
            spacing_x: header.spacing_x,
            spacing_y: header.spacing_y,
            spacing_z: header.spacing_z,
            hu_calibrated: header.hu_calibrated,
            data,
        }))
    }

    /// Writes to a temp file and renames into place, so a reader can never
    /// observe a half-written cache entry (a crash or concurrent request
    /// mid-write leaves the old file, or no file, but never a torn one).
    pub fn put(
        &self,
        series_uid: &str,
        level: u32,
        volume: &Volume,
        source_slice_count: u32,
        source_mtime: SystemTime,
    ) -> io::Result<()> {
        let path = self.path_for(series_uid, level);
        let tmp_path = self.dir.join(format!(
            "{}.tmp-{}",
            path.file_name().unwrap().to_string_lossy(),
            std::process::id()
        ));

        let (hu_min, hu_max) = volume.hu_min_max();
        let (mtime_secs, mtime_nanos) = mtime_to_parts(source_mtime);

        {
            let mut w = BufWriter::new(File::create(&tmp_path)?);
            w.write_all(&MAGIC)?;
            w.write_all(&volume.dim_x.to_le_bytes())?;
            w.write_all(&volume.dim_y.to_le_bytes())?;
            w.write_all(&volume.dim_z.to_le_bytes())?;
            w.write_all(&volume.spacing_x.to_le_bytes())?;
            w.write_all(&volume.spacing_y.to_le_bytes())?;
            w.write_all(&volume.spacing_z.to_le_bytes())?;
            w.write_all(&[volume.hu_calibrated as u8])?;
            w.write_all(&hu_min.to_le_bytes())?;
            w.write_all(&hu_max.to_le_bytes())?;
            w.write_all(&source_slice_count.to_le_bytes())?;
            w.write_all(&mtime_secs.to_le_bytes())?;
            w.write_all(&mtime_nanos.to_le_bytes())?;
            w.write_all(&(volume.data.len() as u64).to_le_bytes())?;
            for v in &volume.data {
                w.write_all(&v.to_le_bytes())?;
            }
            w.flush()?;
        }

        let incoming_bytes = fs::metadata(&tmp_path)?.len();
        if incoming_bytes > self.max_bytes {
            // No amount of evicting other entries makes room for this one;
            // writing it anyway would either blow the budget outright or
            // immediately evict everything else in the cache for a single
            // entry. Skip persisting — the caller still gets its volume
            // from this call, just without a disk-cache speedup next time.
            eprintln!(
                "disk cache: not persisting {series_uid} level {level} ({incoming_bytes} bytes exceeds the {}-byte cache budget)",
                self.max_bytes
            );
            fs::remove_file(&tmp_path)?;
            return Ok(());
        }

        self.make_room_for(incoming_bytes, &path)?;
        fs::rename(&tmp_path, &path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::volume::Volume;
    use std::time::Duration;

    fn sample_volume() -> Volume {
        Volume {
            dim_x: 2,
            dim_y: 2,
            dim_z: 2,
            spacing_x: 0.5,
            spacing_y: 0.5,
            spacing_z: 1.0,
            hu_calibrated: true,
            data: vec![-1024, -500, 0, 1000, 42, -42, i16::MIN, i16::MAX],
        }
    }

    fn cache_in_tempdir() -> (tempfile::TempDir, DiskCache) {
        cache_in_tempdir_with_budget(DEFAULT_MAX_CACHE_BYTES)
    }

    fn cache_in_tempdir_with_budget(max_bytes: u64) -> (tempfile::TempDir, DiskCache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCache::new(dir.path().to_path_buf(), max_bytes).unwrap();
        (dir, cache)
    }

    #[test]
    fn disk_cache_round_trips() {
        let (_dir, cache) = cache_in_tempdir();
        let vol = sample_volume();
        let mtime = SystemTime::now();

        cache.put("SERIES1", 1, &vol, 8, mtime).unwrap();
        let got = cache
            .get("SERIES1", 1, 8, mtime)
            .expect("must hit after put");

        assert_eq!(got.dim_x, vol.dim_x);
        assert_eq!(got.dim_y, vol.dim_y);
        assert_eq!(got.dim_z, vol.dim_z);
        assert_eq!(got.spacing_x, vol.spacing_x);
        assert_eq!(got.spacing_y, vol.spacing_y);
        assert_eq!(got.spacing_z, vol.spacing_z);
        assert_eq!(got.hu_calibrated, vol.hu_calibrated);
        assert_eq!(got.data, vol.data);
    }

    #[test]
    fn disk_cache_detects_stale_source() {
        let (_dir, cache) = cache_in_tempdir();
        let vol = sample_volume();
        let mtime = SystemTime::now();
        cache.put("SERIES1", 0, &vol, 10, mtime).unwrap();

        // Slice count changed (a re-scan added/removed a file).
        assert!(cache.get("SERIES1", 0, 11, mtime).is_none());

        // mtime changed (a file was replaced without changing the count).
        let later = mtime + Duration::from_secs(60);
        assert!(cache.get("SERIES1", 0, 10, later).is_none());

        // Unchanged fingerprint still hits.
        assert!(cache.get("SERIES1", 0, 10, mtime).is_some());
    }

    #[test]
    fn disk_cache_rejects_truncated_file() {
        let (_dir, cache) = cache_in_tempdir();
        let vol = sample_volume();
        let mtime = SystemTime::now();
        cache.put("SERIES1", 0, &vol, 8, mtime).unwrap();

        let path = cache.path_for("SERIES1", 0);
        let full = fs::read(&path).unwrap();
        fs::write(&path, &full[..full.len() - 3]).unwrap(); // chop off part of the payload

        assert!(cache.get("SERIES1", 0, 8, mtime).is_none());
    }

    #[test]
    fn disk_cache_rejects_bad_magic() {
        let (_dir, cache) = cache_in_tempdir();
        let vol = sample_volume();
        let mtime = SystemTime::now();
        cache.put("SERIES1", 0, &vol, 8, mtime).unwrap();

        let path = cache.path_for("SERIES1", 0);
        let mut full = fs::read(&path).unwrap();
        full[0..4].copy_from_slice(b"NOPE");
        fs::write(&path, &full).unwrap();

        assert!(cache.get("SERIES1", 0, 8, mtime).is_none());
    }

    #[test]
    fn cache_evicts_least_recently_used_when_over_budget() {
        let vol = sample_volume();
        let entry_bytes = HEADER_LEN as u64 + vol.data.len() as u64 * 2;
        // Room for exactly two entries; a third must evict the oldest.
        let (_dir, cache) = cache_in_tempdir_with_budget(entry_bytes * 2);
        let mtime = SystemTime::now();

        cache.put("A", 0, &vol, 8, mtime).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        cache.put("B", 0, &vol, 8, mtime).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        cache.put("C", 0, &vol, 8, mtime).unwrap();

        assert!(
            cache.get("A", 0, 8, mtime).is_none(),
            "oldest entry (A) must be evicted to make room for C"
        );
        assert!(
            cache.get("B", 0, 8, mtime).is_some(),
            "B is newer than A and must survive"
        );
        assert!(
            cache.get("C", 0, 8, mtime).is_some(),
            "just-written entry C must survive"
        );
        assert_eq!(cache.entry_count().unwrap(), 2);
        assert!(cache.total_bytes().unwrap() <= entry_bytes * 2);
    }

    #[test]
    fn eviction_never_touches_the_entry_being_written() {
        // Re-putting the same key (a re-scan overwriting a series' own
        // cached level) must not evict itself trying to make room for
        // itself — `keep` in `make_room_for` exists for exactly this.
        let vol = sample_volume();
        let entry_bytes = HEADER_LEN as u64 + vol.data.len() as u64 * 2;
        let (_dir, cache) = cache_in_tempdir_with_budget(entry_bytes);
        let mtime = SystemTime::now();

        cache.put("SERIES1", 0, &vol, 8, mtime).unwrap();
        cache.put("SERIES1", 0, &vol, 8, mtime).unwrap();

        assert!(cache.get("SERIES1", 0, 8, mtime).is_some());
        assert_eq!(cache.entry_count().unwrap(), 1);
    }

    #[test]
    fn total_bytes_and_entry_count_reflect_disk_state() {
        let (_dir, cache) = cache_in_tempdir();
        assert_eq!(cache.entry_count().unwrap(), 0);
        assert_eq!(cache.total_bytes().unwrap(), 0);

        let vol = sample_volume();
        let mtime = SystemTime::now();
        cache.put("SERIES1", 0, &vol, 8, mtime).unwrap();
        cache.put("SERIES1", 1, &vol, 8, mtime).unwrap();

        assert_eq!(cache.entry_count().unwrap(), 2);
        let entry_bytes = HEADER_LEN as u64 + vol.data.len() as u64 * 2;
        assert_eq!(cache.total_bytes().unwrap(), entry_bytes * 2);
    }

    #[test]
    fn an_entry_larger_than_the_whole_budget_is_not_written() {
        let vol = sample_volume();
        let entry_bytes = HEADER_LEN as u64 + vol.data.len() as u64 * 2;
        let (_dir, cache) = cache_in_tempdir_with_budget(entry_bytes - 1);
        let mtime = SystemTime::now();

        cache.put("SERIES1", 0, &vol, 8, mtime).unwrap();

        assert!(cache.get("SERIES1", 0, 8, mtime).is_none());
        assert_eq!(cache.entry_count().unwrap(), 0);
    }
}
