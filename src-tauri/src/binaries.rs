//! Resolve ffmpeg/ffprobe sidecar paths at runtime.
//!
//! Tauri renames sidecars during bundling so the production layout is just
//! `<resource_dir>/ffmpeg[.exe]` (no triple suffix). In dev mode the cargo
//! manifest dir wins (binaries/ffmpeg-<triple>).
//!
//! `binary_path` tries dev paths first (cheap stat), then resource-dir paths.
//! If neither exists the user almost certainly hit a packaging bug; we return
//! Err so the calling command surfaces a clear error.
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};

#[derive(Clone, Copy)]
enum Tool {
    Ffmpeg,
    Ffprobe,
}

impl Tool {
    fn name(self) -> &'static str {
        match self {
            Tool::Ffmpeg => "ffmpeg",
            Tool::Ffprobe => "ffprobe",
        }
    }
}

#[cfg(not(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "aarch64"),
    all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"),
    all(target_os = "windows", target_arch = "x86_64")
)))]
compile_error!("autocut ffmpeg sidecars are not configured for this target");

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const TARGET_TRIPLE: &str = "aarch64-apple-darwin";
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const TARGET_TRIPLE: &str = "x86_64-apple-darwin";
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const TARGET_TRIPLE: &str = "x86_64-unknown-linux-gnu";
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const TARGET_TRIPLE: &str = "aarch64-unknown-linux-gnu";
#[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
const TARGET_TRIPLE: &str = "x86_64-pc-windows-msvc";
#[cfg(all(
    target_os = "windows",
    target_arch = "x86_64",
    not(target_env = "msvc")
))]
const TARGET_TRIPLE: &str = "x86_64-pc-windows-gnu";

static FFMPEG_PATH: OnceLock<PathBuf> = OnceLock::new();
static FFPROBE_PATH: OnceLock<PathBuf> = OnceLock::new();

fn ext() -> &'static str {
    if cfg!(target_os = "windows") {
        ".exe"
    } else {
        ""
    }
}

fn binary_path(tool: Tool, resource_dir: Option<&Path>) -> Result<PathBuf> {
    let leaf = tool.name();
    let suffix = ext();
    let triple = TARGET_TRIPLE;

    // Candidates, tried in priority order. The most important one in
    // production is the directory holding the running executable: on macOS
    // Tauri places sidecar binaries next to the main app binary in
    // Contents/MacOS, NOT in Contents/Resources, so app.path().resource_dir()
    // alone misses them. We also fall back to the build-time dev path
    // (manifest_dir/binaries) so cargo-run / cargo-test work without bundling.
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join(format!("{leaf}{suffix}")));
            candidates.push(parent.join(format!("{leaf}-{triple}{suffix}")));
        }
    }
    if let Some(rd) = resource_dir {
        candidates.push(rd.join(format!("{leaf}{suffix}")));
        candidates.push(rd.join(format!("{leaf}-{triple}{suffix}")));
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(format!("{leaf}-{triple}{suffix}")),
    );

    for c in &candidates {
        if c.exists() {
            return Ok(c.clone());
        }
    }

    let attempted = candidates
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(anyhow!(
        "could not locate {leaf} binary. tried: {attempted}"
    ))
}

fn cached_binary_path(
    cache: &OnceLock<PathBuf>,
    tool: Tool,
    resource_dir: Option<&Path>,
) -> Result<PathBuf> {
    if let Some(path) = cache.get() {
        return Ok(path.clone());
    }
    let path = binary_path(tool, resource_dir)?;
    let _ = cache.set(path.clone());
    Ok(path)
}

pub fn ffmpeg_path(resource_dir: Option<&Path>) -> Result<PathBuf> {
    cached_binary_path(&FFMPEG_PATH, Tool::Ffmpeg, resource_dir).context("resolving ffmpeg")
}

pub fn ffprobe_path(resource_dir: Option<&Path>) -> Result<PathBuf> {
    cached_binary_path(&FFPROBE_PATH, Tool::Ffprobe, resource_dir).context("resolving ffprobe")
}
