//! NVCF gRPC `Recognize` client: retry/backoff policy and response mapping.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use tonic::metadata::{MetadataMap, MetadataValue};
use tonic::transport::Channel;
use tonic::{Request, Status};

pub(crate) use super::nvidia::riva::asr::riva_speech_recognition_client::RivaSpeechRecognitionClient;
use super::nvidia::riva::asr::{RecognitionConfig, RecognizeRequest};
use super::nvidia::riva::{AudioEncoding, RequestId};
use super::{ChunkResult, ChunkWindow, ProgressEvent, RawSeg, WavParams, WINDOW_MS};

const BACKOFF_BASE: Duration = Duration::from_millis(500);
const BACKOFF_CAP: Duration = Duration::from_secs(30);
const MAX_ATTEMPTS: u32 = 6;

pub fn is_retryable(status: &Status) -> bool {
    use tonic::Code::*;
    matches!(
        status.code(),
        ResourceExhausted | Internal | Unknown | Aborted | Unavailable | DataLoss
    )
}

/// `Retry-After` header (delta-seconds), if the server sent one.
pub fn parse_retry_after(status: &Status) -> Option<Duration> {
    status
        .metadata()
        .get("retry-after")?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// Deterministic jitter in [-0.25, +0.25] from a seed (no rand dep needed).
pub(super) fn jitter_fraction(seed: u64) -> f32 {
    let mut h = DefaultHasher::new();
    seed.hash(&mut h);
    let frac = (h.finish() % 10_000) as f32 / 10_000.0;
    (frac - 0.5) * 0.5
}

/// Exponential backoff for attempt `n` (0-based): base*2^n capped at 30 s,
/// with ±25% jitter; never shorter than the server's `Retry-After`.
pub fn backoff_delay(attempt: u32, retry_after: Option<Duration>, seed: u64) -> Duration {
    let capped = BACKOFF_BASE
        .checked_mul(2u32.pow(attempt.min(10)))
        .map(|d| d.min(BACKOFF_CAP))
        .unwrap_or(BACKOFF_CAP);
    let millis = capped.as_millis() as f64;
    let jittered = millis * (1.0 + jitter_fraction(seed + attempt as u64) as f64);
    let delay = Duration::from_millis(jittered.round() as u64).max(Duration::from_millis(1));
    delay.max(retry_after.unwrap_or(Duration::ZERO))
}

/// Current sub-second seed for jitter.
fn jitter_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// One unary `Recognize` with retry-on-429/5xx (exp backoff + jitter,
/// honoring `Retry-After`). Non-retryable statuses fail immediately.
pub(crate) async fn recognize_with_retry(
    client: &mut RivaSpeechRecognitionClient<Channel>,
    key: &str,
    function_id: &str,
    language_code: &str,
    chunk: &ChunkWindow,
    params: WavParams,
    warn_tx: &tokio::sync::mpsc::UnboundedSender<ProgressEvent>,
) -> Result<ChunkResult> {
    let audio = tokio::fs::read(&chunk.path)
        .await
        .with_context(|| format!("reading chunk {}", chunk.path.display()))?;
    let req = RecognizeRequest {
        config: Some(RecognitionConfig {
            encoding: AudioEncoding::LinearPcm as i32,
            sample_rate_hertz: params.sample_rate_hz as i32,
            language_code: language_code.to_string(),
            audio_channel_count: params.channels as i32,
            enable_word_time_offsets: true,
            max_alternatives: 1,
            ..Default::default()
        }),
        audio,
        id: Some(RequestId {
            value: format!("chunk-{}", chunk.index),
        }),
    };

    let mut attempt = 0u32;
    loop {
        let mut req = Request::new(req.clone());
        let mut md = MetadataMap::new();
        md.insert(
            "function-id",
            MetadataValue::from_str(function_id).context("invalid function-id metadata value")?,
        );
        md.insert(
            "authorization",
            MetadataValue::from_str(&format!("Bearer {key}"))
                .context("invalid authorization metadata value")?,
        );
        *req.metadata_mut() = md;

        match client.recognize(req).await {
            Ok(resp) => return Ok(response_to_chunk(chunk, &resp.into_inner())),
            Err(status) if is_retryable(&status) && attempt + 1 < MAX_ATTEMPTS => {
                let retry_after = parse_retry_after(&status);
                let delay = backoff_delay(attempt, retry_after, jitter_seed());
                let _ = warn_tx.send(ProgressEvent::Warning(format!(
                    "chunk {}: retryable {} ({}), backing off {delay:?}",
                    chunk.index,
                    status.code(),
                    status.message()
                )));
                tracing::warn!(chunk = chunk.index, attempt, ?delay, "NVCF retryable error");
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
            Err(status) => anyhow::bail!(
                "chunk {}: NVCF Recognize failed: {}: {}",
                chunk.index,
                status.code(),
                status.message()
            ),
        }
    }
}

/// Map a `RecognizeResponse` to word-level segments (window-relative) plus
/// the chunk's detected language.
pub(super) fn response_to_chunk(
    chunk: &ChunkWindow,
    resp: &super::nvidia::riva::asr::RecognizeResponse,
) -> ChunkResult {
    let mut segments = Vec::new();
    let mut detected_lang: Option<String> = None;
    for result in &resp.results {
        let Some(alt) = result.alternatives.first() else {
            continue;
        };
        if detected_lang.is_none() {
            detected_lang = alt.language_code.first().cloned();
        }
        if alt.words.is_empty() {
            // Whisper NIMs often omit word timings (observed live):
            // fall back to one segment spanning the window.
            let text = alt.transcript.trim().to_string();
            if text.is_empty() {
                tracing::warn!("NVCF alternative without transcript; skipped");
                continue;
            }
            segments.push(RawSeg {
                start_ms: 0,
                end_ms: WINDOW_MS,
                text,
                confidence: (alt.confidence > 0.0).then_some(alt.confidence),
            });
            continue;
        }
        for word in &alt.words {
            let text = word.word.trim().to_string();
            if text.is_empty() || word.end_time <= word.start_time {
                continue;
            }
            let confidence = if word.confidence > 0.0 {
                Some(word.confidence)
            } else if alt.confidence > 0.0 {
                Some(alt.confidence)
            } else {
                None
            };
            segments.push(RawSeg {
                start_ms: word.start_time.max(0) as u64,
                end_ms: word.end_time.max(0) as u64,
                text,
                confidence,
            });
        }
    }
    ChunkResult {
        index: chunk.index,
        window_start_ms: chunk.start_ms,
        segments,
        detected_lang,
    }
}
