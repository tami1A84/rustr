use eframe::egui;
use nostr::nips::nip04;
use nostr::nips::nip47::{Method, NostrWalletConnectURI, Request, RequestParams, Response};
use nostr::{EventBuilder, Kind, Keys, Tag};
use nostr_sdk::Client;
use serde_json;
use std::fs;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use tokio::runtime::Handle;

use crate::types::{Config, NostrStatusAppInternal};
use crate::{nip49, CONFIG_FILE};

pub fn draw_wallet_view(
    ui: &mut egui::Ui,
    app_data: &mut NostrStatusAppInternal,
    app_data_arc: Arc<Mutex<NostrStatusAppInternal>>,
    runtime_handle: Handle,
) {
    ui.heading("ウォレット");
    ui.add_space(10.0);

    if !app_data.is_logged_in {
        ui.label("ウォレット機能を使うにはログインしてください。");
        return;
    }

    if app_data.nwc.is_some() {
        draw_wallet_details(ui, app_data, app_data_arc.clone(), runtime_handle);
    } else {
        draw_setup_view(ui, app_data, app_data_arc.clone(), runtime_handle);
    }

    if let Some(error) = &app_data.nwc_error {
        ui.add_space(10.0);
        ui.colored_label(egui::Color32::RED, error);
    }
}

fn draw_wallet_details(
    ui: &mut egui::Ui,
    app_data: &mut NostrStatusAppInternal,
    app_data_arc: Arc<Mutex<NostrStatusAppInternal>>,
    runtime_handle: Handle,
) {
    ui.label("ウォレット接続済み");
    if let Some(balance) = app_data.wallet_balance {
        ui.label(format!("残高: {} sats", balance / 1000));
    } else {
        ui.label("残高: 取得中...");
    }

    if ui.button("残高を更新").clicked() {
        if let Some(nwc) = app_data.nwc.clone() {
            let app_data_clone = app_data_arc.clone();
            runtime_handle.spawn(async move {
                if let Err(e) = get_balance(nwc, app_data_clone.clone()).await {
                    let mut app_data = app_data_clone.lock().unwrap();
                    app_data.nwc_error = Some(format!("残高の取得エラー: {}", e));
                }
            });
        }
    }
}

fn draw_setup_view(
    ui: &mut egui::Ui,
    app_data: &mut NostrStatusAppInternal,
    app_data_arc: Arc<Mutex<NostrStatusAppInternal>>,
    runtime_handle: Handle,
) {
    ui.label("Nostrウォレットに接続");
    ui.add_space(5.0);
    ui.label("Nostr Wallet ConnectのURIと、暗号化のためのメインパスフレーズを入力してください。");

    ui.horizontal(|ui| {
        ui.label("NWC URI:");
        ui.text_edit_singleline(&mut app_data.nwc_uri_input);
    });

    ui.horizontal(|ui| {
        ui.label("アプリのパスフレーズ:");
        ui.add(egui::TextEdit::singleline(&mut app_data.nwc_passphrase_input).password(true));
    });

    if ui.button("保存して接続").clicked() {
        let nwc_uri = app_data.nwc_uri_input.clone();
        let passphrase = app_data.nwc_passphrase_input.clone();
        app_data.nwc_passphrase_input.clear(); // Clear passphrase after use
        let app_data_clone = app_data_arc.clone();

        runtime_handle.spawn(async move {
            match save_and_connect(nwc_uri, passphrase, app_data_clone.clone()).await {
                Ok(_) => {
                    let mut app_data = app_data_clone.lock().unwrap();
                    app_data.nwc_error = None;
                }
                Err(e) => {
                    let mut app_data = app_data_clone.lock().unwrap();
                    app_data.nwc_error = Some(format!("保存と接続に失敗しました: {}", e));
                }
            }
        });
    }
}

async fn save_and_connect(
    nwc_uri_str: String,
    passphrase: String,
    app_data_arc: Arc<Mutex<NostrStatusAppInternal>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if passphrase.is_empty() {
        return Err("パスフレーズは空にできません".into());
    }
    let nwc_uri = NostrWalletConnectURI::from_str(&nwc_uri_str)?;

    // Read existing config to get the salt
    let config_str = fs::read_to_string(CONFIG_FILE)?;
    let mut config: Config = serde_json::from_str(&config_str)?;

    // Verify passphrase by trying to decrypt the main secret key
    let _ = nip49::decrypt(&config.encrypted_secret_key, &passphrase, &config.salt)?;

    // Encrypt NWC URI with the same salt and passphrase
    let encrypted_nwc_uri =
        nip49::encrypt_with_salt(nwc_uri_str.as_bytes(), &passphrase, &config.salt)?;
    config.encrypted_nwc_uri = Some(encrypted_nwc_uri);

    // Save updated config
    let config_json = serde_json::to_string_pretty(&config)?;
    fs::write(CONFIG_FILE, config_json)?;

    connect_nwc(nwc_uri, app_data_arc).await?;

    Ok(())
}

pub async fn connect_nwc(
    nwc_uri: NostrWalletConnectURI,
    app_data_arc: Arc<Mutex<NostrStatusAppInternal>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let keys = Keys::new(nwc_uri.secret.clone());
    let client = Client::new(keys);

    if let Some(relay_url) = nwc_uri.relays.first() {
        client.add_relay(relay_url.to_string()).await?;
    } else {
        return Err("NWC URIにリレーURLが含まれていません".into());
    }
    client.connect().await;

    // Spawn listener task
    let client_clone = client.clone();
    let nwc_uri_clone = nwc_uri.clone();
    let app_data_clone = app_data_arc.clone();
    tokio::spawn(async move {
        listen_for_nwc_responses(client_clone, nwc_uri_clone, app_data_clone).await;
    });

    {
        let mut app_data = app_data_arc.lock().unwrap();
        app_data.nwc_client = Some(client);
        app_data.nwc = Some(nwc_uri.clone());
    }

    get_balance(nwc_uri, app_data_arc.clone()).await?;

    Ok(())
}

async fn listen_for_nwc_responses(
    client: Client,
    nwc: NostrWalletConnectURI,
    app_data_arc: Arc<Mutex<NostrStatusAppInternal>>,
) {
    let mut notifications = client.notifications();
    loop {
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {}
            Ok(notification) = notifications.recv() => {
                if let nostr_sdk::RelayPoolNotification::Event { event, .. } = notification {
                    if event.kind == Kind::WalletConnectResponse {
                        if let Ok(decrypted_response) = Response::from_event(&nwc, &event) {
                            let mut app_data = app_data_arc.lock().unwrap();
                            if let Some(res) = decrypted_response.result {
                                match res {
                                    nostr::nips::nip47::ResponseResult::GetBalance(balance_res) => {
                                        app_data.wallet_balance = Some(balance_res.balance);
                                        app_data.nwc_error = None;
                                    },
                                    nostr::nips::nip47::ResponseResult::PayInvoice(_pay_invoice_res) => {
                                        app_data.zap_status_message = Some("ZAP成功！".to_string());
                                        // Optionally, you could use the preimage from pay_invoice_res for something.
                                    },
                                    _ => {
                                        // Handle other response types if necessary
                                    }
                                }
                            } else if let Some(error) = decrypted_response.error {
                                // Check if this error is related to a ZAP attempt
                                if app_data.zap_status_message.is_some() {
                                    app_data.zap_status_message = Some(format!("ZAP失敗: {}", error.message));
                                } else {
                                    app_data.nwc_error = Some(format!("NWCエラー: {}", error.message));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

async fn get_balance(
    nwc: NostrWalletConnectURI,
    app_data_arc: Arc<Mutex<NostrStatusAppInternal>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let req = Request {
        method: Method::GetBalance,
        params: RequestParams::GetBalance,
    };

    let client = {
        let app_data = app_data_arc.lock().unwrap();
        app_data
            .nwc_client
            .as_ref()
            .cloned()
            .ok_or("NWCクライアントが接続されていません")?
    };

    let json_req = serde_json::to_string(&req)?;
    let encrypted_req = nip04::encrypt(&nwc.secret, &nwc.public_key, &json_req)?;

    let event = EventBuilder::new(Kind::WalletConnectRequest, encrypted_req)
        .tags([Tag::public_key(nwc.public_key)])
        .sign(&Keys::new(nwc.secret))
        .await?;

    client.send_event(&event).await?;

    Ok(())
}
