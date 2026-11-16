use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::segment::TranscriptDocument;
use crate::translate::Translation;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DubSegment {
    pub id: u32,
    pub start_ms: u64,
    pub end_ms: u64,
    pub vi_text: String,
    pub tts_path: Option<PathBuf>,
    pub tts_ms: Option<u64>,
}

pub fn join(doc: &TranscriptDocument, tr: &[Translation]) -> Vec<DubSegment> {
    let by_id: HashMap<u32, &Translation> = tr.iter().map(|t| (t.id, t)).collect();
    doc.segments
        .iter()
        .map(|s| {
            let vi_text = match by_id.get(&s.id) {
                Some(t) => t.vi_text.clone(),
                None => {
                    tracing::warn!(
                        id = s.id,
                        "pipeline: missing translation; vi_text left empty for retranslate flagging"
                    );
                    String::new()
                }
            };
            DubSegment {
                id: s.id,
                start_ms: s.start_ms,
                end_ms: s.end_ms,
                vi_text,
                tts_path: None,
                tts_ms: None,
            }
        })
        .collect()
}

#[derive(Debug)]
pub struct DubRun {
    pub segments: Vec<DubSegment>,
    pub title: String,
    pub wav_path: PathBuf,
    pub video_path: PathBuf,
    pub meta: crate::sync::AssemblyMeta,
}

pub fn media_cache_path(url: &str) -> PathBuf {
    let digest = Sha256::digest(url.as_bytes());
    let stem: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    std::env::temp_dir()
        .join("vidub-media")
        .join(format!("{stem}.mp4"))
}

pub async fn dub_all(
    client: &reqwest::Client,
    sidecars: &vidub_sidecar::BinPaths,
    src: &crate::transcript::Source,
    provider: vidub_config::Provider,
    key: &str,
    progress: &crate::transcript::ProgressCb,
) -> anyhow::Result<DubRun> {
    if key.trim().is_empty() {
        tracing::warn!("dub_all: empty API key");
        anyhow::bail!("Thiếu API key. Vào Cài đặt để thêm.");
    }

    let mut missing_vi: Vec<&str> = Vec::new();
    let mut missing_en: Vec<&str> = Vec::new();
    if sidecars.piper.is_none() {
        missing_vi.push("máy đọc tiếng (Piper)");
        missing_en.push("piper");
    }
    if sidecars.voice_onnx.is_none() {
        missing_vi.push("giọng đọc tiếng Việt");
        missing_en.push("voice_onnx");
    }
    if sidecars.ffmpeg.is_none() {
        missing_vi.push("công cụ video (ffmpeg)");
        missing_en.push("ffmpeg");
    }
    if matches!(src, crate::transcript::Source::Url(_)) && sidecars.yt_dlp.is_none() {
        missing_vi.push("công cụ tải video (yt-dlp)");
        missing_en.push("yt_dlp");
    }
    if !missing_vi.is_empty() {
        tracing::warn!(
            missing = ?missing_en,
            "dub_all: missing sidecar binaries"
        );
        anyhow::bail!(
            "Thiếu công cụ: {}. Hãy vào Cài đặt và bấm Thiết lập tất cả.",
            missing_vi.join(", ")
        );
    }
    let piper = sidecars.piper.clone().expect("piper checked missing above");
    let voice_model = sidecars
        .voice_onnx
        .clone()
        .expect("voice_onnx checked missing above");
    let ffmpeg = sidecars
        .ffmpeg
        .clone()
        .expect("ffmpeg checked missing above");

    let doc = crate::transcript::acquire(src, sidecars, key, progress)
        .await
        .map_err(|err| {
            tracing::warn!("dub_all: acquire failed: {err:#}");
            anyhow::anyhow!("Không thể đọc video: {err}")
        })?;

    let translations = crate::translate::translate_document(client, &doc, key, provider, progress)
        .await
        .map_err(|err| {
            tracing::warn!("dub_all: translate failed: {err:#}");
            anyhow::anyhow!("Không thể dịch: {err}")
        })?;

    let mut segs = join(&doc, &translations);
    if segs.is_empty() {
        tracing::warn!("dub_all: no segments in transcript document");
        anyhow::bail!("Video không có đoạn nào để lồng tiếng.");
    }

    let work_dir = std::env::temp_dir().join("vidub-dub");
    tokio::fs::create_dir_all(&work_dir).await.map_err(|err| {
        tracing::warn!("dub_all: work dir failed: {err}");
        anyhow::anyhow!("Không chuẩn bị được thư mục làm việc: {err}")
    })?;

    let total = segs
        .iter()
        .filter(|seg| !seg.vi_text.trim().is_empty())
        .count();
    if total == 0 {
        tracing::warn!("dub_all: no non-empty translations");
        anyhow::bail!("Không có lời thoại nào để đọc.");
    }

    let semaphore = Arc::new(Semaphore::new(4));
    let mut tasks: JoinSet<Result<(usize, PathBuf, u64), String>> = JoinSet::new();
    for (idx, seg) in segs.iter().enumerate() {
        if seg.vi_text.trim().is_empty() {
            continue;
        }
        let text = seg.vi_text.clone();
        let out_wav = work_dir.join(format!("seg-{:04}.wav", seg.id));
        let piper = piper.clone();
        let model = voice_model.clone();
        let config = voice_model.with_file_name("config.json");
        let sem = semaphore.clone();
        tasks.spawn(async move {
            let _permit = sem
                .acquire_owned()
                .await
                .map_err(|_| "voice semaphore closed".to_owned())?;
            let meta = crate::voice::synthesize(&piper, &model, &config, &text, &out_wav, 1.0)
                .await
                .map_err(|err| err.to_string())?;
            Ok((idx, out_wav, meta.speech_ms))
        });
    }

    let mut stats: Vec<(usize, PathBuf, u64)> = Vec::new();
    let mut first_err: Option<String> = None;
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(Ok((idx, path, speech_ms))) => {
                stats.push((idx, path, speech_ms));
                progress(crate::transcript::ProgressEvent::ChunkDone {
                    done: stats.len(),
                    total,
                });
            }
            Ok(Err(message)) => {
                if first_err.is_none() {
                    first_err = Some(message);
                }
            }
            Err(join_err) => {
                if first_err.is_none() {
                    first_err = Some(join_err.to_string());
                }
            }
        }
    }
    if let Some(first) = first_err {
        tracing::warn!("dub_all: voice synthesis failed: {first}");
        anyhow::bail!("Không thể tạo giọng nói: {first}");
    }
    for (idx, path, speech_ms) in stats {
        if let Some(seg) = segs.get_mut(idx) {
            seg.tts_path = Some(path);
            seg.tts_ms = Some(speech_ms);
        }
    }

    let dub_wav = work_dir.join("dub.wav");
    let meta = crate::sync::assemble(&ffmpeg, &mut segs, &dub_wav, progress)
        .await
        .map_err(|err| {
            tracing::warn!("dub_all: assemble failed: {err:#}");
            anyhow::anyhow!("Không thể ghép âm thanh: {err}")
        })?;

    let video_path = match src {
        crate::transcript::Source::File(path) => {
            if !path.is_file() {
                tracing::warn!("dub_all: video file not found: {}", path.display());
                anyhow::bail!("Không tìm thấy tệp video: {}", path.display());
            }
            path.clone()
        }
        crate::transcript::Source::Url(url) => {
            let out = media_cache_path(url);
            let dir = out.parent().expect("media cache path has a parent");
            tokio::fs::create_dir_all(dir)
                .await
                .with_context(|| format!("preview cache dir ({})", dir.display()))
                .map_err(|err| {
                    tracing::warn!("dub_all: cache dir failed: {err:#}");
                    anyhow::anyhow!("Không chuẩn bị được bộ đệm video: {err}")
                })?;
            crate::transcript::probe::download_video(
                sidecars.yt_dlp.as_deref(),
                url,
                &out,
                sidecars.qjs.as_deref(),
            )
            .await
            .map_err(|err| {
                tracing::warn!("dub_all: video download failed: {err}");
                anyhow::Error::from(err).context("Không thể tải video")
            })?
        }
    };

    Ok(DubRun {
        segments: segs,
        title: doc.title.clone(),
        wav_path: dub_wav,
        video_path,
        meta,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::{TranscriptSegment, TranscriptSource};

    fn doc(segs: Vec<(u32, u64, u64)>) -> TranscriptDocument {
        TranscriptDocument {
            source: TranscriptSource::CaptionManual,
            detected_lang: "en".to_string(),
            segments: segs
                .into_iter()
                .map(|(id, start_ms, end_ms)| TranscriptSegment {
                    id,
                    start_ms,
                    end_ms,
                    src_text: format!("src {id}"),
                    src_lang: Some("en".to_string()),
                    confidence: None,
                })
                .collect(),
            caption_format: Some("srt".to_string()),
            title: String::new(),
        }
    }

    fn tr(id: u32, vi: &str) -> Translation {
        Translation {
            id,
            vi_text: vi.to_string(),
            overlong: false,
        }
    }

    #[test]
    fn join_all_present() {
        let d = doc(vec![(1, 0, 1000), (2, 1000, 2500)]);
        let out = join(&d, &[tr(1, "xin chao"), tr(2, "the gioi")]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, 1);
        assert_eq!(out[0].vi_text, "xin chao");
        assert_eq!(out[1].vi_text, "the gioi");
        assert_eq!((out[1].start_ms, out[1].end_ms), (1000, 2500));
        assert_eq!(out[0].tts_path, None);
        assert_eq!(out[0].tts_ms, None);
    }

    #[test]
    fn join_missing_translation_yields_empty_vi_text() {
        let d = doc(vec![(7, 100, 200), (8, 200, 300)]);
        let out = join(&d, &[tr(7, "da dich")]);
        assert_eq!(out[1].id, 8);
        assert_eq!(out[1].vi_text, "");
    }

    #[test]
    fn join_no_translations_at_all() {
        let d = doc(vec![(1, 0, 500)]);
        let out = join(&d, &[]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].vi_text, "");
    }

    #[test]
    fn join_empty_document_is_empty() {
        let d = doc(vec![]);
        let out = join(&d, &[tr(1, "x")]);
        assert!(out.is_empty());
    }

    #[test]
    fn join_orphan_translation_ignored_and_order_preserved() {
        let d = doc(vec![(3, 3000, 4000), (2, 0, 1000)]);
        let out = join(&d, &[tr(99, "ghost"), tr(2, "hai")]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, 3);
        assert_eq!(out[0].vi_text, "");
        assert_eq!(out[1].id, 2);
        assert_eq!(out[1].vi_text, "hai");
    }
}
