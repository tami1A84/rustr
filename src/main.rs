mod ui;
mod nostr_client;

use eframe::egui;
use nostr::{Keys, PublicKey};
use nostr_sdk::Client;

use std::path::Path;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;

// NIP-49 (ChaCha20Poly1305) のための暗号クレート

// PBKDF2のためのクレート

// serde と serde_json を使って設定ファイルとNIP-01メタデータを構造体として定義
use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};

use self::nostr_client::{connect_to_relays_with_nip65, fetch_nip01_profile, fetch_relays_for_followed_users};

const CONFIG_FILE: &str = "config.json"; // 設定ファイル名
const CACHE_DIR: &str = "cache";
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
#[derive(Debug, Clone)]
pub struct TimelinePost {
    pub author_pubkey: PublicKey,
    pub author_metadata: ProfileMetadata,
    pub content: String,
    pub created_at: nostr::Timestamp,
}

// アプリケーションの内部状態を保持する構造体
pub struct NostrStatusAppInternal {
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

        // --- モダンなmacOS風デザインのためのスタイル調整 ---
        style.visuals = egui::Visuals::light(); // ライトモードを基準にする

        // カラーパレット
        let background_color = egui::Color32::from_rgb(242, 242, 247); // macOSのウィンドウ背景色に近い
        let panel_color = egui::Color32::from_rgb(255, 255, 255); // パネルは白
        let text_color = egui::Color32::BLACK;
        let accent_color = egui::Color32::from_rgb(0, 110, 230); // 少し落ち着いた青
        let separator_color = egui::Color32::from_gray(225);

        // 全体的なビジュアル設定
        style.visuals.window_fill = background_color;
        style.visuals.panel_fill = panel_color; // 中央パネルなどの背景色
        style.visuals.override_text_color = Some(text_color);
        style.visuals.hyperlink_color = accent_color;
        style.visuals.faint_bg_color = background_color; // ボタンなどの背景に使われる
        style.visuals.extreme_bg_color = egui::Color32::from_gray(230); // テキスト編集フィールドなどの背景

        // ウィジェットのスタイル
        let widget_visuals = &mut style.visuals.widgets;

        // 角丸の設定
        let corner_radius = 6.0;
        widget_visuals.noninteractive.corner_radius = corner_radius.into();
        widget_visuals.inactive.corner_radius = corner_radius.into();
        widget_visuals.hovered.corner_radius = corner_radius.into();
        widget_visuals.active.corner_radius = corner_radius.into();
        widget_visuals.open.corner_radius = corner_radius.into();

        // 非インタラクティブなウィジェット（ラベルなど）
        widget_visuals.noninteractive.bg_fill = egui::Color32::TRANSPARENT; // 背景なし
        widget_visuals.noninteractive.bg_stroke = egui::Stroke::NONE; // 枠線なし
        widget_visuals.noninteractive.fg_stroke = egui::Stroke::new(1.0, text_color); // テキストの色

        // 非アクティブなウィジェット（ボタンなど）
        widget_visuals.inactive.bg_fill = egui::Color32::from_gray(235);
        widget_visuals.inactive.bg_stroke = egui::Stroke::NONE;
        widget_visuals.inactive.fg_stroke = egui::Stroke::new(1.0, text_color);

        // ホバー時のウィジェット
        widget_visuals.hovered.bg_fill = egui::Color32::from_gray(220);
        widget_visuals.hovered.bg_stroke = egui::Stroke::NONE;
        widget_visuals.hovered.fg_stroke = egui::Stroke::new(1.0, text_color);

        // アクティブなウィジェット（クリック中）
        widget_visuals.active.bg_fill = egui::Color32::from_gray(210);
        widget_visuals.active.bg_stroke = egui::Stroke::NONE;
        widget_visuals.active.fg_stroke = egui::Stroke::new(1.0, accent_color);

        // テキスト選択
        style.visuals.selection.bg_fill = accent_color.linear_multiply(0.3); // 少し薄いアクセントカラー
        style.visuals.selection.stroke = egui::Stroke::new(1.0, text_color);

        // ウィンドウとパネルのストローク
        style.visuals.window_stroke = egui::Stroke::new(1.0, separator_color);

        // テキストスタイル
        style.text_styles = [
            (egui::TextStyle::Heading, egui::FontId::new(20.0, egui::FontFamily::Proportional)),
            (egui::TextStyle::Body, egui::FontId::new(13.0, egui::FontFamily::Proportional)),
            (egui::TextStyle::Monospace, egui::FontId::new(12.0, egui::FontFamily::Monospace)),
            (egui::TextStyle::Button, egui::FontId::new(13.0, egui::FontFamily::Proportional)),
            (egui::TextStyle::Small, egui::FontId::new(11.0, egui::FontFamily::Proportional)),
        ].into();

        _cc.egui_ctx.set_style(style);

        let app_data_internal = NostrStatusAppInternal {
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
        };
        let data = Arc::new(Mutex::new(app_data_internal));

        // egui_extrasの画像ローダーをインストール
        egui_extras::install_image_loaders(&_cc.egui_ctx);

        // アプリケーション起動時に設定ファイルをチェック
        let data_clone = data.clone();
        let runtime_handle = runtime.handle().clone();

        runtime_handle.spawn(async move {
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

    // --- キャッシュディレクトリを作成 ---
    let cache_dir = Path::new("cache");
    if !cache_dir.exists() {
        std::fs::create_dir_all(cache_dir).expect("Failed to create cache directory");
    }

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



