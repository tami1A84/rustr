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

// serde と serde_json を使って設定ファイルとNIP-01メタデータを構造体として定義
use serde::{Serialize, Deserialize};
// use serde_json::json; // REMOVED: Unused import

const CONFIG_FILE: &str = "config.json"; // 設定ファイル名
const MAX_STATUS_LENGTH: usize = 140; // ステータス最大文字数

#[derive(Serialize, Deserialize)]
struct Config {
    encrypted_secret_key: String, // NIP-49フォーマットの暗号化された秘密鍵
    salt: String, // PBKDF2に使用するソルト (Base64エンコード)
}

// NIP-01 プロファイルメタデータのための構造体
// フィールドはNIP-01の推奨に従う
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProfileMetadata {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub about: String,
    #[serde(default)]
    pub picture: String,
    #[serde(default)]
    pub nip05: String, // NIP-05 identifier
    #[serde(default)]
    pub lud16: String, // Lightning Address
    #[serde(flatten)] // その他の不明なフィールドを保持
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

// リレーリスト編集のための構造体
#[derive(Debug, Clone, Default)]
pub struct EditableRelay {
    pub url: String,
    pub read: bool,
    pub write: bool,
}


// アプリケーションの内部状態を保持する構造体
pub struct NostrStatusAppInternal {
    pub is_logged_in: bool,
    pub status_message_input: String, // ユーザーが入力するステータス
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
    pub nip01_profile_display: String, // GUI表示用のNIP-01プロファイルJSON文字列
    pub editable_profile: ProfileMetadata, // 編集可能なNIP-01プロファイルデータ
    pub profile_fetch_status: String, // プロファイル取得状態メッセージ
    // リレーリスト編集用のフィールド
    pub nip65_relays: Vec<EditableRelay>,
    pub discover_relays_editor: String,
    pub default_relays_editor: String,
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
        
        // --- フォント設定 ---
        let mut fonts = egui::FontDefinitions::default();

        // **LINE Seed JPを読み込む**
        // `LINESeedJP_TTF_Rg.ttf` はダウンロードしたフォントファイル名に合わせてください。
        // 例えば `LINESeedJP_TTF_Bd.ttf` (Bold) など、他のウェイトも追加できます。
        fonts.font_data.insert(
            "LINESeedJP".to_owned(),
            egui::FontData::from_static(include_bytes!("../assets/fonts/LINESeedJP_TTF_Rg.ttf")).into(),
        );

        // **Proportional（可変幅）フォントファミリーにLINESeedJPを最優先で追加**
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "LINESeedJP".to_owned());

        // **Monospace（等幅）フォントファミリーにもLINESeedJPを追加**
        // 必要に応じて、コーディングフォントなど別の等幅フォントを優先することも可能です。
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("LINESeedJP".to_owned());

        _cc.egui_ctx.set_fonts(fonts);

        // --- クラシックなデザインのためのスタイル調整 ---
        style.visuals = egui::Visuals::light(); 

        let classic_gray_background = egui::Color32::from_rgb(220, 220, 220); 
        let classic_dark_text = egui::Color32::BLACK;
        let classic_white = egui::Color32::WHITE;
        let classic_blue_accent = egui::Color32::from_rgb(0, 100, 180); 

        style.visuals.window_fill = classic_gray_background;
        style.visuals.panel_fill = classic_gray_background;
        style.visuals.override_text_color = Some(classic_dark_text);

        // 角丸設定を修正 (Rounding を CornerRadius に、rounding を corner_radius に変更)
        style.visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::ZERO; 
        style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::ZERO;
        style.visuals.widgets.hovered.corner_radius = egui::CornerRadius::ZERO;
        style.visuals.widgets.active.corner_radius = egui::CornerRadius::ZERO;
        style.visuals.widgets.open.corner_radius = egui::CornerRadius::ZERO;
        
        style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, egui::Color32::DARK_GRAY); 
        style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, classic_dark_text); 
        style.visuals.widgets.inactive.bg_fill = classic_gray_background; 

        style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, egui::Color32::GRAY);
        style.visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, classic_dark_text); 
        style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(230, 230, 230);

        style.visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, egui::Color32::DARK_GRAY); 
        style.visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, classic_dark_text);
        style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(200, 200, 200);

        style.visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(200, 200, 200); 
        style.visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(150, 150, 150);
        style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(120, 120, 120); 
        style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(100, 100, 100); 
        style.visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, egui::Color32::DARK_GRAY);
        style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, egui::Color32::DARK_GRAY);
        style.visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, egui::Color32::DARK_GRAY);
        style.visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, egui::Color32::DARK_GRAY);

        style.visuals.extreme_bg_color = classic_white; 
        style.visuals.selection.bg_fill = classic_blue_accent; 
        style.visuals.selection.stroke = egui::Stroke::new(1.0, classic_white); 
        style.visuals.hyperlink_color = classic_blue_accent;
        style.visuals.widgets.inactive.bg_fill = classic_gray_background; 

        style.text_styles.insert(egui::TextStyle::Body, egui::FontId::new(14.0, egui::FontFamily::Proportional));
        style.text_styles.insert(egui::TextStyle::Button, egui::FontId::new(14.0, egui::FontFamily::Proportional));
        style.text_styles.insert(egui::TextStyle::Heading, egui::FontId::new(16.0, egui::FontFamily::Proportional));
        style.text_styles.insert(egui::TextStyle::Monospace, egui::FontId::new(13.0, egui::FontFamily::Monospace));
        style.text_styles.insert(egui::TextStyle::Small, egui::FontId::new(12.0, egui::FontFamily::Proportional));

        _cc.egui_ctx.set_style(style);

        let app_data_internal = NostrStatusAppInternal {
            is_logged_in: false,
            status_message_input: String::new(),
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
            current_tab: AppTab::Home,
            connected_relays_display: String::new(),
            nip01_profile_display: String::new(), // ここを初期化
            editable_profile: ProfileMetadata::default(), // 編集可能なプロファイルデータ
            profile_fetch_status: "Fetching NIP-01 profile...".to_string(), // プロファイル取得状態
            // リレーリスト編集用のフィールドを初期化
            nip65_relays: Vec::new(),
            discover_relays_editor: "wss://purplepag.es\nwss://directory.yabu.me".to_string(),
            default_relays_editor: "wss://relay.damus.io\nwss://relay.nostr.wirednet.jp\nwss://yabu.me".to_string(),
        };
        let data = Arc::new(Mutex::new(app_data_internal));

        // アプリケーション起動時に設定ファイルをチェック
        let data_clone = data.clone();
        let runtime_handle = runtime.handle().clone();

        runtime_handle.spawn(async move {
            let mut app_data = data_clone.lock().unwrap();
            println!("Checking config file...");

            if Path::new(CONFIG_FILE).exists() {
                println!("Existing user: Please enter your passphrase.");
            } else {
                println!("First-time setup: Enter your secret key and set a passphrase.");
            }
            app_data.should_repaint = true;
        });
        
        Self { data, runtime }
    }
}

// NIP-65とフォールバックを考慮したリレー接続関数
async fn connect_to_relays_with_nip65(
    client: &Client,
    keys: &Keys,
    discover_relays_str: &str,
    default_relays_str: &str,
) -> Result<(String, Vec<(String, Option<String>)>), Box<dyn std::error::Error + Send + Sync>> {
    let bootstrap_relays: Vec<String> = discover_relays_str.lines().map(|s| s.to_string()).collect();

    let client_opts = Options::new().connection_timeout(Some(Duration::from_secs(30)));
    let discover_client = Client::with_opts(&*keys, client_opts.clone()); // A dedicated client for discovery

    let mut status_log = String::new();
    status_log.push_str("NIP-65リレーリストを取得するためにDiscoverリレーに接続中...\n");
    for relay_url in &bootstrap_relays {
        if let Err(e) = discover_client.add_relay(relay_url.clone()).await { // Add to discover_client
            status_log.push_str(&format!("  Discoverリレー追加失敗: {} - エラー: {}\n", relay_url, e));
        } else {
            status_log.push_str(&format!("  Discoverリレー追加: {}\n", relay_url));
        }
    }
    discover_client.connect().await; // Connect discover_client
    tokio::time::sleep(Duration::from_secs(2)).await; // Discoverリレー接続安定待ち

    let filter = Filter::new()
        .authors(vec![keys.public_key()])
        .kind(Kind::RelayList);

    status_log.push_str("NIP-65リレーリストイベントを検索中 (最大10秒)..\n"); // Timeout reduced
    let timeout_filter_id = discover_client.subscribe(vec![filter], Some(SubscribeAutoCloseOptions::default())).await;

    let mut nip65_relays: Vec<(String, Option<String>)> = Vec::new();
    let mut received_nip65_event = false;

    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(10)) => { // Timeout reduced
            status_log.push_str("NIP-65イベント検索タイムアウト。\n");
        }
        _ = async {
            let mut notifications = discover_client.notifications();
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

    discover_client.unsubscribe(timeout_filter_id).await;
    discover_client.shutdown().await?;

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

        for (url, policy) in nip65_relays.iter() { // Iterate over a reference
            if policy.as_deref() == Some("write") || policy.is_none() {
                if let Err(e) = client.add_relay(url.as_str()).await {
                    status_log.push_str(&format!("  リレー追加失敗: {} - エラー: {}\n", url, e));
                } else {
                    status_log.push_str(&format!("  リレー追加: {}\n", url));
                    current_connected_relays.push(url.clone());
                }
            }
        }
        client.connect().await;
        connected_relays_count = client.relays().await.len();
        status_log.push_str(&format!("{}つのリレーに接続しました。\n", connected_relays_count));
    } else {
        status_log.push_str("\nNIP-65リレーリストが見つからなかったため、デフォルトのリレーに接続します。\n");
        let _ = client.remove_all_relays().await;
        
        let fallback_relays: Vec<String> = default_relays_str.lines().map(|s| s.to_string()).collect();
        for relay_url in fallback_relays.iter() {
            if !relay_url.trim().is_empty() {
                if let Err(e) = client.add_relay(relay_url.trim()).await {
                    status_log.push_str(&format!("  デフォルトリレー追加失敗: {} - エラー: {}\n", relay_url, e));
                } else {
                    status_log.push_str(&format!("  デフォルトリレー追加: {}\n", relay_url));
                    current_connected_relays.push(relay_url.to_string());
                }
            }
        }
        client.connect().await;
        connected_relays_count = client.relays().await.len();
        status_log.push_str(&format!("デフォルトのリレーに接続しました。{}つのリレー。\n", connected_relays_count));
    }

    if connected_relays_count == 0 {
        return Err("接続できるリレーがありません。".into());
    }

    // 接続が安定するまで少し待つ
    tokio::time::sleep(Duration::from_secs(2)).await;
    status_log.push_str("リレー接続が安定しました。\n");

    let full_log = format!("{}\n\n--- 現在接続中のリレー ---\n{}", status_log, current_connected_relays.join("\n"));
    Ok((full_log, nip65_relays))
}

// NIP-01 プロファイルメタデータを取得する関数
async fn fetch_nip01_profile(client: &Client, public_key: PublicKey) -> Result<(ProfileMetadata, String), Box<dyn std::error::Error + Send + Sync>> {
    let nip01_filter = Filter::new().authors(vec![public_key]).kind(Kind::Metadata).limit(1);
    let nip01_filter_id = client.subscribe(vec![nip01_filter], Some(SubscribeAutoCloseOptions::default())).await;
    
    let mut profile_json_string = String::new();
    let mut received_nip01 = false;
    
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(10)) => {
            eprintln!("NIP-01 profile fetch timed out.");
        }
        _ = async {
            let mut notifications = client.notifications();
            while let Ok(notification) = notifications.recv().await {
                if let nostr_sdk::RelayPoolNotification::Event { event, .. } = notification {
                    if event.kind == Kind::Metadata && event.pubkey == public_key {
                        println!("NIP-01 profile event received.");
                        profile_json_string = event.content.clone();
                        received_nip01 = true;
                        break;
                    }
                }
            }
        } => {},
    }
    client.unsubscribe(nip01_filter_id).await;

    if received_nip01 {
        let profile_metadata: ProfileMetadata = serde_json::from_str(&profile_json_string)?;
        Ok((profile_metadata, profile_json_string))
    } else {
        let default_metadata = ProfileMetadata::default();
        let default_json = serde_json::to_string_pretty(&default_metadata)?;
        Ok((default_metadata, default_json)) // プロファイルが見つからなかった場合はデフォルト値を返す
    }
}


impl eframe::App for NostrStatusApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // MutexGuardをupdate関数全体のスコープで保持
        let mut app_data = self.data.lock().unwrap(); 

        // app_data_arc をクローンして非同期タスクに渡す
        let app_data_arc_clone = self.data.clone();
        let runtime_handle = self.runtime.handle().clone();

        egui::SidePanel::left("side_panel")
            .min_width(150.0)
            .show(ctx, |ui| {
                ui.add_space(10.0);
                ui.heading("Nostr Status App");
                ui.separator();
                ui.add_space(10.0);

                ui.vertical(|ui| {
                    ui.selectable_value(&mut app_data.current_tab, AppTab::Home, "🏠 Home");
                    if app_data.is_logged_in {
                        ui.selectable_value(&mut app_data.current_tab, AppTab::Relays, "📡 Relays");
                        ui.selectable_value(&mut app_data.current_tab, AppTab::Profile, "👤 Profile");
                    }
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading(
                match app_data.current_tab {
                    AppTab::Home => "Home (Status & Timeline)",
                    AppTab::Relays => "Relay Management",
                    AppTab::Profile => "User Profile",
                }
            );
            ui.separator();
            ui.add_space(10.0);

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
                                            
                                            // --- 2. リレー接続 (NIP-65) ---
                                            println!("Connecting to relays...");
                                            let (discover_relays, default_relays) = {
                                                let app_data = cloned_app_data_arc.lock().unwrap();
                                                (app_data.discover_relays_editor.clone(), app_data.default_relays_editor.clone())
                                            };
                                            let (log_message, fetched_nip65_relays) = connect_to_relays_with_nip65(
                                                &client,
                                                &keys,
                                                &discover_relays,
                                                &default_relays
                                            ).await?;
                                            println!("Relay connection process finished.\n{}", log_message);

                                            // --- 3. フォローリスト取得 (NIP-02) ---
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
                                                // フォローリストが取得できなくても続行
                                            }
                                            println!("Fetched {} followed pubkeys.", followed_pubkeys.len());

                                            // --- 4. タイムライン取得 (NIP-38) ---
                                            let mut final_timeline_display = "No timeline available.".to_string();
                                            if !followed_pubkeys.is_empty() {
                                                println!("Fetching NIP-38 status timeline...");
                                                let timeline_filter = Filter::new().authors(followed_pubkeys.iter().cloned()).kind(Kind::ParameterizedReplaceable(30315)).limit(20);
                                                let timeline_filter_id = client.subscribe(vec![timeline_filter], Some(SubscribeAutoCloseOptions::default())).await;
                                                let mut collected_statuses = Vec::new();
                                                tokio::select! {
                                                    _ = tokio::time::sleep(Duration::from_secs(10)) => { println!("Status timeline search timed out."); } // Timeout reduced
                                                    _ = async {
                                                        let mut notifications = client.notifications();
                                                        while let Ok(notification) = notifications.recv().await {
                                                            if let nostr_sdk::RelayPoolNotification::Event { event, .. } = notification {
                                                                if event.kind == Kind::ParameterizedReplaceable(30315) {
                                                                    let d_tag = event.tags.iter().find_map(|t| if let Tag::Identifier(d) = t { Some(d.clone()) } else { None }).unwrap_or_else(|| "general".to_string());
                                                                    collected_statuses.push((event.pubkey, d_tag, event.content.clone()));
                                                                }
                                                            }
                                                        }
                                                    } => {},
                                                }
                                                client.unsubscribe(timeline_filter_id).await;
                                                
                                                if !collected_statuses.is_empty() {
                                                    final_timeline_display = collected_statuses.iter().map(|(pk, d, c)| format!("{} ({}) says: {}", pk.to_bech32().unwrap_or_default(), d, c)).collect::<Vec<_>>().join("\n\n");
                                                    println!("Fetched {} statuses.", collected_statuses.len());
                                                } else {
                                                    final_timeline_display = "No NIP-38 statuses found for followed users.".to_string();
                                                    println!("No statuses found.");
                                                }
                                            }

                                            // --- 5. NIP-01 プロフィールメタデータ取得 ---
                                            println!("Fetching NIP-01 profile metadata...");
                                            let (profile_metadata, profile_json_string) = fetch_nip01_profile(&client, keys.public_key()).await?;
                                            println!("NIP-01 profile fetch finished.");
                                            
                                            // --- 6. 最終的なUI状態の更新 ---
                                            let mut app_data = cloned_app_data_arc.lock().unwrap();
                                            app_data.my_keys = Some(keys);
                                            app_data.nostr_client = Some(client);
                                            app_data.followed_pubkeys = followed_pubkeys.clone();
                                            app_data.followed_pubkeys_display = followed_pubkeys.iter().map(|pk| pk.to_bech32().unwrap_or_default()).collect::<Vec<_>>().join("\n");
                                            app_data.status_timeline_display = final_timeline_display;
                                            if let Some(pos) = log_message.find("--- 現在接続中のリレー ---") {
                                                app_data.connected_relays_display = log_message[pos..].to_string();
                                            }
                                            // NIP-65エディタの内容を更新
                                            app_data.nip65_relays = fetched_nip65_relays.into_iter().map(|(url, policy)| {
                                                let (read, write) = match policy.as_deref() {
                                                    Some("read") => (true, false),
                                                    Some("write") => (false, true),
                                                    _ => (true, true), // デフォルトは両方 true
                                                };
                                                EditableRelay { url, read, write }
                                            }).collect();

                                            app_data.nip01_profile_display = profile_json_string; // 生のJSON文字列を保持
                                            app_data.editable_profile = profile_metadata; // 編集可能な構造体にロード
                                            app_data.is_logged_in = true;
                                            app_data.current_tab = AppTab::Home;
                                            app_data.profile_fetch_status = "NIP-01 profile loaded.".to_string();
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
                                            eprintln!("Error: Passphrases do not match!");
                                            let mut current_app_data = cloned_app_data_arc.lock().unwrap();
                                            current_app_data.is_loading = false;
                                            current_app_data.should_repaint = true; // 再描画をリクエスト
                                            return;
                                        }

                                        let result: Result<Keys, Box<dyn std::error::Error + Send + Sync>> = (|| {
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

                                        let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                                        app_data_async.is_loading = false;
                                        if let Ok(keys) = result {
                                            app_data_async.my_keys = Some(keys.clone());
                                            let client = Client::new(&keys);
                                            app_data_async.nostr_client = Some(client);
                                            app_data_async.is_logged_in = true;
                                            println!("Registered and logged in. Public Key: {}", keys.public_key().to_bech32().unwrap_or_default());
                                            app_data_async.current_tab = AppTab::Home;
                                            app_data_async.profile_fetch_status = "NIP-01 profile: No profile set yet. Please edit.".to_string(); // 新規登録時のメッセージ
                                        } else {
                                            eprintln!("Failed to register new key: {}", result.unwrap_err());
                                        }
                                        app_data_async.should_repaint = true; // 再描画をリクエスト
                                    });
                                }
                            }
                        }); 
                    }
                } else {
                    match app_data.current_tab {
                        AppTab::Home => {
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

                                if ui.button(egui::RichText::new("🚀 Publish Status").strong()).clicked() && !app_data.is_loading {
                                    let status_message = app_data.status_message_input.clone();
                                    let client_clone_nip38_send = app_data.nostr_client.as_ref().unwrap().clone(); 
                                    let keys_clone_nip38_send = app_data.my_keys.clone().unwrap();
                                    
                                    app_data.is_loading = true;
                                    app_data.should_repaint = true;
                                    println!("Publishing NIP-38 status...");

                                    if status_message.chars().count() > MAX_STATUS_LENGTH {
                                        eprintln!("Error: Status too long! Max {} characters.", MAX_STATUS_LENGTH);
                                        // `app_data`はまだスコープ内なので、直接更新
                                        app_data.is_loading = false;
                                        app_data.should_repaint = true; // 再描画をリクエスト
                                        return;
                                    }
                                    
                                    let cloned_app_data_arc = app_data_arc_clone.clone(); // async moveに渡す
                                    runtime_handle.spawn(async move {
                                        let d_tag_value = "general".to_string();
                                        let event = EventBuilder::new(Kind::ParameterizedReplaceable(30315), status_message.clone(), vec![Tag::Identifier(d_tag_value)]).to_event(&keys_clone_nip38_send);
                                        match event {
                                            Ok(event) => match client_clone_nip38_send.send_event(event).await {
                                                Ok(event_id) => {
                                                    println!("Status published! Event ID: {}", event_id);
                                                    cloned_app_data_arc.lock().unwrap().status_message_input.clear();
                                                }
                                                Err(e) => eprintln!("Failed to publish status: {}", e),
                                            },
                                            Err(e) => eprintln!("Failed to create event: {}", e),
                                        }
                                        let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                                        app_data_async.is_loading = false;
                                        app_data_async.should_repaint = true; // 再描画をリクエスト
                                    });
                                }
                            });

                            ui.add_space(20.0);
                            ui.group(|ui| {
                                ui.heading("Status Timeline");
                                ui.add_space(10.0);
                                if ui.button(egui::RichText::new("🔄 Fetch Latest Statuses").strong()).clicked() && !app_data.is_loading {
                                    let client_clone_nip38_fetch = app_data.nostr_client.as_ref().unwrap().clone(); 
                                    let followed_pubkeys_clone_nip38_fetch = app_data.followed_pubkeys.clone();
                                    
                                    app_data.is_loading = true;
                                    app_data.should_repaint = true;
                                    println!("Fetching NIP-38 status timeline...");

                                    let cloned_app_data_arc = app_data_arc_clone.clone(); // async moveに渡す
                                    runtime_handle.spawn(async move {
                                        if followed_pubkeys_clone_nip38_fetch.is_empty() {
                                            println!("No followed users to fetch status from. Please fetch NIP-02 contacts first.");
                                            let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                                            app_data_async.status_timeline_display = "No timeline available without followed users.".to_string();
                                            app_data_async.is_loading = false;
                                            app_data_async.should_repaint = true; // 再描画をリクエスト
                                            return;
                                        }

                                        let timeline_filter = Filter::new().authors(followed_pubkeys_clone_nip38_fetch.into_iter()).kind(Kind::ParameterizedReplaceable(30315)).limit(20);
                                        let timeline_filter_id = client_clone_nip38_fetch.subscribe(vec![timeline_filter], Some(SubscribeAutoCloseOptions::default())).await;
                                        let mut collected_statuses = Vec::new();
                                        tokio::select! {
                                            _ = tokio::time::sleep(Duration::from_secs(10)) => { println!("Status timeline search timed out."); } // Timeout reduced
                                            _ = async {
                                                let mut notifications = client_clone_nip38_fetch.notifications();
                                                while let Ok(notification) = notifications.recv().await {
                                                    if let nostr_sdk::RelayPoolNotification::Event { event, .. } = notification {
                                                        if event.kind == Kind::ParameterizedReplaceable(30315) {
                                                            let d_tag = event.tags.iter().find_map(|t| if let Tag::Identifier(d) = t { Some(d.clone()) } else { None }).unwrap_or_else(|| "general".to_string());
                                                            collected_statuses.push((event.pubkey, d_tag, event.content.clone()));
                                                        }
                                                    }
                                                }
                                            } => {},
                                        }
                                        client_clone_nip38_fetch.unsubscribe(timeline_filter_id).await;

                                        let mut app_data_async = cloned_app_data_arc.lock().unwrap();
                                        app_data_async.is_loading = false;
                                        if !collected_statuses.is_empty() {
                                            app_data_async.status_timeline_display = collected_statuses.iter().map(|(pk, d, c)| format!("{} ({}) says: {}", pk.to_bech32().unwrap_or_default(), d, c)).collect::<Vec<_>>().join("\n\n");
                                            println!("Fetched {} statuses.", collected_statuses.len());
                                        } else {
                                            app_data_async.status_timeline_display = "No NIP-38 statuses found for followed users.".to_string();
                                            println!("No statuses found.");
                                        }
                                        app_data_async.should_repaint = true; // 再描画をリクエスト
                                    });
                                }
                                ui.add_space(10.0);
                                egui::ScrollArea::vertical().id_salt("timeline_scroll_area").max_height(250.0).show(ui, |ui| {
                                    ui.add(egui::TextEdit::multiline(&mut app_data.status_timeline_display)
                                        .desired_width(ui.available_width())
                                        .interactive(false));
                                });
                            });
                        },
                        AppTab::Relays => {
                            egui::ScrollArea::vertical().id_salt("relays_tab_scroll_area").show(ui, |ui| {
                                // --- 現在の接続状態 ---
                                ui.group(|ui| {
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

                                ui.add_space(20.0);

                                // --- リレーリスト編集 ---
                                ui.group(|ui| {
                                    ui.heading("Edit Relay Lists");
                                    ui.add_space(10.0);
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
                                ui.group(|ui| {
                                    ui.heading("Your Profile");
                                    ui.add_space(10.0);
                                    
                                    ui.heading("My Public Key");
                                    ui.add_space(5.0);
                                    let public_key_bech32 = app_data.my_keys.as_ref().map_or("N/A".to_string(), |k| k.public_key().to_bech32().unwrap_or_default());
                                    ui.horizontal(|ui| {
                                        ui.label(public_key_bech32.clone());
                                        if ui.button("📋 Copy").clicked() {
                                            ctx.copy_text(public_key_bech32);
                                            println!("Public key copied to clipboard!");
                                            app_data.should_repaint = true; // 再描画をリクエスト
                                        }
                                    });
                                    ui.add_space(20.0);

                                    // NIP-01 プロファイルメタデータ表示と編集
                                    ui.heading("NIP-01 Profile Metadata");
                                    ui.add_space(5.0);

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
                                        app_data.status_timeline_display.clear();
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

fn main() -> eframe::Result<()> {
    // env_logger::init(); // 必要に応じて有効化

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 700.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Nostr NIP-38 Status Sender",
        options,
        Box::new(|cc| Ok(Box::new(NostrStatusApp::new(cc)))),
    )
}
