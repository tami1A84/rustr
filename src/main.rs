mod ui;
mod nostr_client;
mod cache_db;

use eframe::egui;
use nostr::{Keys, PublicKey};
use nostr_sdk::Client;

use std::path::Path;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;

use crate::cache_db::LmdbCache;
use crate::ui::migrate_data_from_files;
use self::nostr_client::{connect_to_relays_with_nip65, fetch_nip01_profile, fetch_relays_for_followed_users};
use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};

const CONFIG_FILE: &str = "config.json"; // 設定ファイル名
const CACHE_DIR: &str = "cache"; // Kept for migration
const DB_PATH: &str = "cache_db";
const CACHE_TTL_SECONDS: i64 = 24 * 60 * 60; // 24 hours

const MAX_STATUS_LENGTH: usize = 140; // ステータス最大文字数

#[derive(Serialize, Deserialize)]
pub struct Config {
    encrypted_secret_key: String, // NIP-49フォーマットの暗号化された秘密鍵
    salt: String, // PBKDF2に使用するソルト (Base64エンコード)
}

// --- キャッシュデータ構造 ---
#[derive(Serialize, Deserialize, Clone)]
pub struct Cache<T> {
    pub timestamp: DateTime<Utc>,
    pub data: T,
}

impl<T> Cache<T> {
    pub fn new(data: T) -> Self {
        Self {
            timestamp: Utc::now(),
            data,
        }
    }

    pub fn is_expired(&self) -> bool {
        let now = Utc::now();
        let duration = now.signed_duration_since(self.timestamp);
        duration.num_seconds() > CACHE_TTL_SECONDS
    }
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


// タイムラインの各投稿を表すための構造体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelinePost {
    pub author_pubkey: PublicKey,
    pub author_metadata: ProfileMetadata,
    pub content: String,
    pub created_at: nostr::Timestamp,
}

// アプリケーションの内部状態を保持する構造体
pub struct NostrStatusAppInternal {
    pub cache_db: LmdbCache,
    pub is_logged_in: bool,
    pub status_message_input: String, // ユーザーが入力するステータス
    pub show_post_dialog: bool, // 投稿ダイアログの表示状態
    pub secret_key_input: String, // 初回起動時の秘密鍵入力用
    pub passphrase_input: String,
    pub confirm_passphrase_input: String,
    pub nostr_client: Option<Client>,
    pub my_keys: Option<Keys>,
    pub followed_pubkeys: HashSet<PublicKey>, // NIP-02で取得したフォローリスト
    pub followed_pubkeys_display: String, // GUI表示用の文字列
    pub timeline_posts: Vec<TimelinePost>, // GUI表示用のステータスタイムライン
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
    pub current_theme: AppTheme,
}

// タブの状態を管理するenum
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum AppTab {
    Home, // ログイン/登録画面とタイムラインを含む
    Relays,
    Profile,
}

// UIテーマを管理するenum
#[derive(Debug, PartialEq, Eq, Copy, Clone, Serialize, Deserialize)]
pub enum AppTheme {
    Light,
    Dark,
}

impl AppTheme {
    pub fn card_background_color(&self) -> egui::Color32 {
        match self {
            AppTheme::Light => egui::Color32::from_white_alpha(250),
            AppTheme::Dark => egui::Color32::from_rgb(44, 44, 46),
        }
    }

    pub fn text_color(&self) -> egui::Color32 {
        match self {
            AppTheme::Light => egui::Color32::BLACK,
            AppTheme::Dark => egui::Color32::WHITE,
        }
    }
}

// --- ライトモードのVisualsを返す関数 ---
pub fn light_visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::light();
    let background_color = egui::Color32::from_rgb(242, 242, 247);
    let panel_color = egui::Color32::from_rgb(255, 255, 255);
    let text_color = egui::Color32::BLACK;
    let accent_color = egui::Color32::from_rgb(0, 110, 230);
    let separator_color = egui::Color32::from_gray(225);

    visuals.window_fill = background_color;
    visuals.panel_fill = panel_color;
    visuals.override_text_color = Some(text_color);
    visuals.hyperlink_color = accent_color;
    visuals.faint_bg_color = background_color;
    visuals.extreme_bg_color = egui::Color32::from_gray(230);
    visuals.window_stroke = egui::Stroke::new(1.0, separator_color);
    visuals.selection.bg_fill = accent_color.linear_multiply(0.3);
    visuals.selection.stroke = egui::Stroke::new(1.0, text_color);

    let widget_visuals = &mut visuals.widgets;
    widget_visuals.noninteractive.bg_fill = egui::Color32::TRANSPARENT;
    widget_visuals.noninteractive.bg_stroke = egui::Stroke::NONE;
    widget_visuals.noninteractive.fg_stroke = egui::Stroke::new(1.0, text_color);

    widget_visuals.inactive.bg_fill = egui::Color32::from_gray(235);
    widget_visuals.inactive.bg_stroke = egui::Stroke::NONE;
    widget_visuals.inactive.fg_stroke = egui::Stroke::new(1.0, text_color);

    widget_visuals.hovered.bg_fill = egui::Color32::from_gray(220);
    widget_visuals.hovered.bg_stroke = egui::Stroke::NONE;
    widget_visuals.hovered.fg_stroke = egui::Stroke::new(1.0, text_color);

    widget_visuals.active.bg_fill = egui::Color32::from_gray(210);
    widget_visuals.active.bg_stroke = egui::Stroke::NONE;
    widget_visuals.active.fg_stroke = egui::Stroke::new(1.0, accent_color);

    visuals
}

// --- ダークモードのVisualsを返す関数 ---
pub fn dark_visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();
    let background_color = egui::Color32::from_rgb(29, 29, 31);
    let panel_color = egui::Color32::from_rgb(44, 44, 46);
    let text_color = egui::Color32::from_gray(230);
    let accent_color = egui::Color32::from_rgb(10, 132, 255);
    let separator_color = egui::Color32::from_gray(58);

    visuals.window_fill = background_color;
    visuals.panel_fill = panel_color;
    visuals.override_text_color = Some(text_color);
    visuals.hyperlink_color = accent_color;
    visuals.faint_bg_color = background_color;
    visuals.extreme_bg_color = egui::Color32::from_gray(60);
    visuals.window_stroke = egui::Stroke::new(1.0, separator_color);
    visuals.selection.bg_fill = accent_color.linear_multiply(0.3);
    visuals.selection.stroke = egui::Stroke::new(1.0, text_color);

    let widget_visuals = &mut visuals.widgets;
    widget_visuals.noninteractive.bg_fill = egui::Color32::from_rgb(40, 40, 40);
    widget_visuals.noninteractive.bg_stroke = egui::Stroke::NONE;
    widget_visuals.noninteractive.fg_stroke = egui::Stroke::new(1.0, text_color);

    widget_visuals.inactive.bg_fill = egui::Color32::from_gray(50);
    widget_visuals.inactive.bg_stroke = egui::Stroke::NONE;
    widget_visuals.inactive.fg_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(180, 180, 180));

    widget_visuals.hovered.bg_fill = egui::Color32::from_gray(70);
    widget_visuals.hovered.bg_stroke = egui::Stroke::NONE;
    widget_visuals.hovered.fg_stroke = egui::Stroke::new(1.0, text_color);

    widget_visuals.active.bg_fill = egui::Color32::from_gray(85);
    widget_visuals.active.bg_stroke = egui::Stroke::NONE;
    widget_visuals.active.fg_stroke = egui::Stroke::new(1.0, accent_color);

    visuals
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

        fonts.font_data.insert(
            "LINESeedJP".to_owned(),
            egui::FontData::from_static(include_bytes!("../assets/fonts/LINESeedJP_TTF_Rg.ttf")).into(),
        );

        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "LINESeedJP".to_owned());

        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("LINESeedJP".to_owned());

        _cc.egui_ctx.set_fonts(fonts);

        // --- スタイル調整 ---
        style.visuals = light_visuals(); // ライトモードを基準にする

        // 角丸やテキストスタイルは共通で設定
        let corner_radius = 6.0;
        style.visuals.widgets.noninteractive.corner_radius = corner_radius.into();
        style.visuals.widgets.inactive.corner_radius = corner_radius.into();
        style.visuals.widgets.hovered.corner_radius = corner_radius.into();
        style.visuals.widgets.active.corner_radius = corner_radius.into();
        style.visuals.widgets.open.corner_radius = corner_radius.into();

        style.text_styles = [
            (egui::TextStyle::Heading, egui::FontId::new(20.0, egui::FontFamily::Proportional)),
            (egui::TextStyle::Body, egui::FontId::new(13.0, egui::FontFamily::Proportional)),
            (egui::TextStyle::Monospace, egui::FontId::new(12.0, egui::FontFamily::Monospace)),
            (egui::TextStyle::Button, egui::FontId::new(13.0, egui::FontFamily::Proportional)),
            (egui::TextStyle::Small, egui::FontId::new(11.0, egui::FontFamily::Proportional)),
        ].into();

        _cc.egui_ctx.set_style(style);

        let lmdb_cache = LmdbCache::new(Path::new(DB_PATH)).expect("Failed to initialize LMDB cache");

        let app_data_internal = NostrStatusAppInternal {
            cache_db: lmdb_cache,
            is_logged_in: false,
            status_message_input: String::new(),
            show_post_dialog: false,
            secret_key_input: String::new(),
            passphrase_input: String::new(),
            confirm_passphrase_input: String::new(),
            nostr_client: None,
            my_keys: None,
            followed_pubkeys: HashSet::new(),
            followed_pubkeys_display: String::new(),
            timeline_posts: Vec::new(),
            should_repaint: false,
            is_loading: false,
            current_tab: AppTab::Home,
            connected_relays_display: String::new(),
            nip01_profile_display: String::new(), // ここを初期化
            editable_profile: ProfileMetadata::default(), // 編集可能なプロファイルデータ
            profile_fetch_status: "Fetching profile...".to_string(), // プロファイル取得状態
            // リレーリスト編集用のフィールドを初期化
            nip65_relays: Vec::new(),
            discover_relays_editor: "wss://purplepag.es\nwss://directory.yabu.me".to_string(),
            default_relays_editor: "wss://relay.damus.io\nwss://relay.nostr.wirednet.jp\nwss://yabu.me".to_string(),
            current_theme: AppTheme::Light,
        };
        let data = Arc::new(Mutex::new(app_data_internal));

        // egui_extrasの画像ローダーをインストール
        egui_extras::install_image_loaders(&_cc.egui_ctx);

        // アプリケーション起動時にデータ移行と設定ファイルチェック
        let data_clone = data.clone();
        let runtime_handle = runtime.handle().clone();

        runtime_handle.spawn(async move {
            // Run migration
            let cache_db_clone = {
                let app_data = data_clone.lock().unwrap();
                app_data.cache_db.clone()
            };
            if let Err(e) = migrate_data_from_files(&cache_db_clone).await {
                eprintln!("Data migration failed: {}", e);
            }

            let mut app_data = data_clone.lock().unwrap();
            // println!("Checking config file...");

            if Path::new(CONFIG_FILE).exists() {
                // println!("Existing user: Please enter your passphrase.");
            } else {
                // println!("First-time setup: Enter your secret key and set a passphrase.");
            }
            app_data.should_repaint = true;
        });
        
        Self { data, runtime }
    }
}

fn main() -> eframe::Result<()> {
    env_logger::init(); // 必要に応じて有効化

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 700.0]),
        ..Default::default()
    };

    eframe::run_native(
        "N",
        options,
        Box::new(|cc| Ok(Box::new(NostrStatusApp::new(cc)))),
    )
}
