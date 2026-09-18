use std::io::ErrorKind;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context};
use clap::Parser;

use strata_server::demo;
use strata_server::index::Index;
use strata_server::routes::build_router_with_cache_dir;
use strata_server::ui;

const DEFAULT_ADDR: &str = "127.0.0.1:8080";

/// View a folder of DICOM files (CT, MRI) in your browser, in 2D and 3D.
///
/// Everything runs locally: files are read where they are and nothing is
/// uploaded. For research and education only; not a medical device and not
/// for clinical use.
#[derive(Parser)]
#[command(name = "strata", version, about, long_about)]
struct Cli {
    /// Folder of DICOM files to open (searched recursively).
    #[arg(value_name = "FOLDER", conflicts_with_all = ["data_dir", "demo"])]
    folder: Option<PathBuf>,

    /// Download a small public chest CT (TCGA-LUAD, CC BY 3.0, about 17 MB)
    /// once, cache it, and open it.
    #[arg(long)]
    demo: bool,

    /// Same as FOLDER; kept for older scripts.
    #[arg(long, hide = true, conflicts_with = "demo")]
    data_dir: Option<PathBuf>,

    /// Address to serve on. Defaults to 127.0.0.1:8080, or a free port if
    /// 8080 is taken.
    #[arg(long)]
    addr: Option<SocketAddr>,

    /// Don't open a browser window (also set by STRATA_NO_OPEN=1, e.g. on a
    /// headless machine).
    #[arg(long, env = "STRATA_NO_OPEN", value_parser = clap::builder::FalseyValueParser::new())]
    no_open: bool,

    /// Persist the series index to this SQLite file. By default the index is
    /// rebuilt in memory on every start (1,000 files take about 0.1 s).
    #[arg(long)]
    index: Option<PathBuf>,

    /// Directory for the on-disk volume cache. Defaults to a `volumes`
    /// folder in the OS cache directory (or next to --index if given).
    #[arg(long)]
    cache_dir: Option<PathBuf>,

    /// Total byte budget for the on-disk volume cache. Least-recently
    /// -written entries are evicted first once a write would exceed it.
    #[arg(long, default_value_t = strata_server::disk_cache::DEFAULT_MAX_CACHE_BYTES)]
    max_cache_bytes: u64,
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let data_dir = if cli.demo {
        let series = &demo::TCGA_LUAD_CHEST_CT;
        eprintln!("{}\n", series.attribution());
        let url = std::env::var("STRATA_DEMO_URL").ok();
        demo::ensure_demo(series, &demo::default_demo_root(), url.as_deref())?
    } else {
        match cli.folder.clone().or(cli.data_dir.clone()) {
            Some(dir) => dir,
            None => bail!(
                "no folder given.\n\n  strata <folder>   open a folder of DICOM files\n  \
                 strata --demo     download and open a small public sample CT\n\n\
                 Run `strata --help` for all options."
            ),
        }
    };
    if !data_dir.is_dir() {
        bail!("{} is not a folder", display_path(&data_dir));
    }

    let index = match &cli.index {
        Some(path) => {
            Index::open(path).with_context(|| format!("opening index {}", path.display()))?
        }
        None => Index::open_in_memory()?,
    };

    let started = std::time::Instant::now();
    let scan_result = strata_dicom::scan::scan_directory(&data_dir)
        .with_context(|| format!("scanning {}", display_path(&data_dir)))?;
    for series in &scan_result.series {
        index.insert_series(series)?;
    }
    println!(
        "found {} series in {} ({:.0} ms)",
        scan_result.series.len(),
        display_path(&data_dir),
        started.elapsed().as_secs_f64() * 1000.0
    );
    for series in &scan_result.series {
        let spacing = series
            .spacing_mm
            .map(|mm| format!(", {mm:.1} mm apart"))
            .unwrap_or_default();
        let description = series
            .series_description
            .as_deref()
            .map(|d| format!("  \"{d}\""))
            .unwrap_or_default();
        let calibration = if series.hu_calibrated {
            ""
        } else {
            "  (not calibrated to HU)"
        };
        println!(
            "  {} {} slices{spacing}{description}{calibration}",
            series.modality,
            series.slices.len(),
        );
    }
    for warning in &scan_result.warnings {
        println!("  warning: {warning}");
    }
    if scan_result.series.is_empty() {
        println!("no DICOM series found there; the viewer will say so too");
    }

    let cache_dir = match (&cli.cache_dir, &cli.index) {
        (Some(dir), _) => dir.clone(),
        (None, Some(index)) => index
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .join("strata-cache"),
        (None, None) => strata_server::default_cache_root().join("volumes"),
    };

    let shared: strata_server::routes::SharedIndex = Arc::new(Mutex::new(index));
    let router = ui::with_embedded_ui(build_router_with_cache_dir(
        shared,
        cache_dir,
        cli.max_cache_bytes,
    ));
    if !ui::has_real_ui() {
        eprintln!("warning: this binary was built without the web UI (STRATA_SKIP_WEB_BUILD); only /api works");
    }

    let listener = bind(cli.addr).await?;
    let addr = listener.local_addr()?;
    let url = if addr.ip().is_unspecified() {
        format!("http://127.0.0.1:{}", addr.port())
    } else {
        format!("http://{addr}")
    };
    println!("\nstrata is running at {url}  (Ctrl+C to stop)");
    println!("research and education only; not for clinical use");
    if !cli.no_open {
        if let Err(e) = open::that_detached(&url) {
            println!("could not open a browser ({e}); open {url} yourself");
        }
    }

    axum::serve(listener, router).await?;
    Ok(())
}

/// Binds the requested address. With no explicit `--addr`, a busy default
/// port falls back to any free port instead of failing.
async fn bind(requested: Option<SocketAddr>) -> anyhow::Result<tokio::net::TcpListener> {
    let addr = requested.unwrap_or_else(|| DEFAULT_ADDR.parse().unwrap());
    match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => Ok(l),
        Err(e) if e.kind() == ErrorKind::AddrInUse && requested.is_none() => {
            let fallback = SocketAddr::new(addr.ip(), 0);
            Ok(tokio::net::TcpListener::bind(fallback).await?)
        }
        Err(e) => Err(e).with_context(|| format!("binding {addr}")),
    }
}

/// Shows paths under the home directory as `~/...` to keep output short.
fn display_path(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            return Path::new("~").join(rest).display().to_string();
        }
    }
    path.display().to_string()
}
