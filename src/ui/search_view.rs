use std::sync::{Arc, Mutex};
use eframe::egui;
use tokio::runtime::Handle;
use crate::{
    NostrPostAppInternal,
    nostr_client::search_events,
};

pub fn draw_search_view(
    ui: &mut egui::Ui,
    _ctx: &egui::Context,
    app_data: &mut NostrPostAppInternal,
    app_data_arc: Arc<Mutex<NostrPostAppInternal>>,
    runtime_handle: Handle,
) {
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
    egui::ScrollArea::vertical().show(ui, |ui| {
        if app_data.is_loading {
            ui.spinner();
        } else if app_data.search_results.is_empty() {
            ui.label("検索結果はありません。");
        } else {
            for post in &app_data.search_results {
                // Here you would draw each post.
                // For now, we'll just show the content as a label.
                ui.label(&post.content);
                ui.separator();
            }
        }
    });
}
