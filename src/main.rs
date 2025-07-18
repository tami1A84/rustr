use nostr::{EventBuilder, Kind, Keys, Tag};
use nostr_sdk::{Client, Options};
use std::time::Duration;
use nostr::nips::nip19::ToBech32;
use std::io::{self, Write};
use std::fs;
use std::path::Path;

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

#[derive(Serialize, Deserialize)]
struct Config {
    encrypted_secret_key: String, // NIP-49フォーマットの暗号化された秘密鍵
    salt: String, // PBKDF2に使用するソルト (Base64エンコード)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Nostr NIP-38 ステータス送信ツール");
    println!("==================================");

    let my_keys: Keys; // ユーザーの秘密鍵を格納する変数

    // 設定ファイルが存在するかチェック
    if Path::new(CONFIG_FILE).exists() {
        // 既存ユーザーのログインフロー
        println!("\n既存のパスフレーズを入力してください:");
        let passphrase = prompt_for_passphrase(false)?;

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

        match cipher.decrypt(retrieved_nonce, ciphertext_and_tag) {
            Ok(decrypted_bytes) => {
                let decrypted_secret_key_hex = hex::encode(&decrypted_bytes);
                my_keys = Keys::parse(&decrypted_secret_key_hex)?;
                println!("✅ 秘密鍵の復号化に成功しました！");
            },
            Err(_) => {
                return Err("❌ パスフレーズが正しくありません。".into());
            }
        }
    } else {
        // 初めてのユーザー登録フロー (既存の秘密鍵を入力)
        println!("\n初回起動です。お持ちの秘密鍵をパスフレーズで安全に管理します。");
        println!("あなたのNostrアカウントの秘密鍵（nsecまたはhex形式）を入力してください:");
        let mut secret_key_input = String::new();
        io::stdin().read_line(&mut secret_key_input)?;
        let secret_key_input = secret_key_input.trim();

        let user_provided_keys = match Keys::parse(secret_key_input) {
            Ok(keys) => {
                if keys.secret_key().is_err() {
                    return Err("入力された秘密鍵は無効です。".into());
                }
                keys
            },
            Err(_) => {
                return Err("無効な秘密鍵の形式です。nsecまたはhex形式で入力してください。".into());
            }
        };

        println!("\nこの秘密鍵を保護するための新しいパスフレーズを設定します。");
        println!("忘れないように、安全な場所に控えてください。");
        let passphrase = prompt_for_passphrase(true)?;
        
        // ランダムなソルトを生成 (PBKDF2用)
        let mut salt_bytes = [0u8; 16]; // 16バイトのソルト
        OsRng.fill(&mut salt_bytes);
        let salt_base64 = general_purpose::STANDARD.encode(&salt_bytes);

        // PBKDF2を使用してパスフレーズから暗号鍵を導出
        let mut derived_key_bytes = [0u8; 32];
        pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), &salt_bytes, 100_000, &mut derived_key_bytes);

        let cipher_key = Key::from_slice(&derived_key_bytes);
        let cipher = ChaCha20Poly1305::new(cipher_key);

        let plaintext_bytes = user_provided_keys.secret_key()?.to_secret_bytes();

        let mut nonce_bytes: [u8; 12] = [0u8; 12];
        OsRng.fill(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext_with_tag = cipher.encrypt(nonce, plaintext_bytes.as_slice())
            .map_err(|e| format!("NIP-49 暗号化エラー: {:?}", e))?;

        let mut encoded_data = ciphertext_with_tag.clone();
        encoded_data.extend_from_slice(nonce_bytes.as_ref());
        let nip49_encoded = format!("#nip49:{}", general_purpose::STANDARD.encode(&encoded_data));

        let config = Config {
            encrypted_secret_key: nip49_encoded,
            salt: salt_base64,
        };
        let config_json = serde_json::to_string_pretty(&config)?;
        fs::write(CONFIG_FILE, config_json)?;
        println!("✅ 秘密鍵がパスフレーズで暗号化され、{}に保存されました。", CONFIG_FILE);
        
        my_keys = user_provided_keys;
    }

    println!("--- 自分のキー ---");
    println!("公開鍵 (npub): {}", my_keys.public_key().to_bech32()?);
    println!("秘密鍵 (nsec): {}", my_keys.secret_key()?.to_bech32()?);
    println!("------------------\n");

    // --- NIP-38: ステータスイベントの公開 ---
    println!("=== NIP-38 ステータス公開 ===");
    
    let client_opts = Options::new().connection_timeout(Some(Duration::from_secs(10)));
    let client = Client::with_opts(&my_keys, client_opts);

    client.add_relay("wss://relay.damus.io").await?;
    client.add_relay("wss://relay.nostr.wirednet.jp").await?;
    client.add_relay("wss://yabu.me").await?;
    
    println!("リレーに接続中...");
    client.connect().await; 
    println!("リレーに接続しました。");

    println!("投稿するステータスメッセージを入力してください:");
    let mut status_message = String::new();
    io::stdin().read_line(&mut status_message)?;
    let status_message = status_message.trim(); // 改行文字を削除

    println!("ステータスの種類（dタグの値、例: general, music, work など。空欄で「general」になります）:");
    let mut d_tag_input = String::new();
    io::stdin().read_line(&mut d_tag_input)?;
    let d_tag_value = if d_tag_input.trim().is_empty() {
        "general".to_string() // 空欄の場合は "general" をデフォルトにする
    } else {
        d_tag_input.trim().to_string()
    };
    
    let event = EventBuilder::new(
        Kind::ParameterizedReplaceable(30315),
        status_message,
        vec![Tag::Identifier(d_tag_value)] // ここでdタグを設定
    ).to_event(&my_keys)?;

    println!("NIP-38ステータスイベントを公開中...");
    client.send_event(event).await?;
    println!("ステータスが公開されました！ 🎉");

    client.disconnect().await?;

    Ok(())
}

// パスフレーズを非表示で入力させるヘルパー関数
fn prompt_for_passphrase(is_new_registration: bool) -> Result<String, Box<dyn std::error::Error>> {
    loop {
        print!("パスフレーズ: ");
        io::stdout().flush()?;
        let passphrase: String;
        // パスワード入力中はエコーバックしないようにする
        #[cfg(not(windows))] // Linux/macOS
        {
            passphrase = rpassword::read_password_from_tty(Some(""))?.trim().to_string();
        }
        #[cfg(windows)] // Windows
        {
            passphrase = rpassword::read_password()?.trim().to_string();
        }
        println!(); // 改行

        if passphrase.is_empty() {
            println!("パスフレーズは空にできません。");
            continue;
        }

        if is_new_registration {
            print!("パスフレーズをもう一度入力してください (確認): ");
            io::stdout().flush()?;
            let confirm_passphrase: String;
            #[cfg(not(windows))]
            {
                confirm_passphrase = rpassword::read_password_from_tty(Some(""))?.trim().to_string();
            }
            #[cfg(windows)]
            {
                confirm_passphrase = rpassword::read_password()?.trim().to_string();
            }
            println!();

            if passphrase == confirm_passphrase {
                return Ok(passphrase);
            } else {
                println!("パスフレーズが一致しません。もう一度お試しください。");
            }
        } else {
            return Ok(passphrase);
        }
    }
}
