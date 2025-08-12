use eframe::egui;
use std::sync::{Arc, Mutex};

use nostr::{EventBuilder, Kind, nips::nip19::ToBech32};

use crate::{
    types::*,
    cache_db::{DB_PROFILES},
};

pub fn draw_profile_view(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    app_data: &mut NostrStatusAppInternal,
    app_data_arc: Arc<Mutex<NostrStatusAppInternal>>,
    runtime_handle: tokio::runtime::Handle,
) {
    let profile_heading_text = "あなたのプロフィール";
    let public_key_heading_text = "あなたの公開鍵 (npub)";
    let copy_button_text = "コピー";
    let nip01_profile_heading_text = "NIP-01 プロフィールメタデータ";
    let name_label_text = "名前:";
    let picture_url_label_text = "画像URL:";
    let nip05_label_text = "NIP-05:";
    let lud16_label_text = "LUD-16:";
    let about_label_text = "自己紹介:";
    let other_fields_label_text = "その他のフィールド:";
    let save_profile_button_text = "プロフィールを保存";
    let raw_json_heading_text = "生JSON";
    let logout_button_text = "ログアウト";

    let card_frame = egui::Frame {
        inner_margin: egui::Margin::same(12),
        corner_radius: 8.0.into(),
        shadow: eframe::epaint::Shadow::NONE,
        fill: app_data.current_theme.card_background_color(),
        ..Default::default()
    };

    egui::ScrollArea::vertical().id_salt("profile_tab_scroll_area").show(ui, |ui| {
        card_frame.show(ui, |ui| {
            ui.heading(profile_heading_text);
            ui.add_space(10.0);

            ui.heading(public_key_heading_text);
            ui.add_space(5.0);
            let public_key_bech32 = app_data.my_keys.as_ref().map_or("N/A".to_string(), |k| k.public_key().to_bech32().unwrap_or_default());
            ui.horizontal(|ui| {
                ui.label(public_key_bech32.clone());
                if ui.button(copy_button_text).clicked() {
                    ctx.copy_text(public_key_bech32);
                    app_data.should_repaint = true;
                }
            });
            ui.add_space(15.0);

            ui.heading(nip01_profile_heading_text);
            ui.add_space(10.0);

            ui.label(app_data.profile_fetch_status.as_str());

            ui.horizontal(|ui| {
                ui.label(name_label_text);
                ui.text_edit_singleline(&mut app_data.editable_profile.name);
            });
            ui.horizontal(|ui| {
                ui.label(picture_url_label_text);
                ui.text_edit_singleline(&mut app_data.editable_profile.picture);
            });
            ui.horizontal(|ui| {
                ui.label(nip05_label_text);
                ui.text_edit_singleline(&mut app_data.editable_profile.nip05);
            });
            ui.horizontal(|ui| {
                ui.label(lud16_label_text);
                ui.text_edit_singleline(&mut app_data.editable_profile.lud16);
            });
            ui.label(about_label_text);
            ui.add(egui::TextEdit::multiline(&mut app_data.editable_profile.about)
                .desired_rows(3)
                .desired_width(ui.available_width()));

            if !app_data.editable_profile.extra.is_empty() {
                ui.label(other_fields_label_text);
                for (key, value) in app_data.editable_profile.extra.iter().take(5) {
                    ui.horizontal(|ui| {
                        ui.label(format!("{key}:"));
                        let mut display_value = value.to_string();
                        ui.add(egui::TextEdit::singleline(&mut display_value)
                            .interactive(false));
                    });
                }
                if app_data.editable_profile.extra.len() > 5 {
                    ui.label("... more fields not shown ...");
                }
            }

            ui.add_space(10.0);
            let save_profile_button = egui::Button::new(egui::RichText::new(save_profile_button_text).strong());
            if ui.add_enabled(!app_data.is_loading, save_profile_button).clicked() {
                let client_clone = app_data.nostr_client.as_ref().unwrap().clone();
                let keys_clone = app_data.my_keys.clone().unwrap();
                let editable_profile_clone = app_data.editable_profile.clone();
                let cache_db_clone = app_data.cache_db.clone();

                app_data.is_loading = true;
                app_data.should_repaint = true;

                let cloned_app_data_arc = app_data_arc.clone();
                runtime_handle.spawn(async move {
                    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
                        let profile_content = serde_json::to_string(&editable_profile_clone)?;

                        let event = EventBuilder::new(Kind::Metadata, profile_content.clone())
                            .sign(&keys_clone)
                            .await?;

                        match client_clone.send_event(&event).await {
                            Ok(event_id) => {
                                println!("NIP-01 profile published with event id: {event_id:?}");
                                let pubkey_hex = keys_clone.public_key().to_string();
                                if let Err(e) = cache_db_clone.write_cache(DB_PROFILES, &pubkey_hex, &editable_profile_clone) {
                                    eprintln!("Failed to write profile cache: {e}");
                                }

                                let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                                app_data_async.profile_fetch_status = "Profile saved!".to_string();
                                app_data_async.nip01_profile_display = serde_json::to_string_pretty(&serde_json::from_str::<serde_json::Value>(&profile_content)?)?;
                            }
                            Err(e) => {
                                let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                                app_data_async.profile_fetch_status = format!("Failed to save profile: {e}");
                            }
                        }
                        Ok(())
                    }.await;

                    if let Err(e) = result {
                        let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                        app_data_async.profile_fetch_status = format!("Error saving profile: {e}");
                    }

                    let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                    app_data_async.is_loading = false;
                    app_data_async.should_repaint = true;
                });
            }

            ui.add_space(20.0);
            ui.heading(raw_json_heading_text);
            ui.add_space(5.0);
            egui::ScrollArea::vertical().id_salt("raw_nip01_profile_scroll_area").max_height(200.0).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut app_data.nip01_profile_display)
                    .desired_width(ui.available_width())
                    .interactive(false)
                    .hint_text("Raw NIP-01 Profile Metadata JSON will appear here."));
            });

            ui.add_space(50.0);
            ui.separator();
            if ui.button(egui::RichText::new(logout_button_text).color(egui::Color32::RED)).clicked() {
                let client_to_shutdown = app_data.nostr_client.take();

                app_data.is_logged_in = false;
                app_data.my_keys = None;
                app_data.followed_pubkeys.clear();
                app_data.followed_pubkeys_display.clear();
                app_data.timeline_posts.clear();
                app_data.status_message_input.clear();
                app_data.passphrase_input.clear();
                app_data.confirm_passphrase_input.clear();
                app_data.secret_key_input.clear();
                app_data.current_tab = AppTab::Home;
                app_data.nip01_profile_display.clear();
                app_data.editable_profile = ProfileMetadata::default();
                app_data.profile_fetch_status = "Please log in.".to_string();
                app_data.should_repaint = true;
                println!("Logged out.");

                if let Some(client) = client_to_shutdown {
                    runtime_handle.spawn(async move {
                        client.shutdown().await;
                    });
                }
            }
        });
    });
}
