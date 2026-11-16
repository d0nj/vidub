use crate::transcript::probe::args::*;
use crate::transcript::probe::classify::*;
use crate::transcript::probe::run::*;
use crate::transcript::probe::select::*;
use crate::transcript::probe::{CaptionTrack, ProbeError, ProbeInfo};
use std::ffi::OsString;
use std::path::Path;

fn track(lang: &str, ext: &str, url: &str) -> CaptionTrack {
    CaptionTrack {
        lang: lang.to_string(),
        ext: ext.to_string(),
        url: url.to_string(),
    }
}

fn info(
    manual: Vec<CaptionTrack>,
    auto_caps: Vec<CaptionTrack>,
    src_lang: Option<&str>,
) -> ProbeInfo {
    ProbeInfo {
        title: "t".to_string(),
        src_lang: src_lang.map(str::to_string),
        manual,
        auto_caps,
    }
}

#[test]
fn select_prefers_manual_over_auto() {
    let p = info(
        vec![track("fr", "srt", "manual-fr")],
        vec![track("en", "srt", "auto-en")],
        None,
    );
    let picked = select(&p).expect("a caption");
    assert_eq!(picked.url, "manual-fr");
}

#[test]
fn select_src_lang_beats_en_beats_any() {
    let p = info(
        vec![
            track("en", "srt", "en"),
            track("fr", "srt", "fr"),
            track("de", "srt", "de"),
        ],
        vec![],
        Some("de"),
    );
    assert_eq!(select(&p).unwrap().lang, "de");

    let p = info(
        vec![track("fr", "srt", "fr"), track("en", "srt", "en")],
        vec![],
        Some("de"),
    );
    assert_eq!(select(&p).unwrap().lang, "en");

    let p = info(
        vec![track("fr", "srt", "fr"), track("de", "srt", "de")],
        vec![],
        Some("ja"),
    );
    assert_eq!(select(&p).unwrap().lang, "de");
}

#[test]
fn select_en_star_and_primary_subtag_matching() {
    let p = info(
        vec![track("fr", "srt", "fr"), track("en-GB", "srt", "en-GB")],
        vec![],
        Some("en-US"),
    );
    assert_eq!(select(&p).unwrap().lang, "en-GB");

    let p = info(
        vec![track("en", "srt", "en"), track("zh-Hans", "srt", "zh")],
        vec![],
        Some("zh-Hant"),
    );
    assert_eq!(select(&p).unwrap().lang, "zh-Hans");

    let p = info(
        vec![track("fr", "srt", "fr"), track("en-US", "srt", "en-US")],
        vec![],
        None,
    );
    assert_eq!(select(&p).unwrap().lang, "en-US");
}

#[test]
fn select_never_picks_danmaku_or_live_chat() {
    let p = info(
        vec![track("danmaku", "ass", "dm")],
        vec![track("live_chat", "srt", "lc")],
        None,
    );
    assert!(select(&p).is_none());

    let p = info(
        vec![track("danmaku", "srt", "dm"), track("fr", "srt", "fr")],
        vec![],
        None,
    );
    assert_eq!(select(&p).unwrap().lang, "fr");

    let p = info(
        vec![track("danmaku", "srt", "dm")],
        vec![track("en", "srt", "auto-en")],
        None,
    );
    assert_eq!(select(&p).unwrap().url, "auto-en");
}

#[test]
fn select_none_when_empty() {
    assert!(select(&info(vec![], vec![], None)).is_none());
}

#[test]
fn select_prefers_srt_over_vtt_over_json3() {
    let p = info(
        vec![
            track("en", "json3", "json3"),
            track("en", "vtt", "vtt"),
            track("en", "srt", "srt"),
        ],
        vec![],
        Some("en"),
    );
    assert_eq!(select(&p).unwrap().ext, "srt");

    let p = info(
        vec![track("en", "json3", "j"), track("en", "vtt", "v")],
        vec![],
        None,
    );
    assert_eq!(select(&p).unwrap().ext, "vtt");
}

#[test]
fn parse_minimal_probe_json() {
    let json = r#"{
        "title": "Hello",
        "original_language": "en",
        "subtitles": {"en": [{"ext": "srt", "url": "https://x/en.srt"}, {"ext": "vtt", "url": "https://x/en.vtt"}]},
        "automatic_captions": {"fr": [{"ext": "srt", "url": "https://x/fr.srt"}]},
        "requested_subtitles": {"en": {"ext": "srt", "url": "https://x/en.srt"}}
    }"#;
    let p = parse_probe_json(json).expect("valid JSON");
    assert_eq!(p.title, "Hello");
    assert_eq!(p.src_lang.as_deref(), Some("en"));
    assert_eq!(p.manual.len(), 2);
    assert_eq!(p.auto_caps.len(), 1);
    assert_eq!(select(&p).unwrap().ext, "srt");
}

#[test]
fn parse_language_fallback_and_requested_only() {
    let json = r#"{
        "title": "T",
        "language": "de",
        "requested_subtitles": {"de": {"ext": "srt", "url": "https://x/de.srt"}}
    }"#;
    let p = parse_probe_json(json).expect("valid JSON");
    assert_eq!(p.src_lang.as_deref(), Some("de"));
    assert_eq!(p.manual.len(), 1);
    assert_eq!(p.manual[0].lang, "de");
    assert!(p.auto_caps.is_empty());
}

#[test]
fn parse_drops_excluded_langs_and_bad_entries() {
    let json = r#"{
        "title": "T",
        "subtitles": {
            "danmaku": [{"ext": "ass", "url": "https://x/dm.ass"}],
            "live_chat": [{"ext": "srt", "url": "https://x/lc.srt"}],
            "en": [{"ext": "srt", "url": "https://x/en.srt"}, {"ext": "srt"}, {"url": "https://x/noext"}]
        },
        "automatic_captions": {}
    }"#;
    let p = parse_probe_json(json).expect("valid JSON");
    assert_eq!(p.manual.len(), 1);
    assert_eq!(p.manual[0].lang, "en");
    assert!(select(&p).is_some());
}

#[test]
fn parse_rejects_malformed_json() {
    assert!(matches!(
        parse_probe_json("{not json"),
        Err(ProbeError::InvalidJson(_))
    ));
}

fn strs(args: &[OsString]) -> Vec<String> {
    args.iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn probe_args_exact() {
    let got = strs(&probe_args("https://example.invalid/v", None));
    assert_eq!(
        got,
        vec![
            "-J",
            "--skip-download",
            "--no-playlist",
            "https://example.invalid/v"
        ]
    );
}

#[test]
fn probe_args_qjs_flag() {
    let qjs = Path::new("/nonexistent-tools/qjs");
    let got = strs(&probe_args("https://example.invalid/v", Some(qjs)));
    assert_eq!(got.len(), 6);
    assert_eq!(got[3], "--js-runtimes");
    assert!(got[4].starts_with("quickjs:"));
    assert!(got[4].contains("qjs"));
    assert_eq!(got[5], "https://example.invalid/v");
}

#[test]
fn fetch_args_contract() {
    let dir = Path::new("/tmp/vidub-test-captions");
    let got = strs(&fetch_args("https://example.invalid/v", "en", dir, None));
    assert!(got.contains(&"--write-subs".to_string()));
    assert!(got.contains(&"--write-auto-subs".to_string()));
    assert!(got.contains(&"--sub-langs".to_string()));
    assert!(got.contains(&"--sub-format".to_string()));
    assert!(got.contains(&"srt/vtt/json3/best".to_string()));
    assert!(got.contains(&"--convert-subs".to_string()));
    assert!(got.contains(&"--no-playlist".to_string()));
    assert_eq!(got[got.len() - 1], "https://example.invalid/v");
    assert_eq!(
        got[got.len() - 2],
        dir.join("%(id)s.%(ext)s").to_string_lossy().into_owned()
    );
    let i = got.iter().position(|a| a == "--sub-langs").unwrap();
    assert_eq!(got[i + 1], "en");
}

#[test]
fn download_audio_args_contract() {
    let dir = Path::new("/tmp/vidub-test-audio");
    let got = strs(&download_audio_args("https://example.invalid/v", dir, None));
    assert_eq!(got[0], "-f");
    assert_eq!(got[1], "bestaudio");
    assert!(got.contains(&"-x".to_string()));
    assert!(got.contains(&"--audio-format".to_string()));
    assert!(got.contains(&"wav".to_string()));
    assert!(got.contains(&"--no-playlist".to_string()));
    assert_eq!(got[got.len() - 1], "https://example.invalid/v");
}

#[test]
fn normalize_audio_args_contract() {
    let got = strs(&normalize_audio_args(
        Path::new("/tmp/in.wav"),
        Path::new("/tmp/16k.wav"),
    ));
    assert_eq!(got[0], "-y");
    assert_eq!(got[1], "-i");
    assert_eq!(got[2], "/tmp/in.wav");
    let tail = &got[3..got.len() - 1];
    assert_eq!(
        tail,
        ["-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"]
    );
    assert_eq!(got[got.len() - 1], "/tmp/16k.wav");
}

#[test]
fn temp_dir_path_shape() {
    let p = vidub_temp_dir_path("captions");
    assert_eq!(p.parent(), Some(std::env::temp_dir().as_path()));
    assert!(p
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with("vidub-captions-"))
        .unwrap_or(false));
}

#[tokio::test]
async fn missing_binary_errors_without_spawning() {
    assert!(matches!(
        probe(None, "https://example.invalid/v", None).await,
        Err(ProbeError::MissingBinary("yt-dlp"))
    ));
    assert!(matches!(
        fetch(None, "https://example.invalid/v", "en", None).await,
        Err(ProbeError::MissingBinary("yt-dlp"))
    ));
    assert!(matches!(
        extract_audio(None, None, "https://example.invalid/v", None).await,
        Err(ProbeError::MissingBinary(_))
    ));
}

#[tokio::test]
async fn empty_source_rejected_before_spawning() {
    let bogus = Path::new("/definitely/not/a/binary");
    assert!(matches!(
        probe(Some(bogus), "   ", None).await,
        Err(ProbeError::EmptySource)
    ));
    assert!(matches!(
        probe(None, "", None).await,
        Err(ProbeError::EmptySource)
    ));
}

#[test]
fn download_video_args_shape() {
    let out = Path::new("vidub-media").join("deadbeef.mp4");
    let got = strs(&download_video_args(
        "https://example.invalid/watch?v=x",
        &out,
        None,
    ));
    assert_eq!(got.len(), 6);
    assert_eq!(got[0], "-f");
    assert_eq!(got[1], "bv*[ext=mp4]+ba[ext=m4a]/b[ext=mp4]/b");
    assert_eq!(got[2], "--no-playlist");
    assert_eq!(got[3], "-o");
    assert_eq!(got[4], out.to_string_lossy().into_owned());
    assert_eq!(got[5], "https://example.invalid/watch?v=x");
}

#[test]
fn download_video_args_qjs_flag() {
    let out = Path::new("vidub-media").join("deadbeef.mp4");
    let qjs = Path::new("/nonexistent-tools/qjs.exe");
    let got = strs(&download_video_args(
        "https://example.invalid/watch?v=x",
        &out,
        Some(qjs),
    ));
    assert_eq!(got.len(), 8);
    assert_eq!(got[5], "--js-runtimes");
    assert!(got[6].starts_with("quickjs:"));
    assert_eq!(got[7], "https://example.invalid/watch?v=x");
}

#[test]
fn is_youtube_url_table() {
    assert!(is_youtube_url(
        "https://www.youtube.com/watch?v=u6lihZAcy4s"
    ));
    assert!(is_youtube_url("https://youtu.be/u6lihZAcy4s"));
    assert!(is_youtube_url("https://m.youtube.com/watch?v=x"));
    assert!(is_youtube_url("https://music.youtube.com/watch?v=x"));
    assert!(!is_youtube_url("https://www.bilibili.com/video/BV1xx"));
    assert!(!is_youtube_url("https://youtube.com.evil.example/watch"));
    assert!(!is_youtube_url("https://notyoutube.com/watch"));
    assert!(!is_youtube_url("https://example.invalid/v"));
}

#[test]
fn is_client_retryable_table() {
    assert!(is_client_retryable("ERROR: HTTP Error 403: Forbidden"));
    assert!(is_client_retryable("Requested video is forbidden"));
    assert!(is_client_retryable(
        "Failed to decrypt the player token: PO token missing"
    ));
    assert!(is_client_retryable("signed client request rejected"));
    assert!(is_client_retryable(
        "formats skipped: SABR-only; Requested format is not available"
    ));
    assert!(is_client_retryable(
        "ERROR: Requested format is not available"
    ));
    assert!(is_client_retryable("ERROR: The page needs to be reloaded"));
    assert!(is_client_retryable("SABR-only formats skipped"));
    assert!(!is_client_retryable(
        "Private video. Sign in if you've been granted access"
    ));
    assert!(!is_client_retryable("Video unavailable"));
    assert!(!is_client_retryable("Sign in to confirm your age"));
    assert!(!is_client_retryable("network timeout"));
}

#[test]
fn player_client_injection_shape() {
    let base = probe_args("https://youtu.be/x", None);
    let base_strs = strs(&base);
    assert_eq!(strs(&with_player_client(&base, None)), base_strs);
    let got = strs(&with_player_client(&base, Some("android")));
    assert_eq!(got.len(), base_strs.len() + 2);
    assert_eq!(got[got.len() - 3], "--extractor-args");
    assert_eq!(got[got.len() - 2], "youtube:player_client=android");
    assert_eq!(got[got.len() - 1], "https://youtu.be/x");
    assert_eq!(&got[..got.len() - 3], &base_strs[..base_strs.len() - 1]);
}

#[test]
fn classify_fixture_table() {
    let real = "WARNING: No supported JavaScript runtime could be found. Only deno and node are supported. YouTube extraction without a JS runtime might not work. See https://github.com/yt-dlp/yt-dlp#javascript-runtime\nERROR: [youtube] u6lihZAcy4s: HTTP Error 403: Forbidden";
    assert_eq!(classify_failure(real), MSG_FORBIDDEN);
    assert_eq!(classify_failure("ERROR: HTTP Error 403"), MSG_FORBIDDEN);
    assert_eq!(
        classify_failure("PO token verification failed"),
        MSG_FORBIDDEN
    );
    assert_eq!(
        classify_failure("WARNING: No supported JavaScript runtime could be found"),
        MSG_JS_MISSING
    );
    assert_eq!(classify_failure("ERROR: Private video"), MSG_PRIVATE);
    assert_eq!(classify_failure("Video unavailable"), MSG_PRIVATE);
    assert_eq!(classify_failure("This video has been deleted"), MSG_PRIVATE);
    assert_eq!(
        classify_failure("Sign in to confirm your age"),
        MSG_LOGIN_AGE
    );
    assert_eq!(classify_failure("urlopen error timed out"), MSG_NETWORK);
    assert_eq!(classify_failure("Name resolution failed"), MSG_NETWORK);
    assert_eq!(classify_failure("ERROR: something unexpected"), MSG_DEFAULT);
    assert_eq!(classify_failure(""), MSG_DEFAULT);
}

#[test]
fn user_message_mapping() {
    let failed = ProbeError::CommandFailed {
        binary: "yt-dlp",
        code: Some(1),
        stderr: "WARNING: No supported JavaScript runtime could be found\nERROR: [youtube] x: HTTP Error 403: Forbidden".to_string(),
    };
    assert_eq!(user_message(&failed), MSG_FORBIDDEN);
    assert_eq!(
        user_message(&ProbeError::Timeout { binary: "yt-dlp" }),
        MSG_NETWORK
    );
    assert!(user_message(&ProbeError::MissingBinary("yt-dlp")).contains("yt-dlp"));
    assert!(user_message(&ProbeError::MissingBinary("ffmpeg")).contains("ffmpeg"));
    assert_eq!(user_message(&ProbeError::EmptySource), MSG_DEFAULT);
    assert_eq!(
        user_message(&ProbeError::InvalidJson("x".to_string())),
        MSG_DEFAULT
    );
}

#[test]
fn detects_vietnamese_tags() {
    for tag in ["vi", "vi-VN", "vi_HN", "vi-orig"] {
        assert!(is_vietnamese(tag), "{tag} must count as Vietnamese");
    }
    for tag in ["en", "en-US", "zh-Hans", "vix"] {
        assert!(!is_vietnamese(tag), "{tag} must not count as Vietnamese");
    }
}

#[test]
fn native_vietnamese_manual_track_is_preferred() {
    let p = info(
        vec![track("en", "srt", "en"), track("vi", "srt", "vi")],
        Vec::new(),
        None,
    );
    let picked = vietnamese_manual_track(&p).expect("vi track");
    assert_eq!(picked.url, "vi");
}

#[test]
fn no_vietnamese_track_when_only_foreign_captions() {
    let p = info(
        vec![track("en", "srt", "en"), track("ja", "vtt", "ja")],
        vec![track("vi", "vtt", "auto-vi")],
        None,
    );
    assert!(vietnamese_manual_track(&p).is_none());
}

#[test]
fn rejects_non_text_vietnamese_formats() {
    let p = info(
        vec![track("vi", "srv3", "srv"), track("vi", "ttml", "ttml")],
        Vec::new(),
        None,
    );
    assert!(vietnamese_manual_track(&p).is_none());
}
