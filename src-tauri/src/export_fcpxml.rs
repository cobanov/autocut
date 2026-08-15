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

use anyhow::Result;

use crate::cutlist::CutList;
use crate::timecode::{detect_rate, parse_smpte, seconds_to_rational, TimecodeRate};

/// One piece of source media on the timeline.
///
/// The reference track defines project time: project second `t` is reference
/// source second `t`. Every other track states where its own media begins in
/// that same project time via `offset`, which is what lets one cutlist govern
/// all of them.
pub struct Track<'a> {
    pub path: Option<&'a Path>,
    pub name: &'a str,
    pub duration: f64,
    /// SMPTE timecode this media starts at, if it carries one. Shifts where
    /// inside the file a clip reads from, independently of `offset`.
    pub start_timecode: Option<&'a str>,
    pub has_video: bool,
    pub has_audio: bool,
    /// Project-time second at which this track's media begins. Zero for the
    /// reference by definition.
    pub offset: f64,
}

pub struct FcpxmlParams<'a> {
    pub fps: f64,
    pub title: &'a str,
    pub reference: Track<'a>,
    /// Extra tracks cut in lockstep with the reference. Video tracks stack
    /// upward from lane 1, audio-only tracks downward from lane -1, in the
    /// order given.
    pub linked: &'a [Track<'a>],
}

pub fn render(cutlist: &CutList, params: FcpxmlParams<'_>) -> Result<String> {
    // Structural, not a courtesy check: an FCPXML with an empty spine is
    // well-formed XML that imports as an empty timeline, so there is no later
    // point at which anything would notice.
    cutlist.ensure_exportable()?;

    let drop_frame = params
        .reference
        .start_timecode
        .map(|tc| tc.contains(';'))
        .unwrap_or(false);
    let rate = detect_rate(params.fps, drop_frame);
    let tc_format = if rate.is_drop_frame && (rate.nominal_fps == 30 || rate.nominal_fps == 60) {
        "DF"
    } else {
        "NDF"
    };
    let safe_title = xml_escape(params.title);
    let frame_duration = format!("{}/{}s", rate.frame_duration_num, rate.frame_duration_den);

    // r1 is the reference; linked tracks follow in list order. Lanes fan out
    // from the primary storyline: video upward, audio-only downward.
    let mut resources = String::new();
    let mut lanes: Vec<i32> = Vec::with_capacity(params.linked.len());
    let mut next_video_lane = 1;
    let mut next_audio_lane = -1;
    for (i, track) in std::iter::once(&params.reference)
        .chain(params.linked.iter())
        .enumerate()
    {
        resources.push_str(&asset_element(i + 1, track, params.fps, &rate));
        if i > 0 {
            lanes.push(if track.has_video {
                let lane = next_video_lane;
                next_video_lane += 1;
                lane
            } else {
                let lane = next_audio_lane;
                next_audio_lane -= 1;
                lane
            });
        }
    }

    let reference_tc = parse_smpte(params.reference.start_timecode, params.fps);
    let mut spine = String::new();
    let mut record_cursor = 0.0_f64;
    for cut in cutlist.kept_intervals() {
        let clip = ClipTimes {
            offset: record_cursor,
            start: reference_tc + cut.start,
            duration: cut.duration(),
        };

        let mut connected = String::new();
        for (i, track) in params.linked.iter().enumerate() {
            // Project time the track actually has media for, intersected with
            // this keep. A track that rolled late, stopped early, or is simply
            // shorter contributes nothing here rather than pointing the NLE
            // past the end of its file.
            let visible_from = cut.start.max(track.offset);
            let visible_to = cut.end.min(track.offset + track.duration);
            if visible_to - visible_from <= 0.0 {
                continue;
            }
            let track_tc = parse_smpte(track.start_timecode, params.fps);
            let times = ClipTimes {
                offset: record_cursor + (visible_from - cut.start),
                start: track_tc + (visible_from - track.offset),
                duration: visible_to - visible_from,
            };
            connected.push_str(&clip_element(
                i + 2,
                Some(lanes[i]),
                &times,
                &rate,
                tc_format,
                12,
            ));
        }

        if connected.is_empty() {
            spine.push_str(&clip_element(1, None, &clip, &rate, tc_format, 8));
        } else {
            spine.push_str(&open_clip_element(1, &clip, &rate, tc_format, 8));
            spine.push_str(&connected);
            spine.push_str("        </asset-clip>\n");
        }
        record_cursor += cut.duration();
    }

    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE fcpxml>
<fcpxml version="1.11">
  <resources>
    <format id="f1" name="FFVideoFormat" frameDuration="{frame_duration}" />
{resources}  </resources>
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
    ))
}

/// Where one clip sits on the finished timeline (`offset`), where it reads
/// from inside its own media (`start`), and how much of it plays.
struct ClipTimes {
    offset: f64,
    start: f64,
    duration: f64,
}

fn asset_element(id: usize, track: &Track<'_>, fps: f64, rate: &TimecodeRate) -> String {
    let src = track
        .path
        .map(to_file_uri)
        .unwrap_or_else(|| "source.mp4".to_string());
    let start = seconds_to_rational(parse_smpte(track.start_timecode, fps), rate);
    let duration = seconds_to_rational(track.duration, rate);
    let video_attrs = if track.has_video {
        r#" hasVideo="1" videoSources="1""#
    } else {
        r#" hasVideo="0""#
    };
    let audio_attrs = if track.has_audio {
        r#" hasAudio="1" audioSources="1" audioChannels="2""#
    } else {
        r#" hasAudio="0""#
    };
    format!(
        "    <asset id=\"r{id}\" name=\"{name}\" start=\"{start}\" duration=\"{duration}\"{video_attrs}{audio_attrs}>\n      <media-rep kind=\"original-media\" src=\"{src}\"/>\n    </asset>\n",
        name = xml_escape(track.name),
        src = xml_escape(&src),
    )
}

fn clip_attrs(
    id: usize,
    lane: Option<i32>,
    times: &ClipTimes,
    rate: &TimecodeRate,
    tc_format: &str,
) -> String {
    let lane = lane.map(|l| format!(" lane=\"{l}\"")).unwrap_or_default();
    format!(
        "<asset-clip ref=\"r{id}\"{lane} offset=\"{offset}\" start=\"{start}\" duration=\"{duration}\" tcFormat=\"{tc_format}\"",
        offset = seconds_to_rational(times.offset, rate),
        start = seconds_to_rational(times.start, rate),
        duration = seconds_to_rational(times.duration, rate),
    )
}

fn clip_element(
    id: usize,
    lane: Option<i32>,
    times: &ClipTimes,
    rate: &TimecodeRate,
    tc_format: &str,
    indent: usize,
) -> String {
    format!(
        "{:indent$}{} />\n",
        "",
        clip_attrs(id, lane, times, rate, tc_format),
        indent = indent
    )
}

fn open_clip_element(
    id: usize,
    times: &ClipTimes,
    rate: &TimecodeRate,
    tc_format: &str,
    indent: usize,
) -> String {
    format!(
        "{:indent$}{}>\n",
        "",
        clip_attrs(id, None, times, rate, tc_format),
        indent = indent
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

    /// Two keeps, [0,2) and [4,6), inside a 10s source.
    fn two_keeps() -> CutList {
        cl(
            vec![
                (0.0, 2.0, CutKind::Keep),
                (2.0, 4.0, CutKind::Remove),
                (4.0, 6.0, CutKind::Keep),
                (6.0, 10.0, CutKind::Remove),
            ],
            10.0,
        )
    }

    fn reference(duration: f64) -> Track<'static> {
        Track {
            path: None,
            name: "reference",
            duration,
            start_timecode: None,
            has_video: true,
            has_audio: true,
            offset: 0.0,
        }
    }

    fn linked(name: &str, duration: f64) -> Track<'_> {
        Track {
            path: None,
            name,
            duration,
            start_timecode: None,
            has_video: true,
            has_audio: true,
            offset: 0.0,
        }
    }

    fn params<'a>(reference: Track<'a>, linked: &'a [Track<'a>]) -> FcpxmlParams<'a> {
        FcpxmlParams {
            fps: 25.0,
            title: "demo",
            reference,
            linked,
        }
    }

    /// Every `<asset-clip>` that carries a `lane`, i.e. the connected clips.
    fn connected(xml: &str) -> Vec<&str> {
        xml.lines()
            .map(str::trim)
            .filter(|l| l.starts_with("<asset-clip") && l.contains("lane="))
            .collect()
    }

    // ---- single track: the behaviour that predates linked tracks ----

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
                title: "demo",
                reference: Track {
                    start_timecode: Some("01:00:00;00"),
                    ..reference(3.0)
                },
                linked: &[],
            },
        )
        .expect("this cutlist has keeps");

        assert!(xml.contains("tcFormat=\"DF\""), "{xml}");
        assert_eq!(xml.matches("<asset-clip").count(), 2, "{xml}");
        assert!(xml.contains("frameDuration=\"1001/30000s\""), "{xml}");
        // 01:00:00;00 at 29.97 DF is 107892 continuous frames (108000 display
        // minus 108 dropped), which as a rational is 107892 * 1001 / 30000.
        assert!(xml.contains("start=\"107999892/30000s\""), "{xml}");
    }

    #[test]
    fn ndf_when_no_drop_frame() {
        let cutlist = cl(vec![(0.0, 1.0, CutKind::Keep)], 1.0);
        let xml = render(&cutlist, params(reference(1.0), &[])).expect("this cutlist has keeps");
        assert!(xml.contains("tcFormat=\"NDF\""));
        assert!(xml.contains("frameDuration=\"1/25s\""));
    }

    #[test]
    fn xml_escapes_names() {
        let cutlist = cl(vec![(0.0, 1.0, CutKind::Keep)], 1.0);
        let xml = render(
            &cutlist,
            FcpxmlParams {
                title: "a & b <c>",
                ..params(reference(1.0), &[])
            },
        )
        .expect("this cutlist has keeps");
        assert!(xml.contains("a &amp; b &lt;c&gt;"));
    }

    #[test]
    fn marks_audio_absent_when_source_has_no_audio() {
        let cutlist = cl(vec![(0.0, 1.0, CutKind::Keep)], 1.0);
        let xml = render(
            &cutlist,
            params(
                Track {
                    has_audio: false,
                    ..reference(1.0)
                },
                &[],
            ),
        )
        .expect("this cutlist has keeps");
        assert!(xml.contains("hasAudio=\"0\""), "{xml}");
        assert!(!xml.contains("audioSources="), "{xml}");
    }

    #[test]
    fn refuses_to_render_a_timeline_with_nothing_kept() {
        let cutlist = cl(vec![(0.0, 3.0, CutKind::Remove)], 3.0);
        let err = render(&cutlist, params(reference(3.0), &[]))
            .expect_err("an empty timeline is not a valid export");
        assert!(
            err.to_string().contains("nothing to keep"),
            "unhelpful error: {err}"
        );
    }

    // ---- linked tracks ----

    #[test]
    fn a_linked_video_track_becomes_a_connected_clip_above_the_primary() {
        let tracks = [linked("cam-b", 10.0)];
        let xml =
            render(&two_keeps(), params(reference(10.0), &tracks)).expect("this cutlist has keeps");

        // One asset per track, and the linked one referenced from every keep.
        assert_eq!(xml.matches("<asset id=").count(), 2, "{xml}");
        let clips = connected(&xml);
        assert_eq!(clips.len(), 2, "{xml}");
        assert!(clips.iter().all(|c| c.contains("lane=\"1\"")), "{clips:?}");
        assert!(clips.iter().all(|c| c.contains("ref=\"r2\"")), "{clips:?}");
    }

    #[test]
    fn an_audio_only_track_sits_below_the_primary() {
        let tracks = [Track {
            has_video: false,
            ..linked("lav", 10.0)
        }];
        let xml =
            render(&two_keeps(), params(reference(10.0), &tracks)).expect("this cutlist has keeps");

        let clips = connected(&xml);
        assert_eq!(clips.len(), 2, "{xml}");
        assert!(clips.iter().all(|c| c.contains("lane=\"-1\"")), "{clips:?}");
    }

    #[test]
    fn lanes_stack_outward_in_list_order() {
        // Video lanes climb 1, 2 above the primary storyline; audio lanes
        // descend -1, -2 below it. That is how an NLE lays out V2/V3 and
        // A2/A3, so list order has to survive into the lane numbers.
        let tracks = [
            linked("cam-b", 10.0),
            Track {
                has_video: false,
                ..linked("lav", 10.0)
            },
            linked("cam-c", 10.0),
            Track {
                has_video: false,
                ..linked("room", 10.0)
            },
        ];
        let xml =
            render(&two_keeps(), params(reference(10.0), &tracks)).expect("this cutlist has keeps");

        let first_keep: Vec<_> = connected(&xml).into_iter().take(4).collect();
        let lane_of = |name: &str| {
            first_keep
                .iter()
                .find(|c| c.contains(&format!("ref=\"{name}\"")))
                .map(|c| {
                    let at = c.find("lane=\"").expect("connected clips carry a lane") + 6;
                    c[at..]
                        .split('"')
                        .next()
                        .expect("lane is quoted")
                        .to_string()
                })
        };
        assert_eq!(lane_of("r2").as_deref(), Some("1"));
        assert_eq!(lane_of("r3").as_deref(), Some("-1"));
        assert_eq!(lane_of("r4").as_deref(), Some("2"));
        assert_eq!(lane_of("r5").as_deref(), Some("-2"));
    }

    #[test]
    fn a_track_that_does_not_reach_a_keep_is_omitted_from_it() {
        // The b-roll stops at 3s but the second keep starts at 4s. Emitting a
        // clip there would point the NLE past the end of the media.
        let tracks = [linked("cam-b", 3.0)];
        let xml =
            render(&two_keeps(), params(reference(10.0), &tracks)).expect("this cutlist has keeps");
        assert_eq!(connected(&xml).len(), 1, "{xml}");
    }

    #[test]
    fn a_track_that_only_partly_covers_a_keep_is_trimmed_to_what_it_has() {
        // Media runs out 1s into the second keep, so that clip is 1s, not 2s.
        let tracks = [linked("cam-b", 5.0)];
        let xml =
            render(&two_keeps(), params(reference(10.0), &tracks)).expect("this cutlist has keeps");

        let clips = connected(&xml);
        assert_eq!(clips.len(), 2, "{xml}");
        assert!(clips[0].contains("duration=\"50/25s\""), "{clips:?}");
        assert!(clips[1].contains("duration=\"25/25s\""), "{clips:?}");
    }

    #[test]
    fn a_track_starting_later_reads_from_earlier_in_its_own_media() {
        // Recorder rolled 3s after the camera. At project time 4s the recorder
        // is only 1s into its own file.
        let tracks = [Track {
            offset: 3.0,
            ..linked("lav", 10.0)
        }];
        let xml =
            render(&two_keeps(), params(reference(10.0), &tracks)).expect("this cutlist has keeps");

        let clips = connected(&xml);
        // Nothing to contribute to the keep at [0,2): it had not started.
        assert_eq!(clips.len(), 1, "{xml}");
        assert!(clips[0].contains("start=\"25/25s\""), "{clips:?}");
        assert!(clips[0].contains("offset=\"50/25s\""), "{clips:?}");
    }

    #[test]
    fn a_linked_tracks_own_timecode_shifts_where_it_reads_from() {
        let tracks = [Track {
            start_timecode: Some("00:00:10:00"),
            ..linked("cam-b", 10.0)
        }];
        let xml =
            render(&two_keeps(), params(reference(10.0), &tracks)).expect("this cutlist has keeps");

        let clips = connected(&xml);
        assert!(clips[0].contains("start=\"250/25s\""), "{clips:?}");
    }

    #[test]
    fn connected_clips_ride_the_record_cursor_with_the_primary() {
        // The second keep lands 2s into the finished timeline, so both the
        // primary clip and everything hanging off it are offset by 2s.
        let tracks = [linked("cam-b", 10.0)];
        let xml =
            render(&two_keeps(), params(reference(10.0), &tracks)).expect("this cutlist has keeps");

        let clips = connected(&xml);
        assert!(clips[0].contains("offset=\"0s\""), "{clips:?}");
        assert!(clips[1].contains("offset=\"50/25s\""), "{clips:?}");
    }
}
