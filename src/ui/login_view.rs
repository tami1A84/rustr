use eframe::egui;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::collections::HashSet;
use nostr::{nips::nip47::NostrWalletConnectURI, Keys, PublicKey};
use nostr_sdk::{Client};
use std::str::FromStr;

use crate::{
    types::{Config, NostrPostAppInternal, ProfileMetadata, RelayConfig, TimelinePost, AppTab},
    cache_db::{LmdbCache, DB_FOLLOWED, DB_PROFILES, DB_TIMELINE, DB_NOTIFICATIONS},
    CONFIG_FILE,
    ui::events::{refresh_all_data}
};

// --- Step 1: キャッシュからデータを読み込む ---
struct CachedData {
    followed_pubkeys: HashSet<PublicKey>,
    profile_metadata: ProfileMetadata,
    timeline_posts: Vec<TimelinePost>,
    notification_posts: Vec<TimelinePost>,
}

fn load_data_from_cache(
    cache_db: &LmdbCache,
    pubkey_hex: &str,
) -> Result<CachedData, Box<dyn std::error::Error + Send + Sync>> {
    println!("Loading data from cache for pubkey: {pubkey_hex}");

    let followed_cache = cache_db.read_cache::<HashSet<PublicKey>>(DB_FOLLOWED, pubkey_hex)?;
    let profile_cache = cache_db.read_cache::<ProfileMetadata>(DB_PROFILES, pubkey_hex)?;
    let timeline_cache = cache_db
        .read_cache::<Vec<TimelinePost>>(DB_TIMELINE, pubkey_hex)
        .ok();
    let notification_cache = cache_db
        .read_cache::<Vec<TimelinePost>>(DB_NOTIFICATIONS, pubkey_hex)
        .ok();

    if followed_cache.is_expired() || profile_cache.is_expired() {
        return Err("Cache expired".into());
    }

    println!("Successfully loaded data from cache.");
    Ok(CachedData {
        followed_pubkeys: followed_cache.data,
        profile_metadata: profile_cache.data,
        timeline_posts: timeline_cache.map_or(Vec::new(), |c| c.data),
        notification_posts: notification_cache.map_or(Vec::new(), |c| c.data),
    })
}

pub fn draw_login_view(
    ui: &mut egui::Ui,
    app_data: &mut NostrPostAppInternal,
    app_data_arc: Arc<Mutex<NostrPostAppInternal>>,
    runtime_handle: tokio::runtime::Handle,
) {
    let login_heading_text = "ログインまたは登録";
    let secret_key_label_text = "秘密鍵 (nsec):";
    let secret_key_hint_text = "nsec1...";
    let passphrase_label_text = "パスフレーズ:";
    let passphrase_hint_text = "パスワード";
    let confirm_passphrase_label_text = "パスフレーズの確認:";
    let confirm_passphrase_hint_text = "パスワードを再入力";
    let login_button_text = "ログイン";
    let register_button_text = "登録";

    ui.group(|ui| {
        ui.heading(login_heading_text);
        ui.add_space(10.0);
        if Path::new(CONFIG_FILE).exists() {
            // --- ログイン ---
            ui.horizontal(|ui| {
                ui.label(passphrase_label_text);
                ui.add(egui::TextEdit::singleline(&mut app_data.passphrase_input)
                    .password(true)
                    .hint_text(passphrase_hint_text));
            });

            if ui.button(egui::RichText::new(login_button_text).strong()).clicked() && !app_data.is_loading {
                let passphrase = app_data.passphrase_input.clone();
                let cache_db_clone = app_data.cache_db.clone();
                app_data.is_loading = true;
                app_data.should_repaint = true;
                let cloned_app_data_arc = app_data_arc.clone();
                runtime_handle.clone().spawn(async move {
                    let app_data_for_login_logic = cloned_app_data_arc.clone();
                    let login_result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async move {
                        let (keys, nwc_uri) = (|| -> Result<_, Box<dyn std::error::Error + Send + Sync>> {
                            let config_str = fs::read_to_string(CONFIG_FILE)?;
                            let config: Config = serde_json::from_str(&config_str)?;
                            let decrypted_bytes = crate::nip49::decrypt(
                                &config.encrypted_secret_key,
                                &passphrase,
                                &config.salt,
                            )?;
                            let keys = Keys::parse(&hex::encode(&decrypted_bytes))?;

                            let nwc_uri = if let Some(encrypted_nwc) = config.encrypted_nwc_uri {
                                let decrypted_nwc_bytes = crate::nip49::decrypt(
                                    &encrypted_nwc,
                                    &passphrase,
                                    &config.salt,
                                )?;
                                let nwc_uri_str = String::from_utf8(decrypted_nwc_bytes)?;
                                Some(NostrWalletConnectURI::from_str(&nwc_uri_str)?)
                            } else {
                                None
                            };
                            Ok((keys, nwc_uri))
                        })()?;

                        if let Some(uri) = nwc_uri {
                            let app_data_for_nwc_task = app_data_for_login_logic.clone();
                            runtime_handle.clone().spawn(async move {
                                if let Err(e) =
                                    super::wallet_view::connect_nwc(uri, app_data_for_nwc_task.clone())
                                        .await
                                {
                                    eprintln!("Failed to connect to NWC: {}", e);
                                    let mut app_data =
                                        app_data_for_nwc_task.lock().unwrap();
                                    app_data.nwc_error = Some(format!("NWC auto-connect failed: {}", e));
                                }
                            });
                        }

                        let relay_config = {
                            app_data_for_login_logic.lock().unwrap().relays.clone()
                        };
                        let client = Client::new(keys.clone());
                        // Connect to all relays
                        let all_relays: Vec<String> = relay_config
                            .aggregator
                            .iter()
                            .chain(relay_config.self_hosted.iter())
                            .chain(relay_config.search.iter())
                            .cloned()
                            .collect::<HashSet<_>>()
                            .into_iter()
                            .collect();
                        for relay_url in &all_relays {
                            client.add_relay(relay_url.clone()).await?;
                        }
                        client.connect().await;

                        let pubkey_hex = keys.public_key().to_string();
                        if let Ok(cached_data) = load_data_from_cache(&cache_db_clone, &pubkey_hex) {
                            let mut app_data = app_data_for_login_logic.lock().unwrap();
                            app_data.my_keys = Some(keys.clone());
                            app_data.nostr_client = Some(client.clone());
                            app_data.followed_pubkeys = cached_data.followed_pubkeys;
                            app_data.timeline_posts = cached_data.timeline_posts;
                            app_data.notification_posts = cached_data.notification_posts;
                            app_data.editable_profile = cached_data.profile_metadata;
                            app_data.is_logged_in = true;
                            app_data.is_loading = true;
                        } else {
                            let mut app_data = app_data_for_login_logic.lock().unwrap();
                            app_data.my_keys = Some(keys.clone());
                            app_data.nostr_client = Some(client.clone());
                            app_data.is_logged_in = true;
                            app_data.is_loading = true;
                        }
                        let fresh_data_result = refresh_all_data(&client, &keys, &cache_db_clone, &relay_config).await;
                        if let Ok(fresh_data) = fresh_data_result {
                            // Get relay status before updating app_data
                            let relays = client.relays().await;
                            let mut status_log =
                                format!("\n--- 現在接続中のリレー ({}件) ---\n", relays.len());
                            for (url, relay) in relays.iter() {
                                let status = relay.status();
                                status_log.push_str(&format!("  - {}: {:?}\n", url, status));
                            }
                            status_log.push_str("---------------------------------\n");

                            let mut app_data = app_data_for_login_logic.lock().unwrap();
                            app_data.followed_pubkeys = fresh_data.followed_pubkeys;
                            app_data.timeline_posts = fresh_data.timeline_posts;
                            app_data.notification_posts = fresh_data.notification_posts;
                            app_data.connected_relays_display = status_log;

                            let my_emojis: std::collections::HashMap<String, String> = fresh_data.profile_metadata.emojis
                                .iter()
                                .map(|emoji_pair| (emoji_pair[0].clone(), emoji_pair[1].clone()))
                                .collect();
                            app_data.my_emojis = my_emojis;
                            app_data.editable_profile = fresh_data.profile_metadata;
                            app_data.nip01_profile_display = fresh_data.profile_json_string;
                            app_data.profile_fetch_status = "Profile loaded.".to_string();

                            // --- Fetch NIP-30/51 Emojis with fallback ---
                            let pubkey = keys.public_key();
                            let app_data_clone_for_emojis = app_data_for_login_logic.clone();
                            runtime_handle.clone().spawn(async move {
                                println!("Spawning emoji fetch task for kind:30030...");

                                let custom_emojis = crate::emoji_loader::fetch_emoji_sets(&vec!["wss://yabu.me".to_string()], pubkey).await;

                                if !custom_emojis.is_empty() {
                                    println!("Fetched {} custom emojis from kind:30030.", custom_emojis.len());
                                    let mut app_data = app_data_clone_for_emojis.lock().unwrap();
                                    app_data.my_emojis.extend(custom_emojis);
                                    app_data.should_repaint = true;
                                } else {
                                    println!("No custom emojis found from the aggregator relay.");
                                }
                            });
                            // --- End Fetch Emojis ---
                        } else if let Err(e) = fresh_data_result {
                            let mut app_data = app_data_for_login_logic.lock().unwrap();
                            app_data.profile_fetch_status = format!("Failed to refresh data: {e}");
                        }
                        Ok(())
                    }.await;
                    if let Err(e) = login_result {
                        let client_to_shutdown = {
                            let mut app_data_in_task = cloned_app_data_arc.lock().unwrap();
                            app_data_in_task.nostr_client.take()
                        };
                        if let Some(client) = client_to_shutdown { client.shutdown().await; }
                        let mut app_data_in_task = cloned_app_data_arc.lock().unwrap();
                        app_data_in_task.profile_fetch_status = format!("Login failed: {e}");
                    }
                    let mut app_data_in_task = cloned_app_data_arc.lock().unwrap();
                    app_data_in_task.is_loading = false;
                    app_data_in_task.should_repaint = true;
                });
            }
        } else {
            // --- 新規登録 ---
            ui.horizontal(|ui| {
                ui.label(secret_key_label_text);
                ui.add(egui::TextEdit::singleline(&mut app_data.secret_key_input)
                    .password(true)
                    .hint_text(secret_key_hint_text));
            });

            ui.horizontal(|ui| {
                ui.label(passphrase_label_text);
                ui.add(egui::TextEdit::singleline(&mut app_data.passphrase_input)
                    .password(true)
                    .hint_text(passphrase_hint_text));
            });
            ui.horizontal(|ui| {
                ui.label(confirm_passphrase_label_text);
                ui.add(egui::TextEdit::singleline(&mut app_data.confirm_passphrase_input)
                    .password(true)
                    .hint_text(confirm_passphrase_hint_text));
            });

            if ui.button(egui::RichText::new(register_button_text).strong()).clicked() && !app_data.is_loading {
                let secret_key_input = app_data.secret_key_input.clone();
                let passphrase = app_data.passphrase_input.clone();
                let confirm_passphrase = app_data.confirm_passphrase_input.clone();
                let cache_db_clone = app_data.cache_db.clone();
                app_data.is_loading = true;
                app_data.should_repaint = true;
                let cloned_app_data_arc = app_data_arc.clone();
                runtime_handle.clone().spawn(async move {
                    if passphrase != confirm_passphrase {
                        let mut current_app_data = cloned_app_data_arc.lock().unwrap();
                        current_app_data.profile_fetch_status = "Passphrases do not match.".to_string();
                        current_app_data.is_loading = false;
                        current_app_data.should_repaint = true;
                        return;
                    }
                    let registration_result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
                        let keys = (|| -> Result<Keys, Box<dyn std::error::Error + Send + Sync>> {
                            let user_provided_keys = Keys::parse(&secret_key_input)?;
                            let plaintext_bytes = user_provided_keys.secret_key().to_secret_bytes();
                            let (nip49_encoded, salt_base64) =
                                crate::nip49::encrypt(&plaintext_bytes, &passphrase)?;
                            let config = Config {
                                encrypted_secret_key: nip49_encoded,
                                salt: salt_base64,
                                encrypted_nwc_uri: None,
                                relays: serde_json::to_value(RelayConfig {
                                    aggregator: vec!["wss://yabu.me".to_string()],
                                    ..Default::default()
                                })?,
                                theme: Some(crate::types::AppTheme::Light),
                            };
                            let config_json = serde_json::to_string_pretty(&config)?;
                            fs::write(CONFIG_FILE, config_json)?;
                            Ok(user_provided_keys)
                        })()?;
                        let relay_config = {
                            cloned_app_data_arc.lock().unwrap().relays.clone()
                        };
                        let client = Client::new(keys.clone());
                        // Connect to all relays
                        let all_relays: Vec<String> = relay_config
                            .aggregator
                            .iter()
                            .chain(relay_config.self_hosted.iter())
                            .chain(relay_config.search.iter())
                            .cloned()
                            .collect::<HashSet<_>>()
                            .into_iter()
                            .collect();
                        for relay_url in &all_relays {
                            client.add_relay(relay_url.clone()).await?;
                        }
                        client.connect().await;

                        let fresh_data_result = refresh_all_data(&client, &keys, &cache_db_clone, &relay_config).await;
                        if let Ok(fresh_data) = fresh_data_result {
                            let relays = client.relays().await;
                            let mut status_log =
                                format!("\n--- 現在接続中のリレー ({}件) ---\n", relays.len());
                            for (url, relay) in relays.iter() {
                                let status = relay.status();
                                status_log.push_str(&format!("  - {}: {:?}\n", url, status));
                            }
                            status_log.push_str("---------------------------------\n");

                            let mut app_data = cloned_app_data_arc.lock().unwrap();
                            app_data.my_keys = Some(keys.clone());
                            app_data.nostr_client = Some(client.clone());
                            app_data.is_logged_in = true;
                            app_data.current_tab = AppTab::Home;
                            app_data.followed_pubkeys = fresh_data.followed_pubkeys;
                            app_data.timeline_posts = fresh_data.timeline_posts;
                            app_data.connected_relays_display = status_log;

                            let my_emojis: std::collections::HashMap<String, String> = fresh_data.profile_metadata.emojis
                                .iter()
                                .map(|emoji_pair| (emoji_pair[0].clone(), emoji_pair[1].clone()))
                                .collect();
                            app_data.my_emojis = my_emojis;
                            app_data.editable_profile = fresh_data.profile_metadata;
                            app_data.nip01_profile_display = fresh_data.profile_json_string;
                            app_data.profile_fetch_status = "Profile loaded.".to_string();

                            // --- Fetch NIP-30/51 Emojis with fallback ---
                            let pubkey = keys.public_key();
                            let app_data_clone_for_emojis = cloned_app_data_arc.clone();
                            runtime_handle.clone().spawn(async move {
                                println!("Spawning emoji fetch task for kind:30030...");
                                let custom_emojis = crate::emoji_loader::fetch_emoji_sets(&vec!["wss://yabu.me".to_string()], pubkey).await;

                                if !custom_emojis.is_empty() {
                                    println!("Fetched {} custom emojis from kind:30030.", custom_emojis.len());
                                    let mut app_data = app_data_clone_for_emojis.lock().unwrap();
                                    app_data.my_emojis.extend(custom_emojis);
                                    app_data.should_repaint = true;
                                } else {
                                    println!("No custom emojis found from the aggregator relay.");
                                }
                            });
                            // --- End Fetch Emojis ---
                        } else if let Err(e) = fresh_data_result {
                            eprintln!("Failed to fetch initial data for registration: {e}");
                        }
                        Ok(())
                    }.await;
                    if let Err(e) = registration_result {
                        eprintln!("Failed to register new key: {e}");
                        let client_to_shutdown = {
                            let mut app_data_in_task = cloned_app_data_arc.lock().unwrap();
                            app_data_in_task.nostr_client.take()
                        };
                        if let Some(client) = client_to_shutdown { client.shutdown().await; }
                    }
                    let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                    app_data_async.is_loading = false;
                    app_data_async.should_repaint = true;
                });
            }
        }
    });
}
