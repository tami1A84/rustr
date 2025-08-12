use eframe::egui;
use std::sync::{Arc, Mutex};
use nostr::{EventBuilder, Kind, PublicKey, Tag, nips::nip19::ToBech32, EventId};
use regex::Regex;

use crate::{
    types::*,
    nostr_client::{update_contact_list, fetch_timeline_events},
    cache_db::DB_FOLLOWED,
    MAX_STATUS_LENGTH,
    ui::image_cache,
};

fn render_post_content(
    ui: &mut egui::Ui,
    app_data: &mut NostrStatusAppInternal,
    post: &TimelinePost,
    urls_to_load: &mut Vec<(String, ImageKind)>,
) {
    let re = Regex::new(r":(\w+):").unwrap();
    let mut last_end = 0;
    let text_color = app_data.current_theme.text_color();

    ui.horizontal_wrapped(|ui| {
        for cap in re.captures_iter(&post.content) {
            let full_match = cap.get(0).unwrap();
            let shortcode = cap.get(1).unwrap().as_str();

            let pre_text = &post.content[last_end..full_match.start()];
            if !pre_text.is_empty() {
                ui.label(egui::RichText::new(pre_text).color(text_color));
            }

            if let Some(url) = post.emojis.get(shortcode) {
                let emoji_size = egui::vec2(20.0, 20.0);
                let url_key = url.to_string();

                match app_data.image_cache.get(&url_key) {
                    Some(ImageState::Loaded(texture_handle)) => {
                        let image_widget =
                            egui::Image::new(texture_handle).fit_to_exact_size(emoji_size);
                        ui.add(image_widget);
                    }
                    Some(ImageState::Loading) => {
                        let (rect, _) = ui.allocate_exact_size(emoji_size, egui::Sense::hover());
                        ui.put(rect, egui::Spinner::new());
                    }
                    Some(ImageState::Failed) => {
                        let (rect, _) = ui.allocate_exact_size(emoji_size, egui::Sense::hover());
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "💔".to_string(),
                            egui::FontId::default(),
                            ui.visuals().error_fg_color,
                        );
                    }
                    None => {
                        if !urls_to_load.iter().any(|(u, _)| u == &url_key) {
                            urls_to_load.push((url_key.clone(), ImageKind::Emoji));
                        }
                        let (rect, _) = ui.allocate_exact_size(emoji_size, egui::Sense::hover());
                        ui.put(rect, egui::Spinner::new());
                    }
                }
            } else {
                ui.label(egui::RichText::new(full_match.as_str()).color(text_color));
            }

            last_end = full_match.end();
        }

        let remaining_text = &post.content[last_end..];
        if !remaining_text.is_empty() {
            ui.label(egui::RichText::new(remaining_text).color(text_color));
        }
    });
}

pub fn draw_home_view(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    app_data: &mut NostrStatusAppInternal,
    app_data_arc: Arc<Mutex<NostrStatusAppInternal>>,
    runtime_handle: tokio::runtime::Handle,
) {
    let new_post_window_title_text = "新規投稿";
    let set_status_heading_text = "ステータスを設定";
    let status_input_hint_text = "いまどうしてる？";
    let publish_button_text = "公開";
    let cancel_button_text = "キャンセル";
    let status_too_long_text = "ステータスが長すぎます！";
    let timeline_heading_text = "タイムライン";
    let fetch_latest_button_text = "最新の投稿を取得";
    let no_timeline_message_text = "タイムラインに投稿はまだありません。";

    let card_frame = egui::Frame {
        inner_margin: egui::Margin::same(12),
        corner_radius: 8.0.into(),
        shadow: eframe::epaint::Shadow::NONE,
        fill: app_data.current_theme.card_background_color(),
        ..Default::default()
    };

    if app_data.show_post_dialog {
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Background, "dim_layer".into()));
        let screen_rect = ctx.screen_rect();
        painter.add(egui::Shape::rect_filled(screen_rect, 0.0, egui::Color32::from_black_alpha(128)));

        egui::Window::new(new_post_window_title_text)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.heading(set_status_heading_text);
                ui.add_space(15.0);
                ui.add(egui::TextEdit::multiline(&mut app_data.status_message_input)
                    .desired_rows(5)
                    .hint_text(status_input_hint_text));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(format!("{}/{}", app_data.status_message_input.chars().count(), MAX_STATUS_LENGTH));
                    if app_data.status_message_input.chars().count() > MAX_STATUS_LENGTH {
                        ui.label(egui::RichText::new(status_too_long_text).color(egui::Color32::RED).strong());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(publish_button_text).clicked() && !app_data.is_loading {
                            let status_message = app_data.status_message_input.clone();
                            let client_clone_nip38_send = app_data.nostr_client.as_ref().unwrap().clone();
                            let keys_clone_nip38_send = app_data.my_keys.clone().unwrap();

                            app_data.is_loading = true;
                            app_data.should_repaint = true;
                            println!("Publishing NIP-38 status...");

                            if status_message.chars().count() > MAX_STATUS_LENGTH {
                                eprintln!("Status is too long (max {MAX_STATUS_LENGTH} chars)");
                                app_data.is_loading = false;
                                app_data.should_repaint = true;
                                return;
                            }

                            let cloned_app_data_arc = app_data_arc.clone();
                            runtime_handle.spawn(async move {
                                let d_tag_value = "general".to_string();
                                let event_result = EventBuilder::new(Kind::from(30315), status_message.clone())
                                    .tags(vec![Tag::identifier(d_tag_value)])
                                    .sign(&keys_clone_nip38_send)
                                    .await;
                                match event_result {
                                    Ok(event) => match client_clone_nip38_send.send_event(&event).await {
                                        Ok(event_id) => {
                                            println!("Status published with event id: {event_id:?}");
                                            let mut data = cloned_app_data_arc.lock().unwrap();
                                            data.status_message_input.clear();
                                            data.show_post_dialog = false;
                                        }
                                        Err(e) => {
                                            eprintln!("Failed to publish status: {e}");
                                        }
                                    },
                                    Err(e) => {
                                        eprintln!("Failed to create event: {e}");
                                    }
                                }
                                let mut data = cloned_app_data_arc.lock().unwrap();
                                data.is_loading = false;
                                data.should_repaint = true;
                            });
                        }
                        if ui.button(cancel_button_text).clicked() {
                            app_data.show_post_dialog = false;
                        }
                    });
                });
            });
    }

    card_frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.heading(timeline_heading_text);
            if app_data.is_loading {
                ui.add_space(10.0);
                ui.spinner();
                ui.label("更新中...");
            }
        });
        ui.add_space(15.0);
        let fetch_button = egui::Button::new(egui::RichText::new(fetch_latest_button_text).strong());
        if ui.add_enabled(!app_data.is_loading, fetch_button).clicked() {
            let followed_pubkeys = app_data.followed_pubkeys.clone();
            let discover_relays = app_data.discover_relays_editor.clone();
            let my_keys = app_data.my_keys.clone().unwrap();

            app_data.is_loading = true;
            app_data.should_repaint = true;

            let cloned_app_data_arc = app_data_arc.clone();
            runtime_handle.spawn(async move {
                let timeline_result = fetch_timeline_events(&my_keys, &discover_relays, &followed_pubkeys).await;

                let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                app_data_async.is_loading = false;
                match timeline_result {
                    Ok(new_posts) => {
                        if !new_posts.is_empty() {
                            let mut existing_ids: std::collections::HashSet<EventId> = app_data_async.timeline_posts.iter().map(|p| p.id).collect();
                            let mut added_posts = 0;
                            for post in new_posts {
                                if !existing_ids.contains(&post.id) {
                                    existing_ids.insert(post.id);
                                    app_data_async.timeline_posts.push(post);
                                    added_posts += 1;
                                }
                            }

                            if added_posts > 0 {
                                app_data_async.timeline_posts.sort_by_key(|p| std::cmp::Reverse(p.created_at));
                                println!("Added {} new statuses to the timeline.", added_posts);
                            } else {
                                println!("No new statuses found.");
                            }
                        } else {
                            println!("Fetched 0 statuses.");
                        }
                    },
                    Err(e) => {
                        eprintln!("Failed to fetch timeline: {e}");
                    }
                }
                app_data_async.should_repaint = true;
            });
        }
        ui.add_space(10.0);
        let mut pubkey_to_modify: Option<(PublicKey, bool)> = None;
        let mut urls_to_load: Vec<(String, ImageKind)> = Vec::new();

        if app_data.timeline_posts.is_empty() {
            ui.label(no_timeline_message_text);
        } else {
            let num_posts = app_data.timeline_posts.len();
            let row_height = 90.0;

            egui::ScrollArea::vertical()
                .id_salt("timeline_scroll_area")
                .max_height(ui.available_height() - 100.0)
                .show_rows(ui, row_height, num_posts, |ui, row_range| {
                    for i in row_range {
                        let post = app_data.timeline_posts[i].clone();
                        card_frame.show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let avatar_size = egui::vec2(32.0, 32.0);
                                let corner_radius = 4.0;
                                let url = &post.author_metadata.picture;

                                if !url.is_empty() {
                                    let url_key = url.to_string();
                                    let image_state = app_data.image_cache.get(&url_key).cloned();

                                    match image_state {
                                        Some(ImageState::Loaded(texture_handle)) => {
                                            let image_widget = egui::Image::new(&texture_handle)
                                                .corner_radius(corner_radius)
                                                .fit_to_exact_size(avatar_size);
                                            ui.add(image_widget);
                                        }
                                        Some(ImageState::Loading) => {
                                            let (rect, _) = ui.allocate_exact_size(avatar_size, egui::Sense::hover());
                                            ui.painter().rect_filled(rect, corner_radius, ui.style().visuals.widgets.inactive.bg_fill);
                                            ui.put(rect, egui::Spinner::new());
                                        }
                                        Some(ImageState::Failed) => {
                                            let (rect, _) = ui.allocate_exact_size(avatar_size, egui::Sense::hover());
                                            ui.painter().rect_filled(rect, corner_radius, ui.style().visuals.error_fg_color.linear_multiply(0.2));
                                        }
                                        None => {
                                            if !urls_to_load.iter().any(|(u, _)| u == &url_key) {
                                                urls_to_load.push((url_key.clone(), ImageKind::Avatar));
                                            }
                                            let (rect, _) = ui.allocate_exact_size(avatar_size, egui::Sense::hover());
                                            ui.painter().rect_filled(rect, corner_radius, ui.style().visuals.widgets.inactive.bg_fill);
                                            ui.put(rect, egui::Spinner::new());
                                        }
                                    }
                                } else {
                                    let (rect, _) = ui.allocate_exact_size(avatar_size, egui::Sense::hover());
                                    ui.painter().rect_filled(rect, corner_radius, ui.style().visuals.widgets.inactive.bg_fill);
                                }

                                ui.add_space(8.0);

                                let display_name = if !post.author_metadata.name.is_empty() {
                                    post.author_metadata.name.clone()
                                } else {
                                    let pubkey = post.author_pubkey.to_bech32().unwrap_or_default();
                                    format!("{}...{}", &pubkey[0..8], &pubkey[pubkey.len()-4..])
                                };
                                ui.label(egui::RichText::new(display_name).strong().color(app_data.current_theme.text_color()));

                                let created_at_datetime = chrono::DateTime::from_timestamp(post.created_at.as_u64() as i64, 0).unwrap();
                                let local_datetime = created_at_datetime.with_timezone(&chrono::Local);
                                ui.label(egui::RichText::new(local_datetime.format("%Y-%m-%d %H:%M:%S").to_string()).color(egui::Color32::GRAY).small());

                                if let Some(my_keys) = &app_data.my_keys {
                                    if post.author_pubkey != my_keys.public_key() {
                                        ui.menu_button("...", |ui| {
                                            let is_followed = app_data.followed_pubkeys.contains(&post.author_pubkey);
                                            let button_text = if is_followed { "アンフォロー" } else { "フォロー" };
                                            if ui.button(button_text).clicked() {
                                                pubkey_to_modify = Some((post.author_pubkey, !is_followed));
                                                ui.close();
                                            }
                                        });
                                    }
                                }
                            });
                            ui.add_space(5.0);
                            render_post_content(ui, app_data, &post, &mut urls_to_load);
                        });
                    }
                });
        }

        // --- Image Loading Logic ---

        // First, try to load images from the disk cache for URLs not in memory.
        let mut still_to_load = Vec::new();
        for (url_key, kind) in urls_to_load {
            if let Some(image_bytes) = image_cache::load_from_disk(&url_key) {
                // Image found on disk, process it directly.
                // This is a simplification; for a smoother UI, this should be async.
                if let Ok(mut dynamic_image) = image::load_from_memory(&image_bytes) {
                    let (width, height) = match kind {
                        ImageKind::Avatar => (32, 32),
                        ImageKind::Emoji => (20, 20),
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
            let request = ehttp::Request::get(&url_key);

            ehttp::fetch(request, move |result| {
                let new_state = match result {
                    Ok(response) => {
                        if response.ok {
                            // Save to disk cache first.
                            image_cache::save_to_disk(&response.url, &response.bytes);

                            match image::load_from_memory(&response.bytes) {
                                Ok(mut dynamic_image) => {
                                    let (width, height) = match kind {
                                        ImageKind::Avatar => (32, 32),
                                        ImageKind::Emoji => (20, 20),
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

        if let Some((pubkey, follow)) = pubkey_to_modify {
            if !app_data.is_loading {
                let client = app_data.nostr_client.as_ref().unwrap().clone();
                let keys = app_data.my_keys.as_ref().unwrap().clone();
                let cache_db_clone = app_data.cache_db.clone();

                app_data.is_loading = true;
                app_data.should_repaint = true;

                let cloned_app_data_arc = app_data_arc.clone();
                runtime_handle.spawn(async move {
                    match update_contact_list(&client, &keys, pubkey, follow).await {
                        Ok(new_followed_pubkeys) => {
                            let mut app_data = cloned_app_data_arc.lock().unwrap();
                            app_data.followed_pubkeys = new_followed_pubkeys;
                            if let Some(keys) = &app_data.my_keys {
                                let pubkey_hex = keys.public_key().to_string();
                                if let Err(e) = cache_db_clone.write_cache(DB_FOLLOWED, &pubkey_hex, &app_data.followed_pubkeys) {
                                    eprintln!("Failed to write follow list cache: {e}");
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Failed to update contact list: {e}");
                        }
                    }
                    let mut app_data = cloned_app_data_arc.lock().unwrap();
                    app_data.is_loading = false;
                    app_data.should_repaint = true;
                });
            }
        }
    });

}
