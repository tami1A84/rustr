use eframe::egui;
use nostr::nips::nip19::ToBech32;
use regex::Regex;
use std::collections::HashMap;

use crate::types::{ImageKind, ImageState, NostrPostAppInternal, TimelinePost};

fn render_post_content(
    ui: &mut egui::Ui,
    app_data: &NostrPostAppInternal,
    post: &TimelinePost,
    urls_to_load: &mut Vec<(String, ImageKind)>,
    my_emojis: &HashMap<String, String>,
) {
    let text_color = app_data.current_theme.text_color();

    // Check for music/podcast status
    let d_tag = post
        .tags
        .iter()
        .find(|t| (*t).clone().to_vec().get(0).map(|s| s.as_str()) == Some("d"));

    if let Some(tag) = d_tag {
        let tag_vec = tag.clone().to_vec();
        if tag_vec.get(1).map(|s| s.as_str()) == Some("music") {
            // Music or Podcast status
            ui.horizontal(|ui| {
                ui.label("🎵"); // Use a general music icon for now
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(&post.content).color(text_color));
                    let r_tag = post
                        .tags
                        .iter()
                        .find(|t| (*t).clone().to_vec().get(0).map(|s| s.as_str()) == Some("r"));
                    if let Some(r_tag_value) = r_tag.and_then(|t| t.clone().to_vec().get(1).cloned())
                    {
                        ui.hyperlink_to(
                            egui::RichText::new(&r_tag_value)
                                .small()
                                .color(egui::Color32::GRAY),
                            r_tag_value,
                        );
                    }
                });
            });
            return; // Don't render general content
        }
    }

    // General status (with emojis)
    let re = Regex::new(r":(\w+):").unwrap();
    let mut last_end = 0;

    ui.horizontal_wrapped(|ui| {
        for cap in re.captures_iter(&post.content) {
            let full_match = cap.get(0).unwrap();
            let shortcode = cap.get(1).unwrap().as_str();

            let pre_text = &post.content[last_end..full_match.start()];
            if !pre_text.is_empty() {
                ui.label(egui::RichText::new(pre_text).color(text_color));
            }

            let url = post
                .emojis
                .get(shortcode)
                .or_else(|| my_emojis.get(shortcode));
            if let Some(url) = url {
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

pub fn render_post(
    ui: &mut egui::Ui,
    app_data: &mut NostrPostAppInternal,
    post: &TimelinePost,
    urls_to_load: &mut Vec<(String, ImageKind)>,
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

            if let Some(my_keys) = &app_data.my_keys {
                if post.author_pubkey != my_keys.public_key() {
                    // ZAP button
                    if !post.author_metadata.lud16.is_empty() {
                        if ui.button("⚡").clicked() {
                            app_data.zap_target_post = Some(post.clone());
                            app_data.show_zap_dialog = true;
                            app_data.zap_amount_input = "21".to_string(); // Default amount
                        }
                    }
                }
            }
        });
        ui.add_space(5.0);
        render_post_content(
            ui,
            app_data,
            post,
            urls_to_load,
            &app_data.my_emojis.clone(),
        );
    });
}
