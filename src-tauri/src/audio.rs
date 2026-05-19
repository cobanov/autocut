//! Extract 16kHz mono f32 PCM samples from a video via ffmpeg subprocess.
//!
//! Pipes raw audio through stdout into memory; no intermediate file. silero-vad
//! consumes f32 in [-1.0, 1.0] at 16kHz, so we ask ffmpeg for s16le and convert.

use std::io::Read;
use std::path::Path;
use std::process::{ChildStderr, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

pub const VAD_SAMPLE_RATE: u32 = 16_000;

pub fn extract_pcm(ffmpeg: &Path, video: &Path, range: Option<(f64, f64)>) -> Result<Vec<f32>> {
    extract_pcm_with_cancel(ffmpeg, video, range, None)
}

pub fn extract_pcm_with_cancel(
    ffmpeg: &Path,
    video: &Path,
    range: Option<(f64, f64)>,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<Vec<f32>> {
    let mut cmd = Command::new(ffmpeg);
    if let Some((start, end)) = range {
        // -ss before -i for fast seek. The window may not be frame-accurate
        // but VAD doesn't need it; we only care about audio bulk.
        cmd.args(["-ss", &format!("{:.3}", start.max(0.0))]);
        cmd.args(["-to", &format!("{:.3}", end.max(start))]);
    }
    cmd.arg("-i").arg(video);
    cmd.args([
        "-vn",
        "-ac",
        "1",
        "-ar",
        &VAD_SAMPLE_RATE.to_string(),
        "-f",
        "s16le",
        "-loglevel",
        "error",
        "-",
    ]);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawning ffmpeg for {}", video.display()))?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let stderr = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;
    let stdout_reader = read_stdout(stdout);
    let stderr_reader = read_stderr(stderr);

    let status = loop {
        if is_cancelled(cancel.as_deref()) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(anyhow!("audio extraction cancelled"));
        }
        if let Some(status) = child.try_wait().context("checking ffmpeg status")? {
            break status;
        }
        thread::sleep(Duration::from_millis(25));
    };

    let bytes = stdout_reader
        .join()
        .map_err(|_| anyhow!("ffmpeg stdout reader panicked"))?
        .context("reading ffmpeg stdout")?;
    let err = stderr_reader
        .join()
        .map_err(|_| anyhow!("ffmpeg stderr reader panicked"))?;
    if !status.success() {
        return Err(anyhow!("ffmpeg failed ({status}): {err}"));
    }

    let samples = s16le_to_f32_with_cancel(&bytes, cancel.as_deref())?;
    Ok(samples)
}

fn read_stdout(mut stdout: ChildStdout) -> thread::JoinHandle<std::io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

fn read_stderr(mut stderr: ChildStderr) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut err = String::new();
        let _ = stderr.read_to_string(&mut err);
        err
    })
}

fn is_cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel
        .map(|flag| flag.load(Ordering::SeqCst))
        .unwrap_or(false)
}

#[cfg(test)]
fn s16le_to_f32(bytes: &[u8]) -> Vec<f32> {
    s16le_to_f32_with_cancel(bytes, None).expect("conversion without cancellation cannot fail")
}

fn s16le_to_f32_with_cancel(bytes: &[u8], cancel: Option<&AtomicBool>) -> Result<Vec<f32>> {
    let scale = 1.0_f32 / 32768.0;
    let mut samples = Vec::with_capacity(bytes.len() / 2);
    for (i, chunk) in bytes.chunks_exact(2).enumerate() {
        if i % 262_144 == 0 && is_cancelled(cancel) {
            return Err(anyhow!("audio extraction cancelled"));
        }
        let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
        samples.push((sample as f32) * scale);
    }
    Ok(samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s16_conversion_bounds() {
        let bytes = [0x00, 0x80, 0xff, 0x7f, 0x00, 0x00];
        let samples = s16le_to_f32(&bytes);
        assert_eq!(samples.len(), 3);
        assert!((samples[0] - (-1.0)).abs() < 1e-4);
        assert!((samples[1] - (32767.0 / 32768.0)).abs() < 1e-4);
        assert_eq!(samples[2], 0.0);
    }
}
