//! FCPXML 1.11 emitter tuned for DaVinci Resolve and Premiere Pro.
//!
//! Ported from the Python video-edit project. The critical detail:
//! DaVinci binds an asset to its source media by THREE checks (path,
//! source-media start TC, frame rate format). If the MP4 carries an embedded
//! TC like `15:33:27;24` but the FCPXML asset starts at `0s`, the relink
//! dialog fires even when the path is correct.
//!
//! We:
//!   - Emit the asset's `start` attribute = embedded source TC seconds
//!   - Shift every asset-clip's `start` by the same offset
//!   - Stamp `tcFormat="DF"` for NTSC drop-frame, `"NDF"` otherwise

use std::path::Path;

use crate::cutlist::CutList;
use crate::timecode::{detect_rate, parse_smpte, seconds_to_rational};

pub struct FcpxmlParams<'a> {
    pub fps: f64,
    pub asset_path: Option<&'a Path>,
    pub start_timecode: Option<&'a str>,
    pub title: &'a str,
    pub has_audio: bool,
}

pub fn render(cutlist: &CutList, params: FcpxmlParams<'_>) -> String {
    let drop_frame = params
        .start_timecode
        .map(|tc| tc.contains(';'))
        .unwrap_or(false);
    let rate = detect_rate(params.fps, drop_frame);
    let tc_format = if rate.is_drop_frame && (rate.nominal_fps == 30 || rate.nominal_fps == 60) {
        "DF"
    } else {
        "NDF"
    };

    let source_tc_seconds = parse_smpte(params.start_timecode, params.fps);
    let media_src = params
        .asset_path
        .map(to_file_uri)
        .unwrap_or_else(|| "source.mp4".to_string());
    let safe_title = xml_escape(params.title);

    let asset_start = seconds_to_rational(source_tc_seconds, &rate);
    let asset_duration = seconds_to_rational(cutlist.source_duration, &rate);
    let frame_duration = format!("{}/{}s", rate.frame_duration_num, rate.frame_duration_den);

    let mut spine = String::new();
    let mut record_cursor = 0.0_f64;
    for cut in cutlist.kept_intervals() {
        let source_time = source_tc_seconds + cut.start;
        let offset = seconds_to_rational(record_cursor, &rate);
        let start = seconds_to_rational(source_time, &rate);
        let duration = seconds_to_rational(cut.duration(), &rate);
        spine.push_str(&format!(
            "        <asset-clip ref=\"r1\" offset=\"{offset}\" start=\"{start}\" duration=\"{duration}\" tcFormat=\"{tc_format}\" />\n"
        ));
        record_cursor += cut.duration();
    }

    let media_src_attr = xml_escape(&media_src);
    let audio_attrs = if params.has_audio {
        r#" hasAudio="1" audioSources="1" audioChannels="2""#
    } else {
        r#" hasAudio="0""#
    };

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE fcpxml>
<fcpxml version="1.11">
  <resources>
    <format id="f1" name="FFVideoFormat" frameDuration="{frame_duration}" />
    <asset id="r1" name="{safe_title}" start="{asset_start}" duration="{asset_duration}" hasVideo="1"{audio_attrs} videoSources="1">
      <media-rep kind="original-media" src="{media_src_attr}"/>
    </asset>
  </resources>
  <library>
    <event name="{safe_title}">
      <project name="{safe_title}">
        <sequence format="f1" tcFormat="{tc_format}">
          <spine>
{spine}          </spine>
        </sequence>
      </project>
    </event>
  </library>
</fcpxml>
"#
    )
}

fn to_file_uri(path: &Path) -> String {
    let s = path.to_string_lossy();
    if s.starts_with("file://") || s.starts_with("http://") || s.starts_with("https://") {
        return s.to_string();
    }
    // Best-effort canonical absolute path; fall back to raw.
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let abs_str = abs.to_string_lossy();
    let mut encoded = String::with_capacity(abs_str.len() + 8);
    encoded.push_str("file://");
    for byte in abs_str.bytes() {
        if is_unreserved(byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{:02X}", byte));
        }
    }
    encoded
}

fn is_unreserved(b: u8) -> bool {
    matches!(b,
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':')
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cutlist::{Cut, CutKind, CutList};

    fn cl(intervals: Vec<(f64, f64, CutKind)>, dur: f64) -> CutList {
        CutList {
            source_duration: dur,
            intervals: intervals
                .into_iter()
                .map(|(s, e, k)| Cut {
                    start: s,
                    end: e,
                    kind: k,
                })
                .collect(),
        }
    }

    #[test]
    fn renders_two_keeps_at_2997_drop_frame() {
        let cutlist = cl(
            vec![
                (0.0, 1.0, CutKind::Keep),
                (1.0, 2.0, CutKind::Remove),
                (2.0, 3.0, CutKind::Keep),
            ],
            3.0,
        );
        let xml = render(
            &cutlist,
            FcpxmlParams {
                fps: 29.97,
                asset_path: None,
                start_timecode: Some("01:00:00;00"),
                title: "demo",
                has_audio: true,
            },
        );
        // tcFormat for 29.97 drop-frame
        assert!(xml.contains("tcFormat=\"DF\""), "{xml}");
        // Two asset-clips for two kept intervals
        let clip_count = xml.matches("<asset-clip").count();
        assert_eq!(clip_count, 2, "{xml}");
        // Frame duration preserved canonical
        assert!(xml.contains("frameDuration=\"1001/30000s\""), "{xml}");
        // Asset starts at source TC. 01:00:00;00 at 29.97 DF is 107892
        // continuous frames (108000 display - 108 dropped), which expressed
        // as a rational is 107892 * 1001 / 30000 = 107999892/30000s.
        assert!(xml.contains("start=\"107999892/30000s\""), "{xml}");
    }

    #[test]
    fn ndf_when_no_drop_frame() {
        let cutlist = cl(vec![(0.0, 1.0, CutKind::Keep)], 1.0);
        let xml = render(
            &cutlist,
            FcpxmlParams {
                fps: 25.0,
                asset_path: None,
                start_timecode: None,
                title: "demo",
                has_audio: true,
            },
        );
        assert!(xml.contains("tcFormat=\"NDF\""));
        assert!(xml.contains("frameDuration=\"1/25s\""));
    }

    #[test]
    fn xml_escapes_title() {
        let cutlist = cl(vec![(0.0, 1.0, CutKind::Keep)], 1.0);
        let xml = render(
            &cutlist,
            FcpxmlParams {
                fps: 30.0,
                asset_path: None,
                start_timecode: None,
                title: "a & b <c>",
                has_audio: true,
            },
        );
        assert!(xml.contains("a &amp; b &lt;c&gt;"));
    }

    #[test]
    fn marks_audio_absent_when_source_has_no_audio() {
        let cutlist = cl(vec![(0.0, 1.0, CutKind::Keep)], 1.0);
        let xml = render(
            &cutlist,
            FcpxmlParams {
                fps: 30.0,
                asset_path: None,
                start_timecode: None,
                title: "silent",
                has_audio: false,
            },
        );
        assert!(xml.contains("hasAudio=\"0\""), "{xml}");
        assert!(!xml.contains("audioSources="), "{xml}");
        assert!(!xml.contains("audioChannels="), "{xml}");
    }
}
