use crate::utils::deserialize_string_to_u64;
use crate::{Babel, Status};
use async_trait::async_trait;
use serde::Deserialize;

/// Cosmos node implementation (uses Tendermint/CometBFT RPC)
pub struct CosmosBabel {
    rpc_url: String,
    client: reqwest::Client,
}

#[derive(Deserialize)]
struct NetInfoResult {
    #[serde(deserialize_with = "deserialize_string_to_u64")]
    n_peers: u64,
}

#[derive(Deserialize)]
struct StatusResult {
    sync_info: SyncInfo,
}

#[derive(Deserialize)]
struct SyncInfo {
    #[serde(deserialize_with = "deserialize_string_to_u64")]
    latest_block_height: u64,
    catching_up: bool,
}

#[derive(Deserialize)]
struct ApiResult<T> {
    result: T,
}

impl CosmosBabel {
    pub fn new(rpc_url: String) -> Self {
        Self {
            rpc_url,
            client: reqwest::Client::new(),
        }
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, endpoint: &str) -> eyre::Result<T> {
        let url = format!("{}/{}", self.rpc_url.trim_end_matches('/'), endpoint);

        let response = self.client.get(&url).send().await?;
        let result: ApiResult<T> = response.json().await?;

        Ok(result.result)
    }

    async fn peer_count(&self) -> eyre::Result<NetInfoResult> {
        let net_info: NetInfoResult = self.get("net_info").await?;
        Ok(net_info)
    }

    async fn sync_status(&self) -> eyre::Result<StatusResult> {
        let status: StatusResult = self.get("status").await?;
        Ok(status)
    }
}

#[async_trait]
impl Babel for CosmosBabel {
    async fn status(&self) -> eyre::Result<Status> {
        let peers = self.peer_count().await?.n_peers;
        let sync_info = self.sync_status().await?.sync_info;

        // For Cosmos, we only have the latest block height
        // When not catching up, current == latest
        Ok(Status {
            peers,
            current_block_number: sync_info.latest_block_height,
            latest_block_number: None,
            is_syncing: sync_info.catching_up,
            is_ready: peers > 0 && !sync_info.catching_up,
            is_healthy: true,
        })
    }
}
