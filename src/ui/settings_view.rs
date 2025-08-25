use crate::{
    cache_db::{DB_FOLLOWED, DB_PROFILES, DB_RELAYS, DB_TIMELINE},
    save_config,
    types::{AppTab, AppTheme, NostrPostAppInternal, ProfileMetadata, RelayConfig, TimelinePost, UserBackup},
};
use eframe::egui;
use nostr::{nips::nip19::ToBech32, PublicKey};
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
    runtime_handle: Handle,
) {
    let logout_button_text = "ログアウト";

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
            if let Ok(cache) = app_data
                .cache_db
                .read_cache::<ProfileMetadata>(DB_PROFILES, &pubkey_hex)
            {
                backup.profile = Some(cache.data);
            }
            if let Ok(cache) = app_data
                .cache_db
                .read_cache::<HashSet<PublicKey>>(DB_FOLLOWED, &pubkey_hex)
            {
                backup.followed_pubkeys = Some(cache.data);
            }
            if let Ok(cache) = app_data
                .cache_db
                .read_cache::<RelayConfig>(DB_RELAYS, &pubkey_hex)
            {
                backup.relays = Some(cache.data);
            }
            if let Ok(cache) = app_data
                .cache_db
                .read_cache::<Vec<TimelinePost>>(DB_TIMELINE, &pubkey_hex)
            {
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

    ui.add_space(20.0);
    ui.separator();
    ui.add_space(20.0);

    // --- Danger Zone ---
    let danger_frame = egui::Frame {
        inner_margin: egui::Margin::same(12),
        corner_radius: 8.0.into(),
        shadow: eframe::epaint::Shadow::NONE,
        fill: app_data.current_theme.danger_zone_background_color(),
        stroke: egui::Stroke::new(1.0, app_data.current_theme.danger_zone_stroke_color()),
        ..Default::default()
    };
    danger_frame.show(ui, |ui| {
        ui.heading("公開鍵とログアウト");
        ui.add_space(10.0);

        ui.label("あなたの公開鍵 (npub)");
        let public_key_bech32 = app_data
            .my_keys
            .as_ref()
            .map_or("N/A".to_string(), |k| {
                k.public_key().to_bech32().unwrap_or_default()
            });
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut public_key_bech32.clone())
                .on_hover_text("クリックしてコピー");
            if ui.button("コピー").clicked() {
                ctx.copy_text(public_key_bech32);
            }
        });

        ui.add_space(20.0);
        ui.separator();
        ui.add_space(20.0);

        if ui
            .button(
                egui::RichText::new(logout_button_text)
                    .color(egui::Color32::RED)
                    .strong(),
            )
            .clicked()
        {
            let client_to_shutdown = app_data.nostr_client.take();

            app_data.is_logged_in = false;
            app_data.my_keys = None;
            app_data.followed_pubkeys.clear();
            app_data.followed_pubkeys_display.clear();
            app_data.timeline_posts.clear();
            app_data.post_input.clear();
            app_data.passphrase_input.clear();
            app_data.confirm_passphrase_input.clear();
            app_data.secret_key_input.clear();
            app_data.current_tab = AppTab::Home;
            app_data.nip01_profile_display.clear();
            app_data.editable_profile = ProfileMetadata::default();
            app_data.profile_fetch_status = "ログインしてください".to_string();
            app_data.should_repaint = true;
            println!("Logged out.");

            if let Some(client) = client_to_shutdown {
                runtime_handle.spawn(async move {
                    client.shutdown().await;
                });
            }
        }
    });
}

fn update_theme(theme: AppTheme, ctx: &egui::Context) {
    let visuals = match theme {
        AppTheme::Light => crate::theme::light_visuals(),
        AppTheme::Dark => crate::theme::dark_visuals(),
    };
    ctx.set_visuals(visuals);
}
