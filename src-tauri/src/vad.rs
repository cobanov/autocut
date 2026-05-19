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

use crate::audio::VAD_SAMPLE_RATE;
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

/// Detect speech segments with hysteresis (matches silero's reference
/// implementation): speech *starts* when probability >= `threshold` but
/// *continues* while probability >= `threshold - 0.15`. Without hysteresis,
/// marginal-probability frames in the middle of an utterance flicker on/off
/// and produce false silences. `time_offset` is added to every returned
/// timestamp to keep results source-relative when called with a windowed
/// audio slice.
pub fn detect(samples: &[f32], params: VadParams, time_offset: f64) -> Result<Vec<SpeechSegment>> {
    detect_with_cancel(samples, params, time_offset, None)
}

pub fn detect_with_cancel(
    samples: &[f32],
    params: VadParams,
    time_offset: f64,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<SpeechSegment>> {
    let mut vad = VoiceActivityDetector::builder()
        .sample_rate(VAD_SAMPLE_RATE as i64)
        .chunk_size(CHUNK_SIZE)
        .build()
        .context("initializing silero VAD")?;

    let neg_threshold = (params.threshold - 0.15).max(0.05);
    let mut in_speech = false;
    let mut chunk_is_speech: Vec<bool> = Vec::with_capacity(samples.len() / CHUNK_SIZE + 1);
    for (i, chunk) in samples.chunks(CHUNK_SIZE).enumerate() {
        if i % 64 == 0 && is_cancelled(cancel) {
            return Err(anyhow!("detection cancelled"));
        }
        let prob = vad.predict(chunk.iter().copied());
        if !in_speech && prob >= params.threshold {
            in_speech = true;
        } else if in_speech && prob < neg_threshold {
            in_speech = false;
        }
        chunk_is_speech.push(in_speech);
    }

    let raw = group_runs(&chunk_is_speech);
    let merged = merge_close(raw, ms_to_chunks(params.min_silence_ms));
    let filtered = drop_short(merged, ms_to_chunks(params.min_speech_ms));

    Ok(filtered
        .into_iter()
        .map(|(s, e)| SpeechSegment {
            start: time_offset + s as f64 * CHUNK_SECONDS,
            end: time_offset + e as f64 * CHUNK_SECONDS,
        })
        .collect())
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
