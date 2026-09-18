pub mod disk_cache;
pub mod index;
pub mod pixels;
pub mod routes;
pub mod ui;
pub mod volume;

/// Root for everything strata caches on disk: the demo study and assembled
/// volume pyramids. `STRATA_CACHE_DIR` overrides it; otherwise it is the OS
/// cache directory (`~/Library/Caches/strata` on macOS, `~/.cache/strata` on
/// Linux, `%LOCALAPPDATA%\strata` on Windows).
pub fn default_cache_root() -> std::path::PathBuf {
    if let Some(dir) = std::env::var_os("STRATA_CACHE_DIR").filter(|v| !v.is_empty()) {
        return dir.into();
    }
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("strata")
}
