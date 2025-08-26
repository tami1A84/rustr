use eframe::egui;
use nostr::{nips::nip19::ToBech32, EventBuilder, Kind, Tag, Filter, TagStandard};
use regex::Regex;
use std::sync::{Arc, Mutex};
use std::collections::{HashSet, HashMap};


use crate::{
    types::*,
    ui::{image_cache, post, zap, events},
    MAX_POST_LENGTH,
};

pub fn draw_home_view(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    app_data: &mut NostrPostAppInternal,
    app_data_arc: Arc<Mutex<NostrPostAppInternal>>,
    runtime_handle: tokio::runtime::Handle,
) {
    // --- Quote Fetching Logic ---
    let mut items_to_fetch = HashSet::new();
    if let Ok(mut posts_to_fetch) = app_data.posts_to_fetch.lock() {
        if !posts_to_fetch.is_empty() {
            items_to_fetch = posts_to_fetch.clone();
            posts_to_fetch.clear();
        }
    }

    if !items_to_fetch.is_empty() {
        if let Some(client) = app_data.nostr_client.as_ref() {
            let client = client.clone();
            let app_data_clone = app_data_arc.clone();
            let profile_cache_clone = app_data.profile_cache.clone();

            runtime_handle.spawn(async move {
                // 1. Fetch event content for all items that need fetching.
                let event_ids_to_fetch: HashSet<nostr::EventId> = items_to_fetch.iter().map(|item| *item).collect();
                let events = if !event_ids_to_fetch.is_empty() {
                    let events_filter = Filter::new().ids(event_ids_to_fetch);
                    client.fetch_events(events_filter, std::time::Duration::from_secs(10)).await.unwrap_or_default().into_iter().collect()
                } else {
                    Vec::<nostr::Event>::new()
                };

                if events.is_empty() {
                    return;
                }

                // 2. Determine which author profiles we need to fetch.
                let mut profiles_to_fetch = HashSet::new();
                for event in &events {
                    if !profile_cache_clone.contains_key(&event.pubkey) {
                        profiles_to_fetch.insert(event.pubkey);
                    }
                }

                // 3. Fetch the missing profiles.
                let mut new_profiles = HashMap::new();
                if !profiles_to_fetch.is_empty() {
                    let metadata_filter = Filter::new().authors(profiles_to_fetch).kind(Kind::Metadata);
                    if let Ok(metadata_events) = client.fetch_events(metadata_filter, std::time::Duration::from_secs(5)).await {
                        for event in metadata_events {
                            if let Ok(metadata) = serde_json::from_str(&event.content) {
                                new_profiles.insert(event.pubkey, metadata);
                            }
                        }
                    }
                }

                // 4. Combine existing cache, new profiles, and fetched events to create the final posts.
                let mut profiles = profile_cache_clone;
                profiles.extend(new_profiles.clone());

                let fetched_posts: HashMap<nostr::EventId, Arc<TimelinePost>> = events.into_iter()
                    .filter(|e| e.kind == Kind::TextNote)
                    .map(|event| {
                        let author_metadata = profiles.get(&event.pubkey).cloned().unwrap_or_default();
                        let emojis = event.tags.iter().filter_map(|tag| {
                            if let Some(TagStandard::Emoji { shortcode, url }) = tag.as_standardized() {
                                Some((shortcode.to_string(), url.to_string()))
                            } else {
                                None
                            }
                        }).collect();

                        let timeline_post = Arc::new(TimelinePost {
                            id: event.id,
                            kind: event.kind,
                            author_pubkey: event.pubkey,
                            author_metadata,
                            content: event.content.clone(),
                            created_at: event.created_at,
                            emojis,
                            tags: event.tags.to_vec(),
                        });
                        (event.id, timeline_post)
                    })
                    .collect();

                // 5. Update the application state with the newly fetched data.
                if !fetched_posts.is_empty() {
                    let mut data = app_data_clone.lock().unwrap();
                    data.quoted_posts_cache.extend(fetched_posts);
                    if !new_profiles.is_empty() {
                        data.profile_cache.extend(new_profiles);
                    }
                    data.should_repaint = true;
                }
            });
        }
    }


    let mut urls_to_load: Vec<(String, ImageKind)> = Vec::new();
    let new_post_window_title_text = "新規投稿";
    let post_input_hint_text = "新しい投稿";
    let publish_button_text = "公開";
    let cancel_button_text = "キャンセル";
    let timeline_heading_text = "ホーム";
    let fetch_latest_button_text = "最新の投稿を取得";
    let no_timeline_message_text = "タイムラインに投稿はまだありません。";


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


    // --- Reply Dialog ---
    if app_data.show_reply_dialog {
        if let Some(post_to_reply) = app_data.reply_target_post.clone() {
            let mut close_dialog = false;
            let author_name = if !post_to_reply.author_metadata.name.is_empty() {
                post_to_reply.author_metadata.name.clone()
            } else {
                let pubkey = post_to_reply.author_pubkey.to_bech32().unwrap_or_default();
                format!("{}...{}", &pubkey[0..8], &pubkey[pubkey.len() - 4..])
            };

            egui::Window::new(format!("Replying to {}", author_name))
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .collapsible(false)
                .resizable(true)
                .show(ctx, |ui| {
                    ui.add_space(10.0);
                    ui.label("Original post:");
                    let original_post_frame = egui::Frame {
                        inner_margin: egui::Margin::same(10),
                        corner_radius: 8.0.into(),
                        shadow: eframe::epaint::Shadow::NONE,
                        fill: ui.style().visuals.widgets.inactive.bg_fill,
                        ..Default::default()
                    };
                    original_post_frame.show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(post_to_reply.content.clone())
                                .color(egui::Color32::GRAY)
                                .italics(),
                        );
                    });

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(10.0);

                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut app_data.reply_input)
                                .desired_rows(3)
                                .desired_width(f32::INFINITY)
                                .hint_text("Write your reply..."),
                        );
                    });


                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            close_dialog = true;
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Publish Reply").clicked() {
                                if let (Some(client), Some(keys)) = (
                                    app_data.nostr_client.as_ref(),
                                    app_data.my_keys.as_ref(),
                                ) {
                                    let client = client.clone();
                                    let keys = keys.clone();
                                    let reply_content = app_data.reply_input.clone();
                                    let cloned_app_data_arc = app_data_arc.clone();

                                    runtime_handle.spawn(async move {
                                        let tags = vec![
                                            Tag::event(post_to_reply.id),
                                            Tag::public_key(post_to_reply.author_pubkey),
                                        ];
                                        let event_result =
                                            EventBuilder::new(Kind::TextNote, reply_content)
                                                .tags(tags)
                                                .sign(&keys)
                                                .await;

                                        match event_result {
                                            Ok(event) => match client.send_event(&event).await {
                                                Ok(event_id) => {
                                                    println!("Reply published with event id: {:?}", event_id);
                                                }
                                                Err(e) => eprintln!("Failed to publish reply: {}", e),
                                            },
                                            Err(e) => eprintln!("Failed to create reply event: {}", e),
                                        }

                                        let mut data = cloned_app_data_arc.lock().unwrap();
                                        data.should_repaint = true;
                                    });

                                    close_dialog = true;
                                }
                            }
                        });
                    });
                });

            if close_dialog {
                app_data.show_reply_dialog = false;
                app_data.reply_target_post = None;
                app_data.reply_input.clear();
            }
        }
    }


    if app_data.show_post_dialog {
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Background, "dim_layer".into()));
        let screen_rect = ctx.screen_rect();
        painter.add(egui::Shape::rect_filled(screen_rect, 0.0, egui::Color32::from_black_alpha(128)));

        egui::Window::new(new_post_window_title_text)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .collapsible(false)
            .resizable(true)
            .show(ctx, |ui| {
                egui::TopBottomPanel::bottom("post_dialog_buttons")
                    .show_inside(ui, |ui| {
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if ui.button("😀").clicked() {
                                app_data.show_emoji_picker = !app_data.show_emoji_picker;
                            }

                            let re_nostr = Regex::new(r"nostr:(?:note|nevent)1[a-z0-9]+\s*").unwrap();
                            let user_text = re_nostr.replace_all(&app_data.post_input, "");
                            let effective_len = user_text.chars().count();

                            let counter_string = format!("{}/{}", effective_len, MAX_POST_LENGTH);
                            let mut counter_text = egui::RichText::new(counter_string);
                            if effective_len > MAX_POST_LENGTH {
                                counter_text = counter_text.color(egui::Color32::RED);
                            }
                            ui.label(counter_text);


                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button(cancel_button_text).clicked() {
                                    app_data.show_post_dialog = false;
                                    app_data.post_input.clear();
                                }
                                if ui.button(publish_button_text).clicked() && !app_data.is_loading {
                                    let post_content = app_data.post_input.clone();
                                    let client_clone = app_data.nostr_client.as_ref().unwrap().clone();
                                    let keys_clone = app_data.my_keys.clone().unwrap();

                                    app_data.is_loading = true;
                                    app_data.should_repaint = true;
                                    println!("Publishing post...");

                                    let re_nostr = Regex::new(r"nostr:(?:note|nevent)1[a-z0-9]+\s*").unwrap();
                                    let user_text = re_nostr.replace_all(&post_content, "");
                                    let effective_len = user_text.chars().count();

                                    if effective_len > MAX_POST_LENGTH {
                                        eprintln!("Post is too long (max {MAX_POST_LENGTH} chars)");
                                        app_data.is_loading = false;
                                        app_data.should_repaint = true;
                                        return;
                                    }

                                    let my_emojis = app_data.my_emojis.clone();
                                    let cloned_app_data_arc = app_data_arc.clone();
                                    runtime_handle.spawn(async move {
                                        let mut tags: Vec<Tag> = Vec::new();

                                        // --- Emoji Tags ---
                                        let re = Regex::new(r":(\w+):").unwrap();
                                        let mut used_emojis: std::collections::HashSet<String> = std::collections::HashSet::new();
                                        for cap in re.captures_iter(&post_content) {
                                            if let Some(shortcode) = cap.get(1) {
                                                used_emojis.insert(shortcode.as_str().to_string());
                                            }
                                        }
                                        for shortcode in used_emojis {
                                            if let Some(url) = my_emojis.get(&shortcode) {
                                                if let Ok(tag) = Tag::parse(["emoji", &shortcode, url]) {
                                                    tags.push(tag);
                                                }
                                            }
                                        }

                                        let event_result = EventBuilder::new(Kind::TextNote, post_content.clone())
                                            .tags(tags)
                                            .sign(&keys_clone)
                                            .await;

                                        match event_result {
                                            Ok(event) => match client_clone.send_event(&event).await {
                                                Ok(event_id) => {
                                                    println!("Post published with event id: {event_id:?}");
                                                    let mut data = cloned_app_data_arc.lock().unwrap();
                                                    data.post_input.clear();
                                                    data.show_post_dialog = false;
                                                }
                                                Err(e) => {
                                                    eprintln!("Failed to publish post: {e}");
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
                            });
                        });
                    });

                egui::CentralPanel::default().show_inside(ui, |ui| {
                    ui.add_space(15.0);
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut app_data.post_input)
                                .desired_rows(5)
                                .desired_width(f32::INFINITY)
                                .hint_text(post_input_hint_text),
                        );
                    });
                });
            });

        if app_data.show_emoji_picker {
            egui::Window::new("カスタム絵文字")
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 180.0)) // Adjust position to be below the post dialog
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("絵文字を選択");
                    egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP).with_main_wrap(true), |ui| {
                            if app_data.my_emojis.is_empty() {
                                ui.label("カスタム絵文字が設定されていません。");
                            } else {
                                for (shortcode, url) in app_data.my_emojis.clone().into_iter() {
                                    let emoji_size = egui::vec2(24.0, 24.0);
                                    let url_key = url.to_string();

                                    let sense = egui::Sense::click();
                                    let (rect, response) = ui.allocate_exact_size(emoji_size, sense);

                                    if response.hovered() {
                                        ui.painter().rect_filled(rect.expand(2.0), egui::CornerRadius::from(4.0), ui.visuals().widgets.hovered.bg_fill);
                                    }

                                    match app_data.image_cache.get(&url_key) {
                                        Some(ImageState::Loaded(texture_handle)) => {
                                            let image = egui::Image::new(texture_handle).fit_to_exact_size(emoji_size);
                                            image.paint_at(ui, rect);
                                        }
                                        Some(ImageState::Loading) => {
                                            ui.put(rect, egui::Spinner::new());
                                        }
                                        Some(ImageState::Failed) => {
                                            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "💔", egui::FontId::default(), ui.visuals().error_fg_color);
                                        }
                                        None => {
                                            if !urls_to_load.iter().any(|(u, _)| u == &url_key) {
                                                urls_to_load.push((url_key.clone(), ImageKind::Emoji));
                                            }
                                            ui.put(rect, egui::Spinner::new());
                                        }
                                    }

                                    if response.clicked() {
                                        app_data.post_input.push_str(&format!(":{}:", shortcode));
                                        app_data.show_emoji_picker = false;
                                    }
                                    response.on_hover_text(&format!(":{}:", shortcode));
                                }
                            }
                        });
                    });
                    if ui.button("閉じる").clicked() {
                        app_data.show_emoji_picker = false;
                    }
                });
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
                        match events::refresh_timeline(&client, &keys, &cache_db, &relay_config).await {
                            Ok(timeline_posts) => {
                                let mut app_data = cloned_app_data_arc.lock().unwrap();
                                app_data.timeline_posts = timeline_posts;
                                println!("Refreshed timeline from home view.");
                            }
                            Err(e) => {
                                eprintln!("Failed to refresh timeline: {}", e);
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

        if app_data.timeline_posts.is_empty() {
            ui.label(no_timeline_message_text);
        } else {
            let num_posts = app_data.timeline_posts.len();
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
                    .id_salt("timeline_scroll_area")
                    .max_height(ui.available_height() - 100.0)
                    .show_rows(ui, row_height, num_posts, |ui, row_range| {
                        for i in row_range {
                            let post_data = app_data.timeline_posts[i].clone();
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
