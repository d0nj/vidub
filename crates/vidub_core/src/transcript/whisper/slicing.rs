use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result};
use vidub_process::hide_console;

use super::wav::parse_wav_header;
use super::{ChunkWindow, STEP_MS, WINDOW_MS};

pub fn windows(total_ms: u64) -> Vec<(usize, u64)> {
    let mut out = Vec::new();
    let mut start = 0u64;
    let mut index = 0usize;
    while start < total_ms {
        out.push((index, start));
        start += STEP_MS;
        index += 1;
    }
    out
}

use crate::util::secs;

pub async fn slice_16k(wav: &Path, ffmpeg: &Path, out_dir: &Path) -> Result<Vec<ChunkWindow>> {
    let bytes = tokio::fs::read(wav)
        .await
        .with_context(|| format!("reading wav {}", wav.display()))?;
    let (params, total_ms) = parse_wav_header(&bytes)
        .with_context(|| format!("parsing wav header {}", wav.display()))?;
    tracing::info!(
        sample_rate_hz = params.sample_rate_hz,
        channels = params.channels,
        total_ms,
        "wav header parsed"
    );

    tokio::fs::create_dir_all(out_dir)
        .await
        .with_context(|| format!("creating slice dir {}", out_dir.display()))?;

    let mut chunks = Vec::new();
    for (index, start_ms) in windows(total_ms) {
        let path = out_dir.join(format!("chunk-{index:04}.wav"));
        let mut cmd = tokio::process::Command::new(ffmpeg);
        cmd.args([
            "-ss",
            &secs(start_ms),
            "-t",
            &secs(WINDOW_MS),
            "-i",
            &wav.to_string_lossy(),
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "pcm_s16le",
            &path.to_string_lossy(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        hide_console(&mut cmd);
        let status = cmd
            .status()
            .await
            .with_context(|| format!("spawning ffmpeg for chunk {index}"))?;
        if !status.success() {
            anyhow::bail!("ffmpeg slicing chunk {index} failed ({status})");
        }
        chunks.push(ChunkWindow {
            index,
            start_ms,
            path,
        });
    }
    Ok(chunks)
}
