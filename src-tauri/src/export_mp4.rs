//! Cut MP4 export: feed the kept intervals to ffmpeg via the concat demuxer.
//! A temp list file describes each kept range as a `file` + `inpoint` +
//! `outpoint` triple, ffmpeg seeks to each in turn and re-encodes (libx264 +
//! aac). Lossless smart-cut is deferred.
//!
//! Why concat demuxer and not `filter_complex select`: building a single
//! `select=between(t,a,b)+between(t,c,d)+...` expression scales linearly with
//! the number of keeps. Past a few hundred intervals ffmpeg's expression
//! parser fails to allocate the parse tree ("Cannot allocate memory" inside
//! AVFilterGraph init), killing the export. The concat demuxer reads a flat
//! list and has no such limit.
//!
//! Progress is parsed from ffmpeg stderr `out_time_us=` lines (emitted via
//! `-progress pipe:2`). Total target duration is the sum of kept intervals
//! since the output stream's wall-clock is shorter than the source.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{anyhow, Context, Result};

use crate::cutlist::CutList;
use crate::sync::lock;

pub struct ExportProgress {
    pub pct: f32,
    pub message: String,
}

/// Quality preset chosen by the user. CRF is libx264's rate-distortion knob:
/// lower = higher quality + bigger file. Audio bitrate scales with it.
#[derive(Debug, Clone, Copy)]
pub enum Quality {
    High,
    Medium,
    Small,
}

impl Quality {
    fn crf(self) -> u8 {
        match self {
            Quality::High => 18,
            Quality::Medium => 22,
            Quality::Small => 26,
        }
    }
    fn audio_bitrate(self) -> &'static str {
        match self {
            Quality::High => "192k",
            Quality::Medium => "128k",
            Quality::Small => "96k",
        }
    }
}

/// Output resolution. `Source` keeps the original dimensions; the numeric
/// variants downscale to that target height (using `-2` for width to keep
/// the aspect ratio while staying divisible by 2 for h264).
#[derive(Debug, Clone, Copy)]
pub enum Resolution {
    Source,
    P1080,
    P720,
    P480,
}

impl Resolution {
    fn target_height(self) -> Option<u32> {
        match self {
            Resolution::Source => None,
            Resolution::P1080 => Some(1080),
            Resolution::P720 => Some(720),
            Resolution::P480 => Some(480),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ExportOptions {
    pub quality: Quality,
    pub resolution: Resolution,
    pub has_audio: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            quality: Quality::Medium,
            resolution: Resolution::Source,
            has_audio: true,
        }
    }
}

pub fn export(
    ffmpeg: &Path,
    source: &Path,
    output: &Path,
    cutlist: &CutList,
    options: ExportOptions,
    cancel: Arc<AtomicBool>,
    on_progress: impl FnMut(ExportProgress) + Send + 'static,
) -> Result<()> {
    cutlist.ensure_exportable()?;
    let kept_total: f64 = cutlist.total_kept_duration();

    let list_path = concat_list_path();
    std::fs::write(&list_path, build_concat_list(source, cutlist))
        .with_context(|| format!("writing concat list at {}", list_path.display()))?;
    // Defer file removal so it persists even if we early-return on error —
    // makes failures debuggable. Cleaned up at the bottom of the happy path.

    let crf = options.quality.crf().to_string();

    let mut cmd = Command::new(ffmpeg);
    cmd.args([
        "-y",
        "-hide_banner",
        "-nostats",
        "-progress",
        "pipe:2",
        "-loglevel",
        "error",
    ])
    .args(["-f", "concat", "-safe", "0"])
    .arg("-i")
    .arg(&list_path)
    .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", &crf]);
    // Downscale via -vf when requested. `-2` on the width side asks ffmpeg to
    // pick whatever keeps the aspect ratio and stays divisible by 2 (libx264
    // requirement). Concat demuxer produces continuous timestamps across
    // segments on its own, no setpts needed.
    if let Some(h) = options.resolution.target_height() {
        cmd.args(["-vf", &format!("scale=-2:{h}")]);
    }
    if options.has_audio {
        cmd.args(["-c:a", "aac", "-b:a", options.quality.audio_bitrate()]);
    } else {
        cmd.arg("-an");
    }
    cmd.arg(output);
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());

    let mut child = cmd.spawn().context("spawning ffmpeg for export")?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("ffmpeg stderr unavailable"))?;

    // Keep a rolling buffer of the last few non-progress stderr lines so we
    // can surface ffmpeg's actual error message when the encode fails.
    // Anything matching `key=value` is a progress field; the real diagnostics
    // are the plain-prose lines.
    let stderr_tail: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let stderr_tail_writer = stderr_tail.clone();
    let cancel_thread = cancel.clone();
    let reader = thread::spawn(move || -> Result<()> {
        // The callback is owned outright by this thread, so it needs no
        // synchronisation — it used to sit behind a Mutex that nothing else
        // could ever contend for.
        let mut on_progress = on_progress;
        let buf = BufReader::new(stderr);
        for line in buf.lines() {
            let line = line.context("reading ffmpeg stderr")?;
            if cancel_thread.load(Ordering::SeqCst) {
                break;
            }
            if let Some(us) = line.strip_prefix("out_time_us=") {
                if let Ok(us) = us.trim().parse::<i64>() {
                    let seconds = us.max(0) as f64 / 1_000_000.0;
                    let pct = ((seconds / kept_total) * 100.0).clamp(0.0, 99.0);
                    let msg = format!("encoded {:.2}s of {:.2}s kept", seconds, kept_total);
                    on_progress(ExportProgress {
                        pct: pct as f32,
                        message: msg,
                    });
                }
                continue;
            }
            // Only keep the last 20 diagnostic lines — enough to identify the
            // failure without dumping every progress key.
            if !is_progress_line(&line) && !line.trim().is_empty() {
                let mut tail = lock(&stderr_tail_writer);
                tail.push(line);
                if tail.len() > 20 {
                    let drop = tail.len() - 20;
                    tail.drain(0..drop);
                }
            }
        }
        Ok(())
    });

    // Cancellation: poll the flag and kill the child if set.
    let outcome: Result<()> = loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            break Err(anyhow!("export cancelled"));
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let _ = reader.join();
                if !status.success() {
                    let tail = lock(&stderr_tail);
                    let detail = if tail.is_empty() {
                        String::new()
                    } else {
                        format!("\nffmpeg said:\n{}", tail.join("\n"))
                    };
                    break Err(anyhow!("ffmpeg exited with {status}{detail}"));
                }
                break Ok(());
            }
            Ok(None) => thread::sleep(std::time::Duration::from_millis(100)),
            Err(e) => break Err(anyhow!("waiting on ffmpeg: {e}")),
        }
    };

    // Only clean up the concat list on success; leave it on disk on failure so
    // the user (or a bug report) can inspect what we asked ffmpeg to do.
    if outcome.is_ok() {
        let _ = std::fs::remove_file(&list_path);
    } else {
        // Killing ffmpeg mid-encode leaves a truncated, unplayable mp4 at the
        // path the user chose — and if they were overwriting an earlier
        // export, that file is now gone and replaced with the stub. Remove it
        // so a cancelled or failed export leaves nothing behind pretending to
        // be a result.
        let _ = std::fs::remove_file(output);
    }
    outcome
}

/// Is this stderr line one of ffmpeg's `-progress` fields rather than a
/// diagnostic worth showing the user?
///
/// The progress stream is strictly `snake_case_key=value`. Testing for a bare
/// '=' anywhere in the line also swallowed real errors — ffmpeg says things
/// like "maybe incorrect parameters such as bit_rate, rate, width or height"
/// and quotes filter expressions containing '=' — which left failed exports
/// reporting nothing but the exit status.
fn is_progress_line(line: &str) -> bool {
    let Some((key, _)) = line.split_once('=') else {
        return false;
    };
    !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Build a concat demuxer list (UTF-8, LF line endings) repeating the source
/// path for every kept interval with `inpoint`/`outpoint` directives.
///
/// The path is wrapped in single quotes; embedded single quotes are escaped as
/// `'\''` (close-quote, escaped-quote, reopen-quote — the standard POSIX
/// shell idiom that ffmpeg's concat parser also accepts). Backslashes in
/// Windows paths are literal inside single quotes; no escaping needed.
fn build_concat_list(source: &Path, cutlist: &CutList) -> String {
    let escaped = source.to_string_lossy().replace('\'', "'\\''");
    let mut out = String::new();
    out.push_str("ffconcat version 1.0\n");
    for c in cutlist.kept_intervals() {
        out.push_str(&format!("file '{escaped}'\n"));
        out.push_str(&format!("inpoint {:.6}\n", c.start));
        out.push_str(&format!("outpoint {:.6}\n", c.end));
    }
    out
}

/// Unique path inside the OS temp dir. PID + a process-local atomic counter
/// guarantees no collision between concurrent exports (the cli never runs them
/// in parallel today, but cheap insurance).
fn concat_list_path() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "autocut-concat-{}-{}.txt",
        std::process::id(),
        id
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cutlist::{Cut, CutKind};

    #[test]
    fn concat_list_emits_one_entry_per_kept_interval() {
        let cl = CutList {
            source_duration: 10.0,
            intervals: vec![
                Cut {
                    start: 0.0,
                    end: 1.0,
                    kind: CutKind::Keep,
                },
                Cut {
                    start: 1.0,
                    end: 2.0,
                    kind: CutKind::Remove,
                },
                Cut {
                    start: 2.0,
                    end: 3.5,
                    kind: CutKind::Keep,
                },
            ],
        };
        let list = build_concat_list(Path::new("/tmp/clip.mp4"), &cl);
        assert!(list.starts_with("ffconcat version 1.0\n"), "{list}");
        assert_eq!(list.matches("file '/tmp/clip.mp4'").count(), 2);
        assert!(list.contains("inpoint 0.000000"));
        assert!(list.contains("outpoint 1.000000"));
        assert!(list.contains("inpoint 2.000000"));
        assert!(list.contains("outpoint 3.500000"));
        // The removed range must not appear as a keep entry.
        assert!(!list.contains("inpoint 1.000000\noutpoint 2.000000"));
    }

    #[test]
    fn concat_list_is_just_a_header_when_no_keeps() {
        let cl = CutList {
            source_duration: 5.0,
            intervals: vec![],
        };
        let list = build_concat_list(Path::new("/tmp/clip.mp4"), &cl);
        assert_eq!(list, "ffconcat version 1.0\n");
    }

    #[test]
    fn concat_list_escapes_single_quotes_in_paths() {
        let cl = CutList {
            source_duration: 5.0,
            intervals: vec![Cut {
                start: 0.0,
                end: 1.0,
                kind: CutKind::Keep,
            }],
        };
        let list = build_concat_list(Path::new("/tmp/it's mine.mp4"), &cl);
        assert!(list.contains(r"file '/tmp/it'\''s mine.mp4'"), "{list}");
    }

    #[test]
    fn concat_list_preserves_windows_backslash_paths() {
        let cl = CutList {
            source_duration: 5.0,
            intervals: vec![Cut {
                start: 0.0,
                end: 1.0,
                kind: CutKind::Keep,
            }],
        };
        let list = build_concat_list(Path::new(r"C:\Users\me\my video.mp4"), &cl);
        // Backslashes are literal inside single quotes; nothing should escape them.
        assert!(
            list.contains(r"file 'C:\Users\me\my video.mp4'"),
            "{list}"
        );
    }

    #[test]
    fn concat_list_path_is_unique_per_call() {
        let a = concat_list_path();
        let b = concat_list_path();
        assert_ne!(a, b);
    }

    #[test]
    fn progress_fields_are_recognised() {
        for line in [
            "out_time_us=1234567",
            "frame=42",
            "bitrate=N/A",
            "speed=1.02x",
            "progress=continue",
        ] {
            assert!(is_progress_line(line), "{line} should be a progress field");
        }
    }

    #[test]
    fn diagnostics_containing_an_equals_sign_are_not_mistaken_for_progress() {
        // The tail buffer exists to surface ffmpeg's actual complaint when an
        // encode fails. Treating every line with an '=' in it as a progress
        // field threw away exactly the lines worth keeping, leaving the user
        // with a bare "ffmpeg exited with exit status: 1".
        for line in [
            "[libx264 @ 0x7f8] width not divisible by 2 (1921x1080)",
            "Error while opening encoder for output stream #0:0 - maybe incorrect parameters such as bit_rate, rate, width or height",
            "[Parsed_scale_0 @ 0x600] Error when evaluating the expression 'a=b'",
            "Conversion failed!",
            "",
        ] {
            assert!(
                !is_progress_line(line),
                "{line} should be kept as a diagnostic"
            );
        }
    }
}
