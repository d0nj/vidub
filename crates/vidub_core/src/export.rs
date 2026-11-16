use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context};

use vidub_process::hide_console;

use crate::transcript::{ProgressCb, ProgressEvent};
use crate::util::{truncate_tail, MAX_STDERR_CHARS};

const RESAMPLE_TIMEOUT: Duration = Duration::from_secs(120);

const MEASURE_TIMEOUT: Duration = Duration::from_secs(300);

const MUX_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoudnessMeasured {
    pub i: f32,
    pub tp: f32,
    pub lra: f32,
    pub thresh: f32,
    pub offset: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportMeta {
    pub out_path: PathBuf,
    pub measured: LoudnessMeasured,
    pub dynamic_fallback: bool,
}

fn os(s: &str) -> OsString {
    OsString::from(s)
}

fn os_path(p: &Path) -> OsString {
    p.as_os_str().to_os_string()
}

fn mix_graph() -> &'static str {
    "[1:a]apad,asplit=2[vi][sc];[0:a][sc]sidechaincompress=threshold=0.063:ratio=4:attack=20:release=350:makeup=1[ducked];[vi][ducked]amix=inputs=2:duration=longest:normalize=0[mix]"
}

const TARGET_LRA: f32 = 20.0;

fn measure_filter(target_lufs: f32) -> String {
    format!(
        "{};[mix]loudnorm=I={target_lufs}:TP=-1:LRA={TARGET_LRA}:print_format=json,aresample=48000[aout]",
        mix_graph()
    )
}

fn mux_filter(measured: &LoudnessMeasured, target_lufs: f32) -> String {
    format!(
        "{};[mix]loudnorm=I={target_lufs}:TP=-1:LRA={TARGET_LRA}:measured_I={:.2}:measured_TP={:.2}:measured_LRA={:.2}:measured_thresh={:.2}:offset={:.2}:linear=true,aresample=48000[aout]",
        mix_graph(),
        measured.i,
        measured.tp,
        measured.lra,
        measured.thresh,
        measured.offset,
    )
}

pub fn resample_args(dub_wav: &Path, cache_wav: &Path) -> Vec<OsString> {
    vec![
        os("-y"),
        os("-i"),
        os_path(dub_wav),
        os("-af"),
        os("aresample=48000,aformat=channel_layouts=stereo"),
        os("-c:a"),
        os("pcm_s16le"),
        os_path(cache_wav),
    ]
}

pub fn measure_args(video: &Path, dub_wav_48k: &Path, target_lufs: f32) -> Vec<OsString> {
    vec![
        os("-i"),
        os_path(video),
        os("-i"),
        os_path(dub_wav_48k),
        os("-filter_complex"),
        os(&measure_filter(target_lufs)),
        os("-map"),
        os("[aout]"),
        os("-f"),
        os("null"),
        os("-"),
    ]
}

pub fn mux_args(
    video: &Path,
    dub_wav_48k: &Path,
    out_mp4: &Path,
    measured: &LoudnessMeasured,
    target_lufs: f32,
) -> Vec<OsString> {
    vec![
        os("-y"),
        os("-i"),
        os_path(video),
        os("-i"),
        os_path(dub_wav_48k),
        os("-filter_complex"),
        os(&mux_filter(measured, target_lufs)),
        os("-map"),
        os("0:v"),
        os("-map"),
        os("[aout]"),
        os("-c:v"),
        os("copy"),
        os("-c:a"),
        os("aac"),
        os("-b:a"),
        os("192k"),
        os("-shortest"),
        os_path(out_mp4),
    ]
}

fn resample_cache_path(dub_wav: &Path) -> PathBuf {
    match dub_wav.file_stem() {
        Some(stem) => dub_wav.with_file_name(format!("{}.48k.wav", stem.to_string_lossy())),
        None => dub_wav.with_file_name("dub.48k.wav"),
    }
}

fn extract_last_json_object(stderr: &str) -> Option<&str> {
    let start = stderr.rfind('{')?;
    let end = stderr.rfind('}')?;
    if end < start {
        return None;
    }
    Some(&stderr[start..=end])
}

fn value_f32(value: &serde_json::Value, keys: &[&str]) -> Option<f32> {
    for key in keys {
        match value.get(key) {
            Some(serde_json::Value::Number(n)) => {
                if let Some(f) = n.as_f64() {
                    return Some(f as f32);
                }
            }
            Some(serde_json::Value::String(s)) => {
                if let Ok(f) = s.parse::<f32>() {
                    return Some(f);
                }
            }
            _ => {}
        }
    }
    None
}

fn clean_json(raw: &str) -> String {
    let end = raw.rfind('}').unwrap_or(raw.len());
    let mut body = raw[..end].trim_end().to_string();
    if body.ends_with(',') {
        body.pop();
    }
    body.push('}');
    body
}
pub fn parse_measured_json(stderr: &str) -> anyhow::Result<LoudnessMeasured> {
    let raw = extract_last_json_object(stderr)
        .context("ffmpeg stderr contains no loudnorm JSON measurement")?;
    let value: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => serde_json::from_str(&clean_json(raw)).with_context(|| {
            format!("failed to parse loudnorm JSON: {}", truncate_tail(raw, 512))
        })?,
    };

    let tail = truncate_tail(stderr.trim(), MAX_STDERR_CHARS);
    let missing = |keys: &[&str]| format!("loudnorm JSON misses {}: {}", keys.join(" / "), tail);
    let i = value_f32(&value, &["input_i", "measured_I", "measured_i"])
        .ok_or_else(|| anyhow::anyhow!(missing(&["input_i", "measured_I"])))?;
    let tp = value_f32(&value, &["input_tp", "measured_TP", "measured_tp"])
        .ok_or_else(|| anyhow::anyhow!(missing(&["input_tp", "measured_TP"])))?;
    let lra = value_f32(&value, &["input_lra", "measured_LRA", "measured_lra"])
        .ok_or_else(|| anyhow::anyhow!(missing(&["input_lra", "measured_LRA"])))?;
    let thresh = value_f32(&value, &["input_thresh", "measured_thresh"])
        .ok_or_else(|| anyhow::anyhow!(missing(&["input_thresh", "measured_thresh"])))?;
    let offset = value_f32(&value, &["target_offset", "offset"])
        .ok_or_else(|| anyhow::anyhow!(missing(&["target_offset", "offset"])))?;

    Ok(LoudnessMeasured {
        i,
        tp,
        lra,
        thresh,
        offset,
    })
}

fn json_normalization_type(stderr: &str) -> Option<String> {
    let raw = extract_last_json_object(stderr)?;
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    value
        .get("normalization_type")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

pub fn dynamic_fallback_detected(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    if lower.contains("converted network to dynamic") {
        return true;
    }
    match json_normalization_type(stderr) {
        Some(ty) => !ty.eq_ignore_ascii_case("linear"),
        None => true,
    }
}

async fn run_ffmpeg(
    ffmpeg: &Path,
    args: &[OsString],
    timeout: Duration,
    what: &str,
) -> anyhow::Result<std::process::Output> {
    let mut cmd = tokio::process::Command::new(ffmpeg);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    hide_console(&mut cmd);
    let child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn ffmpeg ({})", ffmpeg.display()))?;

    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| anyhow::anyhow!("ffmpeg {} timed out after {}s", what, timeout.as_secs()))?
        .with_context(|| format!("failed to wait for ffmpeg ({})", ffmpeg.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "ffmpeg {} exited with code {:?}: {}",
            what,
            output.status.code(),
            truncate_tail(stderr.trim(), MAX_STDERR_CHARS)
        );
    }
    Ok(output)
}

async fn ensure_resampled(ffmpeg: &Path, dub_wav: &Path) -> anyhow::Result<PathBuf> {
    let cache = resample_cache_path(dub_wav);
    let fresh = match (
        std::fs::metadata(&cache).and_then(|m| m.modified()),
        std::fs::metadata(dub_wav).and_then(|m| m.modified()),
    ) {
        (Ok(cache_t), Ok(dub_t)) => cache_t >= dub_t,
        _ => false,
    };
    if fresh {
        return Ok(cache);
    }
    run_ffmpeg(
        ffmpeg,
        &resample_args(dub_wav, &cache),
        RESAMPLE_TIMEOUT,
        "resample",
    )
    .await?;
    Ok(cache)
}

pub async fn export_video(
    ffmpeg: &Path,
    video: &Path,
    dub_wav: &Path,
    out_mp4: &Path,
    target_lufs: f32,
    progress: &ProgressCb,
) -> anyhow::Result<ExportMeta> {
    if !ffmpeg.is_file() {
        bail!("required binary missing: ffmpeg ({})", ffmpeg.display());
    }
    if !video.is_file() {
        bail!("required input missing: source video ({})", video.display());
    }
    if !dub_wav.is_file() {
        bail!("required input missing: dub wav ({})", dub_wav.display());
    }

    progress(ProgressEvent::Stage("resampling"));
    let dub_wav_48k = ensure_resampled(ffmpeg, dub_wav).await?;

    progress(ProgressEvent::Stage("measuring"));
    let measure_output = run_ffmpeg(
        ffmpeg,
        &measure_args(video, &dub_wav_48k, target_lufs),
        MEASURE_TIMEOUT,
        "measure",
    )
    .await?;
    let measure_stderr = String::from_utf8_lossy(&measure_output.stderr).into_owned();
    let measured = parse_measured_json(&measure_stderr).with_context(|| {
        format!(
            "failed to parse loudnorm measurement: {}",
            truncate_tail(measure_stderr.trim(), MAX_STDERR_CHARS)
        )
    })?;

    progress(ProgressEvent::Stage("muxing"));
    let mux_args_vec = mux_args(video, &dub_wav_48k, out_mp4, &measured, target_lufs);
    let mux_output = run_ffmpeg(ffmpeg, &mux_args_vec, MUX_TIMEOUT, "mux").await?;
    let mux_stderr = String::from_utf8_lossy(&mux_output.stderr).into_owned();

    let out_meta = std::fs::metadata(out_mp4)
        .with_context(|| format!("exported file missing: {}", out_mp4.display()))?;
    if out_meta.len() == 0 {
        bail!("exported file is empty: {}", out_mp4.display());
    }

    let combined_stderr = format!("{measure_stderr}\n{mux_stderr}");
    let dynamic_fallback = dynamic_fallback_detected(&combined_stderr);
    if dynamic_fallback {
        progress(ProgressEvent::Warning(
            "loudnorm chuyển sang chuẩn hoá động (âm nền bị nâng ở chỗ lặng)".to_string(),
        ));
    }

    Ok(ExportMeta {
        out_path: out_mp4.to_path_buf(),
        measured,
        dynamic_fallback,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const LOUDNORM_JSON: &str = r#"ffmpeg version 7.1 Copyright (c) 2000-2025 the FFmpeg developers
[Parsed_loudnorm_0 @ 0x562f2c1c55a0] {
	"input_i" : "-19.37",
	"input_tp" : "-1.80",
	"input_lra" : "4.60",
	"input_thresh" : "-29.40",
	"output_i" : "-16.00",
	"output_tp" : "-1.00",
	"output_lra" : "4.60",
	"output_thresh" : "-26.00",
	"normalization_type" : "dynamic",
	"target_offset" : "-0.40",
}
size=     123KiB time=00:00:19.10 bitrate=  52.8kbits/s speed= 410x"#;

    fn measured() -> LoudnessMeasured {
        LoudnessMeasured {
            i: -19.37,
            tp: -1.80,
            lra: 4.60,
            thresh: -29.40,
            offset: -0.40,
        }
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn resample_args_exact() {
        let dub = Path::new("C:/proj/dub.wav");
        let cache = Path::new("C:/proj/dub.48k.wav");
        let args = strings(&resample_args(dub, cache));
        assert_eq!(
            args,
            [
                "-y",
                "-i",
                "C:/proj/dub.wav",
                "-af",
                "aresample=48000,aformat=channel_layouts=stereo",
                "-c:a",
                "pcm_s16le",
                "C:/proj/dub.48k.wav"
            ]
        );
    }

    #[test]
    fn cache_path_siblings_named() {
        assert_eq!(
            resample_cache_path(Path::new("dir/dub.wav")),
            PathBuf::from("dir/dub.48k.wav")
        );
        assert_eq!(
            resample_cache_path(Path::new("my.dub.wav")),
            PathBuf::from("my.dub.48k.wav")
        );
    }

    #[test]
    fn measure_args_builds_mix_measure_graph() {
        let args = strings(&measure_args(
            Path::new("v.mp4"),
            Path::new("d.48k.wav"),
            -14.0,
        ));
        assert_eq!(
            args,
            [
                "-i",
                "v.mp4",
                "-i",
                "d.48k.wav",
                "-filter_complex",
                "[1:a]apad,asplit=2[vi][sc];[0:a][sc]sidechaincompress=threshold=0.063:ratio=4:attack=20:release=350:makeup=1[ducked];[vi][ducked]amix=inputs=2:duration=longest:normalize=0[mix];[mix]loudnorm=I=-14:TP=-1:LRA=20:print_format=json,aresample=48000[aout]",
                "-map",
                "[aout]",
                "-f",
                "null",
                "-",
            ]
        );
    }

    #[test]
    fn mux_args_builds_linear_graph_and_maps() {
        let args = strings(&mux_args(
            Path::new("v.mp4"),
            Path::new("d.48k.wav"),
            Path::new("out_vi.mp4"),
            &measured(),
            -14.0,
        ));
        assert_eq!(
            args,
            [
                "-y",
                "-i",
                "v.mp4",
                "-i",
                "d.48k.wav",
                "-filter_complex",
                "[1:a]apad,asplit=2[vi][sc];[0:a][sc]sidechaincompress=threshold=0.063:ratio=4:attack=20:release=350:makeup=1[ducked];[vi][ducked]amix=inputs=2:duration=longest:normalize=0[mix];[mix]loudnorm=I=-14:TP=-1:LRA=20:measured_I=-19.37:measured_TP=-1.80:measured_LRA=4.60:measured_thresh=-29.40:offset=-0.40:linear=true,aresample=48000[aout]",
                "-map",
                "0:v",
                "-map",
                "[aout]",
                "-c:v",
                "copy",
                "-c:a",
                "aac",
                "-b:a",
                "192k",
                "-shortest",
                "out_vi.mp4",
            ]
        );
    }

    #[test]
    fn parse_real_loudnorm_json_with_extra_keys() {
        let m = parse_measured_json(LOUDNORM_JSON).unwrap();
        assert_eq!(m, measured());
    }

    #[test]
    fn parse_accepts_measured_variant_with_numeric_values() {
        let raw = r#"some log lines
{
    "measured_I" : -19.37,
    "measured_TP" : -1.80,
    "measured_LRA" : 4.60,
    "measured_thresh" : -29.40,
    "normalization_type" : "linear",
    "target_offset" : -0.40
}
"#;
        let m = parse_measured_json(raw).unwrap();
        assert_eq!(m, measured());
    }

    #[test]
    fn parse_missing_key_errors_with_stderr_tail() {
        let raw = r#"log line
{
    "input_i" : "-19.37",
    "input_tp" : "-1.80"
}
"#;
        let err = parse_measured_json(raw).unwrap_err();
        assert!(
            err.to_string().contains("input_lra") || err.to_string().contains("measured_LRA"),
            "err={err}"
        );
        assert!(
            err.to_string().contains("log line") || err.to_string().contains("input_i"),
            "err={err}"
        );
    }

    #[test]
    fn parse_garbage_errors() {
        assert!(parse_measured_json("no json at all").is_err());
        assert!(parse_measured_json(r#"{not"valid"json"#).is_err());
        assert!(parse_measured_json("").is_err());
    }

    #[test]
    fn fallback_on_explicit_revert_message() {
        let stderr = "[Parsed_loudnorm_0 @ 0x1] Converted network to dynamic.\n";
        assert!(dynamic_fallback_detected(stderr));
    }

    #[test]
    fn fallback_on_dynamic_normalization_type() {
        assert!(dynamic_fallback_detected(LOUDNORM_JSON));
    }

    #[test]
    fn linear_type_is_not_fallback() {
        let raw = r#"{ "input_i" : "-19.37", "normalization_type" : "linear" }"#;
        assert!(!dynamic_fallback_detected(raw));
    }

    #[test]
    fn missing_confirmation_is_fallback() {
        assert!(dynamic_fallback_detected(""));
        assert!(dynamic_fallback_detected("plain log lines, no json"));
    }

    fn noop_progress() -> Box<dyn Fn(ProgressEvent) + Send + Sync> {
        Box::new(|_| {})
    }

    #[tokio::test]
    async fn missing_ffmpeg_bails_without_spawning() {
        let dir = tempfile::tempdir().unwrap();
        let ffmpeg = dir.path().join("missing-ffmpeg");
        let err = export_video(
            &ffmpeg,
            Path::new("v.mp4"),
            Path::new("d.wav"),
            Path::new("o.mp4"),
            -16.0,
            &noop_progress(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("ffmpeg"), "err={err}");
        assert!(err.to_string().contains("missing"), "err={err}");
    }

    #[tokio::test]
    async fn missing_video_bails_without_spawning() {
        let dir = tempfile::tempdir().unwrap();
        let ffmpeg = dir.path().join("ffmpeg");
        std::fs::write(&ffmpeg, b"not a real binary").unwrap();
        let err = export_video(
            &ffmpeg,
            Path::new("missing.mp4"),
            Path::new("d.wav"),
            Path::new("o.mp4"),
            -16.0,
            &noop_progress(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("source video"), "err={err}");
        assert!(err.to_string().contains("missing"), "err={err}");
    }

    #[tokio::test]
    async fn missing_dub_bails_without_spawning() {
        let dir = tempfile::tempdir().unwrap();
        let ffmpeg = dir.path().join("ffmpeg");
        std::fs::write(&ffmpeg, b"not a real binary").unwrap();
        let video = dir.path().join("v.mp4");
        std::fs::write(&video, b"not a real video").unwrap();
        let err = export_video(
            &ffmpeg,
            &video,
            Path::new("missing-dub.wav"),
            Path::new("o.mp4"),
            -16.0,
            &noop_progress(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("dub wav"), "err={err}");
        assert!(err.to_string().contains("missing"), "err={err}");
    }
}
