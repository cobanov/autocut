//! Downsample a decoded audio track into the amplitude envelope the timeline
//! draws.
//!
//! Bin the 16kHz mono PCM into `target_bins` equal-width slices and take the
//! peak |sample| per bin. Result is a Vec<f32> in [0, 1].
//!
//! The samples come from the caller, not from ffmpeg directly: the VAD wants
//! exactly the same decode, so `commands::AnalysisCache` owns the extraction
//! and both consumers read from it.

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{anyhow, Result};

pub fn waveform_from_samples(
    samples: &[i16],
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
        // unsigned_abs, not abs: i16::MIN has no positive counterpart, so
        // `-32768i16.abs()` overflows.
        let peak = samples[start..end]
            .iter()
            .map(|s| s.unsigned_abs())
            .max()
            .unwrap_or(0);
        out.push(peak as f32 / 32768.0);
    }
    Ok(out)
}

fn is_cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel
        .map(|flag| flag.load(Ordering::SeqCst))
        .unwrap_or(false)
}
