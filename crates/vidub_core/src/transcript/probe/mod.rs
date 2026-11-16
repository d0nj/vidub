use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;

pub mod args;
pub mod classify;
pub mod run;
pub mod select;

#[cfg(test)]
mod tests;

pub use args::{
    create_vidub_temp_dir, download_audio_args, download_video_args, fetch_args,
    normalize_audio_args, probe_args, vidub_temp_dir_path,
};
pub use classify::{classify_failure, user_message};
pub use run::{
    download_video, extract_audio, extract_audio_to, fetch, fetch_to, normalize_local_file, probe,
};
pub use select::{is_vietnamese, parse_probe_json, select, vietnamese_manual_track};

pub const PROBE_TIMEOUT: Duration = Duration::from_secs(60);
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

const CAPTION_EXTS: [&str; 3] = ["srt", "vtt", "json3"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptionTrack {
    pub lang: String,
    pub ext: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeInfo {
    pub title: String,
    pub src_lang: Option<String>,
    pub manual: Vec<CaptionTrack>,
    pub auto_caps: Vec<CaptionTrack>,
}

impl ProbeInfo {}

#[derive(Debug, Error)]
pub enum ProbeError {
    #[error("required binary missing: {0}")]
    MissingBinary(&'static str),
    #[error("invalid source: empty URL/path")]
    EmptySource,
    #[error("{binary} exited with code {code:?}: {stderr}")]
    CommandFailed {
        binary: &'static str,
        code: Option<i32>,
        stderr: String,
    },
    #[error("{binary} timed out")]
    Timeout { binary: &'static str },
    #[error("io error: {source}")]
    Io {
        #[source]
        source: std::io::Error,
    },
    #[error("invalid yt-dlp JSON: {0}")]
    InvalidJson(String),
    #[error("yt-dlp produced no audio file in {dir}")]
    AudioNotFound { dir: PathBuf },
}

impl From<std::io::Error> for ProbeError {
    fn from(source: std::io::Error) -> Self {
        Self::Io { source }
    }
}
