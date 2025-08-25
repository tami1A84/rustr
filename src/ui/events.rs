use std::collections::HashSet;
use std::time::Duration;
use nostr::{Filter, Keys, Kind, PublicKey};
use nostr_sdk::{Client, SubscribeAutoCloseOptions};

use crate::{
    types::{ProfileMetadata, RelayConfig, TimelinePost},
    cache_db::{LmdbCache, DB_FOLLOWED, DB_PROFILES, DB_TIMELINE, DB_NOTIFICATIONS},
    nostr_client::{fetch_nip01_profile, fetch_timeline_events, fetch_notification_events}
};

pub struct FreshData {
    pub followed_pubkeys: HashSet<PublicKey>,
    pub timeline_posts: Vec<TimelinePost>,
    pub notification_posts: Vec<TimelinePost>,
    pub profile_metadata: ProfileMetadata,
    pub profile_json_string: String,
}

pub async fn refresh_all_data(
    client: &Client,
    keys: &Keys,
    cache_db: &LmdbCache,
    relay_config: &RelayConfig,
) -> Result<FreshData, Box<dyn std::error::Error + Send + Sync>> {
    let pubkey_hex = keys.public_key().to_string();

    println!("Refreshing all data from network...");

    // Fetch NIP-02 contact list
    let nip02_filter = Filter::new()
        .authors(vec![keys.public_key()])
        .kind(Kind::ContactList)
        .limit(1);
    let nip02_filter_id = client
        .subscribe(nip02_filter, Some(SubscribeAutoCloseOptions::default()))
        .await?;

    let mut followed_pubkeys = HashSet::new();
    let mut received_nip02 = false;

    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(10)) => {}
        _ = async {
            let mut notifications = client.notifications();
            while let Ok(notification) = notifications.recv().await {
                if let nostr_sdk::RelayPoolNotification::Event { event, .. } = notification {
                    if event.kind == Kind::ContactList && event.pubkey == keys.public_key() {
                        for tag in event.tags.iter() {
                            if let Some(nostr::TagStandard::PublicKey { public_key, .. }) = tag.as_standardized() {
                                followed_pubkeys.insert(*public_key);
                            }
                        }
                        received_nip02 = true;
                        break;
                    }
                }
            }
        } => {},
    }
    client.unsubscribe(&nip02_filter_id).await;

    if received_nip02 {
        cache_db.write_cache(DB_FOLLOWED, &pubkey_hex, &followed_pubkeys)?;
    }

    // Fetch timeline, notifications, and profile in parallel
    let (timeline_result, notification_result, profile_result) = tokio::join!(
        fetch_timeline_events(client, relay_config.aggregator.clone()),
        fetch_notification_events(client, keys.public_key()),
        fetch_nip01_profile(client, keys.public_key())
    );

    let timeline_posts = timeline_result?;
    cache_db.write_cache(DB_TIMELINE, &pubkey_hex, &timeline_posts)?;

    let notification_posts = notification_result?;
    cache_db.write_cache(DB_NOTIFICATIONS, &pubkey_hex, &notification_posts)?;

    let (profile_metadata, profile_json_string) = profile_result?;
    cache_db.write_cache(DB_PROFILES, &pubkey_hex, &profile_metadata)?;

    println!("Finished refreshing all data.");

    Ok(FreshData {
        followed_pubkeys,
        timeline_posts,
        notification_posts,
        profile_metadata,
        profile_json_string,
    })
}
