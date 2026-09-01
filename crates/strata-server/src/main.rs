use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use clap::Parser;

use strata_server::index::Index;
use strata_server::routes::{build_router_with_cache_dir, with_static_files};

#[derive(Parser)]
#[command(name = "strata-server")]
struct Cli {
    /// Directory of DICOM files to scan and index on startup.
    #[arg(long)]
    data_dir: PathBuf,

    /// Address to bind the HTTP server to.
    #[arg(long, default_value = "127.0.0.1:8080")]
    addr: SocketAddr,

    /// SQLite index file (created if missing).
    #[arg(long, default_value = "strata.sqlite")]
    index: PathBuf,

    /// Directory for the on-disk pyramid cache. Defaults to a
    /// `strata-cache` folder next to `--index`.
    #[arg(long)]
    cache_dir: Option<PathBuf>,

    /// Total byte budget for the on-disk pyramid cache. Least-recently
    /// -written entries are evicted first once a write would exceed it.
    #[arg(long, default_value_t = strata_server::disk_cache::DEFAULT_MAX_CACHE_BYTES)]
    max_cache_bytes: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let index = Index::open(&cli.index)?;

    let scan_result = strata_dicom::scan::scan_directory(&cli.data_dir)?;
    for series in &scan_result.series {
        index.insert_series(series)?;
        println!(
            "indexed series {} ({} slices, modality={}, hu_calibrated={}, spacing_mm={:?})",
            series.series_uid,
            series.slices.len(),
            series.modality,
            series.hu_calibrated,
            series.spacing_mm,
        );
    }
    for warning in &scan_result.warnings {
        println!("scan warning: {warning}");
    }

    let cache_dir = cli.cache_dir.clone().unwrap_or_else(|| {
        cli.index
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .join("strata-cache")
    });
    println!(
        "volume disk cache: {} (max {} bytes)",
        cache_dir.display(),
        cli.max_cache_bytes
    );

    let shared: strata_server::routes::SharedIndex = Arc::new(Mutex::new(index));
    let mut router = build_router_with_cache_dir(shared, cache_dir, cli.max_cache_bytes);

    let dist_dir = PathBuf::from("web/dist");
    if dist_dir.exists() {
        router = with_static_files(router, &dist_dir);
        println!("serving static files from {}", dist_dir.display());
    } else {
        println!("web/dist not found; serving API only, no static frontend");
    }

    println!("listening on http://{}", cli.addr);
    let listener = tokio::net::TcpListener::bind(cli.addr).await?;
    axum::serve(listener, router).await?;
    Ok(())
}
