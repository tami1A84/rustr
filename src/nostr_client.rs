use nostr::{Filter, Kind, Keys, PublicKey, Tag};
use nostr_sdk::{Client, Options, SubscribeAutoCloseOptions};
use std::time::Duration;

use crate::ProfileMetadata;

// NIP-65とフォールバックを考慮したリレー接続関数
pub async fn connect_to_relays_with_nip65(
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

// フォローしているユーザーのリレーリスト(kind:10002)を取得する関数
pub async fn fetch_relays_for_followed_users(
    discover_client: &Client,
    pubkeys: Vec<PublicKey>,
) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
    if pubkeys.is_empty() {
        return Ok(Vec::new());
    }

    let filter = Filter::new().authors(pubkeys).kind(Kind::RelayList);

    let events = discover_client.get_events_of(vec![filter], Some(Duration::from_secs(10))).await?;

    let mut relay_urls = std::collections::HashSet::new();
    for event in events {
        for tag in &event.tags {
            if let Tag::RelayMetadata(url, policy) = tag {
                match policy {
                    Some(nostr::RelayMetadata::Write) | None => {
                        relay_urls.insert(url.to_string());
                    }
                    Some(nostr::RelayMetadata::Read) => {}
                }
            }
        }
    }

    Ok(relay_urls.into_iter().collect())
}

// NIP-01 プロファイルメタデータを取得する関数
pub async fn fetch_nip01_profile(client: &Client, public_key: PublicKey) -> Result<(ProfileMetadata, String), Box<dyn std::error::Error + Send + Sync>> {
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


