use eframe::{egui::{self, Margin}};
use nostr::{EventBuilder, Filter, Kind, Keys, PublicKey, Tag};
use nostr_sdk::{Client, Options, SubscribeAutoCloseOptions};
use std::time::Duration;
use nostr::nips::nip19::ToBech32;
use std::fs;
use std::path::Path;
use std::collections::{HashSet, HashMap};

use crate::{
    NostrStatusApp, AppTab, TimelinePost, ProfileMetadata, EditableRelay,
    CONFIG_FILE, MAX_STATUS_LENGTH,
    connect_to_relays_with_nip65, fetch_nip01_profile, fetch_relays_for_followed_users
};

// --- データ取得とUI更新のための構造体 ---
struct InitialData {
    followed_pubkeys: HashSet<PublicKey>,
    timeline_posts: Vec<TimelinePost>,
    log_message: String,
    fetched_nip65_relays: Vec<(String, Option<String>)>,
    profile_metadata: ProfileMetadata,
    profile_json_string: String,
}

// --- 初回データ取得ロジック ---
async fn fetch_initial_data(
    client: &Client,
    keys: &Keys,
    discover_relays: &str,
    default_relays: &str,
) -> Result<InitialData, Box<dyn std::error::Error + Send + Sync>> {
    // --- 1. リレー接続 (NIP-65) ---
    println!("Connecting to relays...");
    let (log_message, fetched_nip65_relays) = connect_to_relays_with_nip65(
        client,
        keys,
        discover_relays,
        default_relays
    ).await?;
    println!("Relay connection process finished.\n{}", log_message);

    // --- 2. フォローリスト取得 (NIP-02) ---
    println!("Fetching NIP-02 contact list...");
    let nip02_filter = Filter::new().authors(vec![keys.public_key()]).kind(Kind::ContactList).limit(1);
    let nip02_filter_id = client.subscribe(vec![nip02_filter], Some(SubscribeAutoCloseOptions::default())).await;

    let mut followed_pubkeys = HashSet::new();
    let mut received_nip02 = false;

    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(10)) => {} // Timeout reduced
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
    }
    println!("Fetched {} followed pubkeys.", followed_pubkeys.len());

    // --- 3. タイムライン取得 ---
    let mut timeline_posts = Vec::new();
    if !followed_pubkeys.is_empty() {
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
    }

    // --- 4. 自身のNIP-01 プロフィールメタデータ取得 ---
    println!("Fetching NIP-01 profile metadata for self...");
    let (profile_metadata, profile_json_string) = fetch_nip01_profile(client, keys.public_key()).await?;
    println!("NIP-01 profile fetch for self finished.");

    Ok(InitialData {
        followed_pubkeys,
        timeline_posts,
        log_message,
        fetched_nip65_relays,
        profile_metadata,
        profile_json_string,
    })
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

        // app_data_arc をクローンして非同期タスクに渡す
        let app_data_arc_clone = self.data.clone();
        let runtime_handle = self.runtime.handle().clone();

        let panel_frame = egui::Frame::default().inner_margin(Margin::same(15)).fill(ctx.style().visuals.panel_fill);

        let card_frame = egui::Frame {
            inner_margin: Margin::same(12),
            corner_radius: 8.0.into(),
            shadow: eframe::epaint::Shadow::NONE,
            fill: egui::Color32::from_white_alpha(250),
            ..Default::default()
        };

        egui::SidePanel::left("side_panel")
            .frame(panel_frame)
            .min_width(220.0)
            .show(ctx, |ui| {
                ui.add_space(5.0);
                ui.heading("Nostr Status");
                ui.add_space(15.0);

                ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                    ui.style_mut().spacing.item_spacing.y = 12.0; // ボタン間の垂直スペース

                    ui.selectable_value(&mut app_data.current_tab, AppTab::Home, "🏠 Home");
                    if app_data.is_logged_in {
                        ui.selectable_value(&mut app_data.current_tab, AppTab::Relays, "📡 Relays");
                        ui.selectable_value(&mut app_data.current_tab, AppTab::Profile, "👤 Profile");
                    }
                });
            });

        egui::CentralPanel::default()
            .frame(panel_frame)
            .show(ctx, |ui| {

            ui.add_enabled_ui(!app_data.is_loading, |ui| {
                if !app_data.is_logged_in {
                    if app_data.current_tab == AppTab::Home {
                        ui.group(|ui| {
                            ui.heading("Login or Register");
                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
                                ui.label("Secret Key (nsec or hex, for first-time setup):");
                                ui.add(egui::TextEdit::singleline(&mut app_data.secret_key_input)
                                    .hint_text("Enter your nsec or hex secret key here"));
                            });
                            ui.horizontal(|ui| {
                                ui.label("Passphrase:");
                                ui.add(egui::TextEdit::singleline(&mut app_data.passphrase_input)
                                    .password(true)
                                    .hint_text("Your secure passphrase"));
                            });

                            if Path::new(CONFIG_FILE).exists() {
                                if ui.button(egui::RichText::new("🔑 Login with Passphrase").strong()).clicked() && !app_data.is_loading {
                                    let passphrase = app_data.passphrase_input.clone();

                                    // ロード状態と再描画フラグを更新（現在のMutexGuardで）
                                    app_data.is_loading = true;
                                    app_data.should_repaint = true;
                                    println!("Attempting to login...");

                                    // app_data_arc_clone を async move ブロックに渡す
                                    let cloned_app_data_arc = app_data_arc_clone.clone();
                                    runtime_handle.spawn(async move {
                                        let login_result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
                                            // --- 1. 鍵の復号 ---
                                            println!("Attempting to decrypt secret key...");
                                            let keys = (|| -> Result<Keys, Box<dyn std::error::Error + Send + Sync>> {
                                                let config_str = fs::read_to_string(CONFIG_FILE)?;
                                                let config: Config = serde_json::from_str(&config_str)?;
                                                let retrieved_salt_bytes = general_purpose::STANDARD.decode(&config.salt)?;
                                                let mut derived_key_bytes = [0u8; 32];
                                                pbkdf2::pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), &retrieved_salt_bytes, 100_000, &mut derived_key_bytes);
                                                let cipher_key = Key::from_slice(&derived_key_bytes);
                                                let cipher = ChaCha20Poly1305::new(cipher_key);
                                                let nip49_encoded = config.encrypted_secret_key;
                                                if !nip49_encoded.starts_with("#nip49:") { return Err("設定ファイルのNIP-49フォーマットが無効です。".into()); }
                                                let decoded_bytes = general_purpose::STANDARD.decode(&nip49_encoded[7..])?;
                                                if decoded_bytes.len() < 12 { return Err("設定ファイルのNIP-49ペイロードが短すぎます。".into()); }
                                                let (ciphertext_and_tag, retrieved_nonce_bytes) = decoded_bytes.split_at(decoded_bytes.len() - 12);
                                                let retrieved_nonce = Nonce::from_slice(retrieved_nonce_bytes);
                                                let decrypted_bytes = cipher.decrypt(retrieved_nonce, ciphertext_and_tag).map_err(|_| "パスフレーズが正しくありません。")?;
                                                let decrypted_secret_key_hex = hex::encode(&decrypted_bytes);
                                                Ok(Keys::parse(&decrypted_secret_key_hex)?)
                                            })()?;
                                            println!("Secret key decrypted successfully. Public Key: {}", keys.public_key().to_bech32().unwrap_or_default());

                                            let client = Client::new(&keys);

                                            // --- 2. データ取得 ---
                                            let (discover_relays, default_relays) = {
                                                let app_data = cloned_app_data_arc.lock().unwrap();
                                                (app_data.discover_relays_editor.clone(), app_data.default_relays_editor.clone())
                                            };
                                            let initial_data = fetch_initial_data(&client, &keys, &discover_relays, &default_relays).await?;

                                            // --- 3. 最終的なUI状態の更新 ---
                                            let mut app_data = cloned_app_data_arc.lock().unwrap();
                                            app_data.my_keys = Some(keys);
                                            app_data.nostr_client = Some(client);
                                            app_data.is_logged_in = true;
                                            app_data.current_tab = AppTab::Home;
                                            // 取得したデータでUIを更新
                                            app_data.followed_pubkeys = initial_data.followed_pubkeys;
                                            app_data.followed_pubkeys_display = app_data.followed_pubkeys.iter().map(|pk| pk.to_bech32().unwrap_or_default()).collect::<Vec<_>>().join("\n");
                                            app_data.timeline_posts = initial_data.timeline_posts;
                                            if let Some(pos) = initial_data.log_message.find("--- 現在接続中のリレー ---") {
                                                app_data.connected_relays_display = initial_data.log_message[pos..].to_string();
                                            }
                                            app_data.nip65_relays = initial_data.fetched_nip65_relays.into_iter().map(|(url, policy)| {
                                                let (read, write) = match policy.as_deref() {
                                                    Some("read") => (true, false),
                                                    Some("write") => (false, true),
                                                    _ => (true, true),
                                                };
                                                EditableRelay { url, read, write }
                                            }).collect();
                                            app_data.editable_profile = initial_data.profile_metadata;
                                            app_data.nip01_profile_display = initial_data.profile_json_string;
                                            app_data.profile_fetch_status = "Profile loaded successfully.".to_string();
                                            println!("Login process complete!");

                                            Ok(())
                                        }.await;

                                        if let Err(e) = login_result {
                                            eprintln!("Login failed: {}", e);
                                            // 失敗した場合、Clientをシャットダウン
                                            // clientをOptionから取り出して所有権を得る
                                            let client_to_shutdown = {
                                                let mut app_data_in_task = cloned_app_data_arc.lock().unwrap();
                                                app_data_in_task.nostr_client.take() // Option::take()で所有権を取得
                                            };
                                            if let Some(client) = client_to_shutdown {
                                                if let Err(e) = client.shutdown().await {
                                                     eprintln!("Failed to shutdown client: {}", e);
                                                }
                                            }
                                            // ログイン失敗時もNIP-01プロファイルをエラーメッセージで更新
                                            let mut app_data_in_task = cloned_app_data_arc.lock().unwrap();
                                            app_data_in_task.nip01_profile_display = format!("Error fetching NIP-01 profile: {}", e);
                                            app_data_in_task.profile_fetch_status = format!("Login failed: {}", e);
                                        }

                                        let mut app_data_in_task = cloned_app_data_arc.lock().unwrap();
                                        app_data_in_task.is_loading = false;
                                        app_data_in_task.should_repaint = true; // 再描画をリクエスト
                                    });
                                }
                            } else {
                                ui.horizontal(|ui| {
                                    ui.label("Confirm Passphrase:");
                                    ui.add(egui::TextEdit::singleline(&mut app_data.confirm_passphrase_input)
                                        .password(true)
                                    .hint_text("Confirm your passphrase"));
                                });

                                if ui.button(egui::RichText::new("✨ Register New Key").strong()).clicked() && !app_data.is_loading {
                                    let secret_key_input = app_data.secret_key_input.clone();
                                    let passphrase = app_data.passphrase_input.clone();
                                    let confirm_passphrase = app_data.confirm_passphrase_input.clone();

                                    app_data.is_loading = true;
                                    app_data.should_repaint = true;
                                    println!("Registering new key...");

                                    let cloned_app_data_arc = app_data_arc_clone.clone();
                                    runtime_handle.spawn(async move {
                                        if passphrase != confirm_passphrase {
                                            let mut current_app_data = cloned_app_data_arc.lock().unwrap();
                                            current_app_data.is_loading = false;
                                            current_app_data.should_repaint = true; // 再描画をリクエスト
                                            return;
                                        }

                                        let _result: Result<Keys, Box<dyn std::error::Error + Send + Sync>> = (|| {
                                            let user_provided_keys = Keys::parse(&secret_key_input)?;
                                            if user_provided_keys.secret_key().is_err() { return Err("入力された秘密鍵は無効です。".into()); }
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
                                            let ciphertext_with_tag = cipher.encrypt(nonce, plaintext_bytes.as_slice()).map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("NIP-49 暗号化エラー: {:?}", e).into() })?;
                                            let mut encoded_data = ciphertext_with_tag.clone();
                                            encoded_data.extend_from_slice(nonce_bytes.as_ref());
                                            let nip49_encoded = format!("#nip49:{}", general_purpose::STANDARD.encode(&encoded_data));
                                            let config = Config { encrypted_secret_key: nip49_encoded, salt: salt_base64 };
                                            let config_json = serde_json::to_string_pretty(&config)?;
                                            fs::write(CONFIG_FILE, config_json)?;
                                            Ok(user_provided_keys)
                                        })();

                                        let registration_result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
                                            // --- 1. 鍵の登録と保存 ---
                                            let keys = (|| -> Result<Keys, Box<dyn std::error::Error + Send + Sync>> {
                                                let user_provided_keys = Keys::parse(&secret_key_input)?;
                                                if user_provided_keys.secret_key().is_err() { return Err("入力された秘密鍵は無効です。".into()); }
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
                                                let ciphertext_with_tag = cipher.encrypt(nonce, plaintext_bytes.as_slice()).map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("NIP-49 暗号化エラー: {:?}", e).into() })?;
                                                let mut encoded_data = ciphertext_with_tag.clone();
                                                encoded_data.extend_from_slice(nonce_bytes.as_ref());
                                                let nip49_encoded = format!("#nip49:{}", general_purpose::STANDARD.encode(&encoded_data));
                                                let config = Config { encrypted_secret_key: nip49_encoded, salt: salt_base64 };
                                                let config_json = serde_json::to_string_pretty(&config)?;
                                                fs::write(CONFIG_FILE, config_json)?;
                                                Ok(user_provided_keys)
                                            })()?;
                                            println!("Registered and logged in. Public Key: {}", keys.public_key().to_bech32().unwrap_or_default());

                                            let client = Client::new(&keys);

                                            // --- 2. 初回データ取得 ---
                                            let (discover_relays, default_relays) = {
                                                let app_data = cloned_app_data_arc.lock().unwrap();
                                                (app_data.discover_relays_editor.clone(), app_data.default_relays_editor.clone())
                                            };
                                            let initial_data = fetch_initial_data(&client, &keys, &discover_relays, &default_relays).await?;

                                            // --- 3. UI状態の更新 ---
                                            let mut app_data = cloned_app_data_arc.lock().unwrap();
                                            app_data.my_keys = Some(keys);
                                            app_data.nostr_client = Some(client);
                                            app_data.is_logged_in = true;
                                            app_data.current_tab = AppTab::Home;
                                            // 取得したデータでUIを更新
                                            app_data.followed_pubkeys = initial_data.followed_pubkeys;
                                            app_data.followed_pubkeys_display = app_data.followed_pubkeys.iter().map(|pk| pk.to_bech32().unwrap_or_default()).collect::<Vec<_>>().join("\n");
                                            app_data.timeline_posts = initial_data.timeline_posts;
                                            if let Some(pos) = initial_data.log_message.find("--- 現在接続中のリレー ---") {
                                                app_data.connected_relays_display = initial_data.log_message[pos..].to_string();
                                            }
                                            app_data.nip65_relays = initial_data.fetched_nip65_relays.into_iter().map(|(url, policy)| {
                                                let (read, write) = match policy.as_deref() {
                                                    Some("read") => (true, false),
                                                    Some("write") => (false, true),
                                                    _ => (true, true),
                                                };
                                                EditableRelay { url, read, write }
                                            }).collect();
                                            app_data.editable_profile = initial_data.profile_metadata;
                                            app_data.nip01_profile_display = initial_data.profile_json_string;
                                            app_data.profile_fetch_status = "Profile loaded successfully.".to_string();

                                            Ok(())
                                        }.await;

                                        if let Err(e) = registration_result {
                                            eprintln!("Failed to register new key: {}", e);
                                            // エラーが発生した場合、作成された可能性のあるクライアントをシャットダウン
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
                                        app_data_async.should_repaint = true; // 再描画をリクエスト
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

                                egui::Window::new("New Post")
                                    .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                                    .collapsible(false)
                                    .resizable(false)
                                    .show(ctx, |ui| {
                                        ui.heading("Set Status");
                                        ui.add_space(15.0);
                                        ui.add(egui::TextEdit::multiline(&mut app_data.status_message_input)
                                            .desired_rows(5)
                                            .hint_text("What's on your mind?"));
                                        ui.add_space(10.0);
                                        ui.horizontal(|ui| {
                                            ui.label(format!("{}/{}", app_data.status_message_input.chars().count(), MAX_STATUS_LENGTH));
                                            if app_data.status_message_input.chars().count() > MAX_STATUS_LENGTH {
                                                ui.label(egui::RichText::new("Too Long!").color(egui::Color32::RED).strong());
                                            }
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                if ui.button("🚀 Publish").clicked() && !app_data.is_loading {
                                                    let status_message = app_data.status_message_input.clone();
                                                    let client_clone_nip38_send = app_data.nostr_client.as_ref().unwrap().clone();
                                                    let keys_clone_nip38_send = app_data.my_keys.clone().unwrap();

                                                    app_data.is_loading = true;
                                                    app_data.should_repaint = true;
                                                    println!("Publishing NIP-38 status...");

                                                    if status_message.chars().count() > MAX_STATUS_LENGTH {
                                                        eprintln!("Error: Status too long! Max {} characters.", MAX_STATUS_LENGTH);
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
                                                                    println!("Status published! Event ID: {}", event_id);
                                                                    let mut data = cloned_app_data_arc.lock().unwrap();
                                                                    data.status_message_input.clear();
                                                                    data.show_post_dialog = false;
                                                                }
                                                                Err(e) => eprintln!("Failed to publish status: {}", e),
                                                            },
                                                            Err(e) => eprintln!("Failed to create event: {}", e),
                                                        }
                                                        let mut data = cloned_app_data_arc.lock().unwrap();
                                                        data.is_loading = false;
                                                        data.should_repaint = true;
                                                    });
                                                }
                                                if ui.button("Cancel").clicked() {
                                                    app_data.show_post_dialog = false;
                                                }
                                            });
                                        });
                                    });
                            }
                            // --- タイムライン表示 ---
                            card_frame.show(ui, |ui| {
                                ui.heading("Timeline");
                                ui.add_space(15.0);
                                if ui.button(egui::RichText::new("🔄 Fetch Latest Statuses").strong()).clicked() && !app_data.is_loading {
                                    let followed_pubkeys = app_data.followed_pubkeys.clone();
                                    let discover_relays = app_data.discover_relays_editor.clone();
                                    let my_keys = app_data.my_keys.clone().unwrap();

                                    app_data.is_loading = true;
                                    app_data.should_repaint = true;
                                    println!("Fetching latest statuses...");

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
                                egui::ScrollArea::vertical().id_salt("timeline_scroll_area").max_height(ui.available_height() - 100.0).show(ui, |ui| {
                                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                        if app_data.timeline_posts.is_empty() {
                                            ui.label("No timeline available. Fetch latest statuses or follow more users.");
                                        } else {
                                            for post in &app_data.timeline_posts {
                                                card_frame.show(ui, |ui| {
                                                    ui.horizontal(|ui| {
                                                        // --- Profile Picture ---
                                                        let avatar_size = egui::vec2(32.0, 32.0);
                                                        if !post.author_metadata.picture.is_empty() {
                                                            ui.add(
                                                                egui::Image::from_uri(&post.author_metadata.picture)
                                                                    .corner_radius(avatar_size.x / 2.0)
                                                                    .fit_to_exact_size(avatar_size)
                                                            );
                                                        } else {
                                                            // フォールバックとして四角いスペースを表示
                                                            let (rect, _) = ui.allocate_exact_size(avatar_size, egui::Sense::hover());
                                                            ui.painter().rect_filled(rect, avatar_size.x / 2.0, ui.style().visuals.widgets.inactive.bg_fill);
                                                        }

                                                        ui.add_space(8.0); // アイコンと名前の間のスペース

                                                        let display_name = if !post.author_metadata.name.is_empty() {
                                                            post.author_metadata.name.clone()
                                                        } else {
                                                            let pubkey = post.author_pubkey.to_bech32().unwrap_or_default();
                                                            format!("{}...{}", &pubkey[0..8], &pubkey[pubkey.len()-4..])
                                                        };
                                                        ui.label(egui::RichText::new(display_name).strong());
                                                    });
                                                    ui.add_space(5.0);
                                                    ui.add(egui::Label::new(&post.content).wrap());
                                                });
                                                ui.add_space(10.0);
                                            }
                                        }
                                    });
                                });
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
                                    ui.heading("Current Connection");
                                    ui.add_space(10.0);
                                    if ui.button(egui::RichText::new("🔗 Re-Connect to Relays").strong()).clicked() && !app_data.is_loading {
                                        let client_clone = app_data.nostr_client.as_ref().unwrap().clone();
                                        let keys_clone = app_data.my_keys.clone().unwrap();
                                        let discover_relays = app_data.discover_relays_editor.clone();
                                        let default_relays = app_data.default_relays_editor.clone();

                                        app_data.is_loading = true;
                                        app_data.should_repaint = true;
                                        println!("Re-connecting to relays...");

                                        let cloned_app_data_arc = app_data_arc_clone.clone(); // async moveに渡す
                                        runtime_handle.spawn(async move {
                                            match connect_to_relays_with_nip65(&client_clone, &keys_clone, &discover_relays, &default_relays).await {
                                                Ok((log_message, fetched_nip65_relays)) => {
                                                    println!("Relay connection successful!\n{}", log_message);
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
                                    ui.heading("Edit Relay Lists");
                                    ui.add_space(15.0);
                                    ui.label("NIP-65 Relay List");
                                    ui.add_space(5.0);

                                    let mut relay_to_remove = None;
                                    egui::ScrollArea::vertical().id_salt("nip65_editor_scroll").max_height(150.0).show(ui, |ui| {
                                        for (i, relay) in app_data.nip65_relays.iter_mut().enumerate() {
                                            ui.horizontal(|ui| {
                                                ui.label(format!("{}.", i + 1));
                                                let text_edit = egui::TextEdit::singleline(&mut relay.url).desired_width(300.0);
                                                ui.add(text_edit);
                                                ui.checkbox(&mut relay.read, "Read");
                                                ui.checkbox(&mut relay.write, "Write");
                                                if ui.button("❌").clicked() {
                                                    relay_to_remove = Some(i);
                                                }
                                            });
                                        }
                                    });

                                    if let Some(i) = relay_to_remove {
                                        app_data.nip65_relays.remove(i);
                                    }

                                    if ui.button("➕ Add Relay").clicked() {
                                        app_data.nip65_relays.push(EditableRelay::default());
                                    }

                                    ui.add_space(15.0);
                                    ui.label("Discover Relays (one URL per line)");
                                    ui.add_space(5.0);
                                     egui::ScrollArea::vertical().id_salt("discover_editor_scroll").max_height(80.0).show(ui, |ui| {
                                        ui.add(egui::TextEdit::multiline(&mut app_data.discover_relays_editor)
                                            .desired_width(ui.available_width()));
                                    });

                                    ui.add_space(15.0);
                                    ui.label("Default Relays (fallback, one URL per line)");
                                    ui.add_space(5.0);
                                    egui::ScrollArea::vertical().id_salt("default_editor_scroll").max_height(80.0).show(ui, |ui| {
                                        ui.add(egui::TextEdit::multiline(&mut app_data.default_relays_editor)
                                            .desired_width(ui.available_width()));
                                    });

                                    ui.add_space(15.0);
                                    if ui.button(egui::RichText::new("💾 Save and Publish NIP-65 List").strong()).clicked() && !app_data.is_loading {
                                        let keys = app_data.my_keys.clone().unwrap();
                                        let nip65_relays = app_data.nip65_relays.clone();
                                        let discover_relays = app_data.discover_relays_editor.clone();

                                        app_data.is_loading = true;
                                        app_data.should_repaint = true;
                                        println!("Publishing NIP-65 list...");

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
                                                println!("NIP-65 list published! Event ID: {}", event_id);

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
                                    ui.heading("Your Profile");
                                    ui.add_space(10.0);

                                    ui.heading("My Public Key");
                                    ui.add_space(5.0);
                                    let public_key_bech32 = app_data.my_keys.as_ref().map_or("N/A".to_string(), |k| k.public_key().to_bech32().unwrap_or_default());
                                    ui.horizontal(|ui| {
                                        ui.label(public_key_bech32.clone());
                                        if ui.button("📋 Copy").clicked() {
                                            ctx.copy_text(public_key_bech32);
                                            app_data.should_repaint = true; // 再描画をリクエスト
                                        }
                                    });
                                    ui.add_space(15.0);

                                    // NIP-01 プロファイルメタデータ表示と編集
                                    ui.heading("NIP-01 Profile Metadata");
                                    ui.add_space(10.0);

                                    ui.label(&app_data.profile_fetch_status); // プロファイル取得状態メッセージを表示

                                    ui.horizontal(|ui| {
                                        ui.label("Name:");
                                        ui.text_edit_singleline(&mut app_data.editable_profile.name);
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label("Picture URL:");
                                        ui.text_edit_singleline(&mut app_data.editable_profile.picture);
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label("NIP-05:");
                                        ui.text_edit_singleline(&mut app_data.editable_profile.nip05);
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label("LUD-16 (Lightning Address):");
                                        ui.text_edit_singleline(&mut app_data.editable_profile.lud16);
                                    });
                                    ui.label("About:");
                                    ui.add(egui::TextEdit::multiline(&mut app_data.editable_profile.about)
                                        .desired_rows(3)
                                        .desired_width(ui.available_width()));

                                    // その他のフィールドも表示（例として最初の数個）
                                    if !app_data.editable_profile.extra.is_empty() {
                                        ui.label("Other Fields (read-only for now):");
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
                                    if ui.button(egui::RichText::new("💾 Save Profile").strong()).clicked() && !app_data.is_loading {
                                        let client_clone = app_data.nostr_client.as_ref().unwrap().clone();
                                        let keys_clone = app_data.my_keys.clone().unwrap();
                                        let editable_profile_clone = app_data.editable_profile.clone(); // 編集中のデータをクローン

                                        app_data.is_loading = true;
                                        app_data.should_repaint = true;
                                        println!("Saving NIP-01 profile...");

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
                                                        println!("NIP-01 profile published! Event ID: {}", event_id);
                                                        let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                                                        app_data_async.profile_fetch_status = "Profile saved successfully!".to_string();
                                                        app_data_async.nip01_profile_display = serde_json::to_string_pretty(&serde_json::from_str::<serde_json::Value>(&profile_content)?)?;
                                                    }
                                                    Err(e) => {
                                                        eprintln!("Failed to publish NIP-01 profile: {}", e);
                                                        let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                                                        app_data_async.profile_fetch_status = format!("Failed to save profile: {}", e);
                                                    }
                                                }
                                                Ok(())
                                            }.await;

                                            if let Err(e) = result {
                                                eprintln!("Error during profile save operation: {}", e);
                                                let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                                                app_data_async.profile_fetch_status = format!("Error: {}", e);
                                            }

                                            let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                                            app_data_async.is_loading = false;
                                            app_data_async.should_repaint = true; // 再描画をリクエスト
                                        });
                                    }

                                    ui.add_space(20.0);
                                    ui.heading("Raw NIP-01 Profile JSON");
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
                                    if ui.button(egui::RichText::new("↩️ Logout").color(egui::Color32::RED)).clicked() {
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
                                        app_data.profile_fetch_status = "Please login.".to_string(); // 状態メッセージもリセット
                                        app_data.should_repaint = true; // 再描画をリクエスト
                                        println!("Logged out.");

                                        // Clientのシャットダウンを非同期タスクで行う
                                        if let Some(client) = client_to_shutdown {
                                            runtime_handle.spawn(async move {
                                                if let Err(e) = client.shutdown().await {
                                                    eprintln!("Failed to shutdown client on logout: {}", e);
                                                }
                                            });
                                        }
                                    }
                                });
                            }); // プロフィールタブ全体のスクロールエリアの終わり
                        },
                    }
                }
            });
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


