use crate::utils::deserialize_string_to_u64;
use crate::{Babel, Status};
use async_trait::async_trait;
use serde::Deserialize;

/// Ethereum Beacon (Consensus Layer) node implementation (uses Beacon API)
pub struct EthereumBeaconBabel {
    api_url: String,
    client: reqwest::Client,
}

#[derive(Deserialize)]
struct PeerCountData {
    #[serde(deserialize_with = "deserialize_string_to_u64")]
    connected: u64,
}

#[derive(Deserialize)]
struct SyncingData {
    #[serde(deserialize_with = "deserialize_string_to_u64")]
    head_slot: u64,
    #[serde(deserialize_with = "deserialize_string_to_u64")]
    sync_distance: u64,
}

#[derive(Deserialize)]
struct ApiResult<T> {
    data: T,
}

impl EthereumBeaconBabel {
    pub fn new(api_url: String) -> Self {
        Self {
            api_url,
            client: reqwest::Client::new(),
        }
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, endpoint: &str) -> eyre::Result<T> {
        let url = format!("{}/{}", self.api_url.trim_end_matches('/'), endpoint);
        let response = self.client.get(&url).send().await?;
        let result: ApiResult<T> = response.json().await?;
        Ok(result.data)
    }

    async fn peer_count(&self) -> eyre::Result<PeerCountData> {
        let peer_count: PeerCountData = self.get("eth/v1/node/peer_count").await?;
        Ok(peer_count)
    }

    async fn syncing_info(&self) -> eyre::Result<SyncingData> {
        let syncing: SyncingData = self.get("eth/v1/node/syncing").await?;
        Ok(syncing)
    }
}

#[async_trait]
impl Babel for EthereumBeaconBabel {
    async fn status(&self) -> eyre::Result<Status> {
        let peers = self.peer_count().await?.connected;
        let syncing_info = self.syncing_info().await?;

        let latest_block_number = if syncing_info.sync_distance != 0 {
            Some(syncing_info.head_slot + syncing_info.sync_distance)
        } else {
            None
        };

        Ok(Status {
            peers,
            current_block_number: syncing_info.head_slot,
            is_syncing: latest_block_number.is_some(),
            latest_block_number,
            is_ready: peers > 0 && latest_block_number.is_none(),
            is_healthy: true,
        })
    }
}
