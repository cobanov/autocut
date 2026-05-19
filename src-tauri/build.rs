//! Build script: ensure `binaries/ffmpeg-<triple>[.exe]` and
//! `binaries/ffprobe-<triple>[.exe]` exist for the current target.
//!
//! Sources:
//!   - macOS (any arch):       evermeet.cx (universal binaries)
//!   - Linux x86_64 / aarch64: BtbN/FFmpeg-Builds GitHub release
//!   - Windows x86_64:         BtbN/FFmpeg-Builds GitHub release
//!
//! Set `AUTOCUT_SKIP_FFMPEG_DOWNLOAD=1` to skip the download step (CI, offline,
//! IDE-driven `cargo check`). The Tauri build will still try to bundle the
//! binaries if they exist; missing binaries fail the bundle phase, not check.
//!
//! Uses system `curl`, `tar`, `unzip` so the build itself has zero Rust HTTP
//! dependencies.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=AUTOCUT_SKIP_FFMPEG_DOWNLOAD");

    if env::var_os("AUTOCUT_SKIP_FFMPEG_DOWNLOAD").is_some() {
        eprintln!("autocut/build: AUTOCUT_SKIP_FFMPEG_DOWNLOAD set, skipping ffmpeg fetch");
    } else if let Err(e) = ensure_ffmpeg() {
        // Non-fatal: cargo check works without binaries; bundle phase will fail
        // loudly with a clearer error if a release build needs them.
        println!("cargo:warning=autocut: ffmpeg fetch skipped ({e})");
    }

    tauri_build::build();
}

fn ensure_ffmpeg() -> Result<(), String> {
    let triple = host_target_triple()?;
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").map_err(|e| e.to_string())?);
    let bin_dir = manifest.join("binaries");
    fs::create_dir_all(&bin_dir).map_err(|e| e.to_string())?;

    let ext = if triple.contains("windows") { ".exe" } else { "" };
    let ffmpeg = bin_dir.join(format!("ffmpeg-{triple}{ext}"));
    let ffprobe = bin_dir.join(format!("ffprobe-{triple}{ext}"));

    if ffmpeg.exists() && ffprobe.exists() {
        return Ok(());
    }

    eprintln!("autocut/build: fetching ffmpeg+ffprobe for {triple}");
    let tmp = manifest.join("target").join("ffmpeg-fetch");
    fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;

    if triple == "aarch64-apple-darwin" {
        fetch_macos_arm64(&tmp, &ffmpeg, &ffprobe)?;
    } else if triple == "x86_64-apple-darwin" {
        fetch_macos_x86_64(&tmp, &ffmpeg, &ffprobe)?;
    } else if triple.contains("linux") {
        let arch = if triple.starts_with("aarch64") { "linuxarm64" } else { "linux64" };
        fetch_btbn(&tmp, arch, "tar.xz", &ffmpeg, &ffprobe)?;
    } else if triple.contains("windows") {
        fetch_btbn(&tmp, "win64", "zip", &ffmpeg, &ffprobe)?;
    } else {
        return Err(format!("unsupported target: {triple}"));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for p in [&ffmpeg, &ffprobe] {
            let mut perm = fs::metadata(p).map_err(|e| e.to_string())?.permissions();
            perm.set_mode(0o755);
            fs::set_permissions(p, perm).map_err(|e| e.to_string())?;
        }
    }

    Ok(())
}

fn host_target_triple() -> Result<String, String> {
    // CARGO_CFG_TARGET_* is available in build scripts and reflects the build
    // target, not the host. That's what we want for binary suffixing.
    let arch = env::var("CARGO_CFG_TARGET_ARCH").map_err(|e| e.to_string())?;
    let os = env::var("CARGO_CFG_TARGET_OS").map_err(|e| e.to_string())?;
    let env_var = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let triple = match (arch.as_str(), os.as_str(), env_var.as_str()) {
        ("aarch64", "macos", _) => "aarch64-apple-darwin",
        ("x86_64", "macos", _) => "x86_64-apple-darwin",
        ("x86_64", "linux", _) => "x86_64-unknown-linux-gnu",
        ("aarch64", "linux", _) => "aarch64-unknown-linux-gnu",
        ("x86_64", "windows", "msvc") => "x86_64-pc-windows-msvc",
        ("x86_64", "windows", _) => "x86_64-pc-windows-gnu",
        (a, o, e) => return Err(format!("unsupported triple: {a}-{o} ({e})")),
    };
    Ok(triple.to_string())
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let status = cmd.status().map_err(|e| format!("{:?}: {e}", cmd))?;
    if !status.success() {
        return Err(format!("{:?} failed with {status}", cmd));
    }
    Ok(())
}

fn curl(url: &str, dst: &Path) -> Result<(), String> {
    run(Command::new("curl").args(["-fsSL", "-o"]).arg(dst).arg(url))
}

fn fetch_macos_x86_64(tmp: &Path, ffmpeg: &Path, ffprobe: &Path) -> Result<(), String> {
    // evermeet.cx's default endpoint returns x86_64 builds.
    fetch_two_zips(
        tmp,
        "https://evermeet.cx/ffmpeg/getrelease/zip",
        "https://evermeet.cx/ffmpeg/getrelease/ffprobe/zip",
        ffmpeg,
        ffprobe,
    )
}

fn fetch_macos_arm64(tmp: &Path, ffmpeg: &Path, ffprobe: &Path) -> Result<(), String> {
    // evermeet.cx is x86_64-only at the time of writing. Running an x86_64
    // ffprobe on an Apple Silicon Mac that hasn't installed Rosetta 2 fails
    // silently when spawned as a sidecar, which is the bug we're fixing.
    // osxexperts.net ships native arm64 builds; their URLs are version-pinned.
    fetch_two_zips(
        tmp,
        "https://www.osxexperts.net/ffmpeg711arm.zip",
        "https://www.osxexperts.net/ffprobe711arm.zip",
        ffmpeg,
        ffprobe,
    )
}

fn fetch_two_zips(
    tmp: &Path,
    ffmpeg_url: &str,
    ffprobe_url: &str,
    ffmpeg: &Path,
    ffprobe: &Path,
) -> Result<(), String> {
    let ffmpeg_zip = tmp.join("ffmpeg.zip");
    let ffprobe_zip = tmp.join("ffprobe.zip");
    curl(ffmpeg_url, &ffmpeg_zip)?;
    curl(ffprobe_url, &ffprobe_zip)?;

    let extract = tmp.join("extract");
    let _ = fs::remove_dir_all(&extract);
    fs::create_dir_all(&extract).map_err(|e| e.to_string())?;
    run(Command::new("unzip")
        .args(["-o", "-q"])
        .arg(&ffmpeg_zip)
        .arg("-d")
        .arg(&extract))?;
    run(Command::new("unzip")
        .args(["-o", "-q"])
        .arg(&ffprobe_zip)
        .arg("-d")
        .arg(&extract))?;

    fs::copy(find_binary(&extract, "ffmpeg")?, ffmpeg).map_err(|e| e.to_string())?;
    fs::copy(find_binary(&extract, "ffprobe")?, ffprobe).map_err(|e| e.to_string())?;
    Ok(())
}

fn fetch_btbn(
    tmp: &Path,
    arch_slug: &str,
    ext: &str,
    ffmpeg: &Path,
    ffprobe: &Path,
) -> Result<(), String> {
    let filename = format!("ffmpeg-master-latest-{arch_slug}-gpl.{ext}");
    let url = format!("https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/{filename}");
    let archive = tmp.join(&filename);
    curl(&url, &archive)?;

    let extract = tmp.join("extract");
    let _ = fs::remove_dir_all(&extract);
    fs::create_dir_all(&extract).map_err(|e| e.to_string())?;

    if ext == "tar.xz" {
        run(Command::new("tar")
            .args(["-xJf"])
            .arg(&archive)
            .arg("-C")
            .arg(&extract))?;
    } else {
        // bsdtar (bundled with Windows 10+) handles zip via -xf; avoids needing `unzip` on PATH.
        run(Command::new("tar")
            .arg("-xf")
            .arg(&archive)
            .arg("-C")
            .arg(&extract))?;
    }

    // BtbN archives unpack to a single top-level directory; find bin/ffmpeg(.exe).
    let want_ext = if ext == "zip" { ".exe" } else { "" };
    let ffmpeg_src = find_binary(&extract, &format!("ffmpeg{want_ext}"))?;
    let ffprobe_src = find_binary(&extract, &format!("ffprobe{want_ext}"))?;
    fs::copy(ffmpeg_src, ffmpeg).map_err(|e| e.to_string())?;
    fs::copy(ffprobe_src, ffprobe).map_err(|e| e.to_string())?;
    Ok(())
}

fn find_binary(root: &Path, leaf: &str) -> Result<PathBuf, String> {
    for entry in walkdir(root) {
        if entry.file_name().and_then(|s| s.to_str()) == Some(leaf) {
            return Ok(entry);
        }
    }
    Err(format!("could not locate {leaf} in {}", root.display()))
}

fn walkdir(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(rd) = fs::read_dir(&p) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}
