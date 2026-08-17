//! Build script: ensure `binaries/ffmpeg-<triple>[.exe]` and
//! `binaries/ffprobe-<triple>[.exe]` exist for the current target.
//!
//! Sources:
//!   - macOS (any arch):       evermeet.cx (universal binaries)
//!   - Linux x86_64 / aarch64: BtbN/FFmpeg-Builds GitHub release
//!   - Windows x86_64:         BtbN/FFmpeg-Builds GitHub release
//!
//! Set `AUTOCUT_SKIP_FFMPEG_DOWNLOAD=1` to skip the download step (offline,
//! IDE-driven `cargo check`). Missing binaries then fail the bundle phase,
//! loudly, which is the point — a release build must not silently ship
//! without them.
//!
//! Set `AUTOCUT_STUB_SIDECARS=1` to skip the download *and* drop empty
//! placeholder files where the sidecars would go. That is the only way to run
//! `cargo test` on a fresh checkout without a ~200MB download first, because
//! `tauri_build` refuses to proceed when a declared `externalBin` is absent.
//! It is deliberately a separate variable from the one above: anything built
//! with it produces a non-functional bundle, so it must never be something a
//! release path could set by accident.
//!
//! Uses system `curl`, `tar`, `unzip` so the build itself has zero Rust HTTP
//! dependencies.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// Pinned BtbN FFmpeg release.
//
// PIN A MONTH-END BUILD. BtbN keeps every daily autobuild for about two weeks
// and then deletes it, retaining only the last build of each month — those go
// back years. A mid-month tag therefore works when you paste it and 404s a
// fortnight later, taking the Windows and Linux release builds down with it.
// That is exactly how autobuild-2026-05-18-18-09 died: fine in May, gone by
// August, and the failure surfaces as a bare `curl: (22) ... 404` followed by
// "resource path binaries\ffmpeg-x86_64-pc-windows-msvc.exe doesn't exist".
// macOS is unaffected — it pulls from evermeet.cx, not here.
//
// To rotate:
//   1. Pick the newest *month-end* release (autobuild-YYYY-MM-{28,30,31}-HH-MM)
//      at https://github.com/BtbN/FFmpeg-Builds/releases
//   2. Update BTBN_RELEASE to its tag
//   3. Update the filename + sha256 of every asset in btbn_asset() below
//      Hashes are listed on the release page; verify locally with
//      `shasum -a 256` (or `sha256sum`) before pasting.
const BTBN_RELEASE: &str = "autobuild-2026-07-31-14-10";

struct BtbnAsset {
    filename: &'static str,
    sha256: &'static str,
}

fn main() {
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=AUTOCUT_SKIP_FFMPEG_DOWNLOAD");
    println!("cargo:rerun-if-env-changed=AUTOCUT_STUB_SIDECARS");

    if env::var_os("AUTOCUT_STUB_SIDECARS").is_some() {
        println!(
            "cargo:warning=autocut: AUTOCUT_STUB_SIDECARS is set. \
             ffmpeg and ffprobe are empty placeholders; this build cannot process video."
        );
        if let Err(e) = stub_ffmpeg() {
            println!("cargo:warning=autocut: could not place sidecar stubs ({e})");
        }
    } else if env::var_os("AUTOCUT_SKIP_FFMPEG_DOWNLOAD").is_some() {
        eprintln!("autocut/build: AUTOCUT_SKIP_FFMPEG_DOWNLOAD set, skipping ffmpeg fetch");
    } else if let Err(e) = ensure_ffmpeg() {
        // Non-fatal: cargo check works without binaries; bundle phase will fail
        // loudly with a clearer error if a release build needs them.
        println!("cargo:warning=autocut: ffmpeg fetch skipped ({e})");
    }

    tauri_build::build();
}

/// Place empty files where the sidecars belong, so `tauri_build`'s
/// externalBin existence check passes without a download. Existing real
/// binaries are left alone — running the test command on a machine that has
/// already built the app must not clobber its working ffmpeg.
fn stub_ffmpeg() -> Result<(), String> {
    let (bin_dir, ffmpeg, ffprobe) = sidecar_paths()?;
    fs::create_dir_all(&bin_dir).map_err(|e| e.to_string())?;
    for path in [&ffmpeg, &ffprobe] {
        if !path.exists() {
            fs::write(path, b"").map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn sidecar_paths() -> Result<(PathBuf, PathBuf, PathBuf), String> {
    let triple = host_target_triple()?;
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").map_err(|e| e.to_string())?);
    let bin_dir = manifest.join("binaries");
    let ext = if triple.contains("windows") {
        ".exe"
    } else {
        ""
    };
    let ffmpeg = bin_dir.join(format!("ffmpeg-{triple}{ext}"));
    let ffprobe = bin_dir.join(format!("ffprobe-{triple}{ext}"));
    Ok((bin_dir, ffmpeg, ffprobe))
}

fn ensure_ffmpeg() -> Result<(), String> {
    let triple = host_target_triple()?;
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").map_err(|e| e.to_string())?);
    let (bin_dir, ffmpeg, ffprobe) = sidecar_paths()?;
    fs::create_dir_all(&bin_dir).map_err(|e| e.to_string())?;

    // A zero-length file here is a stub left by AUTOCUT_STUB_SIDECARS, not a
    // real binary; a build that wants working ffmpeg should replace it.
    let usable = |p: &Path| fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false);
    if usable(&ffmpeg) && usable(&ffprobe) {
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
        let arch = if triple.starts_with("aarch64") {
            "linuxarm64"
        } else {
            "linux64"
        };
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

fn command_output(cmd: &mut Command) -> Result<String, String> {
    let output = cmd.output().map_err(|e| format!("{:?}: {e}", cmd))?;
    if !output.status.success() {
        return Err(format!("{:?} failed with {}", cmd, output.status));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn curl(url: &str, dst: &Path) -> Result<(), String> {
    let status = Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(dst)
        .arg(url)
        .status()
        .map_err(|e| format!("curl {url}: {e}"))?;
    if status.success() {
        return Ok(());
    }
    // curl exits 22 when -f meets an HTTP error. On a pinned BtbN asset that is
    // almost never a network fault — it means the release was garbage collected.
    // Worth naming here, because the next thing to fail is tauri's "resource
    // path binaries\ffmpeg-...exe doesn't exist", which reports the symptom and
    // gives no hint where to look. Checked on the code rather than the rendered
    // status, which prints "exit status: 22" on unix and "exit code: 22" on
    // Windows — and Windows is where this breaks.
    if status.code() == Some(22) && url.contains("BtbN/FFmpeg-Builds") {
        return Err(format!(
            "{url} is gone (HTTP 4xx). BtbN keeps daily autobuilds for about two \
             weeks and only month-end ones after that, so a mid-month pin expires \
             on its own. Re-pin BTBN_RELEASE to the newest month-end tag and \
             refresh the hashes in btbn_asset()."
        ));
    }
    Err(format!("curl {url} failed with {status}"))
}

fn verify_sha256(path: &Path, expected: &str) -> Result<(), String> {
    let actual = sha256_file(path)?;
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!(
            "checksum mismatch for {}: expected {expected}, got {actual}",
            path.display()
        ))
    }
}

fn parse_checksum_output(output: &str) -> Result<String, String> {
    output
        .split_whitespace()
        .next()
        .map(|s| s.to_ascii_lowercase())
        .ok_or_else(|| "empty checksum output".to_string())
}

#[cfg(windows)]
fn sha256_file(path: &Path) -> Result<String, String> {
    // PowerShell's `-Command` mode does not bind trailing positional args to
    // `$args`, so the path has to be embedded in the script. Single-quote it
    // and escape any embedded single quotes by doubling them.
    let escaped = path.display().to_string().replace('\'', "''");
    let script = format!(
        "(Get-FileHash -Algorithm SHA256 -LiteralPath '{escaped}').Hash.ToLowerInvariant()"
    );
    let out = command_output(Command::new("powershell").args(["-NoProfile", "-Command", &script]))?;
    parse_checksum_output(&out)
}

#[cfg(not(windows))]
fn sha256_file(path: &Path) -> Result<String, String> {
    if let Ok(out) = command_output(Command::new("shasum").args(["-a", "256"]).arg(path)) {
        return parse_checksum_output(&out);
    }
    let out = command_output(Command::new("sha256sum").arg(path))?;
    parse_checksum_output(&out)
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
    let asset = btbn_asset(arch_slug, ext)?;
    let url = format!(
        "https://github.com/BtbN/FFmpeg-Builds/releases/download/{BTBN_RELEASE}/{}",
        asset.filename
    );
    let archive = tmp.join(asset.filename);
    curl(&url, &archive)?;
    verify_sha256(&archive, asset.sha256)?;

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

fn btbn_asset(arch_slug: &str, ext: &str) -> Result<BtbnAsset, String> {
    match (arch_slug, ext) {
        ("linux64", "tar.xz") => Ok(BtbnAsset {
            filename: "ffmpeg-N-125875-g5d4d3bdc61-linux64-gpl.tar.xz",
            sha256: "16161335f2323ec74c5cec70427d3365ee9e0f581486eda35f6eba47375c45b4",
        }),
        ("linuxarm64", "tar.xz") => Ok(BtbnAsset {
            filename: "ffmpeg-N-125875-g5d4d3bdc61-linuxarm64-gpl.tar.xz",
            sha256: "a38f9976ff6377ed0a1117ed726c580da968cc8a0e9dc1328297cc60673e6f92",
        }),
        ("win64", "zip") => Ok(BtbnAsset {
            filename: "ffmpeg-N-125875-g5d4d3bdc61-win64-gpl.zip",
            sha256: "68a5e966533002785c3e4b9a98327e21d5277802668bf889d94086cb6426cbb4",
        }),
        _ => Err(format!("no pinned BtbN asset for {arch_slug}.{ext}")),
    }
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
