//! Tiny smoke harness used during development. Probes a video, runs VAD on it,
//! and prints the resulting speech segments and cutlist summary.
//!
//! Run: `cargo run --bin smoke -- /path/to/video.mp4 [pad_seconds]`

use std::path::PathBuf;

use anyhow::Result;
use autocut_lib::audio::extract_pcm;
use autocut_lib::binaries::{ffmpeg_path, ffprobe_path};
use autocut_lib::cutlist::CutList;
use autocut_lib::probe::probe;
use autocut_lib::vad::{detect, VadParams};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let video = args.next().expect("usage: smoke <video> [pad]");
    let pad: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0.3);

    let video = PathBuf::from(video);
    let ffmpeg = ffmpeg_path(None)?;
    let ffprobe = ffprobe_path(None)?;

    let info = probe(&ffprobe, &video)?;
    println!(
        "probe: {}x{} {:.3}fps {:.3}s tc={:?}",
        info.width, info.height, info.fps, info.duration, info.start_timecode
    );

    let samples = extract_pcm(&ffmpeg, &video)?;
    println!(
        "audio: {} samples @16kHz ({:.3}s)",
        samples.len(),
        samples.len() as f64 / 16_000.0
    );

    let segs = detect(&samples, VadParams::default(), 0.0)?;
    println!("vad: {} speech segments", segs.len());
    for s in &segs {
        println!("  {:.3} -> {:.3} ({:.3}s)", s.start, s.end, s.end - s.start);
    }

    let cl = CutList::from_speech_segments(&segs, info.duration, pad);
    let keeps: Vec<_> = cl.kept_intervals().collect();
    println!(
        "cutlist: {} keeps, {:.3}s kept of {:.3}s ({:.1}%)",
        keeps.len(),
        cl.total_kept_duration(),
        info.duration,
        100.0 * cl.total_kept_duration() / info.duration.max(1e-9)
    );

    Ok(())
}
