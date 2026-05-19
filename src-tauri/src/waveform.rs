//! Compute a downsampled amplitude envelope (waveform) from a video's audio
//! track. Used to render the audio waveform in the timeline UI.
//!
//! Strategy: extract 16kHz mono PCM (same path as VAD), then bin into
//! `target_bins` equal-width slices, taking the peak |sample| per bin.
//! Result is a Vec<f32> in [0, 1] with `target_bins` entries.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Result};

use crate::audio::{extract_pcm, extract_pcm_with_cancel};

pub fn extract_waveform(ffmpeg: &Path, video: &Path, target_bins: usize) -> Result<Vec<f32>> {
    extract_waveform_with_cancel(ffmpeg, video, target_bins, None)
}

pub fn extract_waveform_with_cancel(
    ffmpeg: &Path,
    video: &Path,
    target_bins: usize,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<Vec<f32>> {
    let samples = if let Some(flag) = cancel.clone() {
        extract_pcm_with_cancel(ffmpeg, video, None, Some(flag))?
    } else {
        extract_pcm(ffmpeg, video, None)?
    };
    waveform_from_samples(&samples, target_bins, cancel.as_deref())
}

pub fn waveform_from_samples(
    samples: &[f32],
    target_bins: usize,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<f32>> {
    if samples.is_empty() || target_bins == 0 {
        return Ok(Vec::new());
    }
    let bins = target_bins.min(samples.len());
    let bin_size = (samples.len() as f64 / bins as f64).max(1.0);
    let mut out = Vec::with_capacity(bins);
    for i in 0..bins {
        if i % 512 == 0 && is_cancelled(cancel) {
            return Err(anyhow!("waveform extraction cancelled"));
        }
        let start = (i as f64 * bin_size) as usize;
        let end = (((i + 1) as f64 * bin_size) as usize).min(samples.len());
        if end <= start {
            out.push(0.0);
            continue;
        }
        let peak = samples[start..end]
            .iter()
            .map(|s| s.abs())
            .fold(0.0_f32, f32::max);
        out.push(peak);
    }
    Ok(out)
}

fn is_cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel
        .map(|flag| flag.load(Ordering::SeqCst))
        .unwrap_or(false)
}
