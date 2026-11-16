//! Transcript acquisition: captions-first (yt-dlp probe → select → fetch →
//! parse), Whisper fallback (16kHz mono → 30s windows → NVCF gRPC).
//!
//! Thin orchestrator. Spawning lives in [`probe`], parsing in [`parse`],
//! chunked transcription in [`whisper`].

use std::path::{Path, PathBuf};

use crate::segment::{TranscriptDocument, TranscriptSegment, TranscriptSource};
use vidub_sidecar::BinPaths;

pub mod parse;
pub mod probe;
pub mod whisper;

pub use probe::{CaptionTrack, ProbeInfo};

/// Transcript input: remote URL (caption probe available) or local file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Url(String),
    File(PathBuf),
}

/// Progress emitted while [`acquire`] runs (callback must never block it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgressEvent {
    Stage(&'static str),
    ChunkDone { done: usize, total: usize },
    Warning(String),
}

/// Progress callback: invoked (never blocks acquisition) as work advances.
pub type ProgressCb = dyn Fn(ProgressEvent) + Send + Sync;

/// Acquire a transcript: probe captions first. Any caption miss (no tracks,
/// fetch/parse failure) falls back to Whisper; only a Whisper failure
/// surfaces as an error.
pub async fn acquire(
    src: &Source,
    sidecars: &BinPaths,
    key: &str,
    progress: &ProgressCb,
) -> anyhow::Result<TranscriptDocument> {
    match src {
        Source::Url(url) => match try_captions(url, sidecars, progress).await {
            Ok(doc) => Ok(doc),
            Err(err) => {
                progress(ProgressEvent::Warning(format!(
                    "caption path unavailable ({err}); falling back to Whisper"
                )));
                transcribe_from_audio(url, sidecars, key, progress).await
            }
        },
        Source::File(path) => match try_local_captions(path, progress) {
            Ok(doc) => Ok(doc),
            Err(err) => {
                progress(ProgressEvent::Warning(format!(
                    "local caption unavailable ({err}); falling back to Whisper"
                )));
                transcribe_local_file(path, sidecars, key, progress).await
            }
        },
    }
}

/// Captions-first attempt: probe → select → fetch → parse.
async fn try_captions(
    src: &str,
    sidecars: &BinPaths,
    progress: &ProgressCb,
) -> anyhow::Result<TranscriptDocument> {
    progress(ProgressEvent::Stage("probing"));
    let info = probe::probe(sidecars.yt_dlp.as_deref(), src, sidecars.qjs.as_deref()).await?;
    // A manual Vietnamese track already IS the target language: fetch it and
    // mark the document as Vietnamese so translation passes it through.
    if let Some(vi_track) = probe::vietnamese_manual_track(&info) {
        progress(ProgressEvent::Stage("fetching captions"));
        let paths = probe::fetch(
            sidecars.yt_dlp.as_deref(),
            src,
            &vi_track.lang,
            sidecars.qjs.as_deref(),
        )
        .await?;
        for path in ranked_candidates(&paths, &vi_track.lang) {
            if let Ok((segments, fmt)) = parse::parse_caption_file(path, Some(&vi_track.lang)) {
                if !segments.is_empty() {
                    tracing::info!(
                        lang = %vi_track.lang,
                        segments = segments.len(),
                        "using native Vietnamese captions (no translation needed)"
                    );
                    return Ok(TranscriptDocument {
                        source: TranscriptSource::CaptionManual,
                        detected_lang: vi_track.lang.clone(),
                        segments,
                        caption_format: Some(fmt),
                        title: info.title.clone(),
                    });
                }
            }
        }
    }
    let track = probe::select(&info).ok_or_else(|| anyhow::anyhow!("no usable caption track"))?;
    let source = if info.manual.iter().any(|t| t == track) {
        TranscriptSource::CaptionManual
    } else {
        TranscriptSource::CaptionAuto
    };
    progress(ProgressEvent::Stage("fetching captions"));
    let paths = probe::fetch(
        sidecars.yt_dlp.as_deref(),
        src,
        &track.lang,
        sidecars.qjs.as_deref(),
    )
    .await?;
    let lang = info.src_lang.clone().unwrap_or_else(|| track.lang.clone());
    for path in ranked_candidates(&paths, &track.lang) {
        if let Ok((segments, fmt)) = parse::parse_caption_file(path, Some(lang.as_str())) {
            if !segments.is_empty() {
                let detected_lang = lang;
                return Ok(TranscriptDocument {
                    source,
                    detected_lang,
                    segments,
                    caption_format: Some(fmt),
                    title: info.title.clone(),
                });
            }
        }
    }
    anyhow::bail!("no parseable caption among fetched files")
}

/// Whisper fallback for URLs: yt-dlp audio → 16kHz mono → chunked transcription.
async fn transcribe_from_audio(
    src: &str,
    sidecars: &BinPaths,
    key: &str,
    progress: &ProgressCb,
) -> anyhow::Result<TranscriptDocument> {
    progress(ProgressEvent::Stage("extracting audio"));
    let wav = probe::extract_audio(
        sidecars.yt_dlp.as_deref(),
        sidecars.ffmpeg.as_deref(),
        src,
        sidecars.qjs.as_deref(),
    )
    .await?;
    transcribe_wav(&wav, src, sidecars, key, progress).await
}

/// Whisper fallback for local files: ffmpeg direct normalize → transcription.
async fn transcribe_local_file(
    path: &Path,
    sidecars: &BinPaths,
    key: &str,
    progress: &ProgressCb,
) -> anyhow::Result<TranscriptDocument> {
    progress(ProgressEvent::Stage("extracting audio"));
    let dir = probe::create_vidub_temp_dir("audio-local")?;
    let wav = probe::normalize_local_file(sidecars.ffmpeg.as_deref(), path, &dir).await?;
    transcribe_wav(&wav, &path.display().to_string(), sidecars, key, progress).await
}

/// Chunked NVCF transcription of a 16kHz mono wav into a Whisper document.
async fn transcribe_wav(
    wav: &Path,
    src_tag: &str,
    sidecars: &BinPaths,
    key: &str,
    progress: &ProgressCb,
) -> anyhow::Result<TranscriptDocument> {
    let ffmpeg = sidecars
        .ffmpeg
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("ffmpeg sidecar missing"))?;
    progress(ProgressEvent::Stage("transcribing"));
    let function_id = whisper::whisper_function_id()?;
    let work_dir = stable_work_dir(src_tag)?;
    let transcription =
        whisper::transcribe(wav, ffmpeg, key, &function_id, "multi", &work_dir, progress).await?;
    let segments: Vec<TranscriptSegment> = transcription
        .segments
        .into_iter()
        .map(|s| TranscriptSegment {
            id: s.id,
            start_ms: s.start_ms,
            end_ms: s.end_ms,
            src_text: s.src_text,
            src_lang: s.src_lang.clone(),
            confidence: s.confidence,
        })
        .collect();
    let detected_lang = transcription
        .detected_lang
        .clone()
        .or_else(|| segments.first().and_then(|s| s.src_lang.clone()))
        .unwrap_or_else(|| "unknown".to_string());
    Ok(TranscriptDocument {
        source: TranscriptSource::Whisper,
        detected_lang,
        segments,
        caption_format: None,
        title: String::new(),
    })
}

/// Local captions: sibling `<stem>[.<lang>].{srt,vtt}` next to the media file.
fn try_local_captions(path: &Path, progress: &ProgressCb) -> anyhow::Result<TranscriptDocument> {
    progress(ProgressEvent::Stage("probing"));
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let mut sibs: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase());
            let stem_hit = p
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s == stem || s.starts_with(&format!("{stem}.")))
                .unwrap_or(false);
            matches!(ext.as_deref(), Some("srt") | Some("vtt")) && stem_hit
        })
        .collect();
    sibs.sort();
    for sib in &sibs {
        if let Ok((segments, fmt)) = parse::parse_caption_file(sib, None) {
            if !segments.is_empty() {
                let detected_lang = file_lang(sib).unwrap_or("unknown").to_string();
                return Ok(TranscriptDocument {
                    source: TranscriptSource::CaptionManual,
                    detected_lang,
                    segments,
                    caption_format: Some(fmt),
                    title: stem.to_string(),
                });
            }
        }
    }
    anyhow::bail!("no parseable sibling caption")
}

/// Stable per-source work dir so chunk persists survive across runs (resume).
fn stable_work_dir(src_tag: &str) -> anyhow::Result<PathBuf> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    src_tag.hash(&mut hasher);
    let dir = std::env::temp_dir()
        .join("vidub-whisper")
        .join(format!("{:016x}", hasher.finish()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Downloaded files whose stem ends in `.<lang>` first, rest in stable order.
fn ranked_candidates<'a>(paths: &'a [PathBuf], lang: &'a str) -> Vec<&'a PathBuf> {
    let mut ranked: Vec<&PathBuf> = paths.iter().collect();
    ranked.sort_by_key(|p| file_lang(p) != Some(lang));
    ranked
}

/// `yt-dlp` names downloads `<id>.<lang>.<ext>`; extract the lang slot.
fn file_lang(path: &Path) -> Option<&str> {
    path.file_stem()?
        .to_str()?
        .rsplit_once('.')
        .map(|(_, lang)| lang)
}
