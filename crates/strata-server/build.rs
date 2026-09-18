//! Builds the web UI (`web/`) and stages it in `$OUT_DIR/web-dist`, where
//! `src/ui.rs` embeds it into the binary with `include_dir!`. The result is a
//! single executable that serves its own UI with no files next to it.
//!
//! Knobs, for environments without Node.js:
//! - `STRATA_WEB_DIST=/path/to/dist` embeds an already-built UI instead of
//!   running npm.
//! - `STRATA_SKIP_WEB_BUILD=1` embeds a placeholder page that says the UI was
//!   not built. The API still works; the binary prints a warning at startup.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let web_dir = manifest_dir.join("../../web");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap()).join("web-dist");

    println!("cargo:rerun-if-env-changed=STRATA_WEB_DIST");
    println!("cargo:rerun-if-env-changed=STRATA_SKIP_WEB_BUILD");

    if out_dir.exists() {
        fs::remove_dir_all(&out_dir).expect("failed to clear stale web-dist");
    }

    if let Ok(prebuilt) = env::var("STRATA_WEB_DIST") {
        let prebuilt = PathBuf::from(prebuilt);
        println!("cargo:rerun-if-changed={}", prebuilt.display());
        assert!(
            prebuilt.join("index.html").is_file(),
            "STRATA_WEB_DIST={} does not contain index.html",
            prebuilt.display()
        );
        copy_dir(&prebuilt, &out_dir).expect("failed to copy STRATA_WEB_DIST");
        println!("cargo:rustc-env=STRATA_UI_KIND=prebuilt");
        return;
    }

    if env::var_os("STRATA_SKIP_WEB_BUILD").is_some_and(|v| !v.is_empty() && v != "0") {
        write_placeholder(&out_dir);
        println!("cargo:warning=STRATA_SKIP_WEB_BUILD is set: embedding a placeholder instead of the web UI");
        println!("cargo:rustc-env=STRATA_UI_KIND=placeholder");
        return;
    }

    for input in [
        "src",
        "index.html",
        "package.json",
        "package-lock.json",
        "vite.config.ts",
        "tsconfig.json",
    ] {
        println!("cargo:rerun-if-changed={}", web_dir.join(input).display());
    }

    // `npm` is a .cmd shim on Windows, which Command won't resolve on its own.
    let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };

    if !web_dir.join("node_modules").is_dir() {
        run(Command::new(npm).arg("ci").current_dir(&web_dir), "npm ci");
    }
    run(
        Command::new(npm)
            .args(["run", "build", "--", "--emptyOutDir", "--outDir"])
            .arg(&out_dir)
            .current_dir(&web_dir),
        "npm run build",
    );
    assert!(
        out_dir.join("index.html").is_file(),
        "web build finished but {} has no index.html",
        out_dir.display()
    );
    println!("cargo:rustc-env=STRATA_UI_KIND=built");
}

fn run(cmd: &mut Command, what: &str) {
    let status = cmd.status().unwrap_or_else(|e| {
        panic!(
            "could not run `{what}` to build the web UI: {e}\n\
             strata embeds its browser UI, so building it needs Node.js 18+ and npm on PATH.\n\
             Alternatives: set STRATA_WEB_DIST to a prebuilt web/dist, or STRATA_SKIP_WEB_BUILD=1 \
             for an API-only binary."
        )
    });
    assert!(status.success(), "`{what}` failed with {status}");
}

fn write_placeholder(out_dir: &Path) {
    fs::create_dir_all(out_dir).unwrap();
    fs::write(
        out_dir.join("index.html"),
        "<!doctype html><meta charset=utf-8><title>strata</title>\
         <p>This strata binary was built with STRATA_SKIP_WEB_BUILD, so it has no web UI. \
         The HTTP API under <code>/api</code> still works.</p>",
    )
    .unwrap();
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
