//! ffprobe wrapper: pull duration, fps, dimensions, and the embedded source
//! timecode from a video file. Source TC matters for FCPXML (NLE relink-fail
//! avoidance).

use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoInfo {
    pub path: String,
    pub duration: f64,
    pub fps: f64,
    pub width: u32,
    pub height: u32,
    pub has_audio: bool,
    /// SMPTE timecode embedded in the source, when present
    /// (e.g. `15:33:27;24`). Used for FCPXML source-TC alignment.
    pub start_timecode: Option<String>,
}

pub fn probe(ffprobe: &Path, video: &Path) -> Result<VideoInfo> {
    let out = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
        ])
        .arg(video)
        .output()
        .with_context(|| {
            format!(
                "spawning ffprobe binary at {} for video {}",
                ffprobe.display(),
                video.display()
            )
        })?;
    if !out.status.success() {
        return Err(anyhow!(
            "ffprobe at {} exited with {} on {}: {}",
            ffprobe.display(),
            out.status,
            video.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("parsing ffprobe json output")?;

    let streams = json
        .get("streams")
        .and_then(|s| s.as_array())
        .ok_or_else(|| anyhow!("no streams in ffprobe output"))?;

    let video_stream = streams
        .iter()
        .find(|s| s.get("codec_type").and_then(|c| c.as_str()) == Some("video"))
        .ok_or_else(|| anyhow!("no video stream in {}", video.display()))?;
    let has_audio = streams
        .iter()
        .any(|s| s.get("codec_type").and_then(|c| c.as_str()) == Some("audio"));

    let width = video_stream
        .get("width")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let height = video_stream
        .get("height")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let fps = resolve_fps(
        video_stream.get("avg_frame_rate").and_then(|v| v.as_str()),
        video_stream.get("r_frame_rate").and_then(|v| v.as_str()),
    );

    let duration = json
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(|d| d.as_str())
        .and_then(|d| d.parse::<f64>().ok())
        .or_else(|| {
            video_stream
                .get("duration")
                .and_then(|d| d.as_str())
                .and_then(|d| d.parse::<f64>().ok())
        })
        .unwrap_or(0.0);

    // Source timecode can live in stream tags or format tags (camera-dependent).
    let start_timecode = find_timecode(video_stream).or_else(|| {
        json.get("format")
            .and_then(|f| f.get("tags"))
            .and_then(|t| t.get("timecode"))
            .and_then(|t| t.as_str())
            .map(|s| s.to_string())
    });

    Ok(VideoInfo {
        path: video.to_string_lossy().to_string(),
        duration,
        fps,
        width,
        height,
        has_audio,
        start_timecode,
    })
}

/// Assumed when the container declares no usable frame rate. Only FCPXML
/// cares (MP4 export reads the real rate straight from the file), and the
/// topbar shows the resolved value so a wrong guess is at least visible.
const DEFAULT_FPS: f64 = 30.0;

/// Pick the first frame rate that is actually a frame rate.
///
/// `avg_frame_rate` is preferred, but ffprobe reports it as `0/1` for streams
/// it can't average — a well-formed rational that happens to be zero. Rejecting
/// only parse failures accepts that and never falls through to `r_frame_rate`.
fn resolve_fps(avg: Option<&str>, r: Option<&str>) -> f64 {
    [avg, r]
        .into_iter()
        .flatten()
        .filter_map(parse_rational)
        .find(|fps| fps.is_finite() && *fps > 0.0)
        .unwrap_or(DEFAULT_FPS)
}

fn parse_rational(s: &str) -> Option<f64> {
    let (n, d) = s.split_once('/')?;
    let n: f64 = n.parse().ok()?;
    let d: f64 = d.parse().ok()?;
    if d == 0.0 {
        None
    } else {
        Some(n / d)
    }
}

fn find_timecode(stream: &serde_json::Value) -> Option<String> {
    stream
        .get("tags")
        .and_then(|t| t.get("timecode"))
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rational_basics() {
        assert_eq!(parse_rational("30/1"), Some(30.0));
        assert!((parse_rational("30000/1001").unwrap() - 29.97).abs() < 0.01);
        assert_eq!(parse_rational("0/0"), None);
        assert_eq!(parse_rational("garbage"), None);
    }

    #[test]
    fn resolve_fps_prefers_the_average_rate() {
        assert_eq!(resolve_fps(Some("30/1"), Some("60/1")), 30.0);
    }

    #[test]
    fn resolve_fps_falls_back_when_the_average_rate_is_zero() {
        // ffprobe reports `0/1` (not `0/0`) for streams whose average it can't
        // compute. That parses as a perfectly valid rational zero, so a chain
        // that only rejects parse failures accepts it and never consults
        // r_frame_rate.
        assert_eq!(resolve_fps(Some("0/1"), Some("25/1")), 25.0);
    }

    #[test]
    fn resolve_fps_falls_back_when_the_average_rate_is_undefined() {
        assert_eq!(resolve_fps(Some("0/0"), Some("25/1")), 25.0);
    }

    #[test]
    fn resolve_fps_falls_back_to_the_default_when_nothing_is_usable() {
        assert_eq!(resolve_fps(Some("0/0"), Some("0/1")), DEFAULT_FPS);
    }

    #[test]
    fn resolve_fps_falls_back_to_the_default_when_the_stream_declares_nothing() {
        assert_eq!(resolve_fps(None, None), DEFAULT_FPS);
    }
}
