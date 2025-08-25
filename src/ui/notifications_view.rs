use eframe::egui;
use nostr::{nips::nip19::ToBech32};
use std::sync::{Arc, Mutex};

use crate::{
    types::*,
    ui::{image_cache, post, zap, events},
};

pub fn draw_notifications_view(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    app_data: &mut NostrPostAppInternal,
    app_data_arc: Arc<Mutex<NostrPostAppInternal>>,
    runtime_handle: tokio::runtime::Handle,
) {
    let mut urls_to_load: Vec<(String, ImageKind)> = Vec::new();
    let timeline_heading_text = "通知";
    let fetch_latest_button_text = "通知を更新";
    let no_timeline_message_text = "通知はまだありません。";

    // --- ZAP Dialog ---
    if app_data.show_zap_dialog {
        if let Some(post_to_zap) = app_data.zap_target_post.clone() {
            let mut close_dialog = false;
            egui::Window::new("ZAPを送る")
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.vertical_centered_justified(|ui| {
                        ui.add_space(10.0);
                        let display_name = if !post_to_zap.author_metadata.name.is_empty() {
                            post_to_zap.author_metadata.name.clone()
                        } else {
                            let pubkey = post_to_zap.author_pubkey.to_bech32().unwrap_or_default();
                            format!("{}...{}", &pubkey[0..8], &pubkey[pubkey.len()-4..])
                        };
                        ui.label(format!("{} にZAPします", display_name));
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.label("金額 (sats):");
                            ui.add(egui::TextEdit::singleline(&mut app_data.zap_amount_input)
                                .desired_width(120.0));
                        });
                        ui.add_space(10.0);
                    });

                    ui.separator();
                    ui.add_space(5.0);

                    ui.horizontal(|ui| {
                        if ui.button("キャンセル").clicked() {
                           close_dialog = true;
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("ZAP").clicked() {
                                if let (Some(nwc), Some(nwc_client), Some(my_keys)) =
                                    (app_data.nwc.as_ref(), app_data.nwc_client.as_ref(), app_data.my_keys.as_ref())
                                {
                                    if let Ok(amount_sats) = app_data.zap_amount_input.parse::<u64>() {
                                        let nwc_clone = nwc.clone();
                                        let nwc_client_clone = nwc_client.clone();
                                        let my_keys_clone = my_keys.clone();
                                        let app_data_clone = app_data_arc.clone();

                                        runtime_handle.spawn(async move {
                                            {
                                                let mut data = app_data_clone.lock().unwrap();
                                                data.should_repaint = true;
                                            } // Lock is dropped here

                                            let result = zap::send_zap_request(
                                                &nwc_clone,
                                                &nwc_client_clone,
                                                &my_keys_clone,
                                                post_to_zap.author_pubkey,
                                                &post_to_zap.author_metadata.lud16,
                                                amount_sats,
                                                Some(post_to_zap.id),
                                                Some(post_to_zap.kind),
                                            ).await;

                                            let mut data = app_data_clone.lock().unwrap();
                                            match result {
                                                Ok(_) => {
                                                    // ZAPリクエストを送信しました。ウォレットの確認を待っています...
                                                }
                                                Err(e) => {
                                                    eprintln!("ZAPエラー: {}", e);
                                                }
                                            }
                                            data.should_repaint = true;
                                        });

                                        close_dialog = true;

                                    } else {
                                        eprintln!("無効な金額です");
                                    }
                                } else {
                                    eprintln!("ZAPにはNWCの接続が必要です");
                                }
                            }
                        });
                    });
                });
            if close_dialog {
                app_data.show_zap_dialog = false;
                app_data.zap_target_post = None;
            }
        }
    }

    let card_frame = egui::Frame {
        inner_margin: egui::Margin::same(12),
        corner_radius: 8.0.into(),
        shadow: eframe::epaint::Shadow::NONE,
        fill: app_data.current_theme.card_background_color(),
        ..Default::default()
    };
    card_frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.heading(timeline_heading_text);

            let fetch_button = egui::Button::new(egui::RichText::new(fetch_latest_button_text).strong());
            if ui.add_enabled(!app_data.is_loading, fetch_button).clicked() {
                if let (Some(client), Some(keys)) = (
                    app_data.nostr_client.as_ref(),
                    app_data.my_keys.as_ref(),
                ) {
                    let client = client.clone();
                    let keys = keys.clone();
                    let cache_db = app_data.cache_db.clone();
                    let relay_config = app_data.relays.clone();
                    let cloned_app_data_arc = app_data_arc.clone();

                    app_data.is_loading = true;
                    app_data.should_repaint = true;

                    runtime_handle.spawn(async move {
                        match events::refresh_all_data(&client, &keys, &cache_db, &relay_config).await {
                            Ok(fresh_data) => {
                                let mut app_data = cloned_app_data_arc.lock().unwrap();
                                app_data.timeline_posts = fresh_data.timeline_posts;
                                app_data.notification_posts = fresh_data.notification_posts;
                                app_data.editable_profile = fresh_data.profile_metadata;
                                println!("Refreshed all data from notifications view.");
                            }
                            Err(e) => {
                                eprintln!("Failed to refresh data: {}", e);
                            }
                        }
                        let mut app_data = cloned_app_data_arc.lock().unwrap();
                        app_data.is_loading = false;
                        app_data.should_repaint = true;
                    });
                }
            }

            if app_data.is_loading {
                ui.add_space(10.0);
                ui.spinner();
                ui.label("更新中...");
            }
        });
        ui.add_space(10.0);

        if app_data.notification_posts.is_empty() {
            ui.label(no_timeline_message_text);
        } else {
            let num_posts = app_data.notification_posts.len();
            let row_height = 90.0;

            let card_frame = egui::Frame {
                inner_margin: egui::Margin::same(0),
                corner_radius: 8.0.into(),
                shadow: eframe::epaint::Shadow::NONE,
                fill: app_data.current_theme.card_background_color(),
                ..Default::default()
            };
            card_frame.show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("notification_scroll_area")
                    .max_height(ui.available_height() - 100.0)
                    .show_rows(ui, row_height, num_posts, |ui, row_range| {
                        for i in row_range {
                            let post_data = app_data.notification_posts[i].clone();
                            post::render_post(
                                ui,
                                app_data,
                                &post_data,
                                &mut urls_to_load,
                                app_data_arc.clone(),
                                runtime_handle.clone(),
                            );
                            ui.add_space(5.0);
                        }
                    });
            });
        }

        // --- Image Loading Logic ---

        // First, try to load images from the LMDB cache for URLs not in memory.
        let cache_db = app_data.cache_db.clone();
        let mut still_to_load = Vec::new();
        for (url_key, kind) in urls_to_load {
            if let Some(image_bytes) = image_cache::load_from_lmdb(&cache_db, &url_key) {
                // Image found in cache, process it directly.
                // This is a simplification; for a smoother UI, this should be async.
                if let Ok(mut dynamic_image) = image::load_from_memory(&image_bytes) {
                    let (width, height) = match kind {
                        ImageKind::Avatar => (32, 32),
                        ImageKind::Emoji => (20, 20),
                    _ => (32, 32), // Default for Banner, ProfilePicture, etc.
                    };
                    dynamic_image = dynamic_image.thumbnail(width, height);
                    let color_image = egui::ColorImage::from_rgba_unmultiplied(
                        [dynamic_image.width() as usize, dynamic_image.height() as usize],
                        dynamic_image.to_rgba8().as_flat_samples().as_slice(),
                    );
                    let texture_handle = ctx.load_texture(
                        &url_key,
                        color_image,
                        Default::default()
                    );
                    app_data.image_cache.insert(url_key, ImageState::Loaded(texture_handle));
                } else {
                    // Failed to decode, mark as failed.
                    app_data.image_cache.insert(url_key, ImageState::Failed);
                }
            } else {
                // Not on disk, queue for network download.
                still_to_load.push((url_key, kind));
            }
        }

        // Fetch remaining images from the network.
        let data_clone = app_data_arc.clone();
        for (url_key, kind) in still_to_load {
            app_data.image_cache.insert(url_key.clone(), ImageState::Loading);
            app_data.should_repaint = true;

            let app_data_clone = data_clone.clone();
            let ctx_clone = ctx.clone();
            let cache_db_for_fetch = app_data.cache_db.clone();
            let request = ehttp::Request::get(&url_key);

            ehttp::fetch(request, move |result| {
                let new_state = match result {
                    Ok(response) => {
                        if response.ok {
                            // Save to LMDB cache first.
                            image_cache::save_to_lmdb(&cache_db_for_fetch, &response.url, &response.bytes);

                            match image::load_from_memory(&response.bytes) {
                                Ok(mut dynamic_image) => {
                                    let (width, height) = match kind {
                                        ImageKind::Avatar => (32, 32),
                                        ImageKind::Emoji => (20, 20),
                                    _ => (32, 32), // Default for Banner, ProfilePicture, etc.
                                    };
                                    dynamic_image = dynamic_image.thumbnail(width, height);

                                    let color_image = egui::ColorImage::from_rgba_unmultiplied(
                                        [dynamic_image.width() as usize, dynamic_image.height() as usize],
                                        dynamic_image.to_rgba8().as_flat_samples().as_slice(),
                                    );
                                    let texture_handle = ctx_clone.load_texture(
                                        &response.url,
                                        color_image,
                                        Default::default()
                                    );
                                    ImageState::Loaded(texture_handle)
                                }
                                Err(_) => ImageState::Failed,
                            }
                        } else {
                            ImageState::Failed
                        }
                    }
                    Err(_) => ImageState::Failed,
                };

                let mut app_data = app_data_clone.lock().unwrap();
                app_data.image_cache.insert(url_key, new_state);
                ctx_clone.request_repaint();
            });
        }
    });

}
