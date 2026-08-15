//! Silero VAD wrapper. Takes 16kHz f32 PCM, returns SpeechSegments in seconds.
//!
//! Mirrors silero's `get_speech_timestamps` shape:
//!   1. score each 512-sample chunk (32ms at 16kHz)
//!   2. group consecutive chunks above `threshold` into raw speech regions
//!   3. merge regions separated by silence shorter than `min_silence_ms`
//!   4. drop regions shorter than `min_speech_ms`
//!
//! Padding (`speech_pad_ms`) is intentionally NOT applied here. CutList does
//! it during inversion, which means changing pad doesn't need a VAD rerun.

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{anyhow, Context, Result};
use voice_activity_detector::VoiceActivityDetector;

use crate::audio::{to_unit, VAD_SAMPLE_RATE};
use crate::cutlist::SpeechSegment;

const CHUNK_SIZE: usize = 512; // silero V5 fixed window for 16kHz
const CHUNK_SECONDS: f64 = CHUNK_SIZE as f64 / VAD_SAMPLE_RATE as f64; // 0.032

#[derive(Debug, Clone, Copy)]
pub struct VadParams {
    pub threshold: f32,
    pub min_silence_ms: u32,
    pub min_speech_ms: u32,
}

impl Default for VadParams {
    fn default() -> Self {
        // Defaults tuned to favor recall: short silences merged, short utterances
        // kept. Combined with hysteresis this avoids the most common "cut my
        // word in half" complaint at the cost of slightly less aggressive
        // silence removal.
        Self {
            threshold: 0.5,
            min_silence_ms: 100,
            min_speech_ms: 150,
        }
    }
}

/// Score then segment in one shot. The app itself keeps the two halves apart
/// so it can cache the scoring, so this is really the convenience entry point
/// for the `smoke` binary and for anyone reading the module top-down.
pub fn detect(samples: &[i16], params: VadParams, time_offset: f64) -> Result<Vec<SpeechSegment>> {
    let scores = score_chunks(samples, None)?;
    Ok(segments_from_scores(&scores, params, time_offset))
}

/// Run silero over the audio and return its raw speech probability for every
/// 512-sample chunk.
///
/// This is the expensive half — an hour of audio is ~112k model invocations —
/// and it is the half that depends on nothing but the samples. None of
/// VadParams reaches the model: threshold, min-silence and min-speech are all
/// applied afterwards by `segments_from_scores`, and pad is applied later
/// still by CutList. So a caller can score once per file and re-segment for
/// free every time the user moves a slider.
pub fn score_chunks(samples: &[i16], cancel: Option<&AtomicBool>) -> Result<Vec<f32>> {
    let mut vad = VoiceActivityDetector::builder()
        .sample_rate(VAD_SAMPLE_RATE as i64)
        .chunk_size(CHUNK_SIZE)
        .build()
        .context("initializing silero VAD")?;

    let mut scores: Vec<f32> = Vec::with_capacity(samples.len() / CHUNK_SIZE + 1);
    for (i, chunk) in samples.chunks(CHUNK_SIZE).enumerate() {
        if i % 64 == 0 && is_cancelled(cancel) {
            return Err(anyhow!("detection cancelled"));
        }
        // Converted a chunk at a time, straight into the model's iterator, so
        // no float copy of the track ever exists.
        scores.push(vad.predict(chunk.iter().copied().map(to_unit)));
    }
    Ok(scores)
}

/// Turn per-chunk probabilities into speech segments: hysteresis, then group
/// runs, then merge across short silences, then drop short utterances.
///
/// Speech *starts* at `threshold` but *continues* down to `threshold - 0.15`
/// (matching silero's reference implementation). Without that hysteresis,
/// marginal-probability frames in the middle of an utterance flicker on and
/// off and manufacture silences mid-word.
///
/// `time_offset` is added to every timestamp so results stay source-relative
/// when the caller scored a windowed slice of the audio.
pub fn segments_from_scores(
    scores: &[f32],
    params: VadParams,
    time_offset: f64,
) -> Vec<SpeechSegment> {
    let release = (params.threshold - 0.15).max(0.05);
    let mut in_speech = false;
    let mut chunk_is_speech: Vec<bool> = Vec::with_capacity(scores.len());
    for &prob in scores {
        if !in_speech && prob >= params.threshold {
            in_speech = true;
        } else if in_speech && prob < release {
            in_speech = false;
        }
        chunk_is_speech.push(in_speech);
    }

    let raw = group_runs(&chunk_is_speech);
    let merged = merge_close(raw, ms_to_chunks(params.min_silence_ms));
    let filtered = drop_short(merged, ms_to_chunks(params.min_speech_ms));

    filtered
        .into_iter()
        .map(|(s, e)| SpeechSegment {
            start: time_offset + s as f64 * CHUNK_SECONDS,
            end: time_offset + e as f64 * CHUNK_SECONDS,
        })
        .collect()
}

fn is_cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel
        .map(|flag| flag.load(Ordering::SeqCst))
        .unwrap_or(false)
}

fn ms_to_chunks(ms: u32) -> usize {
    ((ms as f64 / 1000.0) / CHUNK_SECONDS).ceil().max(0.0) as usize
}

fn group_runs(flags: &[bool]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, &is_speech) in flags.iter().enumerate() {
        match (start, is_speech) {
            (None, true) => start = Some(i),
            (Some(s), false) => {
                out.push((s, i));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((s, flags.len()));
    }
    out
}

fn merge_close(regions: Vec<(usize, usize)>, min_gap: usize) -> Vec<(usize, usize)> {
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(regions.len());
    for (s, e) in regions {
        if let Some(last) = merged.last_mut() {
            if s.saturating_sub(last.1) < min_gap {
                last.1 = e.max(last.1);
                continue;
            }
        }
        merged.push((s, e));
    }
    merged
}

fn drop_short(regions: Vec<(usize, usize)>, min_len: usize) -> Vec<(usize, usize)> {
    regions
        .into_iter()
        .filter(|(s, e)| e.saturating_sub(*s) >= min_len)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Segment boundaries in chunk units, which is what the assertions below
    /// actually care about. Asserting on raw seconds would just restate every
    /// expectation multiplied by 0.032.
    fn chunk_spans(scores: &[f32], params: VadParams) -> Vec<(usize, usize)> {
        segments_from_scores(scores, params, 0.0)
            .into_iter()
            .map(|s| {
                (
                    (s.start / CHUNK_SECONDS).round() as usize,
                    (s.end / CHUNK_SECONDS).round() as usize,
                )
            })
            .collect()
    }

    fn params(threshold: f32, min_silence_ms: u32, min_speech_ms: u32) -> VadParams {
        VadParams {
            threshold,
            min_silence_ms,
            min_speech_ms,
        }
    }

    #[test]
    fn a_run_of_confident_chunks_becomes_one_segment() {
        let spans = chunk_spans(&[0.0, 0.9, 0.9, 0.9, 0.0], params(0.5, 0, 0));
        assert_eq!(spans, vec![(1, 4)]);
    }

    #[test]
    fn hysteresis_holds_through_a_dip_above_the_release_threshold() {
        // Speech starts at `threshold` but only stops below threshold - 0.15.
        // Without that, marginal frames mid-utterance flicker and manufacture
        // silences in the middle of a word.
        let spans = chunk_spans(&[0.9, 0.4, 0.9], params(0.5, 0, 0));
        assert_eq!(spans, vec![(0, 3)]);
    }

    #[test]
    fn a_dip_below_the_release_threshold_ends_the_segment() {
        let spans = chunk_spans(&[0.9, 0.2, 0.9], params(0.5, 0, 0));
        assert_eq!(spans, vec![(0, 1), (2, 3)]);
    }

    #[test]
    fn the_same_scores_resegment_under_a_different_threshold() {
        // The property the whole score/segment split rests on: every knob in
        // VadParams is post-processing over a fixed probability array, so
        // moving a slider never needs the model to run again.
        let scores = [0.9, 0.4, 0.9];
        assert_eq!(chunk_spans(&scores, params(0.5, 0, 0)), vec![(0, 3)]);
        assert_eq!(
            chunk_spans(&scores, params(0.9, 0, 0)),
            vec![(0, 1), (2, 3)]
        );
    }

    #[test]
    fn min_speech_drops_a_burst_that_is_too_short() {
        // 100ms is 4 chunks; the leading single-chunk burst doesn't survive.
        let spans = chunk_spans(&[0.9, 0.0, 0.9, 0.9, 0.9, 0.9, 0.9], params(0.5, 0, 100));
        assert_eq!(spans, vec![(2, 7)]);
    }

    #[test]
    fn min_silence_merges_bursts_across_a_short_gap() {
        let spans = chunk_spans(&[0.9, 0.0, 0.9], params(0.5, 100, 0));
        assert_eq!(spans, vec![(0, 3)]);
    }

    #[test]
    fn the_time_offset_shifts_every_timestamp() {
        let segs = segments_from_scores(&[0.9], params(0.5, 0, 0), 10.0);
        assert_eq!(segs.len(), 1);
        assert!((segs[0].start - 10.0).abs() < 1e-9, "{}", segs[0].start);
        assert!(
            (segs[0].end - (10.0 + CHUNK_SECONDS)).abs() < 1e-9,
            "{}",
            segs[0].end
        );
    }

    #[test]
    fn silence_throughout_produces_no_segments() {
        assert!(chunk_spans(&[0.0, 0.1, 0.0], params(0.5, 0, 0)).is_empty());
    }

    #[test]
    fn group_runs_basic() {
        let flags = vec![false, true, true, false, false, true, false];
        assert_eq!(group_runs(&flags), vec![(1, 3), (5, 6)]);
    }

    #[test]
    fn group_runs_trailing_speech() {
        let flags = vec![false, true, true];
        assert_eq!(group_runs(&flags), vec![(1, 3)]);
    }

    #[test]
    fn merge_close_combines_short_gap() {
        // gap of 1 chunk, min_gap=2 -> merge
        let r = merge_close(vec![(0, 5), (6, 10)], 2);
        assert_eq!(r, vec![(0, 10)]);
    }

    #[test]
    fn merge_close_keeps_long_gap() {
        // gap of 5 chunks, min_gap=2 -> keep separate
        let r = merge_close(vec![(0, 5), (10, 15)], 2);
        assert_eq!(r, vec![(0, 5), (10, 15)]);
    }

    #[test]
    fn drop_short_filters_below_min() {
        let r = drop_short(vec![(0, 3), (10, 20)], 5);
        assert_eq!(r, vec![(10, 20)]);
    }

    #[test]
    fn ms_to_chunks_rounds_up() {
        // 32ms = exactly 1 chunk; 33ms -> 2 (ceil)
        assert_eq!(ms_to_chunks(32), 1);
        assert_eq!(ms_to_chunks(33), 2);
        assert_eq!(ms_to_chunks(0), 0);
    }
}
