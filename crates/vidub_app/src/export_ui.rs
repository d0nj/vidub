use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use slint::{ComponentHandle, Weak};
use vidub_core::export::export_video;
use vidub_core::transcript::{ProgressCb, ProgressEvent};

use crate::bridge::{lock_unpoisoned, AppState, DubOutput, MediaPaths};
use crate::MainWindow;

pub fn wire_export(ui: &MainWindow, state: &AppState) {
    let ui_weak = ui.as_weak();
    let runtime = state.runtime.clone();
    let paths = state.paths.clone();
    let media = state.media.clone();
    let dub = state.dub.clone();
    let settings = state.settings.clone();
    let busy = Arc::new(AtomicBool::new(false));

    {
        let ui_weak = ui_weak.clone();
        let runtime = runtime.clone();
        let media = media.clone();
        let settings = settings.clone();
        let busy = busy.clone();
        let dub = dub.clone();
        ui.on_browse_export_requested(move || {
            if busy.load(Ordering::SeqCst) {
                return;
            }
            let dir = {
                let settings = lock_unpoisoned(&settings);
                let media = lock_unpoisoned(&media);
                let video = media.as_ref().map(|MediaPaths { video, .. }| video);
                suggested_dir(video, settings.output_dir.as_deref())
            };
            let file_name = {
                let media = lock_unpoisoned(&media);
                let video = media.as_ref().map(|MediaPaths { video, .. }| video);
                default_file_name(video, &current_title(&dub))
            };
            let ui_weak = ui_weak.clone();
            runtime.spawn(async move {
                let mut dialog = rfd::AsyncFileDialog::new()
                    .set_title("Chọn nơi lưu video")
                    .set_directory(&dir)
                    .set_file_name(&file_name)
                    .add_filter("Video MP4", &["mp4"]);
                if let Some(current) = ui_weak
                    .upgrade()
                    .map(|ui| ui.get_export_path())
                    .filter(|p| !p.trim().is_empty())
                {
                    let current = PathBuf::from(current.as_str());
                    if current.parent() == Some(dir.as_path()) {
                        if let Some(name) = current.file_name() {
                            dialog = dialog.set_file_name(name.to_string_lossy());
                        }
                    }
                }
                let Some(file) = dialog.save_file().await else {
                    return;
                };
                let choice = file.path().to_path_buf();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = ui_weak.upgrade() else { return };
                    let name = choice
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| choice.display().to_string());
                    ui.set_export_path(choice.display().to_string().into());
                    ui.set_export_target(name.into());
                    ui.set_export_status("".into());
                });
            });
        });
    }

    ui.on_export_requested(move |out_path| {
        if busy.swap(true, Ordering::SeqCst) {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_export_status("Đang lưu rồi…".into());
            }
            return;
        }
        let release = |busy: &AtomicBool, ui_weak: &Weak<MainWindow>| {
            busy.store(false, Ordering::SeqCst);
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_export_busy(false);
            }
        };

        if out_path.trim().is_empty() {
            release(&busy, &ui_weak);
            if let Some(ui) = ui_weak.upgrade() {
                ui.invoke_browse_export_requested();
            }
            return;
        }

        let video = lock_unpoisoned(&media)
            .as_ref()
            .map(|MediaPaths { video, .. }| video.clone());
        let Some(video) = video else {
            release(&busy, &ui_weak);
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_export_status("Hãy lồng tiếng video trước đã".into());
            }
            return;
        };
        let dub_wav = lock_unpoisoned(&dub)
            .as_ref()
            .map(|d| d.wav_path.clone());
        let Some(dub_wav) = dub_wav else {
            release(&busy, &ui_weak);
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_export_status("Hãy lồng tiếng video trước, rồi mới lưu".into());
            }
            return;
        };

        let ffmpeg = lock_unpoisoned(&paths)
            .as_ref()
            .and_then(|sidecars| sidecars.ffmpeg.clone());
        let Some(ffmpeg) = ffmpeg else {
            release(&busy, &ui_weak);
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_export_status("Không tìm thấy công cụ video — cần ffmpeg để lưu (xem danh sách công cụ); vẫn xem trước được".into());
            }
            return;
        };

        let target_lufs = lock_unpoisoned(&settings).loudness.target_lufs();
        let out_path = PathBuf::from(out_path.trim());
        if let Some(ui) = ui_weak.upgrade() {
            ui.set_export_status("Đang lưu video…".into());
            ui.set_export_busy(true);
            ui.set_export_progress(0.0);
        }

        let ui_weak_task = ui_weak.clone();
        let busy_task = busy.clone();
        runtime.spawn(async move {
            let result = export_async(
                &ffmpeg,
                &video,
                &dub_wav,
                &out_path,
                target_lufs,
                &ui_weak_task,
            )
            .await;
            busy_task.store(false, Ordering::SeqCst);
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = ui_weak_task.upgrade() else { return };
                ui.set_export_busy(false);
                match result {
                    Ok(meta) => {
                        ui.set_export_progress(1.0);
                        let name = meta
                            .out_path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| meta.out_path.display().to_string());
                        ui.set_export_target(name.clone().into());
                        ui.set_export_path(meta.out_path.display().to_string().into());
                        ui.set_export_status(format!("Đã lưu: {name}").into());
                    }
                    Err(message) => {
                        ui.set_export_status(message.into());
                    }
                }
            });
        });
    });
}

fn current_title(dub: &Arc<Mutex<Option<DubOutput>>>) -> String {
    lock_unpoisoned(dub)
        .as_ref()
        .map(|d| d.title.clone())
        .unwrap_or_default()
}

fn default_file_name(video: Option<&PathBuf>, title: &str) -> String {
    let from_title = sanitize_file_stem(title);
    let stem = if from_title.is_empty() {
        video
            .and_then(|v| v.file_stem())
            .and_then(|s| s.to_str())
            .map(sanitize_file_stem)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "video".to_string())
    } else {
        from_title
    };
    format!("{stem}_vi.mp4")
}

fn sanitize_file_stem(raw: &str) -> String {
    const MAX_STEM: usize = 120;
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_matches(|c: char| c == '.' || c.is_whitespace());
    let mut stem: String = trimmed.chars().take(MAX_STEM).collect();
    while stem.ends_with('.') || stem.ends_with(' ') {
        stem.pop();
    }
    stem
}

fn suggested_dir(video: Option<&PathBuf>, output_dir: Option<&Path>) -> PathBuf {
    if let Some(dir) = output_dir.filter(|d| d.is_dir()) {
        return dir.to_path_buf();
    }
    if let Some(parent) = video.and_then(|v| v.parent()).filter(|p| p.is_dir()) {
        return parent.to_path_buf();
    }
    if let Some(dir) = dirs::video_dir().filter(|d| d.is_dir()) {
        return dir;
    }
    if let Some(dir) = dirs::home_dir().filter(|d| d.is_dir()) {
        return dir;
    }
    std::env::temp_dir()
}

async fn export_async(
    ffmpeg: &Path,
    video: &Path,
    dub_wav: &Path,
    out_path: &Path,
    target_lufs: f32,
    ui_weak: &Weak<MainWindow>,
) -> Result<vidub_core::export::ExportMeta, String> {
    let progress_cb: Box<ProgressCb> = {
        let ui_weak = ui_weak.clone();
        Box::new(move |event| {
            let ui_weak = ui_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = ui_weak.upgrade() else { return };
                match event {
                    ProgressEvent::Stage(stage) => {
                        ui.set_export_status(crate::bridge::human_stage(stage).into());
                        if let Some(fraction) = stage_progress(stage) {
                            ui.set_export_progress(fraction);
                        }
                    }
                    ProgressEvent::ChunkDone { done, total } => {
                        let fraction = if total > 0 {
                            done as f32 / total as f32
                        } else {
                            0.0
                        };
                        ui.set_export_progress(fraction);
                    }
                    ProgressEvent::Warning(_) => {}
                }
            });
        })
    };

    export_video(ffmpeg, video, dub_wav, out_path, target_lufs, &*progress_cb)
        .await
        .map_err(|err| format!("Không lưu được video: {err}"))
}

fn stage_progress(stage: &str) -> Option<f32> {
    let stage = stage.to_ascii_lowercase();
    if stage.contains("resampl") {
        Some(0.15)
    } else if stage.contains("measur") || stage.contains("pass 1") {
        Some(0.4)
    } else if stage.contains("mux")
        || stage.contains("render")
        || stage.contains("pass 2")
        || stage.contains("export")
        || stage.contains("loudnorm")
    {
        Some(0.7)
    } else {
        None
    }
}
