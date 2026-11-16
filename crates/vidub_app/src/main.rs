#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod bridge;
mod dub_ui;
mod export_ui;
mod preview_ui;
mod settings_ui;
mod setup_ui;

use bridge::{lock_unpoisoned, refresh_sidecars, resolve_key, AppState};

slint::include_modules!();

fn main() {
    let runtime = tokio::runtime::Runtime::new().expect("failed to build tokio runtime");
    let ui = MainWindow::new().expect("failed to create window");

    let settings = vidub_config::load();
    let provider = settings.provider;
    let state = AppState::new(&ui, runtime.handle().clone(), settings);
    let key = resolve_key(provider, &state.session_keys);
    if let Some(key) = &key {
        ui.set_settings_key(key.clone().into());
    }
    ui.set_provider(settings_ui::provider_index(provider));
    ui.set_key_heading(settings_ui::key_heading(provider).into());
    ui.set_key_hint(settings_ui::key_hint(provider).into());
    ui.set_key_link_url(settings_ui::key_url(provider).into());

    let settings_slot = state.settings.clone();
    let paths_slot = state.paths.clone();
    preview_ui::wire_preview(&ui, &state);
    export_ui::wire_export(&ui, &state);
    settings_ui::wire_settings(&ui, &state);
    dub_ui::wire_dub(&ui, &state);
    setup_ui::wire_setup(&ui, &state);
    refresh_sidecars(state);

    let have_key = key.is_some();
    if !have_key {
        ui.set_page(1);
        ui.set_status(
            format!(
                "Chào bạn! Bước 1: dán API key {} vào đây.",
                settings_ui::provider_name(provider)
            )
            .into(),
        );
    } else {
        let ui_weak = ui.as_weak();
        runtime.spawn(async move {
            for _ in 0..150 {
                if lock_unpoisoned(&paths_slot).is_some() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
            let paths = lock_unpoisoned(&paths_slot).clone();
            let settings = lock_unpoisoned(&settings_slot);
            let voice_present = setup_ui::voice_downloaded(&settings, paths.as_ref());
            let (ready, todo) =
                setup_ui::summarize_setup(&settings, have_key, voice_present, paths.as_ref());
            drop(settings);
            let message = if ready {
                "Sẵn sàng! Dán link video để bắt đầu.".to_owned()
            } else {
                format!("Gần xong — {}", todo.join("; "))
            };
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = ui_weak.upgrade() else { return };
                ui.set_status(message.into());
            });
        });
    }
    ui.run().expect("event loop failed");
}
