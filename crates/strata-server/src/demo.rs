//! `strata --demo`: fetch a small public CT series once, verify it, cache it.
//!
//! Source: one 60-slice chest CT from the TCGA-LUAD collection on The Cancer
//! Imaging Archive (TCIA), downloaded through TCIA's public NBIA REST API. No
//! account is needed. The collection is released under the Creative Commons
//! Attribution 3.0 Unported license (CC BY 3.0,
//! <https://creativecommons.org/licenses/by/3.0/>), as reported by TCIA's own
//! `getSeriesMetaData` endpoint and the LICENSE file inside every download.
//!
//! Citation: Albertina, B., et al. (2016). The Cancer Genome Atlas Lung
//! Adenocarcinoma Collection (TCGA-LUAD) (Version 4) [Data set]. The Cancer
//! Imaging Archive. <https://doi.org/10.7937/K9/TCIA.2016.JGNIHEP5>
//!
//! TCIA's data usage policy asks tools that give access to TCIA data through
//! the REST API to attribute it and link back to the policy, which is what
//! [`DemoSeries::attribution`] prints on every run.
//!
//! The zip TCIA returns is not byte-stable (entry timestamps are set at
//! download time), so the checksum covers the *contents*: SHA-256 over the
//! sorted lines `"<file name> <sha256 of file>\n"` for every `.dcm` file.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// Everything strata needs to know to fetch and trust one demo series.
#[derive(Debug, Clone)]
pub struct DemoSeries {
    /// Folder name under the demo cache directory.
    pub slug: &'static str,
    pub collection: &'static str,
    pub description: &'static str,
    pub series_uid: &'static str,
    pub url: &'static str,
    /// Number of `.dcm` files the series must contain.
    pub file_count: usize,
    /// Content digest (see module docs), lowercase hex.
    pub content_sha256: &'static str,
    /// Approximate zip size, used for the progress bar because TCIA streams
    /// the zip without a Content-Length.
    pub approx_download_bytes: u64,
    pub license: &'static str,
    pub license_url: &'static str,
    pub citation_doi: &'static str,
    pub usage_policy_url: &'static str,
}

impl DemoSeries {
    pub fn attribution(&self) -> String {
        format!(
            "Demo data: {} \"{}\" from The Cancer Imaging Archive (TCIA)\n\
             License: {} ({})\n\
             Citation: {}\n\
             TCIA data usage policy: {}",
            self.collection,
            self.description,
            self.license,
            self.license_url,
            self.citation_doi,
            self.usage_policy_url
        )
    }
}

/// The default demo: 60-slice chest CT, about 17 MB to download, 32 MB on disk.
pub const TCGA_LUAD_CHEST_CT: DemoSeries = DemoSeries {
    slug: "tcga-luad-chest-ct",
    collection: "TCGA-LUAD",
    description: "Chest Routine 1 (60 slices)",
    series_uid: "1.3.6.1.4.1.14519.5.2.1.7777.9002.288863784292986419246212301446",
    url: "https://services.cancerimagingarchive.net/nbia-api/services/v1/getImage?SeriesInstanceUID=1.3.6.1.4.1.14519.5.2.1.7777.9002.288863784292986419246212301446",
    file_count: 60,
    content_sha256: "6fafeda3036d1cda42a564d0f25287fd8586ca0e9faab52ac8c6aed01a76087d",
    approx_download_bytes: 17_384_008,
    license: "Creative Commons Attribution 3.0 Unported (CC BY 3.0)",
    license_url: "https://creativecommons.org/licenses/by/3.0/",
    citation_doi: "https://doi.org/10.7937/K9/TCIA.2016.JGNIHEP5",
    usage_policy_url: "https://www.cancerimagingarchive.net/data-usage-policies-and-restrictions/",
};

#[derive(Debug)]
pub enum DemoError {
    /// Could not reach the server at all (offline, DNS, firewall, timeout).
    Network {
        url: String,
        detail: String,
    },
    /// The server answered, but not with the data we asked for.
    BadResponse {
        url: String,
        detail: String,
    },
    /// The data arrived but does not match the pinned checksum.
    Checksum {
        expected: String,
        actual: String,
        files: usize,
    },
    Io(io::Error),
}

impl fmt::Display for DemoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DemoError::Network { url, detail } => write!(
                f,
                "could not download the demo study from {url}\n  ({detail})\n\
                 strata needs internet access once to fetch it; after that it is cached and works \
                 offline.\n\
                 Check your connection and try again, or open your own files with `strata <folder>`."
            ),
            DemoError::BadResponse { url, detail } => write!(
                f,
                "the demo download from {url} did not return the expected data ({detail}).\n\
                 The archive may be temporarily unavailable; try again later, or open your own \
                 files with `strata <folder>`."
            ),
            DemoError::Checksum {
                expected,
                actual,
                files,
            } => write!(
                f,
                "the downloaded demo study failed verification and was discarded.\n\
                 expected content checksum {expected}\n\
                 got                       {actual} ({files} DICOM files)\n\
                 This usually means the download was altered or the archive changed. Nothing \
                 unverified was kept."
            ),
            DemoError::Io(e) => write!(f, "file error while preparing the demo study: {e}"),
        }
    }
}

impl std::error::Error for DemoError {}

impl From<io::Error> for DemoError {
    fn from(e: io::Error) -> Self {
        DemoError::Io(e)
    }
}

/// Default location for demo data: `<OS cache dir>/strata/demo`
/// (`~/Library/Caches` on macOS, `$XDG_CACHE_HOME` or `~/.cache` on Linux,
/// `%LOCALAPPDATA%` on Windows).
pub fn default_demo_root() -> PathBuf {
    crate::default_cache_root().join("demo")
}

/// Returns a verified local copy of `series`, downloading it only if needed.
///
/// A cached copy is re-verified on every call (hashing ~32 MB takes tens of
/// milliseconds), so a corrupted or edited cache is re-fetched instead of
/// being displayed. `url` overrides `series.url` (used by tests and mirrors).
pub fn ensure_demo(
    series: &DemoSeries,
    root: &Path,
    url: Option<&str>,
) -> Result<PathBuf, DemoError> {
    let final_dir = root.join(series.slug);
    if final_dir.is_dir() {
        if let Ok((count, digest)) = content_digest(&final_dir) {
            if count == series.file_count && digest == series.content_sha256 {
                eprintln!("using cached demo study at {}", final_dir.display());
                return Ok(final_dir);
            }
        }
        eprintln!("cached demo study failed verification; downloading it again");
    }

    fs::create_dir_all(root)?;
    let url = url.unwrap_or(series.url);
    let zip_path = root.join(format!("{}.zip.part", series.slug));
    download(url, &zip_path, series.approx_download_bytes)?;

    let partial = root.join(format!("{}.partial", series.slug));
    let result = extract_and_verify(series, url, &zip_path, &partial);
    let _ = fs::remove_file(&zip_path);
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&partial);
        return Err(e);
    }

    if final_dir.exists() {
        fs::remove_dir_all(&final_dir)?;
    }
    fs::rename(&partial, &final_dir)?;
    eprintln!(
        "verified {} DICOM files (content sha256 {})",
        series.file_count,
        &series.content_sha256[..16]
    );
    Ok(final_dir)
}

fn extract_and_verify(
    series: &DemoSeries,
    url: &str,
    zip_path: &Path,
    dest: &Path,
) -> Result<(), DemoError> {
    let mut magic = [0u8; 4];
    File::open(zip_path)?
        .read_exact(&mut magic)
        .map_err(|_| DemoError::BadResponse {
            url: url.to_string(),
            detail: "empty response".into(),
        })?;
    if magic != *b"PK\x03\x04" {
        return Err(DemoError::BadResponse {
            url: url.to_string(),
            detail: "response is not a zip archive".into(),
        });
    }

    if dest.exists() {
        fs::remove_dir_all(dest)?;
    }
    fs::create_dir_all(dest)?;

    let mut archive =
        zip::ZipArchive::new(File::open(zip_path)?).map_err(|e| DemoError::BadResponse {
            url: url.to_string(),
            detail: format!("corrupt zip: {e}"),
        })?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| DemoError::BadResponse {
            url: url.to_string(),
            detail: format!("corrupt zip entry: {e}"),
        })?;
        if entry.is_dir() {
            continue;
        }
        // Only the bare file name is used, so a hostile archive can't write
        // outside `dest` (no "../" traversal), and only the files we expect.
        let Some(name) = Path::new(entry.name())
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_owned)
        else {
            continue;
        };
        let keep = name.to_ascii_lowercase().ends_with(".dcm") || name == "LICENSE";
        if !keep {
            continue;
        }
        let mut out = File::create(dest.join(&name))?;
        io::copy(&mut entry, &mut out)?;
    }

    let (count, digest) = content_digest(dest)?;
    if count != series.file_count || digest != series.content_sha256 {
        return Err(DemoError::Checksum {
            expected: series.content_sha256.to_string(),
            actual: digest,
            files: count,
        });
    }
    Ok(())
}

/// SHA-256 over the sorted lines `"<name> <sha256(file)>\n"` for every
/// `.dcm` file directly inside `dir`. Returns `(file_count, hex_digest)`.
pub fn content_digest(dir: &Path) -> io::Result<(usize, String)> {
    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type()?.is_file() && name.to_ascii_lowercase().ends_with(".dcm") {
            entries.push((name, entry.path()));
        }
    }
    entries.sort();
    let mut outer = Sha256::new();
    for (name, path) in &entries {
        let mut hasher = Sha256::new();
        io::copy(&mut File::open(path)?, &mut hasher)?;
        outer.update(format!("{name} {}\n", hex(&hasher.finalize())).as_bytes());
    }
    Ok((entries.len(), hex(&outer.finalize())))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn download(url: &str, dest: &Path, approx_bytes: u64) -> Result<(), DemoError> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(60))
        .user_agent(concat!("strata/", env!("CARGO_PKG_VERSION")))
        .build();
    eprintln!(
        "downloading the demo study (one time, about {:.0} MB)...",
        approx_bytes as f64 / 1e6
    );
    let response = agent.get(url).call().map_err(|e| match e {
        ureq::Error::Status(code, _) => DemoError::BadResponse {
            url: url.to_string(),
            detail: format!("HTTP {code}"),
        },
        ureq::Error::Transport(t) => DemoError::Network {
            url: url.to_string(),
            detail: t
                .to_string()
                .trim_start_matches(&format!("{url}: "))
                .to_string(),
        },
    })?;
    let total = response
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok());
    let mut reader = response.into_reader();
    let mut out = File::create(dest)?;
    let mut progress = Progress::new(total, approx_bytes);
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| DemoError::Network {
            url: url.to_string(),
            detail: format!("connection dropped mid-download: {e}"),
        })?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        progress.advance(n as u64);
    }
    out.flush()?;
    progress.finish();
    Ok(())
}

/// Download progress on stderr: a redrawn bar on a terminal, a few plain
/// lines otherwise (CI logs, pipes).
struct Progress {
    total: Option<u64>,
    approx: u64,
    done: u64,
    started: Instant,
    last_draw: Instant,
    tty: bool,
    next_line_pct: u64,
    drawn_len: usize,
}

impl Progress {
    fn new(total: Option<u64>, approx: u64) -> Self {
        let now = Instant::now();
        Progress {
            total,
            approx,
            done: 0,
            started: now,
            last_draw: now - Duration::from_secs(1),
            tty: io::stderr().is_terminal(),
            next_line_pct: 25,
            drawn_len: 0,
        }
    }

    fn advance(&mut self, n: u64) {
        self.done += n;
        let denom = self.total.unwrap_or(self.approx).max(1);
        let pct = (self.done * 100 / denom).min(99);
        if self.tty {
            if self.last_draw.elapsed() >= Duration::from_millis(100) {
                self.last_draw = Instant::now();
                let filled = (pct / 5) as usize;
                let prefix = if self.total.is_some() { "" } else { "~" };
                let line = format!(
                    "  [{}{}] {:>3}%  {:.1} / {prefix}{:.1} MB",
                    "#".repeat(filled),
                    "-".repeat(20 - filled),
                    pct,
                    self.done as f64 / 1e6,
                    denom as f64 / 1e6,
                );
                // Pad over any longer previous line instead of using ANSI
                // erase codes, which older Windows consoles print literally.
                let pad = self.drawn_len.saturating_sub(line.len());
                eprint!("\r{line}{}", " ".repeat(pad));
                self.drawn_len = line.len();
            }
        } else if pct >= self.next_line_pct {
            eprintln!("  {pct}% ({:.1} MB)", self.done as f64 / 1e6);
            self.next_line_pct += 25;
        }
    }

    fn finish(&self) {
        let secs = self.started.elapsed().as_secs_f64();
        if self.tty {
            eprint!("\r{}\r", " ".repeat(self.drawn_len));
        }
        eprintln!(
            "  downloaded {:.1} MB in {:.1}s",
            self.done as f64 / 1e6,
            secs
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    fn zip_bytes(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut cursor = io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut cursor);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, data) in files {
                w.start_file(*name, opts).unwrap();
                w.write_all(data).unwrap();
            }
            w.finish().unwrap();
        }
        cursor.into_inner()
    }

    /// Serves `body` once over plain HTTP (chunk-free, with no
    /// Content-Length, like TCIA) and returns the URL.
    fn serve_once(body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut req = [0u8; 2048];
                let _ = stream.read(&mut req);
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                );
                let _ = stream.write_all(&body);
            }
        });
        format!("http://{addr}/series.zip")
    }

    fn fixture_series(digest: &'static str) -> DemoSeries {
        DemoSeries {
            slug: "fixture",
            file_count: 2,
            content_sha256: digest,
            approx_download_bytes: 1000,
            ..TCGA_LUAD_CHEST_CT
        }
    }

    fn fixture_digest() -> String {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("b.dcm"), b"second").unwrap();
        fs::write(dir.path().join("a.dcm"), b"first").unwrap();
        content_digest(dir.path()).unwrap().1
    }

    #[test]
    fn digest_ignores_non_dicom_files_and_directory_order() {
        let d1 = tempfile::tempdir().unwrap();
        fs::write(d1.path().join("a.dcm"), b"first").unwrap();
        fs::write(d1.path().join("b.dcm"), b"second").unwrap();
        fs::write(d1.path().join("LICENSE"), b"cc by").unwrap();
        let (count, digest) = content_digest(d1.path()).unwrap();
        assert_eq!(count, 2);
        assert_eq!(digest, fixture_digest());
    }

    #[test]
    fn downloads_verifies_and_then_uses_the_cache_offline() {
        let digest: &'static str = Box::leak(fixture_digest().into_boxed_str());
        let series = fixture_series(digest);
        let root = tempfile::tempdir().unwrap();
        let zip = zip_bytes(&[
            ("LICENSE", b"cc by"),
            ("a.dcm", b"first"),
            ("nested/b.dcm", b"second"),
            ("notes.txt", b"ignored"),
        ]);
        let url = serve_once(zip);
        let dir = ensure_demo(&series, root.path(), Some(&url)).unwrap();
        assert!(dir.join("a.dcm").is_file());
        assert!(dir.join("b.dcm").is_file());
        assert!(dir.join("LICENSE").is_file());
        assert!(!dir.join("notes.txt").exists());

        // Second call: nothing is listening on this URL any more, so success
        // proves the verified cache is used without touching the network.
        let dead = "http://127.0.0.1:9/unreachable.zip";
        assert_eq!(ensure_demo(&series, root.path(), Some(dead)).unwrap(), dir);
    }

    #[test]
    fn checksum_mismatch_is_rejected_and_nothing_is_kept() {
        let series =
            fixture_series("0000000000000000000000000000000000000000000000000000000000000000");
        let root = tempfile::tempdir().unwrap();
        let url = serve_once(zip_bytes(&[("a.dcm", b"first"), ("b.dcm", b"tampered")]));
        let err = ensure_demo(&series, root.path(), Some(&url)).unwrap_err();
        assert!(matches!(err, DemoError::Checksum { files: 2, .. }), "{err}");
        assert!(!root.path().join("fixture").exists());
        assert!(!root.path().join("fixture.partial").exists());
        assert!(!root.path().join("fixture.zip.part").exists());
    }

    #[test]
    fn unreachable_server_is_a_network_error_with_guidance() {
        let series = fixture_series("00");
        let root = tempfile::tempdir().unwrap();
        let err = ensure_demo(&series, root.path(), Some("http://127.0.0.1:9/x.zip")).unwrap_err();
        assert!(matches!(err, DemoError::Network { .. }), "{err}");
        assert!(err.to_string().contains("strata <folder>"));
    }

    #[test]
    fn html_error_page_is_a_bad_response_not_a_zip_crash() {
        let series = fixture_series("00");
        let root = tempfile::tempdir().unwrap();
        let url = serve_once(b"<html>maintenance</html>".to_vec());
        let err = ensure_demo(&series, root.path(), Some(&url)).unwrap_err();
        assert!(matches!(err, DemoError::BadResponse { .. }), "{err}");
    }
}
