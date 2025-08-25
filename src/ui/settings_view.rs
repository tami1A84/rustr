use eframe::egui;
use std::sync::{Arc, Mutex};
use tokio::runtime::Handle;

use crate::{
    nostr_client::switch_relays,
    save_config,
    types::{AppTheme, NostrPostAppInternal},
};

pub fn draw_settings_view(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    app_data: &mut NostrPostAppInternal,
    app_data_arc: Arc<Mutex<NostrPostAppInternal>>,
    runtime_handle: Handle,
) {
    ui.heading("設定");
    ui.add_space(10.0);

    // --- テーマ設定 ---
    ui.label("テーマ");
    if ui
        .selectable_value(&mut app_data.current_theme, AppTheme::Light, "ライト")
        .clicked()
        || ui
            .selectable_value(&mut app_data.current_theme, AppTheme::Dark, "ダーク")
            .clicked()
    {
        update_theme(app_data.current_theme, ctx);
        save_config(app_data);
    }

    ui.add_space(20.0);
    ui.separator();
    ui.add_space(20.0);

    // --- リレー設定 ---
    ui.heading("リレー設定");
    ui.add_space(10.0);

    let mut changed = false;

    changed |= draw_relay_category(
        ui,
        "個人用リレー",
        "データをバックアップするためのリレーです。",
        &mut app_data.relays.self_hosted,
        &mut app_data.self_hosted_relay_input,
    );

    if changed {
        save_config(app_data);
        let app_data_clone = app_data_arc.clone();
        runtime_handle.spawn(async move {
            switch_relays(app_data_clone).await;
        });
    }
}

fn draw_relay_category(
    ui: &mut egui::Ui,
    title: &str,
    description: &str,
    relays: &mut Vec<String>,
    relay_input: &mut String,
) -> bool {
    let mut changed = false;

    ui.label(egui::RichText::new(title).strong());
    ui.label(egui::RichText::new(description).small().color(egui::Color32::GRAY));
    ui.add_space(5.0);

    // 新しいリレーの追加
    ui.horizontal(|ui| {
        let response = ui.text_edit_singleline(relay_input);
        if ui.button("追加").clicked()
            || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
        {
            if !relay_input.trim().is_empty() {
                let new_relay = relay_input.trim().to_string();
                if !relays.contains(&new_relay) {
                    relays.push(new_relay);
                    relay_input.clear();
                    changed = true;
                }
            }
        }
    });

    // 現在のリレーリスト
    let mut relay_to_remove = None;
    for (i, relay) in relays.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(relay);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("削除").clicked() {
                    relay_to_remove = Some(i);
                }
            });
        });
    }

    if let Some(i) = relay_to_remove {
        relays.remove(i);
        changed = true;
    }

    changed
}


fn update_theme(theme: AppTheme, ctx: &egui::Context) {
    let visuals = match theme {
        AppTheme::Light => crate::theme::light_visuals(),
        AppTheme::Dark => crate::theme::dark_visuals(),
    };
    ctx.set_visuals(visuals);
}
