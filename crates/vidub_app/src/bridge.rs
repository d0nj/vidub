use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use slint::{ComponentHandle, VecModel, Weak};
use vidub_config::Provider;

use crate::{MainWindow, SidecarRow};

#[derive(Clone)]
pub struct AppState {
    pub ui_weak: Weak<MainWindow>,
    pub runtime: tokio::runtime::Handle,
    pub paths: Arc<Mutex<Option<vidub_sidecar::BinPaths>>>,
    pub dub: Arc<Mutex<Option<DubOutput>>>,
    pub client: reqwest::Client,
    pub media: Arc<Mutex<Option<MediaPaths>>>,
    pub last_input: Arc<Mutex<Option<String>>>,
    pub preview: Arc<Mutex<Option<Arc<vidub_mpv::control::MpvHandle>>>>,
    pub settings: Arc<Mutex<vidub_config::AppSettings>>,
    pub session_keys: Arc<Mutex<HashMap<Provider, String>>>,
}

impl AppState {
    pub fn new(
        ui: &MainWindow,
        runtime: tokio::runtime::Handle,
        settings: vidub_config::AppSettings,
    ) -> Self {
        Self {
            ui_weak: ui.as_weak(),
            runtime,
            paths: Arc::new(Mutex::new(None)),
            dub: Arc::new(Mutex::new(None)),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            media: Arc::new(Mutex::new(None)),
            last_input: Arc::new(Mutex::new(None)),
            preview: Arc::new(Mutex::new(None)),
            settings: Arc::new(Mutex::new(settings)),
            session_keys: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

pub struct DubOutput {
    pub title: String,
    pub wav_path: std::path::PathBuf,
}

pub struct MediaPaths {
    pub video: std::path::PathBuf,
    pub dub_wav: Option<std::path::PathBuf>,
}

pub fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn resolve_key(
    provider: Provider,
    session_keys: &Mutex<HashMap<Provider, String>>,
) -> Option<String> {
    vidub_config::stored_key(provider)
        .filter(|key| !key.trim().is_empty())
        .or_else(|| {
            lock_unpoisoned(session_keys)
                .get(&provider)
                .filter(|key| !key.trim().is_empty())
                .cloned()
        })
        .or_else(|| {
            std::env::var(provider.key_env_var())
                .ok()
                .filter(|key| !key.trim().is_empty())
        })
}

pub fn refresh_sidecars(state: AppState) {
    let ui_weak = state.ui_weak.clone();
    let paths_slot = state.paths.clone();
    state.runtime.spawn(async move {
        let (paths, statuses) = vidub_sidecar::ensure_all().await;
        *lock_unpoisoned(&paths_slot) = Some(paths);
        let rows: Vec<SidecarRow> = statuses
            .into_iter()
            .map(|status| SidecarRow {
                name: status.name.into(),
                version: status.version.unwrap_or_default().into(),
                source: match status.source {
                    vidub_sidecar::SidecarSource::Missing => "missing",
                    _ => "ready",
                }
                .into(),
            })
            .collect();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let model = Rc::new(VecModel::default());
            for row in rows {
                model.push(row);
            }
            ui.set_sidecars(model.into());
        });
    });
}

pub fn human_stage(stage: &str) -> &str {
    match stage {
        "probing" => "Đang xem video…",
        "fetching captions" => "Đang lấy phụ đề…",
        "extracting audio" => "Đang tách âm thanh…",
        "transcribing" => "Đang nghe và ghi lại lời thoại…",
        "slicing" => "Đang chia nhỏ âm thanh…",
        "connecting" => "Đang kết nối…",
        "merging" => "Đang ghép lời thoại…",
        "translating" => "Đang dịch sang tiếng Việt…",
        "sync" => "Đang ghép giọng vào video…",
        "resampling" => "Đang cân bằng âm thanh…",
        "measuring" => "Đang đo âm lượng…",
        "muxing" => "Đang ghép hình và tiếng…",
        other => other,
    }
}
