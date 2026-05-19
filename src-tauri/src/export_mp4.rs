//! Cut MP4 export: feed the kept intervals as ffmpeg `filter_complex select`
//! expressions. Output is re-encoded (libx264 + aac), mirroring the POC.
//! Lossless smart-cut is deferred.
//!
//! Progress is parsed from ffmpeg stderr `out_time_us=` lines (emitted via
//! `-progress pipe:2`). Total target duration is the sum of kept intervals
//! since the output stream's wall-clock is shorter than the source.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{anyhow, Context, Result};

use crate::cutlist::CutList;

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
    let kept_total: f64 = cutlist.total_kept_duration();
    if kept_total <= 0.0 {
        return Err(anyhow!("nothing to keep; cutlist has zero kept duration"));
    }

    let select = build_select_expr(cutlist);
    // Append a scale filter only when downscaling is requested. `-2` on the
    // width side asks ffmpeg to pick whatever keeps the aspect ratio and
    // stays divisible by 2 (libx264 requirement).
    let scale_chain = match options.resolution.target_height() {
        Some(h) => format!(",scale=-2:{h}"),
        None => String::new(),
    };
    let filter_complex = build_filter_complex(&select, &scale_chain, options.has_audio);

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
    .arg("-i")
    .arg(source)
    .args(["-filter_complex", &filter_complex])
    .args(["-map", "[v]"])
    .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", &crf]);
    if options.has_audio {
        cmd.args(["-map", "[a]"])
            .args(["-c:a", "aac", "-b:a", options.quality.audio_bitrate()]);
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

    let on_progress = Mutex::new(on_progress);
    // Keep a rolling buffer of the last few non-progress stderr lines so we
    // can surface ffmpeg's actual error message when the encode fails.
    // Anything matching `key=value` is a progress field; the real diagnostics
    // are the plain-prose lines.
    let stderr_tail: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let stderr_tail_writer = stderr_tail.clone();
    let cancel_thread = cancel.clone();
    let reader = thread::spawn(move || -> Result<()> {
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
                    let mut cb = on_progress.lock().unwrap();
                    cb(ExportProgress {
                        pct: pct as f32,
                        message: msg,
                    });
                }
                continue;
            }
            // Only keep the last 20 prose lines — enough to identify the
            // failure without dumping every progress key.
            if !line.contains('=') && !line.trim().is_empty() {
                let mut tail = stderr_tail_writer.lock().unwrap();
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
    loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(anyhow!("export cancelled"));
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let _ = reader.join();
                if !status.success() {
                    let tail = stderr_tail.lock().unwrap();
                    let detail = if tail.is_empty() {
                        String::new()
                    } else {
                        format!("\nffmpeg said:\n{}", tail.join("\n"))
                    };
                    return Err(anyhow!("ffmpeg exited with {status}{detail}"));
                }
                break;
            }
            Ok(None) => thread::sleep(std::time::Duration::from_millis(100)),
            Err(e) => return Err(anyhow!("waiting on ffmpeg: {e}")),
        }
    }
    Ok(())
}

fn build_select_expr(cutlist: &CutList) -> String {
    let parts: Vec<String> = cutlist
        .kept_intervals()
        .map(|c| format!("between(t,{:.6},{:.6})", c.start, c.end))
        .collect();
    if parts.is_empty() {
        "0".to_string()
    } else {
        parts.join("+")
    }
}

fn build_filter_complex(select: &str, scale_chain: &str, has_audio: bool) -> String {
    let video_filter = format!(
        "[0:v]select='{sel}',setpts=N/FRAME_RATE/TB{scale}[v]",
        sel = select,
        scale = scale_chain
    );
    if has_audio {
        format!("{video_filter};[0:a]aselect='{select}',asetpts=N/SR/TB[a]")
    } else {
        video_filter
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cutlist::{Cut, CutKind};

    #[test]
    fn select_expr_joins_kept_intervals() {
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
        let s = build_select_expr(&cl);
        assert!(s.contains("between(t,0.000000,1.000000)"));
        assert!(s.contains("between(t,2.000000,3.500000)"));
        assert!(s.contains("+"));
    }

    #[test]
    fn select_expr_empty_when_no_keeps() {
        let cl = CutList {
            source_duration: 5.0,
            intervals: vec![],
        };
        assert_eq!(build_select_expr(&cl), "0");
    }

    #[test]
    fn filter_complex_omits_audio_when_source_has_no_audio() {
        let filter = build_filter_complex("between(t,0.000000,1.000000)", "", false);
        assert!(filter.contains("[0:v]select="));
        assert!(!filter.contains("[0:a]"), "{filter}");
        assert!(!filter.contains("[a]"), "{filter}");
    }

    #[test]
    fn filter_complex_includes_audio_when_source_has_audio() {
        let filter = build_filter_complex("between(t,0.000000,1.000000)", "", true);
        assert!(filter.contains("[0:a]aselect="), "{filter}");
        assert!(filter.ends_with("[a]"), "{filter}");
    }
}
