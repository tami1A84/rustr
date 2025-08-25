use nostr::{Filter, Kind, PublicKey};
use nostr_sdk::{Client, SubscribeAutoCloseOptions};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::types::{ProfileMetadata, TimelinePost};

// NIP-01 プロファイルメタデータを取得する関数
pub async fn fetch_nip01_profile(
    client: &Client,
    public_key: PublicKey,
) -> Result<(ProfileMetadata, String), Box<dyn std::error::Error + Send + Sync>> {
    let nip01_filter = Filter::new()
        .authors(vec![public_key])
        .kind(Kind::Metadata)
        .limit(1);
    let nip01_filter_id = client
        .subscribe(nip01_filter, Some(SubscribeAutoCloseOptions::default()))
        .await?;

    let mut profile_json_string = String::new();
    let mut received_nip01 = false;

    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(20)) => {
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
    client.unsubscribe(&nip01_filter_id).await;

    if received_nip01 {
        let profile_metadata: ProfileMetadata = serde_json::from_str(&profile_json_string)?;
        Ok((profile_metadata, profile_json_string))
    } else {
        let default_metadata = ProfileMetadata::default();
        let default_json = serde_json::to_string_pretty(&default_metadata)?;
        Ok((default_metadata, default_json)) // プロファイルが見つからなかった場合はデフォルト値を返す
    }
}

pub async fn get_profile_metadata(
    pubkey: PublicKey,
    client: &Client,
) -> Result<ProfileMetadata, Box<dyn std::error::Error + Send + Sync>> {
    let filter = Filter::new().authors(vec![pubkey]).kind(Kind::Metadata).limit(1);

    // Get relays from client
    let relays = client.relays().await;
    let relay_urls: Vec<String> = relays.keys().map(|url| url.to_string()).collect();

    let events = client.fetch_events_from(relay_urls, filter, Duration::from_secs(5)).await?;

    if let Some(event) = events.first() {
        let metadata: ProfileMetadata = serde_json::from_str(&event.content)?;
        Ok(metadata)
    } else {
        // Return a default profile if none is found
        Ok(ProfileMetadata::default())
    }
}

pub async fn fetch_timeline_events(
    client: &Client,
    aggregator_relays: Vec<String>,
) -> Result<Vec<TimelinePost>, Box<dyn std::error::Error + Send + Sync>> {
    let mut timeline_posts = Vec::new();

    if aggregator_relays.is_empty() {
        return Ok(timeline_posts);
    }

    let timeline_filter = Filter::new()
        .kind(Kind::TextNote)
        .limit(20);

    println!("Fetching timeline from: {:?}", aggregator_relays);
    let note_events = client
        .fetch_events_from(aggregator_relays, timeline_filter, Duration::from_secs(10))
        .await?;

    if !note_events.is_empty() {
        let author_pubkeys: HashSet<PublicKey> =
            note_events.iter().map(|e| e.pubkey).collect();
        let metadata_filter = Filter::new()
            .authors(author_pubkeys.into_iter())
            .kind(Kind::Metadata);
        let metadata_events = client
            .fetch_events(metadata_filter, Duration::from_secs(5))
            .await?;
        let mut profiles: HashMap<PublicKey, ProfileMetadata> = HashMap::new();
        for event in metadata_events {
            if let Ok(metadata) = serde_json::from_str::<ProfileMetadata>(&event.content) {
                profiles.insert(event.pubkey, metadata);
            }
        }

        for event in note_events {
            let emojis = event
                .tags
                .iter()
                .filter_map(|tag| {
                    if let Some(nostr::TagStandard::Emoji { shortcode, url }) =
                        tag.as_standardized()
                    {
                        Some((shortcode.to_string(), url.to_string()))
                    } else {
                        None
                    }
                })
                .collect();

            timeline_posts.push(TimelinePost {
                id: event.id,
                kind: event.kind,
                author_pubkey: event.pubkey,
                author_metadata: profiles.get(&event.pubkey).cloned().unwrap_or_default(),
                content: event.content.clone(),
                created_at: event.created_at,
                emojis,
                tags: event.tags.to_vec(),
            });
        }
        timeline_posts.sort_by_key(|p| std::cmp::Reverse(p.created_at));
    }
    Ok(timeline_posts)
}

pub async fn fetch_notification_events(
    client: &Client,
    my_pubkey: PublicKey,
) -> Result<Vec<TimelinePost>, Box<dyn std::error::Error + Send + Sync>> {
    let mut notification_posts = Vec::new();
    let notification_relays = vec!["wss://yabu.me".to_string()];

    // Filter for replies (Kind 1) and reactions (Kind 7) that tag the user's pubkey
    let notifications_filter = Filter::new()
        .kinds(vec![Kind::TextNote, Kind::Reaction])
        .pubkey(my_pubkey)
        .limit(20);

    println!("Fetching notifications from: {:?}", notification_relays);
    let notification_events = client
        .fetch_events_from(
            notification_relays,
            notifications_filter,
            Duration::from_secs(10),
        )
        .await?;

    if !notification_events.is_empty() {
        let author_pubkeys: HashSet<PublicKey> =
            notification_events.iter().map(|e| e.pubkey).collect();

        if !author_pubkeys.is_empty() {
            let metadata_filter = Filter::new()
                .authors(author_pubkeys.into_iter())
                .kind(Kind::Metadata);

            let metadata_events = client
                .fetch_events(metadata_filter, Duration::from_secs(5))
                .await?;
            let mut profiles: HashMap<PublicKey, ProfileMetadata> = HashMap::new();
            for event in metadata_events {
                if let Ok(metadata) = serde_json::from_str::<ProfileMetadata>(&event.content) {
                    profiles.insert(event.pubkey, metadata);
                }
            }

            for event in notification_events {
                let emojis = event
                    .tags
                    .iter()
                    .filter_map(|tag| {
                        if let Some(nostr::TagStandard::Emoji { shortcode, url }) =
                            tag.as_standardized()
                        {
                            Some((shortcode.to_string(), url.to_string()))
                        } else {
                            None
                        }
                    })
                    .collect();

                notification_posts.push(TimelinePost {
                    id: event.id,
                    kind: event.kind,
                    author_pubkey: event.pubkey,
                    author_metadata: profiles.get(&event.pubkey).cloned().unwrap_or_default(),
                    content: event.content.clone(),
                    created_at: event.created_at,
                    emojis,
                    tags: event.tags.to_vec(),
                });
            }
            notification_posts.sort_by_key(|p| std::cmp::Reverse(p.created_at));
        }
    }

    Ok(notification_posts)
}

pub async fn search_events(
    search_relays: Vec<String>,
    query: String,
) -> Result<Vec<TimelinePost>, Box<dyn std::error::Error + Send + Sync>> {
    if search_relays.is_empty() || query.is_empty() {
        return Ok(Vec::new());
    }

    let client = Client::new(nostr::Keys::generate());
    for relay_url in &search_relays {
        if let Err(e) = client.add_relay(relay_url.clone()).await {
            eprintln!("Failed to add search relay {}: {}", relay_url, e);
        }
    }
    client.connect().await;

    let search_filter = Filter::new().search(query).kind(Kind::TextNote).limit(50);

    let events = client.fetch_events_from(search_relays, search_filter, Duration::from_secs(10)).await?;

    let mut timeline_posts = Vec::new();
    if !events.is_empty() {
        let author_pubkeys: HashSet<PublicKey> =
            events.iter().map(|e| e.pubkey).collect();
        let metadata_filter = Filter::new()
            .authors(author_pubkeys.into_iter())
            .kind(Kind::Metadata);

        // Fetch metadata from the same search relays
        let metadata_events = client.fetch_events(metadata_filter, Duration::from_secs(5)).await?;

        let mut profiles: HashMap<PublicKey, ProfileMetadata> = HashMap::new();
        for event in metadata_events {
            if let Ok(metadata) = serde_json::from_str::<ProfileMetadata>(&event.content) {
                profiles.insert(event.pubkey, metadata);
            }
        }

        for event in events {
            timeline_posts.push(TimelinePost {
                id: event.id,
                kind: event.kind,
                author_pubkey: event.pubkey,
                author_metadata: profiles.get(&event.pubkey).cloned().unwrap_or_default(),
                content: event.content.clone(),
                created_at: event.created_at,
                emojis: HashMap::new(), // Search results don't typically have emoji info in the same way
                tags: event.tags.to_vec(),
            });
        }
        timeline_posts.sort_by_key(|p| std::cmp::Reverse(p.created_at));
    }

    client.disconnect().await;
    Ok(timeline_posts)
}


