use eframe::egui;
use nostr::{EventBuilder, Filter, Kind, Keys, PublicKey, Tag};
use nostr_sdk::{Client, Options, SubscribeAutoCloseOptions};
use std::time::Duration;
use nostr::nips::nip19::ToBech32;

use std::fs;
use std::path::Path;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;

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

// serde を使って設定ファイルを構造体として定義
use serde::{Serialize, Deserialize};

const CONFIG_FILE: &str = "config.json"; // 設定ファイル名
const MAX_STATUS_LENGTH: usize = 140; // ステータス最大文字数

#[derive(Serialize, Deserialize)]
struct Config {
    encrypted_secret_key: String, // NIP-49フォーマットの暗号化された秘密鍵
    salt: String, // PBKDF2に使用するソルト (Base64エンコード)
}

// アプリケーションの内部状態を保持する構造体
pub struct NostrStatusAppInternal {
    pub is_logged_in: bool,
    pub status_message_input: String, // ユーザーが入力するステータス
    pub status_output: String, // アプリケーションの一般的なステータス表示
    pub error_message: String,
    pub secret_key_input: String, // 初回起動時の秘密鍵入力用
    pub passphrase_input: String,
    pub confirm_passphrase_input: String,
    pub nostr_client: Option<Client>,
    pub my_keys: Option<Keys>,
    pub followed_pubkeys: HashSet<PublicKey>, // NIP-02で取得したフォローリスト
    pub followed_pubkeys_display: String, // GUI表示用の文字列
    pub status_timeline_display: String, // GUI表示用のステータスタイムライン
    pub should_repaint: bool, // UIの再描画をトリガーするためのフラグ
    pub is_loading: bool, // 処理中であることを示すフラグ
    pub current_tab: AppTab, // 現在選択されているタブ
    pub connected_relays_display: String, // 接続中のリレー表示用
}

// タブの状態を管理するenum
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum AppTab {
    Home, // ログイン/登録画面とタイムラインを含む
    Relays,
    Profile,
}

// eframe::Appトレイトを実装する構造体
pub struct NostrStatusApp {
    data: Arc<Mutex<NostrStatusAppInternal>>,
    runtime: Runtime, // Tokio Runtimeを保持
}

impl NostrStatusApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let runtime = Runtime::new().expect("Failed to create Tokio runtime");

        // egui のスタイル設定
        _cc.egui_ctx.set_pixels_per_point(1.2); // UIのスケールを調整
        let mut style = (*_cc.egui_ctx.style()).clone();
        
        // --- クラシックなデザインのためのスタイル調整 ---
        // ライトテーマを基本とするが、全体的に落ち着いた色合いに
        style.visuals = egui::Visuals::light(); 

        // 基本色
        let classic_gray_background = egui::Color32::from_rgb(220, 220, 220); // 少し明るいグレー
        let classic_dark_text = egui::Color32::BLACK;
        let classic_white = egui::Color32::WHITE;
        let classic_blue_accent = egui::Color32::from_rgb(0, 100, 180); // 落ち着いた青
        // let classic_red_error = egui::Color32::RED; // ←未使用なので削除またはアンダースコアを追加

        // ウィンドウとパネルの背景色
        style.visuals.window_fill = classic_gray_background;
        style.visuals.panel_fill = classic_gray_background;
        style.visuals.override_text_color = Some(classic_dark_text);

        // ウィジェットの角をわずかに丸める（完全に直角にはしない）
        style.visuals.widgets.noninteractive.rounding = egui::Rounding::ZERO; 
        style.visuals.widgets.inactive.rounding = egui::Rounding::ZERO;
        style.visuals.widgets.hovered.rounding = egui::Rounding::ZERO;
        style.visuals.widgets.active.rounding = egui::Rounding::ZERO;
        style.visuals.widgets.open.rounding = egui::Rounding::ZERO;
        
        // --- ウィジェットのスタイル調整 ---
        // ボタンなどの非アクティブ状態
        style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, egui::Color32::DARK_GRAY); 
        style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, classic_dark_text); 
        style.visuals.widgets.inactive.bg_fill = classic_gray_background; 

        // ホバー時
        style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, egui::Color32::GRAY);
        style.visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, classic_dark_text);
        style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(230, 230, 230); // 少し明るいグレー

        // アクティブ時（クリック時）
        style.visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, egui::Color32::DARK_GRAY); 
        style.visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, classic_dark_text);
        style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(200, 200, 200); // 少し暗いグレー

        // スクロールバーのスタイル
        // トラック (スクロールバーの背景)
        style.visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(200, 200, 200); 
        // サム (動く部分) - 非アクティブ時
        style.visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(150, 150, 150);
        // サム - ホバー時
        style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(120, 120, 120); 
        // サム - アクティブ時
        style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(100, 100, 100); 
        // スクロールバーのボーダー
        style.visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, egui::Color32::DARK_GRAY);
        style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, egui::Color32::DARK_GRAY);
        style.visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, egui::Color32::DARK_GRAY);
        style.visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, egui::Color32::DARK_GRAY);

        // テキスト入力フィールドの背景色を白に
        style.visuals.extreme_bg_color = classic_white; 
        
        // 選択時の色
        style.visuals.selection.bg_fill = classic_blue_accent; 
        style.visuals.selection.stroke = egui::Stroke::new(1.0, classic_white); 

        // リンクの色
        style.visuals.hyperlink_color = classic_blue_accent;

        // GroupBoxのスタイリング - 枠線を残しつつ、背景は基本と同じ
        style.visuals.widgets.inactive.bg_fill = classic_gray_background; 

        // フォントの調整 (Proportional を維持し、サイズを調整)
        style.text_styles.insert(egui::TextStyle::Body, egui::FontId::new(14.0, egui::FontFamily::Proportional));
        style.text_styles.insert(egui::TextStyle::Button, egui::FontId::new(14.0, egui::FontFamily::Proportional));
        style.text_styles.insert(egui::TextStyle::Heading, egui::FontId::new(16.0, egui::FontFamily::Proportional));
        style.text_styles.insert(egui::TextStyle::Monospace, egui::FontId::new(13.0, egui::FontFamily::Monospace));
        style.text_styles.insert(egui::TextStyle::Small, egui::FontId::new(12.0, egui::FontFamily::Proportional));

        _cc.egui_ctx.set_style(style);

        let app_data_internal = NostrStatusAppInternal {
            is_logged_in: false,
            status_message_input: String::new(),
            status_output: "Welcome! Load your key or register a new one.".to_string(),
            error_message: String::new(),
            secret_key_input: String::new(),
            passphrase_input: String::new(),
            confirm_passphrase_input: String::new(),
            nostr_client: None,
            my_keys: None,
            followed_pubkeys: HashSet::new(),
            followed_pubkeys_display: String::new(),
            status_timeline_display: String::new(),
            should_repaint: false,
            is_loading: false,
            current_tab: AppTab::Home, // 初期タブをHomeに設定
            connected_relays_display: String::new(),
        };
        let data = Arc::new(Mutex::new(app_data_internal));

        // アプリケーション起動時に設定ファイルをチェックし、ロード/登録フローを開始
        let data_clone = data.clone();
        let runtime_handle = runtime.handle().clone();

        runtime_handle.spawn(async move {
            let mut app_data = data_clone.lock().unwrap();
            app_data.status_output = "Checking config file...".to_string();
            app_data.should_repaint = true;

            if Path::new(CONFIG_FILE).exists() {
                app_data.status_output = "Existing user: Please enter your passphrase.".to_string();
            } else {
                app_data.status_output = "First-time setup: Enter your secret key and set a passphrase.".to_string();
            }
            app_data.should_repaint = true;
        });
        
        Self { data, runtime }
    }
}

// NIP-65とフォールバックを考慮したリレー接続関数
async fn connect_to_relays_with_nip65(client: &Client, keys: &Keys) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let bootstrap_relays = vec![
        "wss://purplepag.es",    
        "wss://directory.yabu.me", 
    ];

    let client_opts = Options::new().connection_timeout(Some(Duration::from_secs(30)));
    let discover_client = Client::with_opts(&*keys, client_opts.clone());

    let mut status_log = String::new();
    status_log.push_str("NIP-65リレーリストを取得するためにDiscoverリレーに接続中...\n");
    for relay_url in &bootstrap_relays {
        if let Err(e) = discover_client.add_relay(*relay_url).await {
            status_log.push_str(&format!("  Discoverリレー追加失敗: {} - エラー: {}\n", *relay_url, e));
        } else {
            status_log.push_str(&format!("  Discoverリレー追加: {}\n", *relay_url));
        }
    }
    discover_client.connect().await;

    let filter = Filter::new()
        .authors(vec![keys.public_key()])
        .kind(Kind::RelayList);

    status_log.push_str("NIP-65リレーリストイベントを検索中 (最大30秒)...\n");
    let timeout_filter_id = client.subscribe(vec![filter], Some(SubscribeAutoCloseOptions::default())).await;

    let mut nip65_relays: Vec<(String, Option<String>)> = Vec::new();
    let mut received_nip65_event = false;

    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(30)) => {
            status_log.push_str("NIP-65イベント検索タイムアウト。\n");
        }
        _ = async {
            let mut notifications = client.notifications();
            while let Ok(notification) = notifications.recv().await {
                if let nostr_sdk::RelayPoolNotification::Event { event, .. } = notification {
                    if event.kind == Kind::RelayList && event.pubkey == keys.public_key() {
                        status_log.push_str("NIP-65リレーリストイベントを受信しました。\n");
                        for tag in &event.tags {
                            if let Tag::RelayMetadata(url, policy) = tag {
                                let url_string = url.to_string();
                                let policy_string = match policy {
                                    Some(nostr::RelayMetadata::Write) => Some("write".to_string()),
                                    Some(nostr::RelayMetadata::Read) => Some("read".to_string()),
                                    None => None,
                                };
                                nip65_relays.push((url_string, policy_string));
                            }
                        }
                        received_nip65_event = true;
                        break;
                    }
                }
            }
        } => {}
    }

    client.unsubscribe(timeout_filter_id).await;

    status_log.push_str("--- NIP-65で受信したリレー情報 ---\n");
    if nip65_relays.is_empty() {
        status_log.push_str("  有効なNIP-65リレーは受信しませんでした。\n");
    } else {
        for (url, policy) in &nip65_relays {
            status_log.push_str(&format!("  URL: {}, Policy: {:?}\n", url, policy));
        }
    }
    status_log.push_str("---------------------------------\n");

    let connected_relays_count: usize;
    let mut current_connected_relays = Vec::new();

    if received_nip65_event && !nip65_relays.is_empty() {
        status_log.push_str("\nNIP-65で検出されたリレーに接続中...\n");
        let _ = client.remove_all_relays().await;

        for (url, policy) in nip65_relays {
            if policy.as_deref() == Some("write") || policy.is_none() {
                if let Err(e) = client.add_relay(url.as_str()).await {
                    status_log.push_str(&format!("  リレー追加失敗: {} - エラー: {}\n", url, e));
                } else {
                    status_log.push_str(&format!("  リレー追加: {}\n", url));
                    current_connected_relays.push(url);
                }
            }
        }
        client.connect().await;
        connected_relays_count = client.relays().await.len();
        status_log.push_str(&format!("{}つのリレーに接続しました。\n", connected_relays_count));
    } else {
        status_log.push_str("\nNIP-65リレーリストが見つからなかったため、デフォルトのリレーに接続します。\n");
        let _ = client.remove_all_relays().await;
        
        let fallback_relays = ["wss://relay.damus.io", "wss://relay.nostr.wirednet.jp", "wss://yabu.me"];
        for relay_url in fallback_relays.iter() {
            if let Err(e) = client.add_relay(*relay_url).await {
                status_log.push_str(&format!("  デフォルトリレー追加失敗: {} - エラー: {}\n", *relay_url, e));
            } else {
                status_log.push_str(&format!("  デフォルトリレー追加: {}\n", *relay_url));
                current_connected_relays.push(relay_url.to_string());
            }
        }
        client.connect().await;
        connected_relays_count = client.relays().await.len();
        status_log.push_str(&format!("デフォルトのリレーに接続しました。{}つのリレー。\n", connected_relays_count));
    }

    if connected_relays_count == 0 {
        return Err("接続できるリレーがありません。ステータスを公開できません。".into());
    }

    // 接続したリレーのリストを返り値に含める
    Ok(format!("{}\n\n--- 現在接続中のリレー ---\n{}", status_log, current_connected_relays.join("\n")))
}

impl eframe::App for NostrStatusApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // バックグラウンドタスクからの更新を処理し、再描画を要求
        if self.data.lock().unwrap().should_repaint {
            ctx.request_repaint();
            self.data.lock().unwrap().should_repaint = false;
        }

        let app_data_arc = self.data.clone();
        let runtime_handle = self.runtime.handle().clone();

        // サイドパネル
        egui::SidePanel::left("side_panel")
            .min_width(150.0) // サイドパネルの最小幅を調整
            .show(ctx, |ui| {
                let mut app_data = app_data_arc.lock().unwrap();

                ui.add_space(10.0);
                ui.heading("Nostr Status App");
                ui.separator(); // 区切り線
                ui.add_space(10.0);

                ui.vertical(|ui| {
                    ui.selectable_value(&mut app_data.current_tab, AppTab::Home, "🏠 Home");
                    // ログイン後のみ表示するタブ
                    if app_data.is_logged_in {
                        ui.selectable_value(&mut app_data.current_tab, AppTab::Relays, "📡 Relays");
                        ui.selectable_value(&mut app_data.current_tab, AppTab::Profile, "👤 Profile");
                    }
                });
                ui.add_space(20.0);

                // ステータスとエラーメッセージをスクロール可能にする
                // ユニークなIDを使用
                egui::ScrollArea::vertical().id_source("side_panel_status_scroll").max_height(150.0).show(ui, |ui| {
                    ui.label(egui::RichText::new("Status:").small());
                    // ステータス表示をTextEdit::multilineに変更し、常にインタラクティブではないようにする
                    ui.add(
                        egui::TextEdit::multiline(&mut app_data.status_output)
                            .desired_width(ui.available_width()) // 利用可能な幅に合わせる
                            .interactive(false) // ユーザーが編集できないようにする
                            .text_color(egui::Color32::DARK_GRAY) // 色を指定
                            .code_editor() // コードエディタスタイルで表示（改行が保持される）
                    );
                    if !app_data.error_message.is_empty() {
                        ui.add_space(5.0);
                        // エラーメッセージの見出しはRichTextでboldに
                        ui.label(egui::RichText::new("Error:").small().color(egui::Color32::RED).strong());
                        ui.add(
                            egui::TextEdit::multiline(&mut app_data.error_message)
                                .desired_width(ui.available_width())
                                .interactive(false)
                                .text_color(egui::Color32::RED)
                                .code_editor() // .strong()を削除
                        );
                    }
                });
            });

        // 中央パネル
        egui::CentralPanel::default().show(ctx, |ui| {
            let mut app_data = app_data_arc.lock().unwrap();

            ui.heading(
                match app_data.current_tab {
                    AppTab::Home => "Home (Status & Timeline)",
                    AppTab::Relays => "Relay & Follow Management",
                    AppTab::Profile => "User Profile",
                }
            );
            ui.separator();
            ui.add_space(10.0);

            // ロード中であれば全ての入力を無効化
            ui.add_enabled_ui(!app_data.is_loading, |ui| { 
                if !app_data.is_logged_in {
                    // Homeタブのコンテンツ（ログイン/登録）
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
                                    let app_data_arc_clone = app_data_arc.clone(); 

                                    app_data.error_message.clear();
                                    app_data.is_loading = true;
                                    app_data.status_output = "Attempting to decrypt secret key...".to_string();
                                    app_data.should_repaint = true;
                                    
                                    runtime_handle.spawn(async move {
                                        {
                                            let mut current_app_data = app_data_arc_clone.lock().unwrap();
                                            current_app_data.is_loading = true;
                                            current_app_data.should_repaint = true;
                                        } 

                                        let result: Result<Keys, Box<dyn std::error::Error + Send + Sync>> = (|| {
                                            let config_str = fs::read_to_string(CONFIG_FILE)?;
                                            let config: Config = serde_json::from_str(&config_str)?;

                                            let retrieved_salt_bytes = general_purpose::STANDARD.decode(&config.salt)?;
                                            let mut derived_key_bytes = [0u8; 32];
                                            pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), &retrieved_salt_bytes, 100_000, &mut derived_key_bytes);

                                            let cipher_key = Key::from_slice(&derived_key_bytes);
                                            let cipher = ChaCha20Poly1305::new(cipher_key);

                                            let nip49_encoded = config.encrypted_secret_key;
                                            if !nip49_encoded.starts_with("#nip49:") {
                                                return Err("設定ファイルのNIP-49フォーマットが無効です。".into());
                                            }
                                            let encoded_payload = &nip49_encoded[7..];
                                            let decoded_bytes = general_purpose::STANDARD.decode(encoded_payload)?;

                                            if decoded_bytes.len() < 12 {
                                                return Err("設定ファイルのNIP-49ペイロードが短すぎます。".into());
                                            }
                                            let (ciphertext_and_tag, retrieved_nonce_bytes) = decoded_bytes.split_at(decoded_bytes.len() - 12);
                                            let retrieved_nonce = Nonce::from_slice(retrieved_nonce_bytes);

                                            let decrypted_bytes = cipher.decrypt(retrieved_nonce, ciphertext_and_tag)
                                                .map_err(|_| -> Box<dyn std::error::Error + Send + Sync> { "パスフレーズが正しくありません。".into() })?;
                                            let decrypted_secret_key_hex = hex::encode(&decrypted_bytes);
                                            Ok(Keys::parse(&decrypted_secret_key_hex)?)
                                        })();

                                        let mut app_data_async = app_data_arc_clone.lock().unwrap();
                                        app_data_async.is_loading = false;
                                        if let Ok(keys) = result {
                                            app_data_async.my_keys = Some(keys.clone());
                                            let client = Client::with_opts(&keys, Options::new().connection_timeout(Some(Duration::from_secs(30))));
                                            app_data_async.nostr_client = Some(client);
                                            app_data_async.is_logged_in = true;
                                            app_data_async.status_output = format!("Secret key decrypted and client initialized. Public Key: {}", keys.public_key().to_bech32().unwrap_or_default());
                                            app_data_async.current_tab = AppTab::Home; // ログイン後ホームに移動
                                        } else {
                                            app_data_async.error_message = result.unwrap_err().to_string();
                                            app_data_async.status_output = "Failed to load secret key.".to_string();
                                        }
                                        app_data_async.should_repaint = true;
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
                                    let app_data_arc_clone = app_data_arc.clone();

                                    app_data.error_message.clear();
                                    app_data.is_loading = true;
                                    app_data.status_output = "Registering new key...".to_string();
                                    app_data.should_repaint = true;

                                    runtime_handle.spawn(async move {
                                        {
                                            let mut current_app_data = app_data_arc_clone.lock().unwrap();
                                            current_app_data.is_loading = true;
                                            current_app_data.should_repaint = true;
                                        }

                                        if passphrase != confirm_passphrase {
                                            let mut current_app_data = app_data_arc_clone.lock().unwrap();
                                            current_app_data.error_message = "Passphrases do not match!".to_string();
                                            current_app_data.is_loading = false;
                                            current_app_data.should_repaint = true;
                                            return;
                                        }

                                        let result: Result<Keys, Box<dyn std::error::Error + Send + Sync>> = (|| {
                                            let user_provided_keys = Keys::parse(&secret_key_input)?;
                                            if user_provided_keys.secret_key().is_err() {
                                                return Err("入力された秘密鍵は無効です。".into());
                                            }

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

                                            let ciphertext_with_tag = cipher.encrypt(nonce, plaintext_bytes.as_slice())
                                                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("NIP-49 暗号化エラー: {:?}", e).into() })?;

                                            let mut encoded_data = ciphertext_with_tag.clone();
                                            encoded_data.extend_from_slice(nonce_bytes.as_ref());
                                            let nip49_encoded = format!("#nip49:{}", general_purpose::STANDARD.encode(&encoded_data));

                                            let config = Config {
                                                encrypted_secret_key: nip49_encoded,
                                                salt: salt_base64,
                                            };
                                            let config_json = serde_json::to_string_pretty(&config)?;
                                            fs::write(CONFIG_FILE, config_json)?;
                                            
                                            Ok(user_provided_keys)
                                        })();

                                        let mut app_data_async = app_data_arc_clone.lock().unwrap();
                                        app_data_async.is_loading = false;
                                        if let Ok(keys) = result {
                                            app_data_async.my_keys = Some(keys.clone());
                                            let client = Client::with_opts(&keys, Options::new().connection_timeout(Some(Duration::from_secs(30))));
                                            app_data_async.nostr_client = Some(client);
                                            app_data_async.is_logged_in = true;
                                            app_data_async.status_output = format!("Registered and logged in. Public Key: {}", keys.public_key().to_bech32().unwrap_or_default());
                                            app_data_async.current_tab = AppTab::Home; // 登録後ホームに移動
                                        } else {
                                            app_data_async.error_message = result.unwrap_err().to_string();
                                            app_data_async.status_output = "Failed to register new key.".to_string();
                                        }
                                        app_data_async.should_repaint = true;
                                    });
                                }
                            }
                        }); // end group
                    } // end AppTab::Home (login/register)
                } else {
                    // ログイン済みの場合のタブコンテンツ切り替え
                    match app_data.current_tab {
                        AppTab::Home => {
                            // Homeタブにステータス投稿とタイムラインを移動
                            ui.group(|ui| {
                                ui.heading("Set Status (NIP-38)");
                                ui.add_space(10.0);
                                ui.horizontal(|ui| {
                                    ui.label(format!("Characters: {}/{}", app_data.status_message_input.chars().count(), MAX_STATUS_LENGTH));
                                    if app_data.status_message_input.chars().count() > MAX_STATUS_LENGTH {
                                        ui.label(egui::RichText::new("Too Long!").color(egui::Color32::RED).strong());
                                    }
                                });
                                ui.add(egui::TextEdit::multiline(&mut app_data.status_message_input)
                                    .desired_rows(3)
                                    .hint_text("What's on your mind? (max 140 chars)"));

                                ui.add_space(10.0);
                                let status_message = app_data.status_message_input.clone();
                                let client_clone_nip38_send = app_data.nostr_client.clone().unwrap();
                                let keys_clone_nip38_send = app_data.my_keys.clone().unwrap();
                                let app_data_arc_clone_nip38_send = app_data_arc.clone();

                                if ui.button(egui::RichText::new("🚀 Publish Status").strong()).clicked() && !app_data.is_loading {
                                    app_data.error_message.clear();
                                    app_data.is_loading = true;
                                    app_data.status_output = "Publishing NIP-38 status...".to_string();
                                    app_data.should_repaint = true;

                                    if status_message.chars().count() > MAX_STATUS_LENGTH {
                                        app_data.error_message = format!("Status too long! Max {} characters.", MAX_STATUS_LENGTH);
                                        app_data.is_loading = false;
                                        app_data.should_repaint = true;
                                        return;
                                    }

                                    runtime_handle.spawn(async move {
                                        {
                                            let mut current_app_data = app_data_arc_clone_nip38_send.lock().unwrap();
                                            current_app_data.is_loading = true;
                                            current_app_data.should_repaint = true;
                                        } 
                                        
                                        let d_tag_value = "general".to_string();

                                        let event = EventBuilder::new(
                                            Kind::ParameterizedReplaceable(30315),
                                            status_message.clone(),
                                            vec![Tag::Identifier(d_tag_value)]
                                        ).to_event(&keys_clone_nip38_send);

                                        match event {
                                            Ok(event) => {
                                                match client_clone_nip38_send.send_event(event).await {
                                                    Ok(event_id) => {
                                                        let mut app_data_async = app_data_arc_clone_nip38_send.lock().unwrap();
                                                        app_data_async.status_output = format!("Status published! Event ID: {}", event_id);
                                                        app_data_async.status_message_input.clear();
                                                    }
                                                    Err(e) => {
                                                        let mut app_data_async = app_data_arc_clone_nip38_send.lock().unwrap();
                                                        app_data_async.error_message = format!("Failed to publish status: {}", e);
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                let mut app_data_async = app_data_arc_clone_nip38_send.lock().unwrap();
                                                app_data_async.error_message = format!("Failed to create event: {}", e);
                                            }
                                        }
                                        let mut app_data_async = app_data_arc_clone_nip38_send.lock().unwrap();
                                        app_data_async.is_loading = false;
                                        app_data_async.should_repaint = true;
                                    });
                                }
                            }); // end Set Status group

                            ui.add_space(20.0);
                            ui.group(|ui| {
                                ui.heading("Status Timeline");
                                ui.add_space(10.0);
                                let client_clone_nip38_fetch = app_data.nostr_client.clone().unwrap();
                                let followed_pubkeys_clone_nip38_fetch = app_data.followed_pubkeys.clone();
                                let app_data_arc_clone_nip38_fetch = app_data_arc.clone();

                                if ui.button(egui::RichText::new("🔄 Fetch Latest Statuses").strong()).clicked() && !app_data.is_loading {
                                    app_data.error_message.clear();
                                    app_data.is_loading = true;
                                    app_data.status_output = "Fetching NIP-38 status timeline...".to_string();
                                    app_data.should_repaint = true;

                                    runtime_handle.spawn(async move {
                                        {
                                            let mut current_app_data = app_data_arc_clone_nip38_fetch.lock().unwrap();
                                            current_app_data.is_loading = true;
                                            current_app_data.should_repaint = true;
                                        }

                                        if followed_pubkeys_clone_nip38_fetch.is_empty() {
                                            let mut app_data_async = app_data_arc_clone_nip38_fetch.lock().unwrap();
                                            app_data_async.status_output = "No followed users to fetch status from. Please fetch NIP-02 contacts first.".to_string();
                                            app_data_async.status_timeline_display = "No timeline available without followed users.".to_string();
                                            app_data_async.is_loading = false;
                                            app_data_async.should_repaint = true;
                                            return;
                                        }

                                        let timeline_filter = Filter::new()
                                            .authors(followed_pubkeys_clone_nip38_fetch.into_iter())
                                            .kind(Kind::ParameterizedReplaceable(30315))
                                            .limit(20);

                                        let timeline_filter_id = client_clone_nip38_fetch.subscribe(vec![timeline_filter], Some(SubscribeAutoCloseOptions::default())).await;
                                        
                                        let mut collected_statuses: Vec<(PublicKey, String, String)> = Vec::new();

                                        tokio::select! {
                                            _ = tokio::time::sleep(Duration::from_secs(15)) => {
                                                println!("ステータスタイムライン検索タイムアウト。");
                                            }
                                            _ = async {
                                                let mut notifications = client_clone_nip38_fetch.notifications();
                                                while let Ok(notification) = notifications.recv().await {
                                                    if let nostr_sdk::RelayPoolNotification::Event { event, .. } = notification {
                                                        if event.kind == Kind::ParameterizedReplaceable(30315) {
                                                            let d_tag_value = event.tags.iter().find_map(|tag| {
                                                                if let Tag::Identifier(d_value) = tag {
                                                                    Some(d_value.clone())
                                                                } else {
                                                                    None
                                                                }
                                                            }).unwrap_or_else(|| "general".to_string());
                                                            collected_statuses.push((event.pubkey, d_tag_value, event.content.clone()));
                                                        }
                                                    }
                                                }
                                            } => {},
                                        }
                                        client_clone_nip38_fetch.unsubscribe(timeline_filter_id).await;

                                        let mut app_data_async = app_data_arc_clone_nip38_fetch.lock().unwrap();
                                        app_data_async.is_loading = false;

                                        if !collected_statuses.is_empty() {
                                            let formatted_timeline: String = collected_statuses.iter()
                                                .map(|(pubkey, d_tag, content)| {
                                                    format!("{} ({}) says: {}", pubkey.to_bech32().unwrap_or_default(), d_tag, content)
                                                })
                                                .collect::<Vec<String>>()
                                                .join("\n\n");
                                            app_data_async.status_timeline_display = formatted_timeline;
                                            app_data_async.status_output = format!("Fetched {} statuses.", collected_statuses.len());
                                        } else {
                                            app_data_async.status_timeline_display = "No NIP-38 statuses found for followed users.".to_string();
                                            app_data_async.status_output = "No statuses found.".to_string();
                                        }
                                        app_data_async.should_repaint = true;
                                    });
                                }
                                ui.add_space(10.0);
                                // ユニークなIDを使用
                                egui::ScrollArea::vertical().id_source("timeline_scroll_area").max_height(250.0).show(ui, |ui| {
                                    ui.add(egui::TextEdit::multiline(&mut app_data.status_timeline_display)
                                        .desired_width(ui.available_width())
                                        .interactive(false));
                                });
                            }); // end Status Timeline group

                            ui.add_space(20.0);
                            if ui.button(egui::RichText::new("↩️ Logout").color(egui::Color32::RED)).clicked() {
                                app_data.is_logged_in = false;
                                app_data.nostr_client = None;
                                app_data.my_keys = None;
                                app_data.followed_pubkeys.clear();
                                app_data.followed_pubkeys_display.clear();
                                app_data.status_timeline_display.clear();
                                app_data.status_message_input.clear();
                                app_data.passphrase_input.clear();
                                app_data.confirm_passphrase_input.clear();
                                app_data.secret_key_input.clear();
                                app_data.status_output = "Logged out.".to_string();
                                app_data.error_message.clear();
                                app_data.current_tab = AppTab::Home;
                                app_data.should_repaint = true;
                            }
                        },
                        AppTab::Relays => {
                            ui.group(|ui| {
                                ui.heading("Relay Connection");
                                ui.add_space(10.0);
                                let client_clone = app_data.nostr_client.clone().unwrap();
                                let keys_clone = app_data.my_keys.clone().unwrap();
                                let app_data_arc_clone = app_data_arc.clone();

                                if ui.button(egui::RichText::new("🔗 Connect to Relays (NIP-65)").strong()).clicked() && !app_data.is_loading {
                                    app_data.error_message.clear();
                                    app_data.is_loading = true;
                                    app_data.status_output = "Connecting to relays...".to_string();
                                    app_data.should_repaint = true;
                                    
                                    runtime_handle.spawn(async move {
                                        {
                                            let mut current_app_data = app_data_arc_clone.lock().unwrap();
                                            current_app_data.is_loading = true;
                                            current_app_data.should_repaint = true;
                                        } 

                                        match connect_to_relays_with_nip65(&client_clone, &keys_clone).await {
                                            Ok(log_message) => {
                                                let mut app_data_async = app_data_arc_clone.lock().unwrap();
                                                app_data_async.status_output = format!("Relay connection successful!\n{}", log_message);
                                                // 接続したリレーリストを更新
                                                if let Some(pos) = log_message.find("--- 現在接続中のリレー ---") {
                                                    app_data_async.connected_relays_display = log_message[pos..].to_string();
                                                }
                                            }
                                            Err(e) => {
                                                let mut app_data_async = app_data_arc_clone.lock().unwrap();
                                                app_data_async.error_message = format!("Failed to connect to relays: {}", e);
                                                app_data_async.status_output = "Relay connection failed.".to_string();
                                            }
                                        }
                                        let mut app_data_async = app_data_arc_clone.lock().unwrap();
                                        app_data_async.is_loading = false;
                                        app_data_async.should_repaint = true;
                                    });
                                }
                                ui.add_space(10.0);
                                // ユニークなIDを使用
                                egui::ScrollArea::vertical().id_source("relay_connection_scroll_area").max_height(100.0).show(ui, |ui| {
                                    ui.add(egui::TextEdit::multiline(&mut app_data.connected_relays_display)
                                        .desired_width(ui.available_width())
                                        .interactive(false));
                                });
                            }); // end Relay Connection group

                            ui.add_space(20.0);
                            ui.group(|ui| {
                                ui.heading("Followed Public Keys (NIP-02)");
                                ui.add_space(10.0);
                                let client_clone_nip02 = app_data.nostr_client.clone().unwrap();
                                let keys_clone_nip02 = app_data.my_keys.clone().unwrap();
                                let app_data_arc_clone_nip02 = app_data_arc.clone();

                                if ui.button(egui::RichText::new("👥 Fetch My Follows").strong()).clicked() && !app_data.is_loading {
                                    app_data.error_message.clear();
                                    app_data.is_loading = true;
                                    app_data.status_output = "Fetching NIP-02 contact list...".to_string();
                                    app_data.should_repaint = true;

                                    runtime_handle.spawn(async move {
                                        {
                                            let mut current_app_data = app_data_arc_clone_nip02.lock().unwrap();
                                            current_app_data.is_loading = true;
                                            current_app_data.should_repaint = true;
                                        } 

                                        let mut followed_pubkeys: HashSet<PublicKey> = HashSet::new();
                                        
                                        let nip02_filter = Filter::new()
                                            .authors(vec![keys_clone_nip02.public_key()])
                                            .kind(Kind::ContactList)
                                            .limit(1);

                                        let nip02_filter_id = client_clone_nip02.subscribe(vec![nip02_filter], Some(SubscribeAutoCloseOptions::default())).await;

                                        let mut received_nip02_event = false;

                                        tokio::select! {
                                            _ = tokio::time::sleep(Duration::from_secs(10)) => {
                                                println!("フォローリスト検索タイムアウト。");
                                            }
                                            _ = async {
                                                let mut notifications = client_clone_nip02.notifications();
                                                while let Ok(notification) = notifications.recv().await {
                                                    if let nostr_sdk::RelayPoolNotification::Event { event, .. } = notification {
                                                        if event.kind == Kind::ContactList && event.pubkey == keys_clone_nip02.public_key() {
                                                            println!("✅ フォローリストイベントを受信しました。");
                                                            for tag in &event.tags {
                                                                if let Tag::PublicKey { public_key, .. } = tag {
                                                                    followed_pubkeys.insert(*public_key);
                                                                }
                                                            }
                                                            received_nip02_event = true;
                                                            break;
                                                        }
                                                    }
                                                }
                                            } => {},
                                        }
                                        client_clone_nip02.unsubscribe(nip02_filter_id).await;

                                        let mut app_data_async = app_data_arc_clone_nip02.lock().unwrap();
                                        app_data_async.is_loading = false;

                                        if received_nip02_event {
                                            app_data_async.followed_pubkeys = followed_pubkeys;
                                            app_data_async.followed_pubkeys_display = app_data_async.followed_pubkeys.iter()
                                                .map(|pk| pk.to_bech32().unwrap_or_default())
                                                .collect::<Vec<String>>()
                                                .join("\n");
                                            app_data_async.status_output = format!("Fetched {} followed pubkeys.", app_data_async.followed_pubkeys.len());
                                        } else {
                                            app_data_async.status_output = "No NIP-02 contact list found or timed out.".to_string();
                                            app_data_async.followed_pubkeys_display = "No followed users found.".to_string();
                                        }
                                        app_data_async.should_repaint = true;
                                    });
                                }
                                ui.add_space(10.0);
                                // ユニークなIDを使用
                                egui::ScrollArea::vertical().id_source("followed_pubkeys_scroll_area").max_height(250.0).show(ui, |ui| {
                                    ui.add(egui::TextEdit::multiline(&mut app_data.followed_pubkeys_display)
                                        .desired_width(ui.available_width())
                                        .interactive(false));
                                });
                            }); // end Followed Public Keys group
                        },
                        AppTab::Profile => {
                            ui.group(|ui| {
                                ui.heading("Your Profile (NIP-01 Kind 0)");
                                ui.add_space(10.0);
                                ui.label(egui::RichText::new("This section is for managing your Nostr profile metadata (name, picture, about, etc.).").italics());
                                ui.add_space(5.0);
                                ui.label("Nostr profile events (Kind 0) are used to publish your public information.");
                                ui.add_space(15.0);

                                // 公開鍵表示をここに移動
                                ui.heading("My Public Key");
                                ui.add_space(5.0);
                                let public_key_bech32 = app_data.my_keys.as_ref().map_or("N/A".to_string(), |k| k.public_key().to_bech32().unwrap_or_default());
                                ui.horizontal(|ui| {
                                    ui.label(public_key_bech32.clone());
                                    if ui.button("📋 Copy").clicked() {
                                        ctx.copy_text(public_key_bech32);
                                        app_data.status_output = "Public key copied to clipboard!".to_string();
                                        app_data.should_repaint = true;
                                    }
                                });
                                ui.add_space(15.0);
                                // ここにプロフィール編集フォームを追加できます
                                ui.label(egui::RichText::new("Future Feature: Edit your profile metadata here.").strong().color(egui::Color32::from_rgb(0, 0, 150)));
                            });
                        },
                    }
                }
            }); // end ui.add_enabled_ui
        });
    }
}

fn main() -> eframe::Result<()> {
    // env_logger::init(); // ロギングが必要な場合

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 700.0]), // ウィンドウサイズを調整
        ..Default::default()
    };

    eframe::run_native(
        "Nostr NIP-38 Status Sender",
        options,
        Box::new(|cc| Ok(Box::new(NostrStatusApp::new(cc)))),
    )
}
