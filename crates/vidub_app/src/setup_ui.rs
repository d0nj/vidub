use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use slint::ComponentHandle;
use vidub_config::AppSettings;
use vidub_sidecar::setup::{setup_missing, SetupEvent};
use vidub_sidecar::BinPaths;

use crate::bridge::AppState;
use crate::MainWindow;

pub struct SetupItem {
    pub label: String,
    pub done: bool,
    pub hint: String,
}

pub fn setup_checklist(
    settings: &AppSettings,
    has_key: bool,
    voice_present: bool,
    paths: Option<&BinPaths>,
) -> Vec<SetupItem> {
    let fallback;
    let paths = match paths {
        Some(paths) => paths,
        None => {
            fallback = BinPaths::default();
            &fallback
        }
    };
    let voice_name = vidub_config::VOICES
        .iter()
        .find(|voice| voice.file == settings.voice_file.as_str())
        .map(|voice| voice.name.to_owned())
        .unwrap_or_else(|| settings.voice_file.clone());
    vec![
        SetupItem {
            label: format!(
                "Thêm API key {}",
                crate::settings_ui::provider_name(settings.provider)
            ),
            done: has_key,
            hint: "Dán API key trong trang Cài đặt".to_owned(),
        },
        SetupItem {
            label: format!("Tải giọng đọc: {voice_name}"),
            done: voice_present,
            hint: "Chỉ tải một lần rồi dùng luôn".to_owned(),
        },
        SetupItem {
            label: "Kiểm tra công cụ video (ffmpeg)".to_owned(),
            done: paths.ffmpeg.is_some(),
            hint: "Cần có để lưu video".to_owned(),
        },
        SetupItem {
            label: "Kiểm tra công cụ YouTube (yt-dlp)".to_owned(),
            done: paths.yt_dlp.is_some(),
            hint: "Cần có để đọc video YouTube".to_owned(),
        },
        SetupItem {
            label: "Kiểm tra trình phát video (mpv)".to_owned(),
            done: paths.mpv.is_some(),
            hint: "Không bắt buộc — chỉ để xem trước".to_owned(),
        },
    ]
}

pub fn summarize_setup(
    settings: &AppSettings,
    has_key: bool,
    voice_present: bool,
    paths: Option<&BinPaths>,
) -> (bool, Vec<String>) {
    let items = setup_checklist(settings, has_key, voice_present, paths);
    let last = items.len().saturating_sub(1);
    let mut todo = Vec::new();
    let mut ready = true;
    for (index, item) in items.into_iter().enumerate() {
        if item.done {
            continue;
        }
        todo.push(format!("{} — {}", item.label, item.hint));
        if index != last {
            ready = false;
        }
    }
    (ready, todo)
}

pub fn voice_downloaded(settings: &AppSettings, paths: Option<&BinPaths>) -> bool {
    if settings.voice_file.is_empty() {
        return false;
    }
    if settings.voice_file == vidub_sidecar::locator::VOICE_FILE_NAME {
        if let Some(paths) = paths {
            return paths.voice_onnx.is_some();
        }
    }
    let roots = vidub_sidecar::locator::SearchRoots::live();
    roots.bundled_models.join(&settings.voice_file).is_file()
        || roots.cache_models.join(&settings.voice_file).is_file()
}

pub fn wire_setup(ui: &MainWindow, state: &AppState) {
    let ui_weak = ui.as_weak();
    let runtime = state.runtime.clone();
    let state = state.clone();
    let busy = Arc::new(AtomicBool::new(false));

    ui.on_setup_requested(move || {
        if busy.swap(true, Ordering::SeqCst) {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_setup_status("Đang thiết lập…".into());
            }
            return;
        }
        if let Some(ui) = ui_weak.upgrade() {
            ui.set_setup_status("Đang thiết lập công cụ…".into());
            ui.set_setup_busy(true);
        }

        let ui_weak_task = ui_weak.clone();
        let busy_task = busy.clone();
        let state_task = state.clone();
        runtime.spawn(async move {
            let progress_cb: Box<dyn Fn(SetupEvent) + Send + Sync> = {
                let ui_weak = ui_weak_task.clone();
                Box::new(move |event| {
                    let ui_weak = ui_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(ui) = ui_weak.upgrade() else { return };
                        match event {
                            SetupEvent::Checking(name) => {
                                ui.set_setup_status(format!("Đang kiểm tra {name}…").into());
                            }
                            SetupEvent::Downloading(name) => {
                                ui.set_setup_status(format!("Đang tải {name}…").into());
                            }
                            SetupEvent::Ready(name) => {
                                ui.set_setup_status(format!("Đã sẵn sàng: {name}").into());
                            }
                            SetupEvent::Failed { name, msg } => {
                                ui.set_setup_status(format!("Không cài được {name}: {msg}").into());
                            }
                        }
                    });
                })
            };

            let report = setup_missing(&*progress_cb).await;
            busy_task.store(false, Ordering::SeqCst);

            let installed = report.installed.len();
            let failed = report.failed.clone();
            let mut summary = if failed.is_empty() {
                format!("Hoàn tất! Đã sẵn sàng {installed} công cụ.")
            } else {
                format!(
                    "Xong! {installed} công cụ đã sẵn sàng, {} chưa cài được: {}.",
                    failed.len(),
                    failed.join(", ")
                )
            };
            if failed.iter().any(|name| name == "mpv") {
                summary.push_str(" Trình phát video trên Linux: sudo apt install mpv.");
            }
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_weak_task.upgrade() {
                    ui.set_setup_busy(false);
                    ui.set_setup_status(summary.into());
                }
            });

            crate::bridge::refresh_sidecars(state_task);
        });
    });
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn all_paths() -> BinPaths {
        BinPaths {
            yt_dlp: Some(PathBuf::from("yt-dlp")),
            ffmpeg: Some(PathBuf::from("ffmpeg")),
            ffprobe: Some(PathBuf::from("ffprobe")),
            mpv: Some(PathBuf::from("mpv")),
            piper: Some(PathBuf::from("piper")),
            qjs: Some(PathBuf::from("qjs")),
            voice_onnx: Some(PathBuf::from("ngochuyennew.onnx")),
        }
    }

    #[test]
    fn summary_ready_when_everything_present() {
        let settings = AppSettings::default();
        let paths = all_paths();
        let (ready, todo) = summarize_setup(&settings, true, true, Some(&paths));
        assert!(ready);
        assert!(todo.is_empty());
    }

    #[test]
    fn summary_blocks_on_missing_key() {
        let settings = AppSettings::default();
        let paths = all_paths();
        let (ready, todo) = summarize_setup(&settings, false, true, Some(&paths));
        assert!(!ready);
        assert_eq!(todo.len(), 1);
        assert!(todo[0].contains("API key NVIDIA"));
    }

    #[test]
    fn summary_treats_missing_player_as_optional() {
        let settings = AppSettings::default();
        let mut paths = all_paths();
        paths.mpv = None;
        let (ready, todo) = summarize_setup(&settings, true, true, Some(&paths));
        assert!(ready);
        assert_eq!(todo.len(), 1);
    }

    #[test]
    fn summary_blocks_on_missing_voice_and_tools() {
        let settings = AppSettings::default();
        let paths = BinPaths::default();
        let (ready, todo) = summarize_setup(&settings, true, false, Some(&paths));
        assert!(!ready);
        assert_eq!(todo.len(), 4); // voice, ffmpeg, yt-dlp, mpv
    }

    #[test]
    fn summary_before_probe_reports_all_missing() {
        let settings = AppSettings::default();
        let (ready, todo) = summarize_setup(&settings, true, true, None);
        assert!(!ready);
        assert_eq!(todo.len(), 3); // ffmpeg, yt-dlp, mpv (key + voice given)
    }

    #[test]
    fn checklist_is_five_all_done_rows_when_ready() {
        let settings = AppSettings::default();
        let items = setup_checklist(&settings, true, true, Some(&all_paths()));
        assert_eq!(items.len(), 5);
        assert!(items.iter().all(|item| item.done));
    }

    #[test]
    fn voice_downloaded_rejects_empty_file_name() {
        let settings = AppSettings {
            voice_file: String::new(),
            ..AppSettings::default()
        };
        assert!(!voice_downloaded(&settings, None));
    }

    #[test]
    fn voice_url_shapes_bucket_download_offline() {
        let url = vidub_config::voice_url("ngochuyennew.onnx");
        assert!(url.starts_with("https://"), "unexpected voice url: {url}");
        assert!(
            url.ends_with("ngochuyennew.onnx?download=true"),
            "unexpected voice url: {url}"
        );
    }
}
