use eframe::egui;
use std::sync::{Arc, Mutex};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use nostr::{EventBuilder, Filter, Kind, PublicKey, Tag, nips::nip19::ToBech32};
use nostr_sdk::Client;
use regex::Regex;

use crate::{
    types::*,
    nostr_client::{fetch_relays_for_followed_users, update_contact_list},
    cache_db::DB_FOLLOWED,
    MAX_STATUS_LENGTH,
};


fn render_post_content(
    ui: &mut egui::Ui,
    app_data: &mut NostrStatusAppInternal,
    post: &TimelinePost,
    urls_to_load: &mut Vec<String>,
) {
    let re = Regex::new(r":(\w+):").unwrap();
    let mut last_end = 0;
    let text_color = app_data.current_theme.text_color();

    ui.horizontal_wrapped(|ui| {
        for cap in re.captures_iter(&post.content) {
            let full_match = cap.get(0).unwrap();
            let shortcode = cap.get(1).unwrap().as_str();

            // Render text before the emoji
            let pre_text = &post.content[last_end..full_match.start()];
            if !pre_text.is_empty() {
                ui.label(egui::RichText::new(pre_text).color(text_color));
            }

            // Render the emoji
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
                        if !urls_to_load.contains(&url_key) {
                            urls_to_load.push(url_key.clone());
                        }
                        let (rect, _) = ui.allocate_exact_size(emoji_size, egui::Sense::hover());
                        ui.put(rect, egui::Spinner::new());
                    }
                }
            } else {
                // If shortcode not found, render it as text
                ui.label(egui::RichText::new(full_match.as_str()).color(text_color));
            }

            last_end = full_match.end();
        }

        // Render remaining text after the last emoji
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
        // --- 背景を暗くする ---
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
    // --- タイムライン表示 ---
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
            // println!("Fetching latest statuses...");

            let cloned_app_data_arc = app_data_arc.clone();
            runtime_handle.spawn(async move {
                let timeline_result: Result<Vec<TimelinePost>, Box<dyn std::error::Error + Send + Sync>> = async {
                    if followed_pubkeys.is_empty() {
                        println!("No followed users to fetch status from.");
                        return Ok(Vec::new());
                    }

                    // 1. DiscoverリレーでフォローユーザーのNIP-65(kind:10002)を取得
                    let discover_client = Client::new(my_keys.clone());
                    for relay_url in discover_relays.lines().filter(|url| !url.trim().is_empty()) {
                        discover_client.add_relay(relay_url.trim()).await?;
                    }
                    discover_client.connect().await;
                    let followed_pubkeys_vec: Vec<PublicKey> = followed_pubkeys.iter().cloned().collect();
                    let write_relay_urls = fetch_relays_for_followed_users(&discover_client, followed_pubkeys_vec).await?;
                    discover_client.shutdown().await;

                    if write_relay_urls.is_empty() {
                        println!("No writeable relays found for followed users.");
                        return Ok(Vec::new());
                    }

                    // 2. 取得したwriteリレーで新しい一時クライアントを作成
                    let temp_client = Client::new(my_keys.clone());
                    for url in &write_relay_urls {
                        temp_client.add_relay(url.clone()).await?;
                    }
                    temp_client.connect().await;

                    // 3. フォローユーザーのステータス(kind:30315)を取得
                    let timeline_filter = Filter::new().authors(followed_pubkeys).kind(Kind::from(30315)).limit(20);
                    let status_events = temp_client.fetch_events(timeline_filter, Duration::from_secs(10)).await?;
                    println!("Fetched {} statuses from followed users' write relays.", status_events.len());

                    let mut timeline_posts = Vec::new();
                    if !status_events.is_empty() {
                        // 4. ステータス投稿者のプロフィール(kind:0)を取得
                        let author_pubkeys: HashSet<PublicKey> = status_events.iter().map(|e| e.pubkey).collect();
                        println!("Fetching metadata for {} authors.", author_pubkeys.len());
                        let metadata_filter = Filter::new().authors(author_pubkeys.into_iter()).kind(Kind::Metadata);
                        let metadata_events = temp_client.fetch_events(metadata_filter, Duration::from_secs(5)).await?;

                        let mut profiles: HashMap<PublicKey, ProfileMetadata> = HashMap::new();
                        for event in metadata_events {
                            if let Ok(metadata) = serde_json::from_str::<ProfileMetadata>(&event.content) {
                                profiles.insert(event.pubkey, metadata);
                            }
                        }
                        println!("Fetched {} profiles.", profiles.len());

                        // 5. ステータスとメタデータをマージ
                        for event in status_events {
                            let emojis = event.tags.iter().filter_map(|tag| {
                                if let Some(nostr::TagStandard::Emoji { shortcode, url }) = tag.as_standardized() {
                                    Some((shortcode.to_string(), url.to_string()))
                                } else {
                                    None
                                }
                            }).collect();
                            timeline_posts.push(TimelinePost {
                                author_pubkey: event.pubkey,
                                author_metadata: profiles.get(&event.pubkey).cloned().unwrap_or_default(),
                                content: event.content.clone(),
                                created_at: event.created_at,
                                emojis,
                            });
                        }
                    }

                    // 6. 一時クライアントをシャットダウン
                    temp_client.shutdown().await;

                    Ok(timeline_posts)
                }.await;

                let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                app_data_async.is_loading = false;
                match timeline_result {
                    Ok(mut posts) => {
                        if !posts.is_empty() {
                            posts.sort_by_key(|p| std::cmp::Reverse(p.created_at));
                            println!("Fetched {} statuses successfully.", posts.len());
                            app_data_async.timeline_posts = posts;
                        } else {
                            app_data_async.timeline_posts.clear();
                            println!("No new statuses found for followed users.");
                        }
                    },
                    Err(e) => {
                        eprintln!("Failed to fetch timeline: {e}");
                        // エラーが発生してもタイムラインはクリアしない
                    }
                }
                app_data_async.should_repaint = true;
            });
        }
        ui.add_space(10.0);
        let mut pubkey_to_modify: Option<(PublicKey, bool)> = None;
        let mut urls_to_load = Vec::new();
        egui::ScrollArea::vertical().id_salt("timeline_scroll_area").max_height(ui.available_height() - 100.0).show(ui, |ui| {
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                if app_data.timeline_posts.is_empty() {
                    ui.label(no_timeline_message_text);
                } else {
                    let posts_clone = app_data.timeline_posts.clone();
                    for post in &posts_clone {
                        card_frame.show(ui, |ui| {
                            ui.horizontal(|ui| {
                                // --- Profile Picture ---
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
                                            urls_to_load.push(url_key.clone());
                                            let (rect, _) = ui.allocate_exact_size(avatar_size, egui::Sense::hover());
                                            ui.painter().rect_filled(rect, corner_radius, ui.style().visuals.widgets.inactive.bg_fill);
                                            ui.put(rect, egui::Spinner::new());
                                        }
                                    }
                                } else {
                                    let (rect, _) = ui.allocate_exact_size(avatar_size, egui::Sense::hover());
                                    ui.painter().rect_filled(rect, corner_radius, ui.style().visuals.widgets.inactive.bg_fill);
                                }

                                ui.add_space(8.0); // アイコンと名前の間のスペース

                                let display_name = if !post.author_metadata.name.is_empty() {
                                    post.author_metadata.name.clone()
                                } else {
                                    let pubkey = post.author_pubkey.to_bech32().unwrap_or_default();
                                    format!("{}...{}", &pubkey[0..8], &pubkey[pubkey.len()-4..])
                                };
                                ui.label(egui::RichText::new(display_name).strong().color(app_data.current_theme.text_color()));

                                // --- Timestamp ---
                                let created_at_datetime = chrono::DateTime::from_timestamp(post.created_at.as_u64() as i64, 0).unwrap();
                                let local_datetime = created_at_datetime.with_timezone(&chrono::Local);
                                ui.label(egui::RichText::new(local_datetime.format("%Y-%m-%d %H:%M:%S").to_string()).color(egui::Color32::GRAY).small());


                                // --- Context Menu ---
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
                            // Custom emoji rendering
                            render_post_content(ui, app_data, post, &mut urls_to_load);
                        });
                        ui.add_space(10.0);
                    }
                }
            });
        });

        let data_clone = app_data_arc.clone();
        for url_key in urls_to_load {
            app_data.image_cache.insert(url_key.clone(), ImageState::Loading);
            app_data.should_repaint = true;

            let app_data_clone = data_clone.clone();
            let ctx_clone = ctx.clone();
            let request = ehttp::Request::get(&url_key);

            ehttp::fetch(request, move |result| {
                let new_state = match result {
                    Ok(response) => {
                        if response.ok {
                            match image::load_from_memory(&response.bytes) {
                                Ok(dynamic_image) => {
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
                            // キャッシュも更新
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

    // --- フローティングアクションボタン (FAB) ---
    egui::Area::new("fab_area".into())
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-20.0, -20.0))
        .show(ctx, |ui| {
            if ui.button(egui::RichText::new("➕").size(24.0)).clicked() {
                app_data.show_post_dialog = true;
            }
        });
}
