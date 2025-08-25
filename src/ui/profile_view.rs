use eframe::egui::{self, Sense};
use std::sync::{Arc, Mutex};

use nostr::{EventBuilder, Kind, Filter, TagStandard};

use crate::{
    cache_db::DB_PROFILES,
    types::*,
    ui::{image_cache, post},
};

pub fn draw_profile_view(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    app_data: &mut NostrPostAppInternal,
    app_data_arc: Arc<Mutex<NostrPostAppInternal>>,
    runtime_handle: tokio::runtime::Handle,
) {
    let mut urls_to_load: Vec<(String, ImageKind)> = Vec::new();

    let save_profile_button_text = "プロフィールを保存";

    let card_frame = |ui: &egui::Ui| egui::Frame {
        inner_margin: egui::Margin::same(12),
        corner_radius: 8.0.into(),
        shadow: eframe::epaint::Shadow::NONE,
        fill: ui.visuals().widgets.noninteractive.bg_fill,
        ..Default::default()
    };

    egui::ScrollArea::vertical()
        .id_salt("profile_tab_scroll_area")
        .show(ui, |ui| {
            ui.add_space(20.0);

            // --- Combined Profile Section ---
            ui.horizontal_top(|ui| {
                // --- Left side: Avatar ---
                let avatar_size_val = 80.0;
                let (avatar_rect, _) =
                    ui.allocate_exact_size(egui::vec2(avatar_size_val, avatar_size_val), Sense::hover());

                let picture_url = &app_data.editable_profile.picture;
                if !picture_url.is_empty() {
                    let image_state = app_data.image_cache.get(picture_url).cloned();
                    match image_state {
                        Some(ImageState::Loaded(texture_handle)) => {
                            let image_widget = egui::Image::new(&texture_handle)
                                .sense(Sense::hover())
                                .fit_to_exact_size(avatar_rect.size())
                                .corner_radius(8.0);
                            ui.put(avatar_rect, image_widget);
                        }
                        _ => {
                            if !urls_to_load.iter().any(|(u, _)| u == picture_url) {
                                urls_to_load
                                    .push((picture_url.clone(), ImageKind::ProfilePicture));
                            }
                            ui.painter().rect_filled(
                                avatar_rect,
                                8.0,
                                ui.style().visuals.extreme_bg_color,
                            );
                            ui.put(
                                avatar_rect.shrink(avatar_size_val * 0.4),
                                egui::Spinner::new(),
                            );
                        }
                    }
                } else {
                    ui.painter().rect_filled(
                        avatar_rect,
                        8.0,
                        ui.style().visuals.extreme_bg_color,
                    );
                }

                ui.add_space(15.0);

                // --- Right side: Profile Info Card ---
                ui.vertical(|ui| {
                    card_frame(ui).show(ui, |ui| {
                        egui::Grid::new("profile_grid")
                            .num_columns(2)
                            .spacing([20.0, 10.0])
                            .striped(true)
                            .show(ui, |ui| {
                                ui.label("名前:");
                                ui.text_edit_singleline(&mut app_data.editable_profile.name);
                                ui.end_row();

                                ui.label("自己紹介:");
                                ui.add(egui::TextEdit::multiline(&mut app_data.editable_profile.about)
                                    .desired_rows(3)
                                    .desired_width(f32::INFINITY));
                                ui.end_row();

                                ui.label("画像URL:");
                                ui.text_edit_singleline(&mut app_data.editable_profile.picture);
                                ui.end_row();

                                ui.label("NIP-05:");
                                ui.text_edit_singleline(&mut app_data.editable_profile.nip05);
                                ui.end_row();

                                ui.label("LUD-16:");
                                ui.text_edit_singleline(&mut app_data.editable_profile.lud16);
                                ui.end_row();
                            });

                        ui.add_space(15.0);
                        ui.horizontal(|ui| {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                 let save_button = egui::Button::new(egui::RichText::new(save_profile_button_text).strong());
                                 if ui.add_enabled(!app_data.is_loading, save_button).clicked() {
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
                                                    app_data_async.profile_fetch_status = "プロフィールを保存しました！".to_string();
                                                    app_data_async.nip01_profile_display = serde_json::to_string_pretty(&serde_json::from_str::<serde_json::Value>(&profile_content)?)?;
                                                }
                                                Err(e) => {
                                                    let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                                                    app_data_async.profile_fetch_status = format!("プロフィールの保存に失敗しました: {e}");
                                                }
                                            }
                                            Ok(())
                                        }.await;

                                        if let Err(e) = result {
                                            let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                                            app_data_async.profile_fetch_status = format!("プロフィールの保存中にエラー: {e}");
                                        }

                                        let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                                        app_data_async.is_loading = false;
                                        app_data_async.should_repaint = true;
                                    });
                                 }
                                 if app_data.is_loading {
                                     ui.spinner();
                                 }
                                 ui.label(app_data.profile_fetch_status.as_str());
                            });
                        });
                    });
                });
            });


            ui.add_space(20.0);
            ui.separator();
            ui.add_space(10.0);

            // --- Latest Posts Section ---
            ui.horizontal(|ui| {
                ui.heading("最新の投稿");
                ui.add_space(10.0);
                let refresh_button = egui::Button::new("🔄 更新");
                if ui.add_enabled(!app_data.is_fetching_profile_posts, refresh_button).clicked() {
                    if let (Some(client), Some(keys)) = (app_data.nostr_client.as_ref(), app_data.my_keys.as_ref()) {
                        let client = client.clone();
                        let pubkey = keys.public_key();
                        let cloned_app_data_arc = app_data_arc.clone();

                        app_data.is_fetching_profile_posts = true;
                        app_data.should_repaint = true;

                        runtime_handle.spawn(async move {
                            let filter = Filter::new().author(pubkey).kind(Kind::TextNote).limit(20);
                            match client.fetch_events(filter, std::time::Duration::from_secs(10)).await {
                                Ok(events) => {
                                    let mut posts = Vec::new();
                                    let author_metadata = cloned_app_data_arc.lock().unwrap().editable_profile.clone();
                                    for event in events {
                                        let emojis = event
                                            .tags
                                            .iter()
                                            .filter_map(|tag| {
                                                if let Some(TagStandard::Emoji { shortcode, url }) = tag.as_standardized() {
                                                    Some((shortcode.to_string(), url.to_string()))
                                                } else {
                                                    None
                                                }
                                            })
                                            .collect();

                                        let post = TimelinePost {
                                            id: event.id,
                                            kind: event.kind,
                                            author_pubkey: event.pubkey,
                                            author_metadata: author_metadata.clone(),
                                            content: event.content.clone(),
                                            created_at: event.created_at,
                                            emojis,
                                            tags: event.tags.to_vec(),
                                        };
                                        posts.push(post);
                                    }
                                    let mut app_data = cloned_app_data_arc.lock().unwrap();
                                    app_data.profile_posts = posts;
                                }
                                Err(e) => {
                                    eprintln!("Failed to fetch profile posts: {}", e);
                                }
                            }
                            let mut app_data = cloned_app_data_arc.lock().unwrap();
                            app_data.is_fetching_profile_posts = false;
                            app_data.should_repaint = true;
                        });
                    }
                }
                if app_data.is_fetching_profile_posts {
                    ui.spinner();
                }
            });

            ui.add_space(10.0);

            if app_data.profile_posts.is_empty() {
                ui.label("投稿はまだありません。");
            } else {
                let num_posts = app_data.profile_posts.len();
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
                        .id_salt("profile_posts_scroll_area")
                        .max_height(ui.available_height() - 50.0)
                        .show_rows(ui, row_height, num_posts, |ui, row_range| {
                            for i in row_range {
                                let post_data = app_data.profile_posts[i].clone();
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
        });


    // --- Image Loading Logic ---
    let cache_db = app_data.cache_db.clone();
    let mut still_to_load = Vec::new();
    for (url_key, kind) in urls_to_load {
        if let Some(image_bytes) = image_cache::load_from_lmdb(&cache_db, &url_key) {
            if let Ok(mut dynamic_image) = image::load_from_memory(&image_bytes) {
                let (width, height) = match kind {
                    ImageKind::Avatar => (32, 32),
                    ImageKind::Emoji => (20, 20),
                    ImageKind::ProfilePicture => (100, 100),
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
                app_data.image_cache.insert(url_key, ImageState::Failed);
            }
        } else {
            still_to_load.push((url_key, kind));
        }
    }

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
                        image_cache::save_to_lmdb(&cache_db_for_fetch, &response.url, &response.bytes);

                        match image::load_from_memory(&response.bytes) {
                            Ok(mut dynamic_image) => {
                                let (width, height) = match kind {
                                    ImageKind::Avatar => (32, 32),
                                    ImageKind::Emoji => (20, 20),
                                    ImageKind::ProfilePicture => (100, 100),
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
}
