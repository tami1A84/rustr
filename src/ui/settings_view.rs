use crate::{
    cache_db::{DB_FOLLOWED, DB_PROFILES, DB_RELAYS, DB_TIMELINE},
    save_config,
    types::{AppTheme, NostrPostAppInternal, RelayConfig, UserBackup, ProfileMetadata, TimelinePost},
};
use eframe::egui;
use nostr::PublicKey;
use rfd::FileDialog;
use std::collections::HashSet;
use std::fs;
use std::sync::{Arc, Mutex};
use tokio::runtime::Handle;

pub fn draw_settings_view(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    app_data: &mut NostrPostAppInternal,
    _app_data_arc: Arc<Mutex<NostrPostAppInternal>>,
    _runtime_handle: Handle,
) {
    ui.heading("設定");
    ui.add_space(10.0);

    // --- テーマ設定 ---
    ui.label("テーマ");
    if ui
        .selectable_value(&mut app_data.current_theme, AppTheme::Light, "ライト")
        .clicked()
        || ui
            .selectable_value(&mut app_data.current_theme, AppTheme::Dark, "ダーク")
            .clicked()
    {
        update_theme(app_data.current_theme, ctx);
        save_config(app_data);
    }

    ui.add_space(20.0);
    ui.separator();
    ui.add_space(20.0);

    // --- イベントデータのバックアップ ---
    ui.heading("データのバックアップ");
    ui.add_space(10.0);
    ui.label("公開データをファイルにバックアップします。");
    ui.add_space(10.0);

    if ui.button("バックアップをダウンロード").clicked() {
        if let Some(keys) = &app_data.my_keys {
            let pubkey_hex = keys.public_key().to_string();
            let mut backup = UserBackup::default();

            // Fetch data from cache
            if let Ok(cache) = app_data.cache_db.read_cache::<ProfileMetadata>(DB_PROFILES, &pubkey_hex) {
                backup.profile = Some(cache.data);
            }
            if let Ok(cache) = app_data.cache_db.read_cache::<HashSet<PublicKey>>(DB_FOLLOWED, &pubkey_hex) {
                backup.followed_pubkeys = Some(cache.data);
            }
            if let Ok(cache) = app_data.cache_db.read_cache::<RelayConfig>(DB_RELAYS, &pubkey_hex) {
                backup.relays = Some(cache.data);
            }
            if let Ok(cache) = app_data.cache_db.read_cache::<Vec<TimelinePost>>(DB_TIMELINE, &pubkey_hex) {
                backup.timeline = Some(cache.data);
            }

            match serde_json::to_string_pretty(&backup) {
                Ok(json_str) => {
                    let file_path = FileDialog::new()
                        .set_file_name("nostr-backup.json")
                        .add_filter("JSON", &["json"])
                        .save_file();

                    if let Some(path) = file_path {
                        if let Err(e) = fs::write(path, json_str) {
                            eprintln!("Failed to write backup file: {}", e);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Failed to serialize backup data: {}", e);
                }
            }
        } else {
            // This part should ideally not be reached if the user is logged in.
            // Maybe show a message to the user.
            eprintln!("Not logged in, cannot perform backup.");
        }
    }
}

fn update_theme(theme: AppTheme, ctx: &egui::Context) {
    let visuals = match theme {
        AppTheme::Light => crate::theme::light_visuals(),
        AppTheme::Dark => crate::theme::dark_visuals(),
    };
    ctx.set_visuals(visuals);
}
