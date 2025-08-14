use eframe::egui;
use std::sync::{Arc, Mutex};
use tokio::runtime::Handle;

use crate::types::NostrStatusAppInternal;

pub fn draw_wallet_view(
    ui: &mut egui::Ui,
    app_data: &mut NostrStatusAppInternal,
    _app_data_arc: Arc<Mutex<NostrStatusAppInternal>>,
    _runtime_handle: Handle,
) {
    ui.heading("Wallet");
    ui.add_space(10.0);

    ui.label("Nostr Wallet Connect (NIP-47)");
    ui.add_space(5.0);

    ui.horizontal(|ui| {
        ui.label("NWC URI:");
        ui.text_edit_singleline(&mut app_data.nwc_uri_input);
    });

    if ui.button("Save").clicked() {
        // Here you would handle saving the NWC URI
        println!("Save button clicked. NWC URI: {}", app_data.nwc_uri_input);
    }
}
