use nostr::{nips::nip19::ToBech32, Filter, Kind, Keys, PublicKey, Tag};
use nostr_sdk::{Client, Options, SubscribeAutoCloseOptions};
use std::collections::HashSet;
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

    let mut current_connected_relays = Vec::new();
    let mut connected_relays_map: std::collections::HashMap<String, nostr_sdk::RelayStatus> = std::collections::HashMap::new();

    if received_nip65_event && !nip65_relays.is_empty() {
        status_log.push_str("\nNIP-65で検出されたリレーに接続中...\n");
        let _ = client.remove_all_relays().await;

        for (url, policy) in nip65_relays.iter() {
            if policy.as_deref() == Some("write") || policy.is_none() {
                if let Err(e) = client.add_relay(url.as_str()).await {
                    status_log.push_str(&format!("  リレー追加失敗: {} - エラー: {}\n", url, e));
                } else {
                    status_log.push_str(&format!("  リレー追加: {}\n", url));
                }
            }
        }
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
                }
            }
        }
    }

    client.connect().await;
    tokio::time::sleep(Duration::from_secs(2)).await; // 接続安定待ち

    let relays = client.relays().await;
    if relays.is_empty() {
        return Err("接続できるリレーがありません。".into());
    }

    status_log.push_str(&format!("\n--- 現在接続中のリレー ({}件) ---\n", relays.len()));
    for (url, relay) in relays.iter() {
        let status = relay.status().await;
        status_log.push_str(&format!("  - {}: {:?}\n", url, status));
        current_connected_relays.push(format!("- {}: {:?}", url, status));
        connected_relays_map.insert(url.to_string(), status);
    }
    status_log.push_str("---------------------------------\n");


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

// NIP-02 コンタクトリストを更新する関数
pub async fn update_contact_list(
    client: &Client,
    keys: &Keys,
    pubkey_to_modify: PublicKey,
    follow: bool, // trueでフォロー、falseでアンフォロー
) -> Result<HashSet<PublicKey>, Box<dyn std::error::Error + Send + Sync>> {
    // 1. 現在のコンタクトリストを取得
    let filter = Filter::new().authors(vec![keys.public_key()]).kind(Kind::ContactList).limit(1);
    let events = client.get_events_of(vec![filter], Some(Duration::from_secs(10))).await?;

    let mut current_tags: Vec<Tag> = if let Some(event) = events.first() {
        event.tags.clone()
    } else {
        Vec::new()
    };

    let mut followed_pubkeys: HashSet<PublicKey> = current_tags.iter().filter_map(|tag| {
        if let Tag::PublicKey { public_key, .. } = tag {
            Some(*public_key)
        } else {
            None
        }
    }).collect();

    // 2. フォローリストを変更
    if follow {
        if followed_pubkeys.insert(pubkey_to_modify) {
            current_tags.push(Tag::public_key(pubkey_to_modify));
            println!("Following {}", pubkey_to_modify.to_bech32()?);
        }
    } else {
        if followed_pubkeys.remove(&pubkey_to_modify) {
            current_tags.retain(|tag| {
                if let Tag::PublicKey { public_key, .. } = tag {
                    *public_key != pubkey_to_modify
                } else {
                    true
                }
            });
            println!("Unfollowing {}", pubkey_to_modify.to_bech32()?);
        }
    }

    // 3. 新しいコンタクトリストイベントを作成して送信
    use nostr::EventBuilder;
    let event = EventBuilder::new(Kind::ContactList, "", current_tags).to_event(keys)?;
    client.send_event(event).await?;

    println!("Contact list updated successfully.");

    Ok(followed_pubkeys)
}


