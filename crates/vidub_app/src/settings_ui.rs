use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use sha2::{Digest, Sha256};
use slint::{ComponentHandle, VecModel, Weak};

use vidub_config::{
    AppSettings, LoudnessPreset, ModelPreset, Provider, DEFAULT_VOICE_FILE, VOICES,
};
use vidub_sidecar::versions::{self, ManifestEntry};

use crate::bridge::{lock_unpoisoned, resolve_key, AppState};
use crate::MainWindow;

const MIN_VOICE_BYTES: u64 = 10 * 1024 * 1024;

const NVIDIA_MODELS_URL: &str = "https://integrate.api.nvidia.com/v1/models";
const OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
const GEMINI_MODELS_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";

const OPENROUTER_REFERER: &str = "https://openrouter.ai";
const OPENROUTER_TITLE: &str = "ViDub";

pub(crate) fn provider_index(provider: Provider) -> i32 {
    match provider {
        Provider::Nvidia => 0,
        Provider::OpenRouter => 1,
        Provider::Gemini => 2,
    }
}

fn provider_from_index(index: i32) -> Provider {
    match index {
        1 => Provider::OpenRouter,
        2 => Provider::Gemini,
        _ => Provider::Nvidia,
    }
}

pub(crate) fn key_heading(provider: Provider) -> &'static str {
    match provider {
        Provider::Nvidia => "API key NVIDIA của bạn",
        Provider::OpenRouter => "API key OpenRouter của bạn",
        Provider::Gemini => "API key Google Gemini của bạn",
    }
}

pub(crate) fn key_hint(provider: Provider) -> &'static str {
    match provider {
        Provider::Nvidia => "Nhận API key miễn phí tại build.nvidia.com",
        Provider::OpenRouter => "Nhận API key tại openrouter.ai/keys",
        Provider::Gemini => "Nhận API key miễn phí tại aistudio.google.com/apikey",
    }
}

pub(crate) fn key_url(provider: Provider) -> &'static str {
    match provider {
        Provider::Nvidia => "https://build.nvidia.com",
        Provider::OpenRouter => "https://openrouter.ai/keys",
        Provider::Gemini => "https://aistudio.google.com/apikey",
    }
}

pub(crate) fn key_missing_hint(provider: Provider) -> &'static str {
    match provider {
        Provider::Nvidia => "Hãy dán API key NVIDIA trước",
        Provider::OpenRouter => "Hãy dán API key OpenRouter trước",
        Provider::Gemini => "Hãy dán API key Google Gemini trước",
    }
}

pub(crate) fn provider_name(provider: Provider) -> &'static str {
    match provider {
        Provider::Nvidia => "NVIDIA",
        Provider::OpenRouter => "OpenRouter",
        Provider::Gemini => "Google Gemini",
    }
}

fn apply_provider_labels(ui: &MainWindow, provider: Provider) {
    ui.set_key_heading(key_heading(provider).into());
    ui.set_key_hint(key_hint(provider).into());
    ui.set_key_link_url(key_url(provider).into());
}

fn selected_provider(settings_slot: &Arc<Mutex<AppSettings>>) -> Provider {
    lock_unpoisoned(settings_slot).provider
}

fn save_key_with_session_fallback(
    provider: Provider,
    key: &str,
    session_keys: &Mutex<std::collections::HashMap<Provider, String>>,
) -> bool {
    if vidub_config::store_key(provider, key).is_ok() {
        lock_unpoisoned(session_keys).remove(&provider);
        true
    } else {
        lock_unpoisoned(session_keys).insert(provider, key.to_owned());
        false
    }
}

pub fn wire_settings(ui: &MainWindow, state: &AppState) {
    let settings_slot = state.settings.clone();
    let session_keys_slot = state.session_keys.clone();
    let ui_weak = ui.as_weak();

    let (voice_file, preset, output_dir, loudness, provider) = {
        let guard = lock_unpoisoned(&settings_slot);
        (
            guard.voice_file.clone(),
            guard.preset,
            guard.output_dir.clone(),
            guard.loudness,
            guard.provider,
        )
    };
    ui.set_provider(provider_index(provider));
    apply_provider_labels(ui, provider);

    let name_model = Rc::new(VecModel::<slint::SharedString>::default());
    let file_model = Rc::new(VecModel::<slint::SharedString>::default());
    for voice in VOICES {
        name_model.push(voice.name.into());
        file_model.push(voice.file.into());
    }
    ui.set_voice_names(name_model.into());
    ui.set_voice_files(file_model.into());
    let voice_index = VOICES
        .iter()
        .position(|v| v.file == voice_file)
        .unwrap_or(default_voice_index());
    ui.set_selected_voice(voice_index as i32);
    ui.set_voice_downloaded(voice_is_downloaded(VOICES[voice_index].file));
    ui.set_quality(preset_index(preset));
    ui.set_loudness(loudness_index(loudness));
    ui.set_output_folder(
        output_dir
            .map(|path| path.display().to_string())
            .unwrap_or_default()
            .into(),
    );
    ui.set_app_version(env!("CARGO_PKG_VERSION").into());

    let link_ui = ui_weak.clone();
    ui.on_key_link_clicked(move |url| {
        let url = url.to_string();
        if url.trim().is_empty() {
            return;
        }
        let ui_weak_open = link_ui.clone();
        std::thread::spawn(move || {
            if open::that(&url).is_err() {
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = ui_weak_open.upgrade() else {
                        return;
                    };
                    ui.set_key_status(
                        "Không mở được trình duyệt — hãy mở trang lấy API key bằng tay".into(),
                    );
                });
            }
        });
    });

    let key_busy = Arc::new(AtomicBool::new(false));
    let test_ui = ui_weak.clone();
    let test_runtime = state.runtime.clone();
    let test_client = state.client.clone();
    let test_settings = settings_slot.clone();
    let test_session_keys = session_keys_slot.clone();
    ui.on_key_test_requested(move |key| {
        let key = key.to_string().trim().to_owned();
        let provider = selected_provider(&test_settings);
        if key.is_empty() {
            if let Some(ui) = test_ui.upgrade() {
                ui.set_key_status(key_missing_hint(provider).into());
            }
            return;
        }
        if key_busy.swap(true, Ordering::SeqCst) {
            if let Some(ui) = test_ui.upgrade() {
                ui.set_key_status("Đang kiểm tra rồi".into());
            }
            return;
        }
        let storage_ok = save_key_with_session_fallback(provider, &key, &test_session_keys);
        if let Some(ui) = test_ui.upgrade() {
            ui.set_settings_key(key.clone().into());
            ui.set_key_test_busy(true);
            ui.set_key_status("Đang kiểm tra API key…".into());
        }
        let client = test_client.clone();
        let ui_weak_task = test_ui.clone();
        let busy_task = key_busy.clone();
        test_runtime.spawn(async move {
            let result = test_key_async(&client, provider, &key).await;
            busy_task.store(false, Ordering::SeqCst);
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = ui_weak_task.upgrade() else {
                    return;
                };
                ui.set_key_test_busy(false);
                match result {
                    Ok(count) => {
                        let note = if storage_ok {
                            String::new()
                        } else {
                            " (chỉ dùng được lần này)".to_owned()
                        };
                        ui.set_key_status(
                            format!("API key hoạt động ✓ — {count} mô hình sẵn sàng{note}").into(),
                        );
                    }
                    Err(message) => {
                        ui.set_key_status(message.into());
                    }
                }
            });
        });
    });

    let commit_ui = ui_weak.clone();
    let commit_settings = settings_slot.clone();
    let commit_session_keys = session_keys_slot.clone();
    ui.on_settings_key_committed(move |key| {
        let key = key.to_string().trim().to_owned();
        let Some(ui) = commit_ui.upgrade() else {
            return;
        };
        let provider = selected_provider(&commit_settings);
        if key.is_empty() {
            lock_unpoisoned(&commit_session_keys).remove(&provider);
            match vidub_config::clear_key(provider) {
                Ok(()) => ui.set_key_status("Đã xóa API key".into()),
                Err(_) => ui.set_key_status("Không xóa được API key — vẫn đang lưu".into()),
            }
            return;
        }
        if save_key_with_session_fallback(provider, &key, &commit_session_keys) {
            ui.set_key_status("Đã lưu API key".into());
        } else {
            ui.set_key_status("Chỉ lưu được cho lần chạy này".into());
        }
    });

    let provider_ui = ui_weak.clone();
    let provider_settings = settings_slot.clone();
    let provider_session_keys = session_keys_slot.clone();
    ui.on_provider_changed(move |index| {
        let Some(ui) = provider_ui.upgrade() else {
            return;
        };
        let provider = provider_from_index(index);
        let previous = {
            let mut guard = lock_unpoisoned(&provider_settings);
            let previous = guard.provider;
            guard.provider = provider;
            previous
        };
        ui.set_provider(provider_index(provider));
        apply_provider_labels(&ui, provider);
        ui.set_key_status("".into());
        let key = if provider == previous {
            ui.get_settings_key().to_string().trim().to_owned()
        } else {
            resolve_key(provider, &provider_session_keys).unwrap_or_default()
        };
        ui.set_settings_key(key.as_str().into());
        persist(&provider_settings, &ui);
    });

    let voice_ui = ui_weak.clone();
    let voice_settings = settings_slot.clone();
    ui.on_voice_changed(move |name| {
        let Some(index) = VOICES.iter().position(|v| v.name == name.as_str()) else {
            return;
        };
        lock_unpoisoned(&voice_settings).voice_file = VOICES[index].file.to_owned();
        if let Some(ui) = voice_ui.upgrade() {
            ui.set_voice_downloaded(voice_is_downloaded(VOICES[index].file));
            persist(&voice_settings, &ui);
        }
    });

    let quality_ui = ui_weak.clone();
    let quality_settings = settings_slot.clone();
    ui.on_quality_changed(move |index| {
        lock_unpoisoned(&quality_settings).preset = preset_from_index(index);
        if let Some(ui) = quality_ui.upgrade() {
            persist(&quality_settings, &ui);
        }
    });

    let loudness_ui = ui_weak.clone();
    let loudness_settings = settings_slot.clone();
    ui.on_loudness_changed(move |index| {
        lock_unpoisoned(&loudness_settings).loudness = loudness_from_index(index);
        if let Some(ui) = loudness_ui.upgrade() {
            persist(&loudness_settings, &ui);
        }
    });

    let folder_ui = ui_weak.clone();
    let folder_settings = settings_slot.clone();
    ui.on_output_folder_changed(move |folder| {
        let Some(ui) = folder_ui.upgrade() else {
            return;
        };
        let text = folder.to_string().trim().to_owned();
        if text.is_empty() {
            lock_unpoisoned(&folder_settings).output_dir = None;
            persist(&folder_settings, &ui);
            ui.set_settings_status("Đã lưu — video sẽ lưu ngay cạnh video gốc".into());
            return;
        }
        let path = PathBuf::from(&text);
        if !path.is_dir() {
            ui.set_settings_status("Thư mục này chưa tồn tại".into());
            return;
        }
        lock_unpoisoned(&folder_settings).output_dir = Some(path);
        persist(&folder_settings, &ui);
    });

    let dl_busy = Arc::new(AtomicBool::new(false));
    let dl_ui = ui_weak.clone();
    let dl_runtime = state.runtime.clone();
    let dl_client = state.client.clone();
    ui.on_voice_download_requested(move |file| {
        let file = file.to_string();
        if !VOICES.iter().any(|v| v.file == file) {
            if let Some(ui) = dl_ui.upgrade() {
                ui.set_download_status("Hãy chọn giọng đọc trước".into());
            }
            return;
        }
        if dl_busy.swap(true, Ordering::SeqCst) {
            if let Some(ui) = dl_ui.upgrade() {
                ui.set_download_status("Đang tải giọng đọc rồi".into());
            }
            return;
        }
        if let Some(ui) = dl_ui.upgrade() {
            ui.set_download_busy(true);
            ui.set_download_progress(0.0);
            ui.set_download_status("Đang tải giọng đọc…".into());
        }
        let client = dl_client.clone();
        let ui_weak_task = dl_ui.clone();
        let busy_task = dl_busy.clone();
        dl_runtime.spawn(async move {
            let result = download_voice_async(&client, &file, &ui_weak_task).await;
            busy_task.store(false, Ordering::SeqCst);
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = ui_weak_task.upgrade() else {
                    return;
                };
                ui.set_download_busy(false);
                match result {
                    Ok(()) => {
                        ui.set_voice_downloaded(true);
                        ui.set_download_status("".into());
                    }
                    Err(message) => {
                        ui.set_download_status(message.into());
                    }
                }
            });
        });
    });
}

fn persist(settings_slot: &Arc<Mutex<AppSettings>>, ui: &MainWindow) {
    let result = {
        let guard = lock_unpoisoned(settings_slot);
        vidub_config::save(&guard)
    };
    match result {
        Ok(()) => ui.set_settings_status("Đã lưu".into()),
        Err(err) => {
            ui.set_settings_status(format!("Không lưu được cài đặt ({err})").into());
        }
    }
}

async fn test_key_async(
    client: &reqwest::Client,
    provider: Provider,
    key: &str,
) -> Result<usize, String> {
    let unreachable = format!(
        "Không kết nối được {}. Hãy kiểm tra mạng rồi thử lại.",
        provider_name(provider)
    );
    let rejected = format!(
        "API key {} không dùng được. Kiểm tra lại xem đã dán đúng chưa.",
        provider_name(provider)
    );
    let request = match provider {
        Provider::Nvidia => client.get(NVIDIA_MODELS_URL).bearer_auth(key),
        Provider::OpenRouter => client
            .get(OPENROUTER_MODELS_URL)
            .bearer_auth(key)
            .header("HTTP-Referer", OPENROUTER_REFERER)
            .header("X-Title", OPENROUTER_TITLE),
        Provider::Gemini => client.get(GEMINI_MODELS_URL).header("x-goog-api-key", key),
    };
    let resp = request.send().await.map_err(|_| unreachable)?;
    if !resp.status().is_success() {
        return Err(rejected);
    }
    let value: serde_json::Value = resp.json().await.map_err(|_| rejected)?;
    let list_key = match provider {
        Provider::Gemini => "models",
        Provider::Nvidia | Provider::OpenRouter => "data",
    };
    Ok(value
        .get(list_key)
        .and_then(|list| list.as_array())
        .map_or(0, |entries| entries.len()))
}

async fn download_voice_async(
    client: &reqwest::Client,
    file: &str,
    ui_weak: &Weak<MainWindow>,
) -> Result<(), String> {
    let url = vidub_config::voice_url(file);
    let cache_models = vidub_sidecar::locator::SearchRoots::live().cache_models;
    std::fs::create_dir_all(&cache_models)
        .map_err(|err| format!("Không tạo được thư mục giọng đọc ({err})"))?;

    let mut resp = client.get(&url).send().await.map_err(|_| {
        "Không kết nối được máy chủ tải giọng đọc. Kiểm tra mạng rồi thử lại.".to_owned()
    })?;
    if !resp.status().is_success() {
        return Err(format!(
            "Máy chủ tải giọng đọc trả về {}. Hãy thử lại.",
            resp.status()
        ));
    }
    let total = resp.content_length().unwrap_or(0);

    let mut tmp = tempfile::NamedTempFile::new_in(&cache_models)
        .map_err(|err| format!("Không ghi được tệp giọng đọc ({err})"))?;
    let mut hasher = Sha256::new();
    let mut received: u64 = 0;
    loop {
        let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|_| "Quá trình tải bị ngắt giữa chừng. Hãy thử lại.".to_owned())?
        else {
            break;
        };
        hasher.update(&chunk);
        tmp.write_all(&chunk)
            .map_err(|err| format!("Không ghi được tệp giọng đọc ({err})"))?;
        received += chunk.len() as u64;
        let fraction = if total > 0 {
            received as f32 / total as f32
        } else {
            0.0
        };
        let percent = (fraction * 100.0) as usize;
        let ui_weak_chunk = ui_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = ui_weak_chunk.upgrade() else {
                return;
            };
            ui.set_download_progress(fraction);
            if total > 0 {
                ui.set_download_status(format!("Đang tải… {percent}%").into());
            }
        });
    }

    if received <= MIN_VOICE_BYTES {
        return Err("Tệp tải về quá nhỏ để làm giọng đọc. Hãy thử lại.".to_owned());
    }
    let digest = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let dest = cache_models.join(file);
    let _path = tmp
        .persist(&dest)
        .map_err(|_| "Không lưu xong tệp giọng đọc. Hãy thử lại.".to_owned())?;
    stage_voice_config(&cache_models);
    record_voice_sha(file, &digest, &url);
    Ok(())
}

fn voice_is_downloaded(file: &str) -> bool {
    let roots = vidub_sidecar::locator::SearchRoots::live();
    [
        roots.bundled_models.join(file),
        roots.cache_models.join(file),
    ]
    .iter()
    .any(|path| {
        path.metadata()
            .map(|meta| meta.len() > MIN_VOICE_BYTES)
            .unwrap_or(false)
    })
}

fn stage_voice_config(cache_models: &Path) {
    let dest = cache_models.join("config.json");
    if dest.is_file() {
        return;
    }
    let bundled = vidub_sidecar::locator::SearchRoots::live()
        .bundled_models
        .join("config.json");
    if bundled.is_file() {
        let _ = std::fs::copy(&bundled, &dest);
    }
}

fn record_voice_sha(file: &str, sha256: &str, url: &str) {
    let Some(manifest) = vidub_sidecar::cache_root()
        .parent()
        .map(|parent| parent.join("versions.json"))
    else {
        return;
    };
    let mut entries = versions::read_manifest(&manifest);
    entries.retain(|entry| entry.name != file);
    entries.push(ManifestEntry {
        name: file.to_owned(),
        version: String::new(),
        sha256: sha256.to_owned(),
        source_url: url.to_owned(),
    });
    let _ = versions::write_manifest(&manifest, &entries);
}

fn preset_index(preset: ModelPreset) -> i32 {
    match preset {
        ModelPreset::Fast => 0,
        ModelPreset::Balanced => 1,
        ModelPreset::Quality => 2,
    }
}

fn preset_from_index(index: i32) -> ModelPreset {
    match index {
        0 => ModelPreset::Fast,
        2 => ModelPreset::Quality,
        _ => ModelPreset::Balanced,
    }
}

fn loudness_index(loudness: LoudnessPreset) -> i32 {
    match loudness {
        LoudnessPreset::Youtube => 0,
        LoudnessPreset::Podcast => 1,
        LoudnessPreset::Broadcast => 2,
    }
}

fn loudness_from_index(index: i32) -> LoudnessPreset {
    match index {
        0 => LoudnessPreset::Youtube,
        2 => LoudnessPreset::Broadcast,
        _ => LoudnessPreset::Podcast,
    }
}

fn default_voice_index() -> usize {
    VOICES
        .iter()
        .position(|v| v.file == DEFAULT_VOICE_FILE)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radio_indices_round_trip() {
        for (index, preset) in [
            (0, ModelPreset::Fast),
            (1, ModelPreset::Balanced),
            (2, ModelPreset::Quality),
        ] {
            assert_eq!(preset_index(preset), index);
            assert_eq!(preset_from_index(index), preset);
        }
        assert_eq!(preset_from_index(99), ModelPreset::Balanced);

        for (index, loudness) in [
            (0, LoudnessPreset::Youtube),
            (1, LoudnessPreset::Podcast),
            (2, LoudnessPreset::Broadcast),
        ] {
            assert_eq!(loudness_index(loudness), index);
            assert_eq!(loudness_from_index(index), loudness);
        }
        assert_eq!(loudness_from_index(99), LoudnessPreset::Podcast);

        for (index, provider) in [
            (0, Provider::Nvidia),
            (1, Provider::OpenRouter),
            (2, Provider::Gemini),
        ] {
            assert_eq!(provider_index(provider), index);
            assert_eq!(provider_from_index(index), provider);
        }
        assert_eq!(provider_from_index(99), Provider::Nvidia);
    }
}
