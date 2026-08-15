//! Extract 16kHz mono PCM samples from a video via ffmpeg subprocess.
//!
//! Pipes raw audio through stdout into memory; no intermediate file.
//!
//! Samples are kept as `i16`, ffmpeg's native s16le output, and converted to
//! the [-1.0, 1.0] floats silero wants one chunk at a time at the point of
//! use. Two reasons: a decoded hour costs 115MB instead of 230MB in the cache
//! that holds it for the whole session, and the conversion happens as the
//! bytes arrive rather than over a fully buffered copy — the old code held the
//! entire s16 blob and the entire float vector simultaneously, so a two-hour
//! source peaked at roughly 690MB to produce 460MB of samples.

use std::io::Read;
use std::path::Path;
use std::process::{ChildStderr, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

pub const VAD_SAMPLE_RATE: u32 = 16_000;

/// Scale a raw sample into the [-1.0, 1.0) range models and meters expect.
#[inline]
pub fn to_unit(sample: i16) -> f32 {
    sample as f32 / 32768.0
}

pub fn extract_pcm(ffmpeg: &Path, video: &Path) -> Result<Vec<i16>> {
    extract_pcm_with_cancel(ffmpeg, video, None)
}

pub fn extract_pcm_with_cancel(
    ffmpeg: &Path,
    video: &Path,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<Vec<i16>> {
    let mut cmd = Command::new(ffmpeg);
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
    let stdout_reader = read_samples(stdout, cancel.clone());
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

    let mut samples = stdout_reader
        .join()
        .map_err(|_| anyhow!("ffmpeg stdout reader panicked"))?
        .context("reading ffmpeg stdout")?;
    let err = stderr_reader
        .join()
        .map_err(|_| anyhow!("ffmpeg stderr reader panicked"))?;
    if !status.success() {
        return Err(anyhow!("ffmpeg failed ({status}): {err}"));
    }

    // Growth doubling can leave the vector holding up to twice what it needs,
    // and this one is about to sit in a session-long cache.
    samples.shrink_to_fit();
    Ok(samples)
}

/// 64 KiB of s16le is 32k samples: large enough that read syscalls aren't the
/// bottleneck, small enough that the staging buffer never shows up next to the
/// sample vector in a memory profile.
const READ_CHUNK: usize = 64 * 1024;

/// Decode ffmpeg's stdout into samples as the bytes arrive.
fn read_samples(
    mut stdout: ChildStdout,
    cancel: Option<Arc<AtomicBool>>,
) -> thread::JoinHandle<Result<Vec<i16>>> {
    thread::spawn(move || {
        let mut buf = vec![0u8; READ_CHUNK];
        let mut samples: Vec<i16> = Vec::new();
        let mut carry: Option<u8> = None;
        loop {
            if is_cancelled(cancel.as_deref()) {
                return Err(anyhow!("audio extraction cancelled"));
            }
            let read = stdout.read(&mut buf).context("reading ffmpeg stdout")?;
            if read == 0 {
                break;
            }
            carry = drain_samples(carry, &buf[..read], &mut samples);
        }
        Ok(samples)
    })
}

/// Append every complete little-endian i16 in `bytes` to `out`.
///
/// `carry` is the odd byte left over from the previous call, if the stream
/// happened to split a sample across two reads. Returns the new leftover.
fn drain_samples(carry: Option<u8>, bytes: &[u8], out: &mut Vec<i16>) -> Option<u8> {
    let mut rest = bytes;
    if let Some(low) = carry {
        let Some((high, tail)) = rest.split_first() else {
            return Some(low);
        };
        out.push(i16::from_le_bytes([low, *high]));
        rest = tail;
    }
    let mut chunks = rest.chunks_exact(2);
    for pair in &mut chunks {
        out.push(i16::from_le_bytes([pair[0], pair[1]]));
    }
    chunks.remainder().first().copied()
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
mod tests {
    use super::*;

    #[test]
    fn drains_whole_samples_and_carries_nothing() {
        let mut out = Vec::new();
        assert_eq!(
            drain_samples(None, &[0x00, 0x80, 0xff, 0x7f], &mut out),
            None
        );
        assert_eq!(out, vec![i16::MIN, i16::MAX]);
    }

    #[test]
    fn carries_the_trailing_byte_of_an_odd_length_read() {
        let mut out = Vec::new();
        assert_eq!(
            drain_samples(None, &[0x00, 0x80, 0xff], &mut out),
            Some(0xff)
        );
        assert_eq!(out, vec![i16::MIN]);
    }

    #[test]
    fn reassembles_a_sample_split_across_two_reads() {
        // ffmpeg's pipe hands us arbitrary byte counts, so a 16-bit sample
        // straddling a read boundary is routine rather than exotic. Dropping
        // the odd byte would desynchronise every sample after it, turning the
        // rest of the track into noise.
        let mut out = Vec::new();
        let carry = drain_samples(None, &[0xff], &mut out);
        assert_eq!(carry, Some(0xff));
        assert!(out.is_empty());
        assert_eq!(drain_samples(carry, &[0x7f], &mut out), None);
        assert_eq!(out, vec![i16::MAX]);
    }

    #[test]
    fn an_empty_read_preserves_a_pending_carry() {
        let mut out = Vec::new();
        assert_eq!(drain_samples(Some(0x12), &[], &mut out), Some(0x12));
        assert!(out.is_empty());
    }

    #[test]
    fn unit_scaling_spans_the_full_range() {
        assert!((to_unit(i16::MIN) - (-1.0)).abs() < 1e-6);
        assert!((to_unit(i16::MAX) - (32767.0 / 32768.0)).abs() < 1e-6);
        assert_eq!(to_unit(0), 0.0);
    }
}
