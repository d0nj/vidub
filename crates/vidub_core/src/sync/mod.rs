use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context};
use vidub_process::hide_console;

use crate::pipeline::DubSegment;
use crate::transcript::{ProgressCb, ProgressEvent};

pub mod pace;
pub mod tiling;

#[cfg(test)]
mod tests;

pub use pace::{
    atempo_filter, atempos_for, fade_filter, pace_action, pace_filter, tile_filter, PaceAction,
    BORROW_MARGIN_MS, HARD_CUT_TEMPO,
};
pub use tiling::{mix_timeline, ms_to_samples, read_wav_i16, tile_exact, write_wav};

#[derive(Debug, Clone, PartialEq)]
pub struct AssemblyMeta {
    pub actions: Vec<(u32, PaceAction)>,
    pub rushed: Vec<u32>,
    pub cut: Vec<u32>,
}

pub async fn assemble(
    ffmpeg: &Path,
    segs: &mut [DubSegment],
    out_wav: &Path,
    progress: &ProgressCb,
) -> anyhow::Result<AssemblyMeta> {
    if segs.is_empty() {
        bail!("assemble: no segments");
    }
    let out_dir: PathBuf = out_wav
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    let mut meta = AssemblyMeta {
        actions: Vec::with_capacity(segs.len()),
        rushed: Vec::new(),
        cut: Vec::new(),
    };
    let mut sample_rate: Option<u32> = None;
    let mut pieces: Vec<(u64, Vec<i16>)> = Vec::with_capacity(segs.len());
    let mut max_end_ms: u64 = 0;

    for idx in 0..segs.len() {
        progress(ProgressEvent::Stage("sync"));
        let id = segs[idx].id;
        let start_ms = segs[idx].start_ms;
        let end_ms = segs[idx].end_ms;
        let window = end_ms.saturating_sub(start_ms);
        if window == 0 {
            bail!("segment {id}: empty window (end_ms <= start_ms)");
        }
        let (tts_path, tts_ms) = match (&segs[idx].tts_path, segs[idx].tts_ms) {
            (Some(p), Some(ms)) => (p.clone(), ms),
            _ => bail!("segment {id}: missing tts_path/tts_ms (run voice synthesis first)"),
        };
        let gap_ms = segs
            .get(idx + 1)
            .map(|n| n.start_ms.saturating_sub(end_ms))
            .unwrap_or(0);
        let r = tts_ms as f32 / window as f32;
        let action = pace_action(r, window, gap_ms);
        let eff_window = match action {
            PaceAction::BorrowGap { new_end_ms } => new_end_ms,
            _ => window,
        };
        let r_eff = tts_ms as f32 / eff_window as f32;
        let filter = pace_filter(r_eff, &action, eff_window);

        let tmp = tempfile::Builder::new()
            .prefix("vidub-tile-")
            .suffix(".wav")
            .tempfile_in(&out_dir)
            .with_context(|| format!("segment {id}: tiled temp file"))?;
        ffmpeg_tile(ffmpeg, &tts_path, &filter, tmp.path())
            .await
            .with_context(|| format!("segment {id}: ffmpeg tiling"))?;
        let (rate, raw) =
            read_wav_i16(tmp.path()).with_context(|| format!("segment {id}: read tiled wav"))?;
        match sample_rate {
            None => sample_rate = Some(rate),
            Some(prev) if prev != rate => {
                bail!("segment {id}: sample-rate mismatch ({prev} vs {rate}); synthesize all segments at one rate")
            }
            _ => {}
        }

        let expected = ms_to_samples(eff_window, rate) as usize;
        let samples = tile_exact(raw, expected);
        let start_sample = ms_to_samples(start_ms, rate);
        pieces.push((start_sample, samples));
        max_end_ms = max_end_ms.max(start_ms.saturating_add(eff_window));
        segs[idx].tts_ms = Some(eff_window);

        match action {
            PaceAction::HardCut => {
                meta.rushed.push(id);
                meta.cut.push(id);
                progress(ProgressEvent::Warning(format!(
                    "segment {id}: hard-cut (r={r:.3} exceeds 1.50; tempo capped, remainder cut)"
                )));
            }
            PaceAction::BorrowGap { .. }
            | PaceAction::Chained { .. }
            | PaceAction::ShortenRetry => {
                meta.rushed.push(id);
                progress(ProgressEvent::Warning(format!(
                    "segment {id}: rushed speech (r={r:.3}, pace={action:?})"
                )));
            }
            PaceAction::Pad | PaceAction::Fit { .. } => {}
        }
        meta.actions.push((id, action));
    }

    let rate = sample_rate.context("assemble: no sample rate determined")?;
    let total_samples = ms_to_samples(max_end_ms, rate);
    let refs: Vec<(u64, &[i16])> = pieces.iter().map(|(off, v)| (*off, v.as_slice())).collect();
    let mixed = mix_timeline(&refs, total_samples);
    write_wav(out_wav, rate, &mixed)
        .with_context(|| format!("write out wav ({})", out_wav.display()))?;
    progress(ProgressEvent::Stage("sync"));
    Ok(meta)
}

async fn ffmpeg_tile(
    ffmpeg: &Path,
    input: &Path,
    filter: &str,
    output: &Path,
) -> anyhow::Result<()> {
    let mut cmd = tokio::process::Command::new(ffmpeg);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    hide_console(&mut cmd);
    cmd.arg("-y")
        .arg("-i")
        .arg(input)
        .arg("-af")
        .arg(filter)
        .arg("-ac")
        .arg("1")
        .arg(output);
    let out = tokio::time::timeout(Duration::from_secs(120), cmd.output())
        .await
        .map_err(|_| {
            anyhow::anyhow!("ffmpeg timed out after 120s (input {})", input.display())
        })??;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("ffmpeg failed ({}): {}", input.display(), stderr.trim());
    }
    Ok(())
}
