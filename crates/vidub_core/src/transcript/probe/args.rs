//! Pure argv builders and URL/stderr predicates for yt-dlp, plus temp-dir
//! helpers. No spawning.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// `--js-runtimes quickjs:<abs>` argv pair when `qjs` is staged. The path is
/// canonicalized when it exists (yt-dlp wants an absolute exe path); a
/// missing/unresolvable path falls back to its display form. `None` omits
/// the flag — the async runners warn in that case (dev-without-staging).
fn js_runtime_args(qjs: Option<&Path>) -> Vec<OsString> {
    match qjs {
        Some(p) => {
            let abs = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
            vec![
                OsString::from("--js-runtimes"),
                OsString::from(format!("quickjs:{}", abs.display())),
            ]
        }
        None => Vec::new(),
    }
}

/// `true` when `src` is a YouTube URL (host-based; `youtu.be` included).
/// Player-client fallback applies to YouTube only — other sites keep the
/// single original attempt.
pub(super) fn is_youtube_url(src: &str) -> bool {
    let s = src.trim().to_lowercase();
    let after_scheme = s.split("://").nth(1).unwrap_or(&s);
    let host = after_scheme.split('/').next().unwrap_or("");
    let host = host
        .split('@')
        .next_back()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    host == "youtube.com"
        || host.ends_with(".youtube.com")
        || host == "youtu.be"
        || host.ends_with(".youtu.be")
}

/// `true` when a YouTube failure is worth retrying with the next player
/// client: the HTTP 403 family (bare `403` / `forbidden`, PO-token
/// rejection, signed-client complaint) PLUS the per-client failures seen
/// live (SABR-only formats skipped → "Requested format is not available",
/// "page needs to be reloaded"). Case-insensitive.
///
/// Terminal states (private/deleted, login/age-gate, copyright, unsupported
/// URL) deliberately do NOT retry — they go straight to guidance (see
/// [`super::run::classify_failure`]). Solving PO tokens (bgutil-style) is out
/// of scope; this predicate only switches clients around the block.
pub(super) fn is_client_retryable(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    s.contains("403")
        || s.contains("forbidden")
        || s.contains("po token")
        || s.contains("po_token")
        || s.contains("potoken")
        || (s.contains("signed") && s.contains("client"))
        || s.contains("requested format is not available")
        || s.contains("page needs to be reloaded")
        || s.contains("sabr")
}

/// `true` for caption languages that must NEVER be selected
/// (`danmaku` = Bilibili comment stream, `live_chat` = YouTube live chat).
pub fn is_excluded_lang(lang: &str) -> bool {
    lang.eq_ignore_ascii_case("danmaku") || lang.eq_ignore_ascii_case("live_chat")
}

/// yt-dlp probe args: `-J --skip-download --no-playlist [--js-runtimes
/// quickjs:<abs>] <src>` (60s timeout applied by [`super::run::probe`]).
pub fn probe_args(src: &str, qjs: Option<&Path>) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("-J"),
        OsString::from("--skip-download"),
        OsString::from("--no-playlist"),
    ];
    args.extend(js_runtime_args(qjs));
    args.push(OsString::from(src));
    args
}

/// yt-dlp caption fetch args. Downloads every language except `live_chat`,
/// converts everything to SRT (kills Bilibili ASS arrivals) into
/// `<out_dir>/%(id)s.%(ext)s` (yt-dlp emits `<id>.<lang>.srt` per lang).
pub fn fetch_args(src: &str, lang: &str, out_dir: &Path, qjs: Option<&Path>) -> Vec<OsString> {
    let tmpl = out_dir.join("%(id)s.%(ext)s");
    let mut args = vec![
        OsString::from("--skip-download"),
        OsString::from("--write-subs"),
        OsString::from("--write-auto-subs"),
        OsString::from("--sub-langs"),
        // Selected language ONLY: fetching `all` fires one timedtext request
        // per lang×format and trips YouTube 429s (observed live).
        OsString::from(lang),
        OsString::from("--sub-format"),
        OsString::from("srt/vtt/json3/best"),
        OsString::from("--convert-subs"),
        OsString::from("srt"),
        OsString::from("--no-playlist"),
        OsString::from("-o"),
        tmpl.into_os_string(),
    ];
    args.extend(js_runtime_args(qjs));
    args.push(OsString::from(src));
    args
}

/// yt-dlp bestaudio download args: `-f bestaudio -x --audio-format wav`
/// into `<out_dir>/%(id)s.%(ext)s`. Output is re-encoded by ffmpeg to the
/// canonical 16kHz mono form — never trust yt-dlp's wav sample rate.
pub fn download_audio_args(src: &str, out_dir: &Path, qjs: Option<&Path>) -> Vec<OsString> {
    let tmpl = out_dir.join("%(id)s.%(ext)s");
    let mut args = vec![
        OsString::from("-f"),
        OsString::from("bestaudio"),
        OsString::from("-x"),
        OsString::from("--audio-format"),
        OsString::from("wav"),
        OsString::from("--no-playlist"),
        OsString::from("-o"),
        tmpl.into_os_string(),
    ];
    args.extend(js_runtime_args(qjs));
    args.push(OsString::from(src));
    args
}

/// ffmpeg normalise args: `-vn -ac 1 -ar 16000 -c:a pcm_s16le`
/// (16kHz mono s16le WAV — the Whisper contract).
pub fn normalize_audio_args(input: &Path, output: &Path) -> Vec<OsString> {
    vec![
        OsString::from("-y"),
        OsString::from("-i"),
        input.as_os_str().to_os_string(),
        OsString::from("-vn"),
        OsString::from("-ac"),
        OsString::from("1"),
        OsString::from("-ar"),
        OsString::from("16000"),
        OsString::from("-c:a"),
        OsString::from("pcm_s16le"),
        output.as_os_str().to_os_string(),
    ]
}

/// Path (not yet created) for a fresh `temp_dir()/vidub-<prefix>-<pid>-<nanos>`
/// directory. Pure — safe to call in unit tests.
pub fn vidub_temp_dir_path(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("vidub-{prefix}-{}-{nanos}", std::process::id()))
}

/// Create a unique `temp_dir()/vidub-<prefix>-*` dir (retries on collision).
pub fn create_vidub_temp_dir(prefix: &str) -> std::io::Result<PathBuf> {
    for _ in 0..10 {
        let path = vidub_temp_dir_path(prefix);
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("could not create unique vidub-{prefix} temp dir"),
    ))
}

/// yt-dlp video download args: `-f bv*[ext=mp4]+ba[ext=m4a]/b[ext=mp4]/b
/// --no-playlist -o <out> <src>`. Prefers an MP4 video+audio pair (preview
/// plays it directly), then any MP4, then any format.
pub fn download_video_args(src: &str, out_path: &Path, qjs: Option<&Path>) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("-f"),
        OsString::from("bv*[ext=mp4]+ba[ext=m4a]/b[ext=mp4]/b"),
        OsString::from("--no-playlist"),
        OsString::from("-o"),
        out_path.as_os_str().to_os_string(),
    ];
    args.extend(js_runtime_args(qjs));
    args.push(OsString::from(src));
    args
}
