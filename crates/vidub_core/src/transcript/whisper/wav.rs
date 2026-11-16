//! RIFF/WAVE header parsing plus per-chunk JSON persistence (resume).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::ChunkResult;

/// WAV parameters + total duration read from the RIFF header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavParams {
    pub sample_rate_hz: u32,
    pub channels: u16,
}

/// Parse a RIFF/WAVE header: requires PCM (format 1) so raw bytes match
/// `LINEAR_PCM`; derives duration from the data chunk size.
pub fn parse_wav_header(bytes: &[u8]) -> Result<(WavParams, u64)> {
    fn u16_at(b: &[u8], off: usize) -> Result<u16> {
        b.get(off..off + 2)
            .map(|s| u16::from_le_bytes([s[0], s[1]]))
            .ok_or_else(|| anyhow::anyhow!("truncated wav header"))
    }
    fn u32_at(b: &[u8], off: usize) -> Result<u32> {
        b.get(off..off + 4)
            .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
            .ok_or_else(|| anyhow::anyhow!("truncated wav header"))
    }
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        anyhow::bail!("not a RIFF/WAVE file");
    }
    let mut pos = 12usize;
    let mut params: Option<WavParams> = None;
    let mut data_bytes: Option<usize> = None;
    let mut byte_rate: Option<u32> = None;
    while let Some(hdr) = bytes.get(pos..pos + 8) {
        let id = &hdr[0..4];
        let size = u32_at(bytes, pos + 4)? as usize;
        if id == b"fmt " {
            let format = u16_at(bytes, pos + 8)?;
            if format != 1 {
                anyhow::bail!("expected PCM wav (format 1), got {format}");
            }
            params = Some(WavParams {
                sample_rate_hz: u32_at(bytes, pos + 12)?,
                channels: u16_at(bytes, pos + 10)?,
            });
            byte_rate = Some(u32_at(bytes, pos + 16)?);
        } else if id == b"data" {
            data_bytes = Some(size.min(bytes.len().saturating_sub(pos + 8)));
            break;
        }
        pos += 8 + size + (size % 2); // word-aligned chunks
    }
    let params = params.ok_or_else(|| anyhow::anyhow!("wav missing fmt chunk"))?;
    let rate = byte_rate.unwrap_or_else(|| params.sample_rate_hz * params.channels as u32 * 2);
    if rate == 0 {
        anyhow::bail!("wav zero byte rate");
    }
    let data = data_bytes.ok_or_else(|| anyhow::anyhow!("wav missing data chunk"))? as u64;
    Ok((params, data * 1000 / rate as u64))
}

/// Persisted per-chunk JSON path in the work dir.
pub fn chunk_path(work_dir: &Path, index: usize) -> PathBuf {
    work_dir.join(format!("chunk-{index:04}.json"))
}

/// Load a previously persisted chunk result (resume), if present and valid.
pub fn load_chunk(path: &Path) -> Option<ChunkResult> {
    let json = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&json).ok()
}

/// Persist a chunk result atomically (tmp file + rename) so an interrupted
/// run never leaves half-written JSON behind.
pub fn store_chunk(path: &Path, chunk: &ChunkResult) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string(chunk)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("persisting {}", path.display()))?;
    Ok(())
}
