use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use slint::{ComponentHandle, Weak};
use vidub_core::transcript::{ProgressCb, ProgressEvent};

use crate::bridge::{lock_unpoisoned, resolve_key, AppState, DubOutput, MediaPaths};
use crate::MainWindow;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Transcribe,
    Synthesize,
    Assemble,
}

pub fn wire_dub(ui: &MainWindow, state: &AppState) {
    let ui_weak = ui.as_weak();
    let runtime = state.runtime.clone();
    let client = state.client.clone();
    let paths = state.paths.clone();
    let dub_slot = state.dub.clone();
    let media_slot = state.media.clone();
    let preview_slot = state.preview.clone();
    let last_input_slot = state.last_input.clone();
    let settings_slot = state.settings.clone();
    let session_keys_slot = state.session_keys.clone();
    let phase = Arc::new(Mutex::new(Phase::Transcribe));
    let busy = Arc::new(AtomicBool::new(false));

    ui.on_dub_requested(move |url_or_path| {
        let input = url_or_path.trim().to_owned();
        if input.is_empty() {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_status(
                    "Dán liên kết YouTube/Bilibili hoặc đường dẫn tệp video vào đây.".into(),
                );
            }
            return;
        }
        if busy.swap(true, Ordering::SeqCst) {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_status("Đang lồng tiếng…".into());
            }
            return;
        }

        let provider = lock_unpoisoned(&settings_slot).provider;
        let Some(key) = resolve_key(provider, &session_keys_slot) else {
            busy.store(false, Ordering::SeqCst);
            if let Some(ui) = ui_weak.upgrade() {
                let msg = format!(
                    "Chưa có API key {}. Vào Cài đặt để thêm.",
                    crate::settings_ui::provider_name(provider)
                );
                ui.set_status(msg.as_str().into());
                ui.set_page(1);
            }
            return;
        };

        let sidecars = lock_unpoisoned(&paths).clone();
        let Some(sidecars) = sidecars else {
            busy.store(false, Ordering::SeqCst);
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_status("Đang kiểm tra công cụ — hãy thử lại sau giây lát.".into());
            }
            return;
        };

        *lock_unpoisoned(&last_input_slot) = Some(input.clone());
        *lock_unpoisoned(&phase) = Phase::Transcribe;
        if let Some(ui) = ui_weak.upgrade() {
            ui.set_status("Đang lồng tiếng…".into());
            ui.set_progress(0.0);
        }

        let ui_weak_task = ui_weak.clone();
        let client_task = client.clone();
        let busy_task = busy.clone();
        let dub_slot_task = dub_slot.clone();
        let media_slot_task = media_slot.clone();
        let preview_slot_task = preview_slot.clone();
        let phase_task = phase.clone();
        runtime.spawn(async move {
            let src = if input.starts_with("http://") || input.starts_with("https://") {
                vidub_core::transcript::Source::Url(input.clone())
            } else {
                vidub_core::transcript::Source::File(PathBuf::from(input.clone()))
            };

            let progress_cb: Box<ProgressCb> = {
                let ui_weak = ui_weak_task.clone();
                let phase = phase_task.clone();
                Box::new(move |event| {
                    let ui_weak = ui_weak.clone();
                    let phase = phase.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(ui) = ui_weak.upgrade() else { return };
                        match event {
                            ProgressEvent::Stage(stage) => {
                                on_stage(&ui, &phase, stage);
                            }
                            ProgressEvent::ChunkDone { done, total } => {
                                on_chunk(&ui, &phase, done, total);
                            }
                            ProgressEvent::Warning(_) => {}
                        }
                    });
                })
            };

            let result = vidub_core::pipeline::dub_all(
                &client_task,
                &sidecars,
                &src,
                provider,
                &key,
                &*progress_cb,
            )
            .await;
            busy_task.store(false, Ordering::SeqCst);

            match result {
                Ok(run) => {
                    *lock_unpoisoned(&dub_slot_task) = Some(DubOutput {
                        title: run.title.clone(),
                        wav_path: run.wav_path.clone(),
                    });
                    *lock_unpoisoned(&media_slot_task) = Some(MediaPaths {
                        video: run.video_path.clone(),
                        dub_wav: Some(run.wav_path.clone()),
                    });
                    let ui_weak_preview = ui_weak_task.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_weak_preview.upgrade() {
                            ui.set_status("Xong! Đang mở bản xem trước…".into());
                        }
                    });
                    start_preview_for_run(
                        sidecars,
                        run.video_path,
                        run.wav_path,
                        &preview_slot_task,
                        &ui_weak_task,
                    )
                    .await;
                }
                Err(err) => {
                    let status = err
                        .downcast_ref::<vidub_core::transcript::probe::ProbeError>()
                        .map(vidub_core::transcript::probe::user_message)
                        .unwrap_or_else(|| format!("{err:#}"));
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(ui) = ui_weak_task.upgrade() else {
                            return;
                        };
                        ui.set_status(status.into());
                    });
                }
            }
        });
    });
}

fn on_stage(ui: &MainWindow, phase: &Arc<Mutex<Phase>>, stage: &str) {
    let (label, anchor) = stage_label(stage);
    match stage {
        "translating" => {
            *lock_unpoisoned(phase) = Phase::Synthesize;
        }
        "sync" => {
            *lock_unpoisoned(phase) = Phase::Assemble;
        }
        _ => {}
    }
    ui.set_status(label.into());
    if let Some(anchor) = anchor {
        ui.set_progress(anchor);
    }
}

fn on_chunk(ui: &MainWindow, phase: &Arc<Mutex<Phase>>, done: usize, total: usize) {
    let fraction = if total > 0 {
        done as f32 / total as f32
    } else {
        0.0
    };
    let current = *lock_unpoisoned(phase);
    let mapped = match current {
        Phase::Transcribe => 0.05 + 0.20 * fraction,
        Phase::Synthesize => 0.45 + 0.40 * fraction,
        Phase::Assemble => 0.85 + 0.15 * fraction,
    };
    ui.set_progress(mapped.clamp(0.0, 1.0));
}

fn stage_label(stage: &str) -> (&'static str, Option<f32>) {
    match stage {
        "probing" => ("Đang xem video…", Some(0.05)),
        "fetching captions" => ("Đang lấy phụ đề…", Some(0.05)),
        "captions ready" => ("Phụ đề tiếng Việt có sẵn — bỏ qua bước dịch", Some(0.45)),
        "extracting audio" => ("Đang tách âm thanh…", Some(0.10)),
        "transcribing" => ("Đang nghe và ghi lại lời thoại…", Some(0.15)),
        "translating" => ("Đang dịch sang tiếng Việt…", Some(0.45)),
        "sync" => ("Đang ghép giọng vào video…", Some(0.85)),
        _ => ("Đang xử lý…", None),
    }
}

async fn start_preview_for_run(
    sidecars: vidub_sidecar::BinPaths,
    video: PathBuf,
    dub_wav: PathBuf,
    preview_slot: &Arc<Mutex<Option<Arc<vidub_mpv::control::MpvHandle>>>>,
    ui_weak: &Weak<MainWindow>,
) {
    let ui_weak = ui_weak.clone();
    let Some(mpv) = sidecars.mpv.clone() else {
        let message = "Xong! Lồng tiếng xong. Không tìm thấy trình phát video để xem trước (trên Linux: sudo apt install mpv); bạn vẫn có thể xuất video."
            .to_owned();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_status(message.clone().into());
                ui.set_preview_status(message.clone().into());
            }
        });
        return;
    };

    match crate::preview_ui::spawn_into_slot(&mpv, &video, Some(&dub_wav), preview_slot).await {
        Ok(()) => {
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_progress(1.0);
                    ui.set_playing(true);
                    ui.set_seek(0.0);
                    ui.set_time_label("0:00 / 0:00".into());
                    ui.set_status("Xong! Video đã sẵn sàng.".into());
                    ui.set_preview_status("Đã mở bản xem trước.".into());
                }
            });
        }
        Err(err) => {
            let message =
                format!("Xong! Lồng tiếng xong, nhưng không mở được bản xem trước: {err}");
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = ui_weak.upgrade() else { return };
                ui.set_status(message.clone().into());
                ui.set_preview_status(message.clone().into());
            });
        }
    }
}
