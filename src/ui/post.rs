use eframe::egui;
use nostr::nips::nip19::{ToBech32, FromBech32, Nip19Event};
use nostr::{EventBuilder, Kind, Tag, EventId, RelayUrl};
use regex::Regex;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::runtime::Handle;
use crate::types::{ImageKind, ImageState, NostrPostAppInternal, TimelinePost, AppTheme};

fn find_post_by_id(app_data: &NostrPostAppInternal, event_id: EventId) -> Option<Arc<TimelinePost>> {
    if let Some(post) = app_data.quoted_posts_cache.get(&event_id) {
        return Some(post.clone());
    }
    if let Some(post) = app_data.timeline_posts.iter().find(|p| p.id == event_id) {
        return Some(Arc::new(post.clone()));
    }
    if let Some(post) = app_data.notification_posts.iter().find(|p| p.id == event_id) {
        return Some(Arc::new(post.clone()));
    }
    if let Some(post) = app_data.search_results.iter().find(|p| p.id == event_id) {
        return Some(Arc::new(post.clone()));
    }
    None
}

fn render_quoted_post(
    ui: &mut egui::Ui,
    app_data: &NostrPostAppInternal,
    post: &TimelinePost,
    urls_to_load: &mut Vec<(String, ImageKind)>,
) {
    let (fill_color, stroke_color) = match app_data.current_theme {
        AppTheme::Light => (egui::Color32::from_gray(240), egui::Color32::from_gray(220)),
        AppTheme::Dark => (egui::Color32::from_rgb(30, 30, 32), egui::Color32::from_rgb(60, 60, 62)),
    };

    let card_frame = egui::Frame {
        inner_margin: egui::Margin::same(8),
        corner_radius: 6.0.into(),
        shadow: eframe::epaint::Shadow::NONE,
        fill: fill_color,
        stroke: egui::Stroke::new(1.0, stroke_color),
        ..Default::default()
    };

    ui.group(|ui| {
        card_frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                let avatar_size = egui::vec2(24.0, 24.0);
                let corner_radius = 3.0;
                let url = &post.author_metadata.picture;

                if !url.is_empty() {
                    let url_key = url.to_string();
                     match app_data.image_cache.get(&url_key) {
                        Some(ImageState::Loaded(texture_handle)) => {
                            let image_widget = egui::Image::new(texture_handle)
                                .corner_radius(corner_radius)
                                .fit_to_exact_size(avatar_size);
                            ui.add(image_widget);
                        }
                        Some(ImageState::Loading) => {
                             let (rect, _) = ui.allocate_exact_size(avatar_size, egui::Sense::hover());
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
                            ui.put(rect, egui::Spinner::new());
                        }
                    }
                } else {
                    let (rect, _) = ui.allocate_exact_size(avatar_size, egui::Sense::hover());
                    ui.painter().rect_filled(rect, corner_radius, ui.style().visuals.widgets.inactive.bg_fill);
                }
                ui.add_space(4.0);

                let display_name = if !post.author_metadata.name.is_empty() {
                    post.author_metadata.name.clone()
                } else {
                    let pubkey = post.author_pubkey.to_bech32().unwrap_or_default();
                    format!("{}...{}", &pubkey[0..8], &pubkey[pubkey.len() - 4..])
                };
                ui.label(egui::RichText::new(display_name).strong().small().color(app_data.current_theme.text_color()));

                let created_at_datetime =
                    chrono::DateTime::from_timestamp(post.created_at.as_u64() as i64, 0).unwrap();
                let local_datetime = created_at_datetime.with_timezone(&chrono::Local);
                ui.label(
                    egui::RichText::new(local_datetime.format("%H:%M").to_string())
                        .color(egui::Color32::GRAY)
                        .small(),
                );
            });

            ui.add_space(4.0);

            let mut truncated_content = post.content.replace('\n', " ");
            let max_len = 120;
            if truncated_content.chars().count() > max_len {
                truncated_content = truncated_content.chars().take(max_len).collect::<String>() + "...";
            }
            ui.label(egui::RichText::new(truncated_content).small().color(app_data.current_theme.text_color()));
        });
    });
}


fn render_post_content(
    ui: &mut egui::Ui,
    app_data: &mut NostrPostAppInternal,
    post: &TimelinePost,
    urls_to_load: &mut Vec<(String, ImageKind)>,
    my_emojis: &HashMap<String, String>,
) {
    let text_color = app_data.current_theme.text_color();

    // Music/Podcast status check
    if let Some(d_tag) = post.tags.iter().find(|t| (*t).clone().to_vec().get(0).map(|s| s.as_str()) == Some("d")) {
        if d_tag.clone().to_vec().get(1).map(|s| s.as_str()) == Some("music") {
            ui.horizontal(|ui| {
                ui.label("🎵");
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(&post.content).color(text_color));
                    if let Some(r_tag_value) = post.tags.iter().find(|t| (*t).clone().to_vec().get(0).map(|s| s.as_str()) == Some("r")).and_then(|t| t.clone().to_vec().get(1).cloned()) {
                        ui.hyperlink_to(egui::RichText::new(&r_tag_value).small().color(egui::Color32::GRAY), r_tag_value);
                    }
                });
            });
            return;
        }
    }

    // Refactored logic to handle quotes and text separately
    let re_nostr = Regex::new(r"nostr:(?:note|nevent)1[a-z0-9]+").unwrap();
    let mut last_end = 0;

    for mat in re_nostr.find_iter(&post.content) {
        let pre_text = &post.content[last_end..mat.start()];
        render_text_with_emojis(ui, pre_text, text_color, app_data, post, urls_to_load, my_emojis);

        let bech32_uri = mat.as_str();
        let event_id = EventId::from_bech32(bech32_uri).ok()
            .or_else(|| nostr::nips::nip19::Nip19Event::from_bech32(bech32_uri).ok().map(|e| e.event_id));

        if let Some(id) = event_id {
            if let Some(quoted_post) = app_data.quoted_posts_cache.get(&id) {
                render_quoted_post(ui, app_data, quoted_post, urls_to_load);
            } else {
                if let Ok(mut posts_to_fetch) = app_data.posts_to_fetch.lock() {
                    if !posts_to_fetch.contains(&id) {
                        posts_to_fetch.insert(id);
                        app_data.should_repaint = true;
                    }
                }
                let (_rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), egui::Sense::hover());
                let spinner_frame = egui::Frame {
                    inner_margin: egui::Margin::same(8),
                    ..Default::default()
                };
                spinner_frame.show(ui, |ui| {
                    ui.add(egui::Spinner::new());
                    ui.label("Loading quote...");
                });
            }
        } else {
             ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(bech32_uri).color(text_color));
             });
        }

        last_end = mat.end();
    }

    let remaining_text = &post.content[last_end..];
    render_text_with_emojis(ui, remaining_text, text_color, app_data, post, urls_to_load, my_emojis);
}

fn render_text_with_emojis(
    ui: &mut egui::Ui,
    text: &str,
    text_color: egui::Color32,
    app_data: &NostrPostAppInternal,
    post: &TimelinePost,
    urls_to_load: &mut Vec<(String, ImageKind)>,
    my_emojis: &HashMap<String, String>,
) {
    if text.is_empty() {
        return;
    }

    ui.horizontal_wrapped(|ui| {
        let re_emoji = Regex::new(r":(\w+):").unwrap();
        let mut last_end = 0;

        for cap in re_emoji.captures_iter(text) {
            let full_match = cap.get(0).unwrap();
            let shortcode = cap.get(1).unwrap().as_str();

            let pre_text = &text[last_end..full_match.start()];
            if !pre_text.is_empty() {
                ui.label(egui::RichText::new(pre_text).color(text_color));
            }

            let url = post.emojis.get(shortcode).or_else(|| my_emojis.get(shortcode));
            if let Some(url) = url {
                let emoji_size = egui::vec2(20.0, 20.0);
                let url_key = url.to_string();

                match app_data.image_cache.get(&url_key) {
                    Some(ImageState::Loaded(texture_handle)) => {
                        let image_widget = egui::Image::new(texture_handle).fit_to_exact_size(emoji_size);
                        ui.add(image_widget);
                    }
                    Some(ImageState::Loading) => {
                        let (rect, _) = ui.allocate_exact_size(emoji_size, egui::Sense::hover());
                        ui.put(rect, egui::Spinner::new());
                    }
                    Some(ImageState::Failed) => {
                        let (rect, _) = ui.allocate_exact_size(emoji_size, egui::Sense::hover());
                        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "💔", egui::FontId::default(), ui.visuals().error_fg_color);
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

        let remaining_text = &text[last_end..];
        if !remaining_text.is_empty() {
            ui.label(egui::RichText::new(remaining_text).color(text_color));
        }
    });
}

pub fn render_post(
    ui: &mut egui::Ui,
    app_data: &mut NostrPostAppInternal,
    post: &TimelinePost,
    urls_to_load: &mut Vec<(String, ImageKind)>,
    app_data_arc: Arc<Mutex<NostrPostAppInternal>>,
    runtime_handle: Handle,
) {
    let card_frame = egui::Frame {
        inner_margin: egui::Margin::same(12),
        corner_radius: 8.0.into(),
        shadow: eframe::epaint::Shadow::NONE,
        fill: app_data.current_theme.card_background_color(),
        ..Default::default()
    };

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
                        ui.painter().rect_filled(
                            rect,
                            corner_radius,
                            ui.style().visuals.widgets.inactive.bg_fill,
                        );
                        ui.put(rect, egui::Spinner::new());
                    }
                    Some(ImageState::Failed) => {
                        let (rect, _) = ui.allocate_exact_size(avatar_size, egui::Sense::hover());
                        ui.painter().rect_filled(
                            rect,
                            corner_radius,
                            ui.style().visuals.error_fg_color.linear_multiply(0.2),
                        );
                    }
                    None => {
                        if !urls_to_load.iter().any(|(u, _)| u == &url_key) {
                            urls_to_load.push((url_key.clone(), ImageKind::Avatar));
                        }
                        let (rect, _) = ui.allocate_exact_size(avatar_size, egui::Sense::hover());
                        ui.painter().rect_filled(
                            rect,
                            corner_radius,
                            ui.style().visuals.widgets.inactive.bg_fill,
                        );
                        ui.put(rect, egui::Spinner::new());
                    }
                }
            } else {
                let (rect, _) = ui.allocate_exact_size(avatar_size, egui::Sense::hover());
                ui.painter().rect_filled(
                    rect,
                    corner_radius,
                    ui.style().visuals.widgets.inactive.bg_fill,
                );
            }

            ui.add_space(8.0);

            let display_name = if !post.author_metadata.name.is_empty() {
                post.author_metadata.name.clone()
            } else {
                let pubkey = post.author_pubkey.to_bech32().unwrap_or_default();
                format!("{}...{}", &pubkey[0..8], &pubkey[pubkey.len() - 4..])
            };
            ui.label(
                egui::RichText::new(display_name)
                    .strong()
                    .color(app_data.current_theme.text_color()),
            );

            let created_at_datetime =
                chrono::DateTime::from_timestamp(post.created_at.as_u64() as i64, 0).unwrap();
            let local_datetime = created_at_datetime.with_timezone(&chrono::Local);
            ui.label(
                egui::RichText::new(local_datetime.format("%Y-%m-%d %H:%M:%S").to_string())
                    .color(egui::Color32::GRAY)
                    .small(),
            );
        });
        ui.add_space(5.0);
        if post.kind == Kind::Reaction {
            let reacted_event_id = post.tags.iter().find_map(|tag| {
                if let Some(nostr::TagStandard::Event { event_id, .. }) = tag.as_standardized() {
                    Some(*event_id)
                } else {
                    None
                }
            });

            if let Some(event_id) = reacted_event_id {
                if let Some(reacted_post) = find_post_by_id(app_data, event_id) {
                    ui.vertical(|ui| {
                        let reaction_emoji = &post.content;
                        ui.label(format!("あなたの投稿に {} しました", reaction_emoji));
                        ui.add_space(4.0);
                        render_quoted_post(ui, app_data, &reacted_post, urls_to_load);
                    });
                } else {
                    if let Ok(mut posts_to_fetch) = app_data.posts_to_fetch.lock() {
                        if !posts_to_fetch.contains(&event_id) {
                            posts_to_fetch.insert(event_id);
                            app_data.should_repaint = true;
                        }
                    }
                    ui.horizontal(|ui|{
                        ui.spinner();
                        ui.label("Loading reaction...");
                    });
                }
            } else {
                // Reaction without 'e' tag, just show content
                render_post_content(
                    ui,
                    app_data,
                    post,
                    urls_to_load,
                    &app_data.my_emojis.clone(),
                );
            }
        } else if post.kind == Kind::TextNote {
            let event_tag_id = post.tags.iter().find_map(|tag| {
                if let Some(nostr::TagStandard::Event { event_id, .. }) = tag.as_standardized() {
                    Some(*event_id)
                } else {
                    None
                }
            });

            // Check if the content contains a nostr: link (quote)
            let re_nostr = Regex::new(r"nostr:(?:note|nevent)1[a-z0-9]+").unwrap();
            let is_quote_in_content = re_nostr.is_match(&post.content);

            if let Some(event_id) = event_tag_id {
                if is_quote_in_content {
                    // This is a quote post, render_post_content will handle the preview.
                    render_post_content(ui, app_data, post, urls_to_load, &app_data.my_emojis.clone());
                } else {
                    // This is a reply, so show the content and then the replied-to post.
                    ui.vertical(|ui| {
                        render_post_content(ui, app_data, post, urls_to_load, &app_data.my_emojis.clone());
                        ui.add_space(8.0);
                        let reply_label = egui::RichText::new("に返信しました:")
                            .color(egui::Color32::GRAY)
                            .small();
                        ui.label(reply_label);

                        if let Some(replied_post) = find_post_by_id(app_data, event_id) {
                            render_quoted_post(ui, app_data, &replied_post, urls_to_load);
                        } else {
                            if let Ok(mut posts_to_fetch) = app_data.posts_to_fetch.lock() {
                                if !posts_to_fetch.contains(&event_id) {
                                    posts_to_fetch.insert(event_id);
                                    app_data.should_repaint = true;
                                }
                            }
                            let spinner_frame = egui::Frame {
                                inner_margin: egui::Margin::same(8),
                                ..Default::default()
                            };
                            spinner_frame.show(ui, |ui| {
                                ui.horizontal(|ui|{
                                    ui.add(egui::Spinner::new());
                                    ui.label("返信を読み込み中...");
                                });
                            });
                        }
                    });
                }
            } else {
                // Not a reply or quote, just a regular text note.
                render_post_content(ui, app_data, post, urls_to_load, &app_data.my_emojis.clone());
            }
        }
        else {
            render_post_content(
                ui,
                app_data,
                post,
                urls_to_load,
                &app_data.my_emojis.clone(),
            );
        }

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(5.0);

        ui.horizontal(|ui| {
            if ui.button("💬").on_hover_text("Reply").clicked() {
                app_data.show_reply_dialog = true;
                app_data.reply_target_post = Some(post.clone());
                app_data.reply_input.clear();
            }

            ui.add_space(15.0);

            if ui.button("🔁").on_hover_text("Repost").clicked() {
                if let (Some(client), Some(keys)) =
                    (app_data.nostr_client.as_ref(), app_data.my_keys.as_ref())
                {
                    let client = client.clone();
                    let keys = keys.clone();
                    let reposted_event_id = post.id;
                    let reposted_author_pubkey = post.author_pubkey;
                    let cloned_app_data_arc = app_data_arc.clone();

                    runtime_handle.spawn(async move {
                        let tags = vec![
                            Tag::event(reposted_event_id),
                            Tag::public_key(reposted_author_pubkey),
                        ];
                        let event_result = EventBuilder::new(Kind::Repost, "").tags(tags).sign(&keys).await;

                        match event_result {
                            Ok(event) => match client.send_event(&event).await {
                                Ok(event_id) => {
                                    println!("Repost published with event id: {:?}", event_id);
                                }
                                Err(e) => eprintln!("Failed to publish repost: {}", e),
                            },
                            Err(e) => eprintln!("Failed to create repost event: {}", e),
                        }
                        cloned_app_data_arc.lock().unwrap().should_repaint = true;
                    });
                }
            }

            ui.add_space(15.0);

            if ui.button("✏️").on_hover_text("Quote").clicked() {
                app_data.show_post_dialog = true;
                app_data.post_input.clear();

                let mut nip19_event = Nip19Event::new(post.id);
                nip19_event.author = Some(post.author_pubkey);
                nip19_event.relays = app_data.relays.aggregator.iter().filter_map(|s| RelayUrl::parse(s).ok()).collect();

                if let Ok(nevent) = nip19_event.to_bech32() {
                    app_data.post_input = format!("nostr:{}\n\n", nevent);
                } else {
                    // Fallback to note ID if nevent fails for some reason
                    match post.id.to_bech32() {
                        Ok(note_id) => {
                            app_data.post_input = format!("nostr:{}\n\n", note_id);
                        }
                        Err(e) => {
                            eprintln!("Failed to create note_id for quote fallback: {}", e);
                        }
                    }
                }
            }

            ui.add_space(15.0);

            if ui.button("❤️").on_hover_text("React").clicked() {
                if let (Some(client), Some(keys)) =
                    (app_data.nostr_client.as_ref(), app_data.my_keys.as_ref())
                {
                    let client = client.clone();
                    let keys = keys.clone();
                    let reacted_event_id = post.id;
                    let reacted_author_pubkey = post.author_pubkey;
                    let cloned_app_data_arc = app_data_arc.clone();

                    runtime_handle.spawn(async move {
                        let tags = vec![
                            Tag::event(reacted_event_id),
                            Tag::public_key(reacted_author_pubkey),
                        ];
                        let event_result = EventBuilder::new(Kind::Reaction, "+")
                            .tags(tags)
                            .sign(&keys)
                            .await;

                        match event_result {
                            Ok(event) => match client.send_event(&event).await {
                                Ok(event_id) => {
                                    println!("Reaction published with event id: {:?}", event_id);
                                }
                                Err(e) => eprintln!("Failed to publish reaction: {}", e),
                            },
                            Err(e) => eprintln!("Failed to create reaction event: {}", e),
                        }
                        cloned_app_data_arc.lock().unwrap().should_repaint = true;
                    });
                }
            }

            ui.add_space(15.0);

            if let Some(my_keys) = &app_data.my_keys {
                if post.author_pubkey != my_keys.public_key() {
                    if !post.author_metadata.lud16.is_empty() {
                        if ui.button("⚡").on_hover_text("Zap").clicked() {
                            app_data.zap_target_post = Some(post.clone());
                            app_data.show_zap_dialog = true;
                            app_data.zap_amount_input = "21".to_string();
                        }
                    }
                }
            }
        });
    });
}
