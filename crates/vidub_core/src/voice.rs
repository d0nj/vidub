use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context};
use vidub_process::hide_console;

use crate::util::{truncate_tail, MAX_STDERR_CHARS};

const SYNTH_TIMEOUT: Duration = Duration::from_secs(120);

const SILENCE_THRESHOLD: f32 = 0.01;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavMeta {
    pub sample_rate: u32,
    pub channels: u16,
    pub duration_ms: u64,
    pub speech_ms: u64,
}

pub fn piper_args(model: &Path, config: &Path, out_wav: &Path, length_scale: f32) -> Vec<OsString> {
    vec![
        OsString::from("--model"),
        model.as_os_str().to_os_string(),
        OsString::from("--config"),
        config.as_os_str().to_os_string(),
        OsString::from("--output_file"),
        out_wav.as_os_str().to_os_string(),
        OsString::from("--length_scale"),
        OsString::from(length_scale.to_string()),
    ]
}

pub async fn synthesize(
    piper: &Path,
    model: &Path,
    config: &Path,
    text: &str,
    out_wav: &Path,
    length_scale: f32,
) -> anyhow::Result<WavMeta> {
    if !piper.is_file() {
        bail!("required binary missing: piper ({})", piper.display());
    }
    if !model.is_file() {
        bail!("required binary missing: piper model ({})", model.display());
    }
    if !config.is_file() {
        bail!(
            "required binary missing: piper config ({})",
            config.display()
        );
    }

    let args = piper_args(model, config, out_wav, length_scale);
    let mut cmd = tokio::process::Command::new(piper);
    cmd.args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    hide_console(&mut cmd);
    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn piper ({})", piper.display()))?;

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin
            .write_all(text.as_bytes())
            .await
            .with_context(|| format!("failed to write stdin to piper ({})", piper.display()))?;
        stdin.shutdown().await.ok();
    }

    let output = tokio::time::timeout(SYNTH_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| anyhow::anyhow!("piper timed out after {}s", SYNTH_TIMEOUT.as_secs()))?
        .with_context(|| format!("failed to wait for piper ({})", piper.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "piper exited with code {:?}: {}",
            output.status.code(),
            truncate_tail(stderr.trim(), MAX_STDERR_CHARS)
        );
    }

    analyze_wav(out_wav)
}

pub fn analyze_wav(path: &Path) -> anyhow::Result<WavMeta> {
    let mut reader = hound::WavReader::open(path)
        .with_context(|| format!("failed to read wav ({})", path.display()))?;
    let spec = reader.spec();
    let sample_rate = spec.sample_rate;
    let channels = spec.channels;
    let duration_ms = u64::from(reader.duration()) * 1000 / u64::from(sample_rate);

    let bounds = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, _) => speech_bounds::<_, f32>(&mut reader, channels)?,
        (_, 16) => speech_bounds::<_, i16>(&mut reader, channels)?,
        (_, 32) => speech_bounds::<_, i32>(&mut reader, channels)?,
        _ => speech_bounds::<_, i32>(&mut reader, channels)?,
    };
    let speech_ms = match bounds {
        Some((first_frame, last_frame)) => {
            let frames = last_frame - first_frame + 1;
            (frames * 1000 / u64::from(sample_rate)).max(1)
        }
        None => {
            tracing::warn!("all-silence wav: speech_ms=0 ({})", path.display());
            0
        }
    };

    Ok(WavMeta {
        sample_rate,
        channels,
        duration_ms,
        speech_ms,
    })
}

fn speech_bounds<R, S>(
    reader: &mut hound::WavReader<R>,
    channels: u16,
) -> anyhow::Result<Option<(u64, u64)>>
where
    R: std::io::Read,
    S: hound::Sample,
{
    let channels = u64::from(channels.max(1));
    let mut first: Option<u64> = None;
    let mut last: u64 = 0;
    for (index, sample) in reader.samples::<S>().enumerate() {
        let sample = sample.context("failed to read wav samples")?;
        let amplitude = f32::from(sample.as_i16()) / 32768.0;
        if amplitude.abs() >= SILENCE_THRESHOLD {
            let frame = index as u64 / channels;
            first.get_or_insert(frame);
            last = frame;
        }
    }
    Ok(first.map(|first_frame| (first_frame, last)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn write_wav(path: &Path, sample_rate: u32, channels: u16, samples: &[i16]) {
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for &sample in samples {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
    }

    fn sine_block(frames: usize, sample_rate: u32, amplitude: i16) -> Vec<i16> {
        (0..frames)
            .map(|n| {
                let phase = 2.0 * std::f32::consts::PI * 220.0 * n as f32 / sample_rate as f32;
                (phase.sin() * amplitude as f32) as i16
            })
            .collect()
    }

    fn temp_wav(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        (dir, path)
    }

    #[test]
    fn trim_sine_sandwich_exact_duration() {
        let (_dir, path) = temp_wav("sandwich.wav");
        let rate = 16000u32;
        let mut samples = vec![0i16; 3200];
        samples.extend(sine_block(8000, rate, 16384));
        samples.extend(vec![0i16; 4800]);
        write_wav(&path, rate, 1, &samples);

        let meta = analyze_wav(&path).unwrap();
        assert_eq!(meta.sample_rate, 16000);
        assert_eq!(meta.channels, 1);
        assert_eq!(meta.duration_ms, 1000);
        assert!(
            (495..=500).contains(&meta.speech_ms),
            "speech_ms={}",
            meta.speech_ms
        );
    }

    #[test]
    fn trim_dc_block_exact_speech() {
        let (_dir, path) = temp_wav("dc.wav");
        let rate = 22050u32;
        let mut samples = vec![0i16; 2205];
        samples.extend(vec![16384i16; 8820]);
        samples.extend(vec![0i16; 4410]);
        write_wav(&path, rate, 1, &samples);

        let meta = analyze_wav(&path).unwrap();
        assert_eq!(meta.sample_rate, 22050);
        assert_eq!(meta.channels, 1);
        assert_eq!(meta.duration_ms, 700);
        assert_eq!(meta.speech_ms, 400);
    }

    #[test]
    fn all_silence_yields_zero_speech() {
        let (_dir, path) = temp_wav("silence.wav");
        write_wav(&path, 16000, 1, &vec![0i16; 4800]);

        let meta = analyze_wav(&path).unwrap();
        assert_eq!(meta.duration_ms, 300);
        assert_eq!(meta.speech_ms, 0);
    }

    #[test]
    fn sub_threshold_amplitude_is_silence() {
        let (_dir, path) = temp_wav("below.wav");
        write_wav(&path, 16000, 1, &vec![163i16; 4800]);

        let meta = analyze_wav(&path).unwrap();
        assert_eq!(meta.speech_ms, 0);
    }

    #[test]
    fn just_above_threshold_is_speech() {
        let (_dir, path) = temp_wav("above.wav");
        write_wav(&path, 16000, 1, &vec![655i16; 3200]);

        let meta = analyze_wav(&path).unwrap();
        assert_eq!(meta.speech_ms, 200);
    }

    #[test]
    fn single_loud_frame_clamped_to_one_ms() {
        let (_dir, path) = temp_wav("tick.wav");
        let mut samples = vec![0i16; 4410];
        samples[100] = 16384;
        write_wav(&path, 22050, 1, &samples);

        let meta = analyze_wav(&path).unwrap();
        assert_eq!(meta.speech_ms, 1);
    }

    #[test]
    fn stereo_counts_frames_not_samples() {
        let (_dir, path) = temp_wav("stereo.wav");
        let mut samples = Vec::new();
        for frame in 0..6400 {
            let speech = (1600..4800).contains(&frame);
            samples.push(if speech { 16384 } else { 0 }); // left
            samples.push(0); // right stays silent
        }
        write_wav(&path, 16000, 2, &samples);

        let meta = analyze_wav(&path).unwrap();
        assert_eq!(meta.channels, 2);
        assert_eq!(meta.duration_ms, 400);
        assert_eq!(meta.speech_ms, 200);
    }

    #[test]
    fn arg_shaping_matches_piper_cli() {
        let args = piper_args(
            Path::new("vi_VN.onnx"),
            Path::new("config.json"),
            Path::new("out.wav"),
            1.0,
        );
        let shown: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            shown,
            [
                "--model",
                "vi_VN.onnx",
                "--config",
                "config.json",
                "--output_file",
                "out.wav",
                "--length_scale",
                "1"
            ]
        );
        let parsed: f32 = shown[7].parse().unwrap();
        assert_eq!(parsed, 1.0);
    }

    #[tokio::test]
    async fn missing_piper_bails_without_spawning() {
        let dir = tempfile::tempdir().unwrap();
        let piper = dir.path().join("piper-not-here");
        let model = dir.path().join("model.onnx");
        let out = dir.path().join("out.wav");

        let err = synthesize(
            &piper,
            &model,
            &dir.path().join("config.json"),
            "xin chào",
            &out,
            1.0,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("piper"), "err={err}");
        assert!(err.to_string().contains("missing"), "err={err}");
        assert!(!out.exists(), "must not have spawned / written anything");
    }

    #[tokio::test]
    async fn missing_model_bails_before_spawn() {
        let dir = tempfile::tempdir().unwrap();
        let piper = dir.path().join("piper-dummy");
        std::fs::write(&piper, b"not a real binary").unwrap();
        let model = dir.path().join("missing.onnx");
        let out = dir.path().join("out.wav");

        let err = synthesize(
            &piper,
            &model,
            &dir.path().join("config.json"),
            "xin chào",
            &out,
            1.0,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("model"), "err={err}");
        assert!(err.to_string().contains("missing"), "err={err}");
    }

    #[test]
    fn analyze_missing_wav_errors() {
        let err = analyze_wav(Path::new("definitely-not-here/voice.wav")).unwrap_err();
        assert!(err.to_string().contains("failed to read wav"), "err={err}");
    }

    #[test]
    fn speech_bounds_handles_cursor_reader() {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut buf = Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(&mut buf, spec).unwrap();
            for _ in 0..1600 {
                writer.write_sample(0i16).unwrap();
            }
            for _ in 0..3200 {
                writer.write_sample(16384i16).unwrap();
            }
            for _ in 0..1600 {
                writer.write_sample(0i16).unwrap();
            }
            writer.finalize().unwrap();
        }
        buf.set_position(0);
        let mut reader = hound::WavReader::new(buf).unwrap();
        let bounds = speech_bounds::<_, i32>(&mut reader, 1).unwrap().unwrap();
        assert_eq!(bounds, (1600, 4799));
    }
}
