use eframe::egui;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use nostr::{Filter, Keys, Kind, PublicKey};
use nostr_sdk::{Client, SubscribeAutoCloseOptions};

// NIP-49 (ChaCha20Poly1305) のための暗号クレート
use base64::{Engine as _, engine::general_purpose};
use chacha20poly1305::{
    ChaCha20Poly1305, Key, Nonce,
    aead::{Aead, KeyInit},
};
use rand::Rng;
use rand::rngs::OsRng;
// PBKDF2のためのクレート
use pbkdf2::pbkdf2_hmac;
use sha2::Sha256;

use crate::{
    types::{NostrStatusAppInternal, Config, EditableRelay, AppTab, ProfileMetadata, TimelinePost},
    cache_db::{LmdbCache, DB_FOLLOWED, DB_RELAYS, DB_PROFILES, DB_TIMELINE},
    CONFIG_FILE,
    nostr_client::{connect_to_relays_with_nip65, fetch_nip01_profile, fetch_relays_for_followed_users}
};

// --- Step 1: キャッシュからデータを読み込む ---
struct CachedData {
    followed_pubkeys: HashSet<PublicKey>,
    nip65_relays: Vec<(String, Option<String>)>,
    profile_metadata: ProfileMetadata,
    timeline_posts: Vec<TimelinePost>,
}

fn load_data_from_cache(
    cache_db: &LmdbCache,
    pubkey_hex: &str,
) -> Result<CachedData, Box<dyn std::error::Error + Send + Sync>> {
    println!("Loading data from cache for pubkey: {pubkey_hex}");

    let followed_cache = cache_db.read_cache::<HashSet<PublicKey>>(DB_FOLLOWED, pubkey_hex)?;
    let nip65_cache =
        cache_db.read_cache::<Vec<(String, Option<String>)>>(DB_RELAYS, pubkey_hex)?;
    let profile_cache = cache_db.read_cache::<ProfileMetadata>(DB_PROFILES, pubkey_hex)?;
    let timeline_cache = cache_db
        .read_cache::<Vec<TimelinePost>>(DB_TIMELINE, pubkey_hex)
        .ok();

    if followed_cache.is_expired() || nip65_cache.is_expired() || profile_cache.is_expired() {
        return Err("Cache expired".into());
    }

    println!("Successfully loaded data from cache.");
    Ok(CachedData {
        followed_pubkeys: followed_cache.data,
        nip65_relays: nip65_cache.data,
        profile_metadata: profile_cache.data,
        timeline_posts: timeline_cache.map_or(Vec::new(), |c| c.data),
    })
}

// --- Step 2: ネットワークから新しいデータを取得 ---
struct FreshData {
    followed_pubkeys: HashSet<PublicKey>,
    timeline_posts: Vec<TimelinePost>,
    log_message: String,
    fetched_nip65_relays: Vec<(String, Option<String>)>,
    profile_metadata: ProfileMetadata,
    profile_json_string: String,
}

async fn fetch_fresh_data_from_network(
    client: &Client,
    keys: &Keys,
    discover_relays: &str,
    default_relays: &str,
    cache_db: &LmdbCache,
) -> Result<FreshData, Box<dyn std::error::Error + Send + Sync>> {
    let pubkey_hex = keys.public_key().to_string();

    println!("Fetching fresh data from network...");

    let (log_message, fetched_nip65_relays) =
        connect_to_relays_with_nip65(client, keys, discover_relays, default_relays).await?;
    cache_db.write_cache(DB_RELAYS, &pubkey_hex, &fetched_nip65_relays)?;

    println!("Fetching NIP-02 contact list...");
    let nip02_filter = Filter::new()
        .authors(vec![keys.public_key()])
        .kind(Kind::ContactList)
        .limit(1);
    let nip02_filter_id = client
        .subscribe(nip02_filter, Some(SubscribeAutoCloseOptions::default()))
        .await?;

    let mut followed_pubkeys = HashSet::new();
    let mut received_nip02 = false;

    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(10)) => {}
        _ = async {
            let mut notifications = client.notifications();
            while let Ok(notification) = notifications.recv().await {
                if let nostr_sdk::RelayPoolNotification::Event { event, .. } = notification {
                    if event.kind == Kind::ContactList && event.pubkey == keys.public_key() {
                        for tag in event.tags.iter() { if let Some(nostr::TagStandard::PublicKey { public_key, .. }) = tag.as_standardized() { followed_pubkeys.insert(*public_key); } }
                        received_nip02 = true;
                        break;
                    }
                }
            }
        } => {},
    }
    client.unsubscribe(&nip02_filter_id).await;

    if received_nip02 {
        cache_db.write_cache(DB_FOLLOWED, &pubkey_hex, &followed_pubkeys)?;
    }

    let timeline_posts = fetch_timeline_posts(keys, discover_relays, &followed_pubkeys).await?;
    cache_db.write_cache(DB_TIMELINE, &pubkey_hex, &timeline_posts)?;

    let (profile_metadata, profile_json_string) =
        fetch_nip01_profile(client, keys.public_key()).await?;
    cache_db.write_cache(DB_PROFILES, &pubkey_hex, &profile_metadata)?;

    Ok(FreshData {
        followed_pubkeys,
        timeline_posts,
        log_message,
        fetched_nip65_relays,
        profile_metadata,
        profile_json_string,
    })
}

async fn fetch_timeline_posts(
    keys: &Keys,
    discover_relays: &str,
    followed_pubkeys: &HashSet<PublicKey>,
) -> Result<Vec<TimelinePost>, Box<dyn std::error::Error + Send + Sync>> {
    let mut timeline_posts = Vec::new();
    if followed_pubkeys.is_empty() {
        return Ok(timeline_posts);
    }

    let temp_discover_client = Client::new(keys.clone());
    for relay_url in discover_relays.lines().filter(|url| !url.trim().is_empty()) {
        temp_discover_client.add_relay(relay_url.trim()).await?;
    }
    temp_discover_client.connect().await;
    let followed_pubkeys_vec: Vec<PublicKey> = followed_pubkeys.iter().cloned().collect();
    let write_relay_urls =
        fetch_relays_for_followed_users(&temp_discover_client, followed_pubkeys_vec).await?;
    temp_discover_client.shutdown().await;

    if !write_relay_urls.is_empty() {
        let temp_fetch_client = Client::new(keys.clone());
        for url in &write_relay_urls {
            temp_fetch_client.add_relay(url.clone()).await?;
        }
        temp_fetch_client.connect().await;

        let timeline_filter = Filter::new()
            .authors(followed_pubkeys.clone())
            .kind(Kind::from(30315))
            .limit(20);
        let status_events = temp_fetch_client
            .fetch_events(timeline_filter, Duration::from_secs(10))
            .await?;

        if !status_events.is_empty() {
            let author_pubkeys: HashSet<PublicKey> =
                status_events.iter().map(|e| e.pubkey).collect();
            let metadata_filter = Filter::new()
                .authors(author_pubkeys.into_iter())
                .kind(Kind::Metadata);
            let metadata_events = temp_fetch_client
                .fetch_events(metadata_filter, Duration::from_secs(5))
                .await?;
            let mut profiles: HashMap<PublicKey, ProfileMetadata> = HashMap::new();
            for event in metadata_events {
                if let Ok(metadata) = serde_json::from_str::<ProfileMetadata>(&event.content) {
                    profiles.insert(event.pubkey, metadata);
                }
            }

            for event in status_events {
                let emojis = event
                    .tags
                    .iter()
                    .filter_map(|tag| {
                        if let Some(nostr::TagStandard::Emoji { shortcode, url }) =
                            tag.as_standardized()
                        {
                            Some((shortcode.to_string(), url.to_string()))
                        } else {
                            None
                        }
                    })
                    .collect();

                timeline_posts.push(TimelinePost {
                    author_pubkey: event.pubkey,
                    author_metadata: profiles.get(&event.pubkey).cloned().unwrap_or_default(),
                    content: event.content.clone(),
                    created_at: event.created_at,
                    emojis,
                });
            }
            timeline_posts.sort_by_key(|p| std::cmp::Reverse(p.created_at));
        }
        temp_fetch_client.shutdown().await;
    }
    Ok(timeline_posts)
}


pub fn draw_login_view(
    ui: &mut egui::Ui,
    app_data: &mut NostrStatusAppInternal,
    app_data_arc: Arc<Mutex<NostrStatusAppInternal>>,
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
                runtime_handle.spawn(async move {
                    let login_result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
                        let keys = (|| -> Result<Keys, Box<dyn std::error::Error + Send + Sync>> {
                            let config_str = fs::read_to_string(CONFIG_FILE)?;
                            let config: Config = serde_json::from_str(&config_str)?;
                            let retrieved_salt_bytes = general_purpose::STANDARD.decode(&config.salt)?;
                            let mut derived_key_bytes = [0u8; 32];
                            pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), &retrieved_salt_bytes, 100_000, &mut derived_key_bytes);
                            let cipher_key = Key::from_slice(&derived_key_bytes);
                            let cipher = ChaCha20Poly1305::new(cipher_key);
                            let nip49_encoded = config.encrypted_secret_key;
                            if !nip49_encoded.starts_with("#nip49:") { return Err("Invalid NIP-49 format".into()); }
                            let decoded_bytes = general_purpose::STANDARD.decode(&nip49_encoded[7..])?;
                            if decoded_bytes.len() < 12 { return Err("Invalid NIP-49 payload".into()); }
                            let (ciphertext_and_tag, retrieved_nonce_bytes) = decoded_bytes.split_at(decoded_bytes.len() - 12);
                            let retrieved_nonce = Nonce::from_slice(retrieved_nonce_bytes);
                            let decrypted_bytes = cipher.decrypt(retrieved_nonce, ciphertext_and_tag).map_err(|_| "Incorrect passphrase")?;
                            let decrypted_secret_key_hex = hex::encode(&decrypted_bytes);
                            Ok(Keys::parse(&decrypted_secret_key_hex)?)
                        })()?;
                        let client = Client::new(keys.clone());
                        let pubkey_hex = keys.public_key().to_string();
                        let (discover_relays, default_relays) = {
                            let app_data = cloned_app_data_arc.lock().unwrap();
                            (app_data.discover_relays_editor.clone(), app_data.default_relays_editor.clone())
                        };
                        if let Ok(cached_data) = load_data_from_cache(&cache_db_clone, &pubkey_hex) {
                            let mut app_data = cloned_app_data_arc.lock().unwrap();
                            app_data.my_keys = Some(keys.clone());
                            app_data.nostr_client = Some(client.clone());
                            app_data.followed_pubkeys = cached_data.followed_pubkeys;
                            app_data.timeline_posts = cached_data.timeline_posts;
                            app_data.editable_profile = cached_data.profile_metadata;
                            app_data.nip65_relays = cached_data.nip65_relays.into_iter().map(|(url, policy)| {
                                let (read, write) = match policy.as_deref() {
                                    Some("read") => (true, false),
                                    Some("write") => (false, true),
                                    _ => (true, true),
                                };
                                EditableRelay { url, read, write }
                            }).collect();
                            app_data.is_logged_in = true;
                            app_data.is_loading = true;
                        } else {
                            let mut app_data = cloned_app_data_arc.lock().unwrap();
                            app_data.my_keys = Some(keys.clone());
                            app_data.nostr_client = Some(client.clone());
                            app_data.is_logged_in = true;
                            app_data.is_loading = true;
                        }
                        let fresh_data_result = fetch_fresh_data_from_network(&client, &keys, &discover_relays, &default_relays, &cache_db_clone).await;
                        if let Ok(fresh_data) = fresh_data_result {
                            let mut app_data = cloned_app_data_arc.lock().unwrap();
                            app_data.followed_pubkeys = fresh_data.followed_pubkeys;
                            app_data.timeline_posts = fresh_data.timeline_posts;
                            if let Some(pos) = fresh_data.log_message.find("--- 現在接続中のリレー ---") {
                                app_data.connected_relays_display = fresh_data.log_message[pos..].to_string();
                            }
                            app_data.nip65_relays = fresh_data.fetched_nip65_relays.into_iter().map(|(url, policy)| {
                                let (read, write) = match policy.as_deref() {
                                    Some("read") => (true, false),
                                    Some("write") => (false, true),
                                    _ => (true, true),
                                };
                                EditableRelay { url, read, write }
                            }).collect();
                            app_data.editable_profile = fresh_data.profile_metadata;
                            app_data.nip01_profile_display = fresh_data.profile_json_string;
                            app_data.profile_fetch_status = "Profile loaded.".to_string();
                        } else if let Err(e) = fresh_data_result {
                            let mut app_data = cloned_app_data_arc.lock().unwrap();
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
                runtime_handle.spawn(async move {
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
                            let mut salt_bytes = [0u8; 16];
                            OsRng.fill(&mut salt_bytes);
                            let salt_base64 = general_purpose::STANDARD.encode(salt_bytes);
                            let mut derived_key_bytes = [0u8; 32];
                            pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), &salt_bytes, 100_000, &mut derived_key_bytes);
                            let cipher_key = Key::from_slice(&derived_key_bytes);
                            let cipher = ChaCha20Poly1305::new(cipher_key);
                            let plaintext_bytes = user_provided_keys.secret_key().to_secret_bytes();
                            let mut nonce_bytes: [u8; 12] = [0u8; 12];
                            OsRng.fill(&mut nonce_bytes);
                            let nonce = Nonce::from_slice(&nonce_bytes);
                            let ciphertext_with_tag = cipher.encrypt(nonce, plaintext_bytes.as_slice()).map_err(|e| format!("NIP-49 encryption error: {e:?}"))?;
                            let mut encoded_data = ciphertext_with_tag.clone();
                            encoded_data.extend_from_slice(nonce_bytes.as_ref());
                            let nip49_encoded = format!("#nip49:{}", general_purpose::STANDARD.encode(&encoded_data));
                            let config = Config { encrypted_secret_key: nip49_encoded, salt: salt_base64 };
                            let config_json = serde_json::to_string_pretty(&config)?;
                            fs::write(CONFIG_FILE, config_json)?;
                            Ok(user_provided_keys)
                        })()?;
                        let client = Client::new(keys.clone());
                        let (discover_relays, default_relays) = {
                            let app_data = cloned_app_data_arc.lock().unwrap();
                            (app_data.discover_relays_editor.clone(), app_data.default_relays_editor.clone())
                        };
                        let fresh_data_result = fetch_fresh_data_from_network(&client, &keys, &discover_relays, &default_relays, &cache_db_clone).await;
                        if let Ok(fresh_data) = fresh_data_result {
                            let mut app_data = cloned_app_data_arc.lock().unwrap();
                            app_data.my_keys = Some(keys);
                            app_data.nostr_client = Some(client);
                            app_data.is_logged_in = true;
                            app_data.current_tab = AppTab::Home;
                            app_data.followed_pubkeys = fresh_data.followed_pubkeys;
                            app_data.timeline_posts = fresh_data.timeline_posts;
                            if let Some(pos) = fresh_data.log_message.find("--- 現在接続中のリレー ---") {
                                app_data.connected_relays_display = fresh_data.log_message[pos..].to_string();
                            }
                            app_data.nip65_relays = fresh_data.fetched_nip65_relays.into_iter().map(|(url, policy)| {
                                let (read, write) = match policy.as_deref() {
                                    Some("read") => (true, false),
                                    Some("write") => (false, true),
                                    _ => (true, true),
                                };
                                EditableRelay { url, read, write }
                            }).collect();
                            app_data.editable_profile = fresh_data.profile_metadata;
                            app_data.nip01_profile_display = fresh_data.profile_json_string;
                            app_data.profile_fetch_status = "Profile loaded.".to_string();
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
