//! Absolute-timeline sample math and mono 16-bit WAV IO: milliseconds →
//! samples, exact-length tiling, and offset placement onto a silence-filled
//! buffer.

use std::path::Path;

use anyhow::{bail, Context};

/// Integer milliseconds → samples: `ms * rate / 1000` (floor). No floats, so
/// placement math cannot drift; all callers MUST use this (never float
/// accumulation across segments).
pub fn ms_to_samples(ms: u64, sample_rate: u32) -> u64 {
    ms * u64::from(sample_rate) / 1000
}

/// Force `samples` to exactly `expected` entries: truncate the tail or append
/// silence. Rust-side belt-and-braces after ffmpeg tiling (ffmpeg codes in
/// floats; this guarantees exact windows for the concat).
pub fn tile_exact(mut samples: Vec<i16>, expected: usize) -> Vec<i16> {
    if samples.len() >= expected {
        samples.truncate(expected);
    } else {
        samples.resize(expected, 0);
    }
    samples
}

/// Overlap-merge timeline pieces at absolute sample offsets onto a zeroed
/// buffer of `total_samples` (gap regions stay silent). Pieces are placed in
/// slice order; on overlap the later piece wins. `pieces` offsets must be
/// `< total_samples` (offsets at/after the end are dropped).
pub fn mix_timeline(pieces: &[(u64, &[i16])], total_samples: u64) -> Vec<i16> {
    let mut out = vec![0i16; total_samples as usize];
    for (offset, samples) in pieces {
        if *offset >= total_samples {
            continue;
        }
        let start = *offset as usize;
        let end = (start + samples.len()).min(out.len());
        out[start..end].copy_from_slice(&samples[..end - start]);
    }
    out
}

/// Write `samples` as mono 16-bit PCM WAV at `sample_rate`.
pub fn write_wav(path: &Path, sample_rate: u32, samples: &[i16]) -> anyhow::Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("write wav ({})", path.display()))?;
    for &sample in samples {
        writer
            .write_sample(sample)
            .with_context(|| format!("write wav ({})", path.display()))?;
    }
    writer
        .finalize()
        .with_context(|| format!("write wav ({})", path.display()))?;
    Ok(())
}

/// Read a WAV as mono 16-bit PCM (returns `(sample_rate, samples)`).
///
/// Requires `SampleFormat::Int` with 16 bits/sample (what the ffmpeg tiling
/// step and `hound`-synthetic tests produce); stereo/multi-channel input is
/// downmixed per-frame by mean (integer division). Anything else bails.
pub fn read_wav_i16(path: &Path) -> anyhow::Result<(u32, Vec<i16>)> {
    let mut reader =
        hound::WavReader::open(path).with_context(|| format!("read wav ({})", path.display()))?;
    let spec = reader.spec();
    match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => {}
        (fmt, bits) => bail!(
            "wav ({}): unsupported format {fmt:?}/{bits}-bit, need 16-bit int",
            path.display()
        ),
    }
    let channels = u64::from(spec.channels);
    if channels == 0 {
        bail!("wav ({}): zero channels", path.display());
    }
    let raw: Vec<i16> = reader
        .samples::<i16>()
        .collect::<Result<_, _>>()
        .with_context(|| format!("read wav ({})", path.display()))?;
    if channels == 1 {
        return Ok((spec.sample_rate, raw));
    }
    if !raw.len().is_multiple_of(channels as usize) {
        bail!("wav ({}): truncated interleaved frame", path.display());
    }
    let frames = raw.len() / channels as usize;
    let mut mono = Vec::with_capacity(frames);
    for frame in raw.chunks_exact(channels as usize) {
        let sum: i32 = frame.iter().map(|s| i32::from(*s)).sum();
        mono.push((sum / channels as i32) as i16);
    }
    Ok((spec.sample_rate, mono))
}
