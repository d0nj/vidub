use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tonic::transport::{Channel, ClientTlsConfig};

use super::{ProgressCb, ProgressEvent};

pub mod client;
pub mod config;
pub mod merge;
pub mod slicing;
pub mod wav;

#[cfg(test)]
mod tests;

pub use client::{backoff_delay, is_retryable, parse_retry_after};
pub(crate) use client::{recognize_with_retry, RivaSpeechRecognitionClient};
pub use config::{parse_function_id, whisper_function_id};
pub use merge::merge;
pub use slicing::{slice_16k, windows};
pub(crate) use wav::WavParams;
pub use wav::{chunk_path, load_chunk, parse_wav_header, store_chunk};

pub mod nvidia {
    pub mod riva {
        tonic::include_proto!("nvidia.riva");
        pub mod asr {
            tonic::include_proto!("nvidia.riva.asr");
        }
    }
}

pub const NVCF_ENDPOINT: &str = "https://grpc.nvcf.nvidia.com:443";

pub const WINDOW_MS: u64 = 30_000;
pub const OVERLAP_MS: u64 = 2_000;
pub const STEP_MS: u64 = WINDOW_MS - OVERLAP_MS;

pub const CONCURRENCY: usize = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WhisperSegment {
    pub id: u32,
    pub start_ms: u64,
    pub end_ms: u64,
    pub src_text: String,
    pub src_lang: Option<String>,
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, Default)]
pub struct Transcription {
    pub segments: Vec<WhisperSegment>,
    pub detected_lang: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawSeg {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChunkResult {
    pub index: usize,
    pub window_start_ms: u64,
    pub segments: Vec<RawSeg>,
    pub detected_lang: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkWindow {
    pub index: usize,
    pub start_ms: u64,
    pub path: PathBuf,
}

pub async fn transcribe(
    wav: &Path,
    ffmpeg: &Path,
    key: &str,
    function_id: &str,
    language_code: &str,
    work_dir: &Path,
    progress: &ProgressCb,
) -> Result<Transcription> {
    let wav_bytes = tokio::fs::read(wav)
        .await
        .with_context(|| format!("reading wav {}", wav.display()))?;
    let (params, total_ms) = parse_wav_header(&wav_bytes)
        .with_context(|| format!("parsing wav header {}", wav.display()))?;
    if total_ms == 0 {
        progress(ProgressEvent::Warning("empty audio: no chunks".into()));
        return Ok(Transcription::default());
    }

    progress(ProgressEvent::Stage("slicing"));
    let chunks = slice_16k(wav, ffmpeg, work_dir).await?;
    let total = chunks.len();

    progress(ProgressEvent::Stage("connecting"));
    let channel = Channel::from_static(NVCF_ENDPOINT)
        .tls_config(ClientTlsConfig::new().with_native_roots())
        .context("invalid NVCF TLS config")?
        .connect()
        .await
        .context("connecting to NVCF gRPC")?;

    progress(ProgressEvent::Stage("transcribing"));
    let mut results: Vec<ChunkResult> = Vec::with_capacity(total);
    let mut done = 0usize;

    let mut pending: Vec<ChunkWindow> = Vec::new();
    for chunk in chunks {
        if let Some(saved) = load_chunk(&chunk_path(work_dir, chunk.index)) {
            tracing::info!(chunk = chunk.index, "resuming persisted chunk result");
            results.push(saved);
            done += 1;
            progress(ProgressEvent::ChunkDone { done, total });
        } else {
            pending.push(chunk);
        }
    }

    let (warn_tx, mut warn_rx) = tokio::sync::mpsc::unbounded_channel::<ProgressEvent>();

    let mut running = tokio::task::JoinSet::new();
    let mut iter = pending.into_iter().peekable();
    while iter.peek().is_some() || !running.is_empty() {
        while running.len() < CONCURRENCY {
            let Some(chunk) = iter.next() else { break };
            let channel = channel.clone();
            let key = key.to_string();
            let function_id = function_id.to_string();
            let language_code = language_code.to_string();
            let work_dir = work_dir.to_path_buf();
            let warn_tx = warn_tx.clone();
            running.spawn(async move {
                let mut client = RivaSpeechRecognitionClient::new(channel);
                let result = recognize_with_retry(
                    &mut client,
                    &key,
                    &function_id,
                    &language_code,
                    &chunk,
                    params,
                    &warn_tx,
                )
                .await;
                match result {
                    Ok(chunk_result) => {
                        if let Err(e) =
                            store_chunk(&chunk_path(&work_dir, chunk.index), &chunk_result)
                        {
                            let _ = warn_tx.send(ProgressEvent::Warning(format!(
                                "chunk {}: persist failed: {e}",
                                chunk.index
                            )));
                            tracing::warn!(chunk = chunk.index, error = ?e, "chunk persist failed");
                        }
                        Ok(chunk_result)
                    }
                    Err(e) => Err(e),
                }
            });
        }
        while let Ok(ev) = warn_rx.try_recv() {
            progress(ev);
        }
        match running.join_next().await {
            Some(Ok(Ok(chunk_result))) => {
                results.push(chunk_result);
                done += 1;
                progress(ProgressEvent::ChunkDone { done, total });
            }
            Some(Ok(Err(e))) => anyhow::bail!("{e:#}"),
            Some(Err(join)) => anyhow::bail!("chunk task panicked: {join}"),
            None => break,
        }
    }

    while let Ok(ev) = warn_rx.try_recv() {
        progress(ev);
    }

    progress(ProgressEvent::Stage("merging"));
    results.sort_by_key(|c| c.index);
    let segments = merge(&results);
    let detected_lang = results
        .iter()
        .max_by_key(|c| c.segments.len())
        .and_then(|c| c.detected_lang.clone());
    Ok(Transcription {
        segments,
        detected_lang,
    })
}
