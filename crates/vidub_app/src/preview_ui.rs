use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use slint::{ComponentHandle, Weak};
use vidub_mpv::control::{MpvControl, MpvHandle};

use crate::bridge::{lock_unpoisoned, AppState, DubOutput, MediaPaths};
use crate::MainWindow;

#[derive(Clone)]
struct PreviewShared {
    runtime: tokio::runtime::Handle,
    paths: Arc<Mutex<Option<vidub_sidecar::BinPaths>>>,
    dub: Arc<Mutex<Option<DubOutput>>>,
    last_input: Arc<Mutex<Option<String>>>,
    media: Arc<Mutex<Option<MediaPaths>>>,
    preview: Arc<Mutex<Option<Arc<MpvHandle>>>>,
    busy: Arc<AtomicBool>,
    paused: Arc<Mutex<bool>>,
    use_dub: Arc<Mutex<bool>>,
    duration_ms: Arc<Mutex<Option<u64>>>,
}

pub fn wire_preview(ui: &MainWindow, state: &AppState) {
    let shared = PreviewShared {
        runtime: state.runtime.clone(),
        paths: state.paths.clone(),
        dub: state.dub.clone(),
        last_input: state.last_input.clone(),
        media: state.media.clone(),
        preview: state.preview.clone(),
        busy: Arc::new(AtomicBool::new(false)),
        paused: Arc::new(Mutex::new(false)),
        use_dub: Arc::new(Mutex::new(true)),
        duration_ms: Arc::new(Mutex::new(None)),
    };

    {
        let ui_weak = ui.as_weak();
        let shared = shared.clone();
        ui.on_preview_requested(move || {
            if shared.busy.swap(true, Ordering::SeqCst) {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_preview_status("Đang mở xem trước rồi".into());
                }
                return;
            }
            let input = lock_unpoisoned(&shared.last_input).clone();
            let Some(input) = input.filter(|s| !s.trim().is_empty()) else {
                shared.busy.store(false, Ordering::SeqCst);
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_preview_status("Hãy lồng tiếng video trước đã".into());
                }
                return;
            };
            let dub_wav = lock_unpoisoned(&shared.dub)
                .as_ref()
                .map(|d| d.wav_path.clone());
            let Some(dub_wav) = dub_wav else {
                shared.busy.store(false, Ordering::SeqCst);
                if let Some(ui) = ui_weak.upgrade() {
                    let msg = "Hãy lồng tiếng video trước, rồi mới xem trước";
                    ui.set_preview_status(msg.into());
                }
                return;
            };
            let sidecars = lock_unpoisoned(&shared.paths).clone();
            let Some(sidecars) = sidecars else {
                shared.busy.store(false, Ordering::SeqCst);
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_preview_status("Đang kiểm tra công cụ — thử lại sau giây lát".into());
                }
                return;
            };
            let Some(mpv) = sidecars.mpv.clone() else {
                shared.busy.store(false, Ordering::SeqCst);
                let msg = "Không tìm thấy trình phát video — cần mpv để xem trước (trên Linux: sudo apt install mpv); bạn vẫn lưu video được"
                    .to_owned();
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_preview_status(msg.into());
                }
                return;
            };
            let want_dub = *lock_unpoisoned(&shared.use_dub);
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_preview_status("Đang mở xem trước…".into());
            }
            let shared_task = shared.clone();
            let ui_weak_task = ui_weak.clone();
            shared.runtime.spawn(async move {
                let result = preview_async(
                    input,
                    dub_wav,
                    mpv,
                    sidecars.yt_dlp.clone(),
                    sidecars.qjs.clone(),
                    want_dub,
                    &shared_task,
                    &ui_weak_task,
                )
                .await;
                shared_task.busy.store(false, Ordering::SeqCst);
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = ui_weak_task.upgrade() else { return };
                    match result {
                        Ok(_) => {
                            ui.set_playing(true);
                            ui.set_seek(0.0);
                            ui.set_time_label("0:00 / 0:00".into());
                            ui.set_preview_status("Đã mở xem trước".into());
                        }
                        Err(message) => {
                            ui.set_preview_status(message.into());
                        }
                    }
                });
            });
        });
    }

    {
        let ui_weak = ui.as_weak();
        let shared = shared.clone();
        ui.on_play_pause_requested(move || {
            let handle = lock_unpoisoned(&shared.preview).clone();
            let Some(handle) = handle else {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_preview_status("Chưa mở xem trước".into());
                }
                return;
            };
            let new_paused = {
                let mut guard = lock_unpoisoned(&shared.paused);
                *guard = !*guard;
                *guard
            };
            let shared_task = shared.clone();
            let ui_weak_task = ui_weak.clone();
            shared.runtime.spawn(async move {
                match handle.set_pause(new_paused).await {
                    Ok(()) => {
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak_task.upgrade() {
                                ui.set_playing(!new_paused);
                            }
                        });
                    }
                    Err(err) => {
                        *lock_unpoisoned(&shared_task.paused) = !new_paused;
                        let message = format!("Không điều khiển được xem trước: {err}");
                        let _ = slint::invoke_from_event_loop(move || {
                            let Some(ui) = ui_weak_task.upgrade() else {
                                return;
                            };
                            ui.set_preview_status(message.clone().into());
                        });
                    }
                }
            });
        });
    }

    {
        let ui_weak = ui.as_weak();
        let shared = shared.clone();
        ui.on_seek_requested(move |value| {
            let handle = lock_unpoisoned(&shared.preview).clone();
            let Some(handle) = handle else { return };
            let duration = *lock_unpoisoned(&shared.duration_ms);
            let Some(total) = duration else { return };
            if total == 0 {
                return;
            }
            let pos_ms = (value.clamp(0.0, 1.0) * total as f32).round() as u64;
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_time_label(format!("{} / {}", format_ms(pos_ms), format_ms(total)).into());
            }
            let ui_weak_task = ui_weak.clone();
            shared.runtime.spawn(async move {
                if let Err(err) = handle.seek_ms(pos_ms, true).await {
                    let message = format!("Không tua được video: {err}");
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(ui) = ui_weak_task.upgrade() else {
                            return;
                        };
                        ui.set_preview_status(message.clone().into());
                    });
                }
            });
        });
    }

    {
        let ui_weak = ui.as_weak();
        let shared = shared.clone();
        ui.on_audio_toggle_requested(move |want_dub| {
            *lock_unpoisoned(&shared.use_dub) = want_dub;
            let handle = lock_unpoisoned(&shared.preview).clone();
            let Some(handle) = handle else { return };
            let wav = if want_dub {
                lock_unpoisoned(&shared.media)
                    .as_ref()
                    .and_then(|m| m.dub_wav.clone())
            } else {
                None
            };
            let ui_weak_task = ui_weak.clone();
            shared.runtime.spawn(async move {
                if let Err(err) = handle.set_audio(wav.as_deref()).await {
                    let message = format!("Không đổi được âm thanh: {err}");
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(ui) = ui_weak_task.upgrade() else {
                            return;
                        };
                        ui.set_preview_status(message.clone().into());
                    });
                }
            });
        });
    }

    {
        let ui_weak = ui.as_weak();
        let shared = shared.clone();
        let poll_timer = slint::Timer::default();
        poll_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(500),
            move || {
                let handle = lock_unpoisoned(&shared.preview).clone();
                let Some(handle) = handle else { return };
                let shared_task = shared.clone();
                let ui_weak_task = ui_weak.clone();
                shared.runtime.spawn(async move {
                    let pos = handle.time_pos_ms().await;
                    let dur = handle.duration_ms().await;
                    match (pos, dur) {
                        (Err(pos_err), Err(_)) => {
                            *lock_unpoisoned(&shared_task.preview) = None;
                            *lock_unpoisoned(&shared_task.duration_ms) = None;
                            let message = format!(
                                "Mất kết nối xem trước ({pos_err}) — nhấn Xem trước để mở lại"
                            );
                            let _ = slint::invoke_from_event_loop(move || {
                                let Some(ui) = ui_weak_task.upgrade() else {
                                    return;
                                };
                                ui.set_playing(false);
                                ui.set_seek(0.0);
                                ui.set_time_label("0:00 / 0:00".into());
                                ui.set_preview_status(message.clone().into());
                            });
                        }
                        (pos, dur) => {
                            let pos_ms = pos.ok().flatten();
                            let dur_ms = dur.ok().flatten();
                            if let Some(d) = dur_ms {
                                *lock_unpoisoned(&shared_task.duration_ms) = Some(d);
                            }
                            if pos_ms.is_none() && dur_ms.is_none() {
                                return;
                            }
                            let _ = slint::invoke_from_event_loop(move || {
                                let Some(ui) = ui_weak_task.upgrade() else {
                                    return;
                                };
                                if let (Some(p), Some(d)) = (pos_ms, dur_ms) {
                                    if d > 0 {
                                        ui.set_seek(p as f32 / d as f32);
                                        ui.set_time_label(
                                            format!("{} / {}", format_ms(p), format_ms(d)).into(),
                                        );
                                    } else {
                                        ui.set_time_label(
                                            format!("{} / 0:00", format_ms(p)).into(),
                                        );
                                    }
                                } else if let Some(p) = pos_ms {
                                    ui.set_time_label(format_ms(p).into());
                                }
                            });
                        }
                    }
                });
            },
        );
        std::mem::forget(poll_timer);
    }

    {
        let shared = shared.clone();
        ui.window().on_close_requested(move || {
            let handle = lock_unpoisoned(&shared.preview).take();
            if let Some(handle) = handle {
                shared.runtime.spawn(async move {
                    let _ = handle.close().await;
                });
            }
            slint::CloseRequestResponse::HideWindow
        });
    }
}

#[allow(clippy::too_many_arguments)]
async fn preview_async(
    input: String,
    dub_wav: PathBuf,
    mpv: PathBuf,
    yt_dlp: Option<PathBuf>,
    qjs: Option<PathBuf>,
    use_dub: bool,
    shared: &PreviewShared,
    ui_weak: &Weak<MainWindow>,
) -> Result<PathBuf, String> {
    let video = if input.starts_with("http://") || input.starts_with("https://") {
        report_status(ui_weak, "Đang tải video…");
        let out = vidub_core::pipeline::media_cache_path(&input);
        let dir = out.parent().expect("media cache path has a parent");
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| format!("Không tạo được thư mục tạm: {e}"))?;
        vidub_core::transcript::probe::download_video(
            yt_dlp.as_deref(),
            &input,
            &out,
            qjs.as_deref(),
        )
        .await
        .map_err(|e| {
            use vidub_core::transcript::probe::user_message;
            format!("Tải video thất bại: {}", user_message(&e))
        })?
    } else {
        let path = PathBuf::from(&input);
        if !path.is_file() {
            return Err(format!("Không tìm thấy file video: {}", path.display()));
        }
        path
    };
    report_status(ui_weak, "Đang mở xem trước…");
    let dub_arg = if use_dub {
        Some(dub_wav.as_path())
    } else {
        None
    };
    spawn_into_slot(&mpv, &video, dub_arg, &shared.preview)
        .await
        .map_err(|e| format!("Mở xem trước thất bại: {e}"))?;
    *lock_unpoisoned(&shared.media) = Some(MediaPaths {
        video: video.clone(),
        dub_wav: Some(dub_wav),
    });
    *lock_unpoisoned(&shared.paused) = false;
    *lock_unpoisoned(&shared.duration_ms) = None;
    Ok(video)
}

fn report_status(ui_weak: &Weak<MainWindow>, msg: &str) {
    let msg = msg.to_owned();
    let ui_weak = ui_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = ui_weak.upgrade() {
            ui.set_preview_status(msg.into());
        }
    });
}

fn format_ms(ms: u64) -> String {
    let total_s = ms / 1000;
    format!("{}:{:02}", total_s / 60, total_s % 60)
}

pub async fn spawn_into_slot(
    mpv: &std::path::Path,
    video: &std::path::Path,
    dub: Option<&std::path::Path>,
    preview_slot: &Arc<Mutex<Option<Arc<MpvHandle>>>>,
) -> anyhow::Result<()> {
    let old = lock_unpoisoned(preview_slot).take();
    if let Some(old) = old {
        tokio::spawn(async move {
            let _ = old.close().await;
        });
    }
    let handle = MpvHandle::spawn(mpv, video, dub).await?;
    *lock_unpoisoned(preview_slot) = Some(Arc::new(handle));
    Ok(())
}
