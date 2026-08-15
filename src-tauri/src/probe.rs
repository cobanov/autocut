//! ffprobe wrapper: pull duration, fps, dimensions, and the embedded source
//! timecode from a media file. Source TC matters for FCPXML — it's both how an
//! NLE relinks a clip to its media and how autocut aligns linked tracks
//! against the reference.
//!
//! Audio-only files probe fine. A separate sound recorder's WAV is a
//! first-class track even though it has no picture; only the reference clip a
//! project is built around has to carry video, and that check lives in the
//! command layer rather than here.

use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaInfo {
    pub path: String,
    pub duration: f64,
    /// Frame rate of the video stream, or the assumed default for audio-only
    /// files — FCPXML always needs a sequence rate to express times against.
    pub fps: f64,
    /// Zero for audio-only media.
    pub width: u32,
    pub height: u32,
    pub has_video: bool,
    pub has_audio: bool,
    /// SMPTE timecode embedded in the source, when present
    /// (e.g. `15:33:27;24`). Used for FCPXML source-TC alignment and for
    /// working out where a linked track sits relative to the reference.
    pub start_timecode: Option<String>,
}

pub fn probe(ffprobe: &Path, media: &Path) -> Result<MediaInfo> {
    let out = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
        ])
        .arg(media)
        .output()
        .with_context(|| {
            format!(
                "spawning ffprobe binary at {} for {}",
                ffprobe.display(),
                media.display()
            )
        })?;
    if !out.status.success() {
        return Err(anyhow!(
            "ffprobe at {} exited with {} on {}: {}",
            ffprobe.display(),
            out.status,
            media.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("parsing ffprobe json output")?;
    parse_probe_json(&json, media)
}

/// Turn ffprobe's JSON into a MediaInfo. Split out from the subprocess call so
/// the parsing — which is where all the container-dependent guesswork lives —
/// can be tested against fixtures.
fn parse_probe_json(json: &serde_json::Value, media: &Path) -> Result<MediaInfo> {
    let streams = json
        .get("streams")
        .and_then(|s| s.as_array())
        .ok_or_else(|| anyhow!("no streams in ffprobe output"))?;

    let of_type = |kind: &'static str| {
        streams
            .iter()
            .find(move |s| s.get("codec_type").and_then(|c| c.as_str()) == Some(kind))
    };
    let video_stream = of_type("video");
    let has_audio = of_type("audio").is_some();
    if video_stream.is_none() && !has_audio {
        return Err(anyhow!("no video or audio stream in {}", media.display()));
    }

    let number = |v: Option<&serde_json::Value>| v.and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let width = number(video_stream.and_then(|s| s.get("width")));
    let height = number(video_stream.and_then(|s| s.get("height")));
    let fps = match video_stream {
        Some(s) => resolve_fps(
            s.get("avg_frame_rate").and_then(|v| v.as_str()),
            s.get("r_frame_rate").and_then(|v| v.as_str()),
        ),
        // Audio-only media has no rate of its own, but every time in an FCPXML
        // is expressed against the sequence rate, so it still needs one.
        None => DEFAULT_FPS,
    };

    let seconds = |v: Option<&serde_json::Value>| {
        v.and_then(|d| d.as_str())
            .and_then(|d| d.parse::<f64>().ok())
    };
    let duration = seconds(json.get("format").and_then(|f| f.get("duration")))
        .or_else(|| seconds(video_stream.and_then(|s| s.get("duration"))))
        .or_else(|| seconds(of_type("audio").and_then(|s| s.get("duration"))))
        .unwrap_or(0.0);

    // Source timecode can live in stream tags or format tags (camera-dependent).
    let start_timecode = video_stream
        .and_then(find_timecode)
        .or_else(|| of_type("audio").and_then(find_timecode))
        .or_else(|| {
            json.get("format")
                .and_then(|f| f.get("tags"))
                .and_then(|t| t.get("timecode"))
                .and_then(|t| t.as_str())
                .map(|s| s.to_string())
        });

    Ok(MediaInfo {
        path: media.to_string_lossy().to_string(),
        duration,
        fps,
        width,
        height,
        has_video: video_stream.is_some(),
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

    fn json(raw: &str) -> serde_json::Value {
        serde_json::from_str(raw).expect("fixture is valid json")
    }

    #[test]
    fn reads_a_video_with_sound() {
        let info = parse_probe_json(
            &json(
                r#"{
                    "streams": [
                        {"codec_type": "video", "width": 1920, "height": 1080,
                         "avg_frame_rate": "30000/1001", "r_frame_rate": "30000/1001"},
                        {"codec_type": "audio"}
                    ],
                    "format": {"duration": "12.5"}
                }"#,
            ),
            Path::new("/clips/a.mov"),
        )
        .expect("a normal video probes cleanly");

        assert!(info.has_video);
        assert!(info.has_audio);
        assert_eq!((info.width, info.height), (1920, 1080));
        assert!((info.fps - 29.97).abs() < 0.01, "{}", info.fps);
        assert!((info.duration - 12.5).abs() < 1e-9);
    }

    #[test]
    fn reads_an_audio_only_file() {
        // The whole point of the multi-track work: a separate sound recorder's
        // WAV is a legitimate track, and probing one must not be an error.
        let info = parse_probe_json(
            &json(
                r#"{
                    "streams": [{"codec_type": "audio"}],
                    "format": {"duration": "61.25"}
                }"#,
            ),
            Path::new("/clips/lav.wav"),
        )
        .expect("an audio-only file is a valid track");

        assert!(!info.has_video);
        assert!(info.has_audio);
        assert_eq!((info.width, info.height), (0, 0));
        assert!((info.duration - 61.25).abs() < 1e-9);
    }

    #[test]
    fn rejects_a_file_with_no_streams_at_all() {
        let err = parse_probe_json(
            &json(r#"{"streams": [], "format": {"duration": "3.0"}}"#),
            Path::new("/clips/empty.bin"),
        )
        .expect_err("a file with neither picture nor sound is not media");
        assert!(err.to_string().contains("no video or audio"), "{err}");
    }

    #[test]
    fn takes_the_timecode_from_the_video_stream_when_present() {
        let info = parse_probe_json(
            &json(
                r#"{
                    "streams": [
                        {"codec_type": "video", "width": 1920, "height": 1080,
                         "avg_frame_rate": "25/1", "tags": {"timecode": "01:00:00:00"}}
                    ],
                    "format": {"duration": "5", "tags": {"timecode": "02:00:00:00"}}
                }"#,
            ),
            Path::new("/clips/a.mov"),
        )
        .expect("probes cleanly");
        assert_eq!(info.start_timecode.as_deref(), Some("01:00:00:00"));
    }

    #[test]
    fn falls_back_to_the_container_timecode() {
        // Camera-dependent: some write it on the stream, some on the container.
        let info = parse_probe_json(
            &json(
                r#"{
                    "streams": [{"codec_type": "audio"}],
                    "format": {"duration": "5", "tags": {"timecode": "02:00:00:00"}}
                }"#,
            ),
            Path::new("/clips/lav.wav"),
        )
        .expect("probes cleanly");
        assert_eq!(info.start_timecode.as_deref(), Some("02:00:00:00"));
    }

    #[test]
    fn falls_back_to_the_stream_duration_when_the_container_omits_it() {
        let info = parse_probe_json(
            &json(
                r#"{
                    "streams": [
                        {"codec_type": "video", "width": 640, "height": 480,
                         "avg_frame_rate": "25/1", "duration": "9.5"}
                    ],
                    "format": {}
                }"#,
            ),
            Path::new("/clips/a.mkv"),
        )
        .expect("probes cleanly");
        assert!((info.duration - 9.5).abs() < 1e-9);
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
