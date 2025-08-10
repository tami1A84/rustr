use eframe::{egui::{self, Margin}};
use nostr::{EventBuilder, Filter, Kind, Keys, PublicKey, Tag};
use nostr_sdk::{Client, Options, SubscribeAutoCloseOptions};
use std::time::Duration;
use nostr::nips::nip19::ToBech32;
use std::fs;
use std::path::Path;
use std::collections::{HashSet, HashMap};
use ehttp;
use image;

use crate::{
    NostrStatusApp, AppTab, TimelinePost, ProfileMetadata, EditableRelay, AppTheme,
    CONFIG_FILE, MAX_STATUS_LENGTH, Cache, ImageState,
    connect_to_relays_with_nip65, fetch_nip01_profile, fetch_relays_for_followed_users, nostr_client::update_contact_list,
    light_visuals, dark_visuals,
    cache_db::{LmdbCache, DB_PROFILES, DB_FOLLOWED, DB_RELAYS, DB_TIMELINE},
};
use serde::de::DeserializeOwned;
use std::io::Read;

// --- Migration Helper ---
fn read_file_cache<T: DeserializeOwned>(path: &Path) -> Result<Cache<T>, Box<dyn std::error::Error + Send + Sync>> {
    let mut file = fs::File::open(path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    let cache: Cache<T> = serde_json::from_str(&contents)?;
    Ok(cache)
}

pub async fn migrate_data_from_files(
    cache_db: &LmdbCache,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cache_dir = Path::new(crate::CACHE_DIR);
    if !cache_dir.exists() {
        println!("Old cache directory not found, no migration needed.");
        return Ok(());
    }

    println!("Starting migration from file-based cache to LMDB...");

    for entry in fs::read_dir(cache_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(pubkey_hex) = path.file_name().and_then(|s| s.to_str()) {
                if pubkey_hex.ends_with("_migrated") {
                    continue;
                }
                println!("Migrating data for pubkey: {}", pubkey_hex);

                // Migrate followed_pubkeys.json
                let followed_path = path.join("followed_pubkeys.json");
                if followed_path.exists() {
                    if let Ok(cache) = read_file_cache::<HashSet<PublicKey>>(&followed_path) {
                        if let Err(e) = cache_db.write_cache(DB_FOLLOWED, pubkey_hex, &cache.data) {
                            eprintln!("Failed to migrate followed_pubkeys for {}: {}", pubkey_hex, e);
                        }
                    }
                }

                // Migrate nip65_relays.json
                let relays_path = path.join("nip65_relays.json");
                if relays_path.exists() {
                    if let Ok(cache) = read_file_cache::<Vec<(String, Option<String>)>>(&relays_path) {
                        if let Err(e) = cache_db.write_cache(DB_RELAYS, pubkey_hex, &cache.data) {
                            eprintln!("Failed to migrate nip65_relays for {}: {}", pubkey_hex, e);
                        }
                    }
                }

                // Migrate my_profile.json
                let profile_path = path.join("my_profile.json");
                if profile_path.exists() {
                    if let Ok(cache) = read_file_cache::<ProfileMetadata>(&profile_path) {
                         if let Err(e) = cache_db.write_cache(DB_PROFILES, pubkey_hex, &cache.data) {
                            eprintln!("Failed to migrate profile for {}: {}", pubkey_hex, e);
                        }
                    }
                }

                // Migrate timeline_posts.json
                let timeline_path = path.join("timeline_posts.json");
                if timeline_path.exists() {
                    if let Ok(cache) = read_file_cache::<Vec<TimelinePost>>(&timeline_path) {
                        if let Err(e) = cache_db.write_cache(DB_TIMELINE, pubkey_hex, &cache.data) {
                            eprintln!("Failed to migrate timeline for {}: {}", pubkey_hex, e);
                        }
                    }
                }

                // Rename the directory to mark it as migrated
                let migrated_path = path.with_file_name(format!("{}_migrated", pubkey_hex));
                if let Err(e) = fs::rename(&path, &migrated_path) {
                    eprintln!("Failed to rename migrated directory for {}: {}", pubkey_hex, e);
                } else {
                    println!("Finished migrating and renamed directory for pubkey: {}", pubkey_hex);
                }
            }
        }
    }
    println!("Migration complete.");
    Ok(())
}


// --- データ取得とUI更新のための構造体 ---
// `InitialData`は`FreshData`に置き換えられたため、削除

// --- データ取得ロジック ---

// --- Step 1: キャッシュからデータを読み込む ---
struct CachedData {
    followed_pubkeys: HashSet<PublicKey>,
    nip65_relays: Vec<(String, Option<String>)>,
    profile_metadata: ProfileMetadata,
    timeline_posts: Vec<TimelinePost>, // タイムラインもキャッシュから読む
}

fn load_data_from_cache(
    cache_db: &LmdbCache,
    pubkey_hex: &str,
) -> Result<CachedData, Box<dyn std::error::Error + Send + Sync>> {
    println!("Loading data from cache for pubkey: {}", pubkey_hex);

    let followed_cache = cache_db.read_cache::<HashSet<PublicKey>>(DB_FOLLOWED, pubkey_hex)?;
    let nip65_cache = cache_db.read_cache::<Vec<(String, Option<String>)>>(DB_RELAYS, pubkey_hex)?;
    let profile_cache = cache_db.read_cache::<ProfileMetadata>(DB_PROFILES, pubkey_hex)?;
    let timeline_cache = cache_db.read_cache::<Vec<TimelinePost>>(DB_TIMELINE, pubkey_hex).ok();

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

    // --- 1. リレー接続 (NIP-65) ---
    println!("Connecting to relays...");
    let (log_message, fetched_nip65_relays) = connect_to_relays_with_nip65(
        client,
        keys,
        discover_relays,
        default_relays
    ).await?;
    println!("Relay connection process finished.\n{}", log_message);
    cache_db.write_cache(DB_RELAYS, &pubkey_hex, &fetched_nip65_relays)?;


    // --- 2. フォローリスト取得 (NIP-02) ---
    println!("Fetching NIP-02 contact list...");
    let nip02_filter = Filter::new().authors(vec![keys.public_key()]).kind(Kind::ContactList).limit(1);
    let nip02_filter_id = client.subscribe(vec![nip02_filter], Some(SubscribeAutoCloseOptions::default())).await;

    let mut followed_pubkeys = HashSet::new();
    let mut received_nip02 = false;

    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(10)) => {}
        _ = async {
            let mut notifications = client.notifications();
            while let Ok(notification) = notifications.recv().await {
                if let nostr_sdk::RelayPoolNotification::Event { event, .. } = notification {
                    if event.kind == Kind::ContactList && event.pubkey == keys.public_key() {
                        println!("Contact list event received.");
                        for tag in &event.tags { if let Tag::PublicKey { public_key, .. } = tag { followed_pubkeys.insert(*public_key); } }
                        received_nip02 = true;
                        break;
                    }
                }
            }
        } => {},
    }
    client.unsubscribe(nip02_filter_id).await;

    if !received_nip02 {
        eprintln!("Failed to fetch contact list (timed out or not found).");
    } else {
        cache_db.write_cache(DB_FOLLOWED, &pubkey_hex, &followed_pubkeys)?;
    }
    println!("Fetched {} followed pubkeys.", followed_pubkeys.len());


    // --- 3. タイムライン取得 ---
    let timeline_posts = fetch_timeline_posts(keys, discover_relays, &followed_pubkeys).await?;
    cache_db.write_cache(DB_TIMELINE, &pubkey_hex, &timeline_posts)?;


    // --- 4. 自身のNIP-01 プロフィールメタデータ取得 ---
    println!("Fetching NIP-01 profile metadata for self...");
    let (profile_metadata, profile_json_string) = fetch_nip01_profile(client, keys.public_key()).await?;
    println!("NIP-01 profile fetch for self finished.");
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

    // 3a. フォローユーザーのNIP-65(kind:10002)を取得
    let temp_discover_client = Client::new(keys);
    for relay_url in discover_relays.lines().filter(|url| !url.trim().is_empty()) {
        temp_discover_client.add_relay(relay_url.trim()).await?;
    }
    temp_discover_client.connect().await;
    let followed_pubkeys_vec: Vec<PublicKey> = followed_pubkeys.iter().cloned().collect();
    let write_relay_urls = fetch_relays_for_followed_users(&temp_discover_client, followed_pubkeys_vec).await?;
    temp_discover_client.shutdown().await?;

    if !write_relay_urls.is_empty() {
        // 3b. 取得したwriteリレーで新しい一時クライアントを作成
        let temp_fetch_client = Client::new(keys);
        for url in &write_relay_urls {
            temp_fetch_client.add_relay(url.clone()).await?;
        }
        temp_fetch_client.connect().await;

        // 3c. フォローユーザーのステータス(kind:30315)を取得
        let timeline_filter = Filter::new().authors(followed_pubkeys.clone()).kind(Kind::ParameterizedReplaceable(30315)).limit(20);
        let status_events = temp_fetch_client.get_events_of(vec![timeline_filter], Some(Duration::from_secs(10))).await?;
        println!("Fetched {} statuses from followed users' write relays.", status_events.len());

        if !status_events.is_empty() {
            // 3d. ステータス投稿者のプロフィール(kind:0)を取得
            let author_pubkeys: HashSet<PublicKey> = status_events.iter().map(|e| e.pubkey).collect();
            let metadata_filter = Filter::new().authors(author_pubkeys.into_iter()).kind(Kind::Metadata);
            let metadata_events = temp_fetch_client.get_events_of(vec![metadata_filter], Some(Duration::from_secs(5))).await?;
            let mut profiles: HashMap<PublicKey, ProfileMetadata> = HashMap::new();
            for event in metadata_events {
                if let Ok(metadata) = serde_json::from_str::<ProfileMetadata>(&event.content) {
                    profiles.insert(event.pubkey, metadata);
                }
            }

            // 3e. ステータスとメタデータをマージ
            for event in status_events {
                timeline_posts.push(TimelinePost {
                    author_pubkey: event.pubkey,
                    author_metadata: profiles.get(&event.pubkey).cloned().unwrap_or_default(),
                    content: event.content.clone(),
                    created_at: event.created_at,
                });
            }
            timeline_posts.sort_by_key(|p| std::cmp::Reverse(p.created_at));
        }
        temp_fetch_client.shutdown().await?;
    }
    Ok(timeline_posts)
}


// NIP-49 (ChaCha20Poly1305) のための暗号クレート
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce, Key,
};
use rand::Rng;
use rand::rngs::OsRng;
use base64::{Engine as _, engine::general_purpose};
use hex;

// PBKDF2のためのクレート
use pbkdf2::pbkdf2_hmac;
use sha2::Sha256;

use crate::Config;

impl eframe::App for NostrStatusApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // MutexGuardをupdate関数全体のスコープで保持
        let mut app_data = self.data.lock().unwrap();

        // --- 日本語の文字列 ---
        let home_tab_text = "ホーム";
        let relays_tab_text = "リレー";
        let profile_tab_text = "プロフィール";
        let login_heading_text = "ログインまたは登録";
        let secret_key_label_text = "秘密鍵 (nsec):";
        let secret_key_hint_text = "nsec1...";
        let passphrase_label_text = "パスフレーズ:";
        let passphrase_hint_text = "パスワード";
        let confirm_passphrase_label_text = "パスフレーズの確認:";
        let confirm_passphrase_hint_text = "パスワードを再入力";
        let login_button_text = "ログイン";
        let register_button_text = "登録";
        let timeline_heading_text = "タイムライン";
        let fetch_latest_button_text = "最新の投稿を取得";
        let new_post_window_title_text = "新規投稿";
        let set_status_heading_text = "ステータスを設定";
        let status_input_hint_text = "いまどうしてる？";
        let publish_button_text = "公開";
        let cancel_button_text = "キャンセル";
        let status_too_long_text = "ステータスが長すぎます！";
        let no_timeline_message_text = "タイムラインに投稿はまだありません。";
        let current_connection_heading_text = "現在の接続";
        let reconnect_button_text = "再接続";
        let edit_relay_lists_heading_text = "リレーリストを編集";
        let nip65_relay_list_label_text = "あなたのリレーリスト (NIP-65)";
        let add_relay_button_text = "リレーを追加";
        let read_checkbox_text = "読み取り";
        let write_checkbox_text = "書き込み";
        let discover_relays_label_text = "発見リレー (他ユーザーを見つけるため)";
        let default_relays_label_text = "デフォルトリレー (フォールバック用)";
        let save_nip65_button_text = "保存して発見リレーに公開";
        let profile_heading_text = "あなたのプロフィール";
        let public_key_heading_text = "あなたの公開鍵 (npub)";
        let copy_button_text = "コピー";
        let nip01_profile_heading_text = "NIP-01 プロフィールメタデータ";
        let name_label_text = "名前:";
        let picture_url_label_text = "画像URL:";
        let nip05_label_text = "NIP-05:";
        let lud16_label_text = "LUD-16:";
        let about_label_text = "自己紹介:";
        let other_fields_label_text = "その他のフィールド:";
        let save_profile_button_text = "プロフィールを保存";
        let raw_json_heading_text = "生JSON";
        let logout_button_text = "ログアウト";

        // app_data_arc をクローンして非同期タスクに渡す
        let app_data_arc_clone = self.data.clone();
        let runtime_handle = self.runtime.handle().clone();

        let panel_frame = egui::Frame::default().inner_margin(Margin::same(15)).fill(ctx.style().visuals.panel_fill);

        let card_frame = egui::Frame {
            inner_margin: Margin::same(12),
            corner_radius: 8.0.into(),
            shadow: eframe::epaint::Shadow::NONE,
            fill: app_data.current_theme.card_background_color(),
            ..Default::default()
        };

        egui::SidePanel::left("side_panel")
            .frame(panel_frame)
            .min_width(220.0)
            .show(ctx, |ui| {
                ui.add_space(5.0);

                ui.horizontal(|ui| {
                    ui.heading("N");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (icon, new_theme) = match app_data.current_theme {
                            AppTheme::Light => ("☀️", AppTheme::Dark),
                            AppTheme::Dark => ("🌙", AppTheme::Light),
                        };
                        if ui.button(icon).clicked() {
                            app_data.current_theme = new_theme;
                            let new_visuals = match new_theme {
                                AppTheme::Light => light_visuals(),
                                AppTheme::Dark => dark_visuals(),
                            };
                            ctx.set_visuals(new_visuals);
                        }
                    });
                });

                ui.add_space(15.0);

                ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                    ui.style_mut().spacing.item_spacing.y = 12.0; // ボタン間の垂直スペース

                    ui.selectable_value(&mut app_data.current_tab, AppTab::Home, home_tab_text);
                    if app_data.is_logged_in {
                        ui.selectable_value(&mut app_data.current_tab, AppTab::Relays, relays_tab_text);
                        ui.selectable_value(&mut app_data.current_tab, AppTab::Profile, profile_tab_text);
                    }
                });
            });

        egui::CentralPanel::default()
            .frame(panel_frame)
            .show(ctx, |ui| {

            // ui.add_enabled_ui(!app_data.is_loading, |ui| { // この行を削除
                if !app_data.is_logged_in {
                    if app_data.current_tab == AppTab::Home {
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
                                    let cloned_app_data_arc = app_data_arc_clone.clone();
                                    runtime_handle.spawn(async move {
                                        let login_result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
                                            let keys = (|| -> Result<Keys, Box<dyn std::error::Error + Send + Sync>> {
                                                let config_str = fs::read_to_string(CONFIG_FILE)?;
                                                let config: Config = serde_json::from_str(&config_str)?;
                                                let retrieved_salt_bytes = general_purpose::STANDARD.decode(&config.salt)?;
                                                let mut derived_key_bytes = [0u8; 32];
                                                pbkdf2::pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), &retrieved_salt_bytes, 100_000, &mut derived_key_bytes);
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
                                            println!("Key decrypted for pubkey: {}", keys.public_key().to_bech32().unwrap_or_default());
                                            let client = Client::new(&keys);
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
                                                app_data.should_repaint = true;
                                            } else {
                                                let mut app_data = cloned_app_data_arc.lock().unwrap();
                                                app_data.my_keys = Some(keys.clone());
                                                app_data.nostr_client = Some(client.clone());
                                                app_data.is_logged_in = true;
                                                app_data.is_loading = true;
                                                app_data.should_repaint = true;
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
                                                println!("Network fetch complete!");
                                            } else if let Err(e) = fresh_data_result {
                                                eprintln!("Failed to fetch fresh data: {}", e);
                                                let mut app_data = cloned_app_data_arc.lock().unwrap();
                                                app_data.profile_fetch_status = format!("Failed to refresh data: {}", e);
                                            }
                                            Ok(())
                                        }.await;
                                        if let Err(e) = login_result {
                                            eprintln!("Login failed: {}", e);
                                            let client_to_shutdown = {
                                                let mut app_data_in_task = cloned_app_data_arc.lock().unwrap();
                                                app_data_in_task.nostr_client.take()
                                            };
                                            if let Some(client) = client_to_shutdown {
                                                if let Err(_e) = client.shutdown().await {
                                                     eprintln!("Failed to shutdown client: {}", _e);
                                                }
                                            }
                                            let mut app_data_in_task = cloned_app_data_arc.lock().unwrap();
                                            app_data_in_task.profile_fetch_status = format!("Login failed: {}", e);
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
                                    let cloned_app_data_arc = app_data_arc_clone.clone();
                                    runtime_handle.spawn(async move {
                                        if passphrase != confirm_passphrase {
                                            let mut current_app_data = cloned_app_data_arc.lock().unwrap();
                                            current_app_data.is_loading = false;
                                            current_app_data.should_repaint = true;
                                            return;
                                        }
                                        let registration_result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
                                            let keys = (|| -> Result<Keys, Box<dyn std::error::Error + Send + Sync>> {
                                                let user_provided_keys = Keys::parse(&secret_key_input)?;
                                                if user_provided_keys.secret_key().is_err() { return Err("Invalid secret key".into()); }
                                                let mut salt_bytes = [0u8; 16];
                                                OsRng.fill(&mut salt_bytes);
                                                let salt_base64 = general_purpose::STANDARD.encode(&salt_bytes);
                                                let mut derived_key_bytes = [0u8; 32];
                                                pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), &salt_bytes, 100_000, &mut derived_key_bytes);
                                                let cipher_key = Key::from_slice(&derived_key_bytes);
                                                let cipher = ChaCha20Poly1305::new(cipher_key);
                                                let plaintext_bytes = user_provided_keys.secret_key()?.to_secret_bytes();
                                                let mut nonce_bytes: [u8; 12] = [0u8; 12];
                                                OsRng.fill(&mut nonce_bytes);
                                                let nonce = Nonce::from_slice(&nonce_bytes);
                                                let ciphertext_with_tag = cipher.encrypt(nonce, plaintext_bytes.as_slice()).map_err(|e| format!("NIP-49 encryption error: {:?}", e))?;
                                                let mut encoded_data = ciphertext_with_tag.clone();
                                                encoded_data.extend_from_slice(nonce_bytes.as_ref());
                                                let nip49_encoded = format!("#nip49:{}", general_purpose::STANDARD.encode(&encoded_data));
                                                let config = Config { encrypted_secret_key: nip49_encoded, salt: salt_base64 };
                                                let config_json = serde_json::to_string_pretty(&config)?;
                                                fs::write(CONFIG_FILE, config_json)?;
                                                Ok(user_provided_keys)
                                            })()?;
                                            println!("Registered and logged in with pubkey: {}", keys.public_key().to_bech32().unwrap_or_default());
                                            let client = Client::new(&keys);
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
                                                app_data.followed_pubkeys_display = app_data.followed_pubkeys.iter().map(|pk| pk.to_bech32().unwrap_or_default()).collect::<Vec<_>>().join("\n");
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
                                                eprintln!("Failed to fetch initial data for registration: {}", e);
                                            }
                                            Ok(())
                                        }.await;
                                        if let Err(e) = registration_result {
                                            eprintln!("Failed to register new key: {}", e);
                                            let client_to_shutdown = {
                                                let mut app_data_in_task = cloned_app_data_arc.lock().unwrap();
                                                app_data_in_task.nostr_client.take()
                                            };
                                            if let Some(client) = client_to_shutdown {
                                                if let Err(shutdown_err) = client.shutdown().await {
                                                    eprintln!("Failed to shutdown client: {}", shutdown_err);
                                                }
                                            }
                                        }
                                        let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                                        app_data_async.is_loading = false;
                                        app_data_async.should_repaint = true;
                                    });
                                }
                            }
                        });
                    }
                } else {
                    match app_data.current_tab {
                        AppTab::Home => {
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
                                                        eprintln!("Status is too long (max {} chars)", MAX_STATUS_LENGTH);
                                                        app_data.is_loading = false;
                                                        app_data.should_repaint = true;
                                                        return;
                                                    }

                                                    let cloned_app_data_arc = app_data_arc_clone.clone();
                                                    runtime_handle.spawn(async move {
                                                        let d_tag_value = "general".to_string();
                                                        let event = EventBuilder::new(Kind::ParameterizedReplaceable(30315), status_message.clone(), vec![Tag::Identifier(d_tag_value)]).to_event(&keys_clone_nip38_send);
                                                        match event {
                                                            Ok(event) => match client_clone_nip38_send.send_event(event).await {
                                                                Ok(event_id) => {
                                                                    println!("Status published with event id: {}", event_id);
                                                                    let mut data = cloned_app_data_arc.lock().unwrap();
                                                                    data.status_message_input.clear();
                                                                    data.show_post_dialog = false;
                                                                }
                                                                Err(e) => {
                                                                    eprintln!("Failed to publish status: {}", e);
                                                                }
                                                            },
                                                            Err(e) => {
                                                                eprintln!("Failed to create event: {}", e);
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

                                    let cloned_app_data_arc = app_data_arc_clone.clone();
                                    runtime_handle.spawn(async move {
                                        let timeline_result: Result<Vec<TimelinePost>, Box<dyn std::error::Error + Send + Sync>> = async {
                                            if followed_pubkeys.is_empty() {
                                                println!("No followed users to fetch status from.");
                                                return Ok(Vec::new());
                                            }

                                            // 1. DiscoverリレーでフォローユーザーのNIP-65(kind:10002)を取得
                                            let discover_client = Client::new(&my_keys);
                                            for relay_url in discover_relays.lines().filter(|url| !url.trim().is_empty()) {
                                                discover_client.add_relay(relay_url.trim()).await?;
                                            }
                                            discover_client.connect().await;
                                            let followed_pubkeys_vec: Vec<PublicKey> = followed_pubkeys.iter().cloned().collect();
                                            let write_relay_urls = fetch_relays_for_followed_users(&discover_client, followed_pubkeys_vec).await?;
                                            discover_client.shutdown().await?;

                                            if write_relay_urls.is_empty() {
                                                println!("No writeable relays found for followed users.");
                                                return Ok(Vec::new());
                                            }

                                            // 2. 取得したwriteリレーで新しい一時クライアントを作成
                                            let temp_client = Client::new(&my_keys);
                                            for url in &write_relay_urls {
                                                temp_client.add_relay(url.clone()).await?;
                                            }
                                            temp_client.connect().await;

                                            // 3. フォローユーザーのステータス(kind:30315)を取得
                                            let timeline_filter = Filter::new().authors(followed_pubkeys).kind(Kind::ParameterizedReplaceable(30315)).limit(20);
                                            let status_events = temp_client.get_events_of(vec![timeline_filter], Some(Duration::from_secs(10))).await?;
                                            println!("Fetched {} statuses from followed users' write relays.", status_events.len());

                                            let mut timeline_posts = Vec::new();
                                            if !status_events.is_empty() {
                                                // 4. ステータス投稿者のプロフィール(kind:0)を取得
                                                let author_pubkeys: HashSet<PublicKey> = status_events.iter().map(|e| e.pubkey).collect();
                                                println!("Fetching metadata for {} authors.", author_pubkeys.len());
                                                let metadata_filter = Filter::new().authors(author_pubkeys.into_iter()).kind(Kind::Metadata);
                                                let metadata_events = temp_client.get_events_of(vec![metadata_filter], Some(Duration::from_secs(5))).await?;

                                                let mut profiles: HashMap<PublicKey, ProfileMetadata> = HashMap::new();
                                                for event in metadata_events {
                                                    if let Ok(metadata) = serde_json::from_str::<ProfileMetadata>(&event.content) {
                                                        profiles.insert(event.pubkey, metadata);
                                                    }
                                                }
                                                println!("Fetched {} profiles.", profiles.len());

                                                // 5. ステータスとメタデータをマージ
                                                for event in status_events {
                                                    timeline_posts.push(TimelinePost {
                                                        author_pubkey: event.pubkey,
                                                        author_metadata: profiles.get(&event.pubkey).cloned().unwrap_or_default(),
                                                        content: event.content.clone(),
                                                        created_at: event.created_at,
                                                    });
                                                }
                                            }

                                            // 6. 一時クライアントをシャットダウン
                                            temp_client.shutdown().await?;

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
                                                eprintln!("Failed to fetch timeline: {}", e);
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
                                            for post in &app_data.timeline_posts {
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
                                                        let created_at_datetime = chrono::DateTime::from_timestamp(post.created_at.as_i64(), 0).unwrap();
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
                                                    ui.add(egui::Label::new(egui::RichText::new(&post.content).color(app_data.current_theme.text_color())).wrap());
                                                });
                                                ui.add_space(10.0);
                                            }
                                        }
                                    });
                                });

                                for url_key in urls_to_load {
                                    app_data.image_cache.insert(url_key.clone(), ImageState::Loading);
                                    app_data.should_repaint = true;

                                    let app_data_clone = self.data.clone();
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

                                        let cloned_app_data_arc = app_data_arc_clone.clone();
                                        runtime_handle.spawn(async move {
                                            match update_contact_list(&client, &keys, pubkey, follow).await {
                                                Ok(new_followed_pubkeys) => {
                                                    let mut app_data = cloned_app_data_arc.lock().unwrap();
                                                    app_data.followed_pubkeys = new_followed_pubkeys;
                                                    // キャッシュも更新
                                                    if let Some(keys) = &app_data.my_keys {
                                                        let pubkey_hex = keys.public_key().to_string();
                                                        if let Err(e) = cache_db_clone.write_cache(DB_FOLLOWED, &pubkey_hex, &app_data.followed_pubkeys) {
                                                            eprintln!("Failed to write follow list cache: {}", e);
                                                        }
                                                    }
                                                }
                                                Err(e) => {
                                                    eprintln!("Failed to update contact list: {}", e);
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
                        },
                        AppTab::Relays => {
                            egui::ScrollArea::vertical().id_salt("relays_tab_scroll_area").show(ui, |ui| {
                                // --- 現在の接続状態 ---
                                card_frame.show(ui, |ui| {
                                    ui.heading(current_connection_heading_text);
                                    ui.add_space(10.0);
                                    let reconnect_button = egui::Button::new(egui::RichText::new(reconnect_button_text).strong());
                                    if ui.add_enabled(!app_data.is_loading, reconnect_button).clicked() {
                                        let client_clone = app_data.nostr_client.as_ref().unwrap().clone();
                                        let keys_clone = app_data.my_keys.clone().unwrap();
                                        let discover_relays = app_data.discover_relays_editor.clone();
                                        let default_relays = app_data.default_relays_editor.clone();
                                        let cache_db_clone = app_data.cache_db.clone();

                                        app_data.is_loading = true;
                                        app_data.should_repaint = true;
                                        // println!("Re-connecting to relays...");

                                        let cloned_app_data_arc = app_data_arc_clone.clone(); // async moveに渡す
                                        runtime_handle.spawn(async move {
                                            match connect_to_relays_with_nip65(&client_clone, &keys_clone, &discover_relays, &default_relays).await {
                                                Ok((log_message, fetched_nip65_relays)) => {
                                                    println!("Relay connection successful!\n{}", log_message);
                                                    let pubkey_hex = keys_clone.public_key().to_string();
                                                    if let Err(e) = cache_db_clone.write_cache(DB_RELAYS, &pubkey_hex, &fetched_nip65_relays) {
                                                        eprintln!("Failed to write NIP-65 cache: {}", e);
                                                    }

                                                    let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                                                    if let Some(pos) = log_message.find("--- 現在接続中のリレー ---") {
                                                        app_data_async.connected_relays_display = log_message[pos..].to_string();
                                                    }
                                                    // NIP-65エディタの内容を更新
                                                    app_data_async.nip65_relays = fetched_nip65_relays.into_iter().map(|(url, policy)| {
                                                        let (read, write) = match policy.as_deref() {
                                                            Some("read") => (true, false),
                                                            Some("write") => (false, true),
                                                            _ => (true, true), // デフォルトは両方 true
                                                        };
                                                        EditableRelay { url, read, write }
                                                    }).collect();
                                                }
                                                Err(e) => {
                                                    eprintln!("Failed to connect to relays: {}", e);
                                                }
                                            }
                                            let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                                            app_data_async.is_loading = false;
                                            app_data_async.should_repaint = true; // 再描画をリクエスト
                                        });
                                    }
                                    ui.add_space(10.0);
                                    egui::ScrollArea::vertical().id_salt("relay_connection_scroll_area").max_height(150.0).show(ui, |ui| {
                                        ui.add(egui::TextEdit::multiline(&mut app_data.connected_relays_display)
                                            .desired_width(ui.available_width())
                                            .interactive(false));
                                    });
                                });

                                ui.add_space(15.0);

                                // --- リレーリスト編集 ---
                                card_frame.show(ui, |ui| {
                                    ui.heading(edit_relay_lists_heading_text);
                                    ui.add_space(15.0);
                                    ui.label(nip65_relay_list_label_text);
                                    ui.add_space(5.0);

                                    let mut relay_to_remove = None;
                                    egui::ScrollArea::vertical().id_salt("nip65_editor_scroll").max_height(150.0).show(ui, |ui| {
                                        for (i, relay) in app_data.nip65_relays.iter_mut().enumerate() {
                                            ui.horizontal(|ui| {
                                                ui.label(format!("{}.", i + 1));
                                                let text_edit = egui::TextEdit::singleline(&mut relay.url).desired_width(300.0);
                                                ui.add(text_edit);
                                                ui.checkbox(&mut relay.read, read_checkbox_text);
                                                ui.checkbox(&mut relay.write, write_checkbox_text);
                                                if ui.button("❌").clicked() {
                                                    relay_to_remove = Some(i);
                                                }
                                            });
                                        }
                                    });

                                    if let Some(i) = relay_to_remove {
                                        app_data.nip65_relays.remove(i);
                                    }

                                    if ui.button(add_relay_button_text).clicked() {
                                        app_data.nip65_relays.push(EditableRelay::default());
                                    }

                                    ui.add_space(15.0);
                                    ui.label(discover_relays_label_text);
                                    ui.add_space(5.0);
                                     egui::ScrollArea::vertical().id_salt("discover_editor_scroll").max_height(80.0).show(ui, |ui| {
                                        ui.add(egui::TextEdit::multiline(&mut app_data.discover_relays_editor)
                                            .desired_width(ui.available_width()));
                                    });

                                    ui.add_space(15.0);
                                    ui.label(default_relays_label_text);
                                    ui.add_space(5.0);
                                    egui::ScrollArea::vertical().id_salt("default_editor_scroll").max_height(80.0).show(ui, |ui| {
                                        ui.add(egui::TextEdit::multiline(&mut app_data.default_relays_editor)
                                            .desired_width(ui.available_width()));
                                    });

                                    ui.add_space(15.0);
                                    let save_nip65_button = egui::Button::new(egui::RichText::new(save_nip65_button_text).strong());
                                    if ui.add_enabled(!app_data.is_loading, save_nip65_button).clicked() {
                                        let keys = app_data.my_keys.clone().unwrap();
                                        let nip65_relays = app_data.nip65_relays.clone();
                                        let discover_relays = app_data.discover_relays_editor.clone();

                                        app_data.is_loading = true;
                                        app_data.should_repaint = true;
                                        // println!("Publishing NIP-65 list...");

                                        let cloned_app_data_arc = app_data_arc_clone.clone();
                                        runtime_handle.spawn(async move {
                                            let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
                                                let tags: Vec<Tag> = nip65_relays
                                                    .iter()
                                                    .filter_map(|relay| {
                                                        if relay.url.trim().is_empty() {
                                                            return None;
                                                        }
                                                        let policy = if relay.read && !relay.write {
                                                            Some(nostr::RelayMetadata::Read)
                                                        } else if !relay.read && relay.write {
                                                            Some(nostr::RelayMetadata::Write)
                                                        } else {
                                                            // read & write or none are represented as no policy marker
                                                            None
                                                        };
                                                        Some(Tag::RelayMetadata(relay.url.clone().into(), policy))
                                                    })
                                                    .collect();

                                                if tags.is_empty() {
                                                            println!("Warning: Publishing an empty NIP-65 list.");
                                                }

                                                let event = EventBuilder::new(Kind::RelayList, "", tags).to_event(&keys)?;

                                                // Discoverリレーに接続してイベントを送信
                                                let opts = Options::new().connection_timeout(Some(Duration::from_secs(20)));
                                                let discover_client = Client::with_opts(&keys, opts);

                                                for relay_url in discover_relays.lines() {
                                                    if !relay_url.trim().is_empty() {
                                                        discover_client.add_relay(relay_url.trim()).await?;
                                                    }
                                                }
                                                discover_client.connect().await;

                                                        let event_id = discover_client.send_event(event).await?;
                                                        println!("NIP-65 list published with event id: {}", event_id);

                                                discover_client.shutdown().await?;
                                                Ok(())
                                            }.await;

                                            if let Err(e) = result {
                                                eprintln!("Failed to publish NIP-65 list: {}", e);
                                            }

                                            let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                                            app_data_async.is_loading = false;
                                            app_data_async.should_repaint = true;
                                        });
                                    }
                                });
                            });
                        },
                        AppTab::Profile => {
                            egui::ScrollArea::vertical().id_salt("profile_tab_scroll_area").show(ui, |ui| { // プロフィールタブ全体をスクロール可能に
                                card_frame.show(ui, |ui| {
                                    ui.heading(profile_heading_text);
                                    ui.add_space(10.0);

                                    ui.heading(public_key_heading_text);
                                    ui.add_space(5.0);
                                    let public_key_bech32 = app_data.my_keys.as_ref().map_or("N/A".to_string(), |k| k.public_key().to_bech32().unwrap_or_default());
                                    ui.horizontal(|ui| {
                                        ui.label(public_key_bech32.clone());
                                        if ui.button(copy_button_text).clicked() {
                                            ctx.copy_text(public_key_bech32);
                                            app_data.should_repaint = true; // 再描画をリクエスト
                                        }
                                    });
                                    ui.add_space(15.0);

                                    // NIP-01 プロファイルメタデータ表示と編集
                                    ui.heading(nip01_profile_heading_text);
                                    ui.add_space(10.0);

                                    ui.label(app_data.profile_fetch_status.as_str()); // プロファイル取得状態メッセージを表示

                                    ui.horizontal(|ui| {
                                        ui.label(name_label_text);
                                        ui.text_edit_singleline(&mut app_data.editable_profile.name);
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label(picture_url_label_text);
                                        ui.text_edit_singleline(&mut app_data.editable_profile.picture);
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label(nip05_label_text);
                                        ui.text_edit_singleline(&mut app_data.editable_profile.nip05);
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label(lud16_label_text);
                                        ui.text_edit_singleline(&mut app_data.editable_profile.lud16);
                                    });
                                    ui.label(about_label_text);
                                    ui.add(egui::TextEdit::multiline(&mut app_data.editable_profile.about)
                                        .desired_rows(3)
                                        .desired_width(ui.available_width()));

                                    // その他のフィールドも表示（例として最初の数個）
                                    if !app_data.editable_profile.extra.is_empty() {
                                        ui.label(other_fields_label_text);
                                        for (key, value) in app_data.editable_profile.extra.iter().take(5) { // 最初の5つだけ表示
                                            ui.horizontal(|ui| {
                                                ui.label(format!("{}:", key));
                                                let mut display_value = value.to_string(); // Create a temporary String for display
                                                ui.add(egui::TextEdit::singleline(&mut display_value)
                                                    .interactive(false)); // Make it read-only
                                            });
                                        }
                                        if app_data.editable_profile.extra.len() > 5 {
                                            ui.label("... more fields not shown ...");
                                        }
                                    }


                                    ui.add_space(10.0);
                                    let save_profile_button = egui::Button::new(egui::RichText::new(save_profile_button_text).strong());
                                    if ui.add_enabled(!app_data.is_loading, save_profile_button).clicked() {
                                        let client_clone = app_data.nostr_client.as_ref().unwrap().clone();
                                        let keys_clone = app_data.my_keys.clone().unwrap();
                                        let editable_profile_clone = app_data.editable_profile.clone(); // 編集中のデータをクローン
                                        let cache_db_clone = app_data.cache_db.clone();

                                        app_data.is_loading = true;
                                        app_data.should_repaint = true;
                                        // println!("Saving NIP-01 profile...");

                                        let cloned_app_data_arc = app_data_arc_clone.clone();
                                        runtime_handle.spawn(async move {
                                            let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
                                                // editable_profileから新しいJSONコンテンツを生成
                                                let profile_content = serde_json::to_string(&editable_profile_clone)?;

                                                // Kind::Metadata (Kind 0) イベントを作成
                                                let event = EventBuilder::new(Kind::Metadata, profile_content.clone(), vec![]).to_event(&keys_clone)?;

                                                // イベントをリレーに送信
                                                match client_clone.send_event(event).await {
                                                    Ok(event_id) => {
                                                        println!("NIP-01 profile published with event id: {}", event_id);

                                                        // プロフィールをキャッシュに保存
                                                        let pubkey_hex = keys_clone.public_key().to_string();
                                                        if let Err(e) = cache_db_clone.write_cache(DB_PROFILES, &pubkey_hex, &editable_profile_clone) {
                                                            eprintln!("Failed to write profile cache: {}", e);
                                                        }

                                                        let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                                                        app_data_async.profile_fetch_status = "Profile saved!".to_string();
                                                        app_data_async.nip01_profile_display = serde_json::to_string_pretty(&serde_json::from_str::<serde_json::Value>(&profile_content)?)?;
                                                    }
                                                    Err(e) => {
                                                        let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                                                        app_data_async.profile_fetch_status = format!("Failed to save profile: {}", e);
                                                    }
                                                }
                                                Ok(())
                                            }.await;

                                            if let Err(e) = result {
                                                let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                                                app_data_async.profile_fetch_status = format!("Error saving profile: {}", e);
                                            }

                                            let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                                            app_data_async.is_loading = false;
                                            app_data_async.should_repaint = true; // 再描画をリクエスト
                                        });
                                    }

                                    ui.add_space(20.0);
                                    ui.heading(raw_json_heading_text);
                                    ui.add_space(5.0);
                                    egui::ScrollArea::vertical().id_salt("raw_nip01_profile_scroll_area").max_height(200.0).show(ui, |ui| {
                                        ui.add(egui::TextEdit::multiline(&mut app_data.nip01_profile_display)
                                            .desired_width(ui.available_width())
                                            .interactive(false)
                                            .hint_text("Raw NIP-01 Profile Metadata JSON will appear here."));
                                    });

                                    // --- ログアウトボタン ---
                                    ui.add_space(50.0);
                                    ui.separator();
                                    if ui.button(egui::RichText::new(logout_button_text).color(egui::Color32::RED)).clicked() {
                                        // MutexGuardを解放する前に、所有権をタスクに移動させる
                                        let client_to_shutdown = app_data.nostr_client.take(); // Option::take()で所有権を取得

                                        // UIの状態をリセット
                                        app_data.is_logged_in = false;
                                        app_data.my_keys = None;
                                        app_data.followed_pubkeys.clear();
                                        app_data.followed_pubkeys_display.clear();
                                        app_data.timeline_posts.clear();
                                        app_data.status_message_input.clear();
                                        app_data.passphrase_input.clear();
                                        app_data.confirm_passphrase_input.clear();
                                        app_data.secret_key_input.clear();
                                        app_data.current_tab = AppTab::Home;
                                        app_data.nip01_profile_display.clear(); // ログアウト時もクリア
                                        app_data.editable_profile = ProfileMetadata::default(); // 編集可能プロファイルもリセット
                                        app_data.profile_fetch_status = "Please log in.".to_string(); // 状態メッセージもリセット
                                        app_data.should_repaint = true; // 再描画をリクエスト
                                        println!("Logged out.");

                                        // Clientのシャットダウンを非同期タスクで行う
                                        if let Some(client) = client_to_shutdown {
                                            runtime_handle.spawn(async move {
                                                if let Err(e) = client.shutdown().await {
                                                    eprintln!("Failed to shutdown client: {}", e);
                                                }
                                            });
                                        }
                                    }
                                });
                            }); // プロフィールタブ全体のスクロールエリアの終わり
                        },
                    }
                }
            // }); // この閉じ括弧も削除
        });

        // update メソッドの最後に should_repaint をチェックし、再描画をリクエスト
        if app_data.should_repaint {
            ctx.request_repaint();
            app_data.should_repaint = false; // リクエスト後にフラグをリセット
        }

        // ロード中もUIを常に更新するようリクエスト
        if app_data.is_loading {
            ctx.request_repaint();
        }
    }
}
