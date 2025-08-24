use crate::{
    nostr_client::search_events,
    types::{ImageKind, ImageState, NostrPostAppInternal},
    ui::{image_cache, post},
};
use eframe::egui;
use std::sync::{Arc, Mutex};
use tokio::runtime::Handle;

pub fn draw_search_view(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    app_data: &mut NostrPostAppInternal,
    app_data_arc: Arc<Mutex<NostrPostAppInternal>>,
    runtime_handle: Handle,
) {
    let mut urls_to_load: Vec<(String, ImageKind)> = Vec::new();
    // --- Search bar and button ---
    ui.horizontal(|ui| {
        ui.label("検索:");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("検索").clicked() {
                let query = app_data.search_input.clone();
                if !query.is_empty() {
                    app_data.is_loading = true;
                    app_data.search_results.clear();
                    let search_relays = app_data.relays.search.clone();
                    let app_data_clone = app_data_arc.clone();
                    runtime_handle.spawn(async move {
                        let results = match search_events(search_relays, query).await {
                            Ok(posts) => posts,
                            Err(e) => {
                                eprintln!("Search failed: {}", e);
                                // Optionally, set an error message in app_data to show in the UI
                                Vec::new()
                            }
                        };
                        let mut data = app_data_clone.lock().unwrap();
                        data.search_results = results;
                        data.is_loading = false;
                        data.should_repaint = true;
                    });
                }
            }
            ui.add(
                egui::TextEdit::singleline(&mut app_data.search_input)
                    .hint_text("キーワードを入力...")
                    .desired_width(ui.available_width()),
            );
        });
    });

    ui.add_space(10.0);
    if app_data.is_loading {
        ui.spinner();
    } else if app_data.search_results.is_empty() {
        ui.label("検索結果はありません。");
    } else {
        let num_posts = app_data.search_results.len();
        let row_height = 90.0; // Adjust as needed
        let card_frame = egui::Frame {
            inner_margin: egui::Margin::same(0),
            corner_radius: 8.0.into(),
            shadow: eframe::epaint::Shadow::NONE,
            fill: app_data.current_theme.card_background_color(),
            ..Default::default()
        };

        card_frame.show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("search_scroll_area")
                .max_height(ui.available_height() - 50.0)
                .show_rows(ui, row_height, num_posts, |ui, row_range| {
                    for i in row_range {
                        if let Some(post_data) = app_data.search_results.get(i).cloned() {
                            post::render_post(ui, app_data, &post_data, &mut urls_to_load);
                            ui.add_space(5.0);
                        }
                    }
                });
        });
    }

    // --- Image Loading Logic ---
    let cache_db = app_data.cache_db.clone();
    let mut still_to_load = Vec::new();
    for (url_key, kind) in urls_to_load {
        if let Some(image_bytes) = image_cache::load_from_lmdb(&cache_db, &url_key) {
            if let Ok(mut dynamic_image) = image::load_from_memory(&image_bytes) {
                let (width, height) = match kind {
                    ImageKind::Avatar => (32, 32),
                    ImageKind::Emoji => (20, 20),
                    _ => (32, 32),
                };
                dynamic_image = dynamic_image.thumbnail(width, height);
                let color_image = egui::ColorImage::from_rgba_unmultiplied(
                    [
                        dynamic_image.width() as usize,
                        dynamic_image.height() as usize,
                    ],
                    dynamic_image.to_rgba8().as_flat_samples().as_slice(),
                );
                let texture_handle =
                    ctx.load_texture(&url_key, color_image, Default::default());
                app_data
                    .image_cache
                    .insert(url_key, ImageState::Loaded(texture_handle));
            } else {
                app_data.image_cache.insert(url_key, ImageState::Failed);
            }
        } else {
            still_to_load.push((url_key, kind));
        }
    }

    let data_clone = app_data_arc.clone();
    for (url_key, kind) in still_to_load {
        app_data
            .image_cache
            .insert(url_key.clone(), ImageState::Loading);
        app_data.should_repaint = true;

        let app_data_clone = data_clone.clone();
        let ctx_clone = ctx.clone();
        let cache_db_for_fetch = app_data.cache_db.clone();
        let request = ehttp::Request::get(&url_key);

        ehttp::fetch(request, move |result| {
            let new_state = match result {
                Ok(response) => {
                    if response.ok {
                        image_cache::save_to_lmdb(
                            &cache_db_for_fetch,
                            &response.url,
                            &response.bytes,
                        );

                        match image::load_from_memory(&response.bytes) {
                            Ok(mut dynamic_image) => {
                                let (width, height) = match kind {
                                    ImageKind::Avatar => (32, 32),
                                    ImageKind::Emoji => (20, 20),
                                    _ => (32, 32),
                                };
                                dynamic_image = dynamic_image.thumbnail(width, height);

                                let color_image = egui::ColorImage::from_rgba_unmultiplied(
                                    [
                                        dynamic_image.width() as usize,
                                        dynamic_image.height() as usize,
                                    ],
                                    dynamic_image.to_rgba8().as_flat_samples().as_slice(),
                                );
                                let texture_handle = ctx_clone.load_texture(
                                    &response.url,
                                    color_image,
                                    Default::default(),
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
