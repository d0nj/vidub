use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::args::{
    create_vidub_temp_dir, download_audio_args, download_video_args, fetch_args,
    is_client_retryable, is_youtube_url, normalize_audio_args, probe_args,
};
use super::select::parse_probe_json;
use super::{ProbeError, ProbeInfo, CAPTION_EXTS, DOWNLOAD_TIMEOUT, PROBE_TIMEOUT};
use crate::util::{truncate_tail, MAX_STDERR_CHARS};

use vidub_process::hide_console;

async fn run_checked(
    binary: &'static str,
    bin: &Path,
    args: &[OsString],
    timeout: Duration,
) -> Result<std::process::Output, ProbeError> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    hide_console(&mut cmd);
    let output = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| ProbeError::Timeout { binary })?
        .map_err(|source| ProbeError::Io { source })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ProbeError::CommandFailed {
            binary,
            code: output.status.code(),
            stderr: truncate_tail(stderr.trim(), MAX_STDERR_CHARS),
        });
    }
    Ok(output)
}

pub(super) const PLAYER_CLIENTS: [Option<&str>; 4] =
    [None, Some("android"), Some("tv"), Some("mweb")];

pub(super) fn with_player_client(base_args: &[OsString], client: Option<&str>) -> Vec<OsString> {
    let mut args = base_args.to_vec();
    if let Some(name) = client {
        let at = args.len().saturating_sub(1);
        args.splice(
            at..at,
            [
                OsString::from("--extractor-args"),
                OsString::from(format!("youtube:player_client={name}")),
            ],
        );
    }
    args
}

async fn run_ytdlp_with_fallback(
    bin: &Path,
    src: &str,
    qjs: Option<&Path>,
    base_args: &[OsString],
    timeout: Duration,
) -> Result<std::process::Output, ProbeError> {
    if qjs.is_none() {
        tracing::warn!(
            "yt-dlp invoked without JS runtime (qjs missing); YouTube extraction may fail"
        );
    }
    let youtube = is_youtube_url(src);
    let mut attempts = 0usize;
    let mut last_err: Option<ProbeError> = None;
    for client in PLAYER_CLIENTS {
        attempts += 1;
        let args = with_player_client(base_args, client);
        match run_checked("yt-dlp", bin, &args, timeout).await {
            Ok(output) => {
                if attempts > 1 {
                    tracing::info!(
                        attempts,
                        client = client.unwrap_or("default"),
                        "yt-dlp succeeded after player-client fallback"
                    );
                }
                return Ok(output);
            }
            Err(err) => {
                let retry = youtube && is_retryable_error(&err);
                if !retry {
                    return Err(err);
                }
                last_err = Some(err);
            }
        }
    }
    if attempts > 1 {
        tracing::info!(
            attempts,
            "yt-dlp still failing after player-client fallback"
        );
    }
    Err(last_err.unwrap_or(ProbeError::EmptySource))
}

fn is_retryable_error(err: &ProbeError) -> bool {
    match err {
        ProbeError::CommandFailed { stderr, .. } => is_client_retryable(stderr),
        _ => false,
    }
}

#[tracing::instrument(skip(yt_dlp, qjs))]
pub async fn probe(
    yt_dlp: Option<&Path>,
    src: &str,
    qjs: Option<&Path>,
) -> Result<ProbeInfo, ProbeError> {
    if src.trim().is_empty() {
        return Err(ProbeError::EmptySource);
    }
    let bin = yt_dlp.ok_or(ProbeError::MissingBinary("yt-dlp"))?;
    let args = probe_args(src, qjs);
    let output = run_ytdlp_with_fallback(bin, src, qjs, &args, PROBE_TIMEOUT).await?;
    let json = String::from_utf8_lossy(&output.stdout);
    parse_probe_json(&json)
}

#[tracing::instrument(skip(yt_dlp, qjs))]
pub async fn fetch(
    yt_dlp: Option<&Path>,
    src: &str,
    lang: &str,
    qjs: Option<&Path>,
) -> Result<Vec<PathBuf>, ProbeError> {
    let dir = create_vidub_temp_dir("captions").map_err(|source| ProbeError::Io { source })?;
    fetch_to(yt_dlp, src, lang, &dir, qjs).await
}

#[tracing::instrument(skip(yt_dlp, out_dir, qjs))]
pub async fn fetch_to(
    yt_dlp: Option<&Path>,
    src: &str,
    lang: &str,
    out_dir: &Path,
    qjs: Option<&Path>,
) -> Result<Vec<PathBuf>, ProbeError> {
    if src.trim().is_empty() {
        return Err(ProbeError::EmptySource);
    }
    let bin = yt_dlp.ok_or(ProbeError::MissingBinary("yt-dlp"))?;
    std::fs::create_dir_all(out_dir).map_err(|source| ProbeError::Io { source })?;
    let args = fetch_args(src, lang, out_dir, qjs);
    run_ytdlp_with_fallback(bin, src, qjs, &args, DOWNLOAD_TIMEOUT).await?;

    let mut files = Vec::new();
    let entries = std::fs::read_dir(out_dir).map_err(|source| ProbeError::Io { source })?;
    for entry in entries {
        let entry = entry.map_err(|source| ProbeError::Io { source })?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .unwrap_or_default();
        if CAPTION_EXTS.contains(&ext.as_str()) {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

#[tracing::instrument(skip(yt_dlp, ffmpeg, qjs))]
pub async fn extract_audio(
    yt_dlp: Option<&Path>,
    ffmpeg: Option<&Path>,
    src: &str,
    qjs: Option<&Path>,
) -> Result<PathBuf, ProbeError> {
    let dir = create_vidub_temp_dir("audio").map_err(|source| ProbeError::Io { source })?;
    extract_audio_to(yt_dlp, ffmpeg, src, &dir, qjs).await
}

#[tracing::instrument(skip(yt_dlp, ffmpeg, out_dir, qjs))]
pub async fn extract_audio_to(
    yt_dlp: Option<&Path>,
    ffmpeg: Option<&Path>,
    src: &str,
    out_dir: &Path,
    qjs: Option<&Path>,
) -> Result<PathBuf, ProbeError> {
    if src.trim().is_empty() {
        return Err(ProbeError::EmptySource);
    }
    let dl = yt_dlp.ok_or(ProbeError::MissingBinary("yt-dlp"))?;
    let ff = ffmpeg.ok_or(ProbeError::MissingBinary("ffmpeg"))?;
    std::fs::create_dir_all(out_dir).map_err(|source| ProbeError::Io { source })?;

    let dl_args = download_audio_args(src, out_dir, qjs);
    run_ytdlp_with_fallback(dl, src, qjs, &dl_args, DOWNLOAD_TIMEOUT).await?;
    let input = find_downloaded_audio(out_dir)?;
    let output = out_dir.join("16k.wav");
    run_checked(
        "ffmpeg",
        ff,
        &normalize_audio_args(&input, &output),
        DOWNLOAD_TIMEOUT,
    )
    .await?;
    if !output.is_file() {
        return Err(ProbeError::AudioNotFound {
            dir: out_dir.to_path_buf(),
        });
    }
    Ok(output)
}

#[tracing::instrument(skip(ffmpeg, out_dir))]
pub async fn normalize_local_file(
    ffmpeg: Option<&Path>,
    input: &Path,
    out_dir: &Path,
) -> Result<PathBuf, ProbeError> {
    let ff = ffmpeg.ok_or(ProbeError::MissingBinary("ffmpeg"))?;
    std::fs::create_dir_all(out_dir).map_err(|source| ProbeError::Io { source })?;
    let output = out_dir.join("16k.wav");
    run_checked(
        "ffmpeg",
        ff,
        &normalize_audio_args(input, &output),
        DOWNLOAD_TIMEOUT,
    )
    .await?;
    if !output.is_file() {
        return Err(ProbeError::AudioNotFound {
            dir: out_dir.to_path_buf(),
        });
    }
    Ok(output)
}

fn find_downloaded_audio(dir: &Path) -> Result<PathBuf, ProbeError> {
    let entries = std::fs::read_dir(dir).map_err(|source| ProbeError::Io { source })?;
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    for f in &files {
        if f.extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .as_deref()
            == Some("wav")
        {
            return Ok(f.clone());
        }
    }
    files
        .into_iter()
        .next()
        .ok_or_else(|| ProbeError::AudioNotFound {
            dir: dir.to_path_buf(),
        })
}

#[tracing::instrument(skip(yt_dlp, out_path, qjs))]
pub async fn download_video(
    yt_dlp: Option<&Path>,
    src: &str,
    out_path: &Path,
    qjs: Option<&Path>,
) -> Result<PathBuf, ProbeError> {
    if src.trim().is_empty() {
        return Err(ProbeError::EmptySource);
    }
    if let Ok(meta) = std::fs::metadata(out_path) {
        if meta.len() > 0 {
            return Ok(out_path.to_path_buf());
        }
    }
    let bin = yt_dlp.ok_or(ProbeError::MissingBinary("yt-dlp"))?;
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ProbeError::Io { source })?;
    }
    let args = download_video_args(src, out_path, qjs);
    run_ytdlp_with_fallback(bin, src, qjs, &args, DOWNLOAD_TIMEOUT).await?;
    let size = std::fs::metadata(out_path).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        return Err(ProbeError::Io {
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "yt-dlp reported success but {} is missing/empty",
                    out_path.display()
                ),
            ),
        });
    }
    Ok(out_path.to_path_buf())
}
