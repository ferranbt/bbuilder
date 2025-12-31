use crate::Babel;
use async_trait::async_trait;
use serde::Deserialize;

/// Cosmos node implementation (uses Tendermint/CometBFT RPC)
pub struct CosmosBabel {
    rpc_url: String,
    client: reqwest::Client,
}

#[derive(Deserialize)]
struct NetInfoResponse {
    result: NetInfoResult,
}

#[derive(Deserialize)]
struct NetInfoResult {
    n_peers: String,
}

impl CosmosBabel {
    pub fn new(rpc_url: String) -> Self {
        Self {
            rpc_url,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Babel for CosmosBabel {
    async fn peer_count(&self) -> eyre::Result<u64> {
        // Cosmos/Tendermint uses REST endpoint: /net_info
        let url = format!("{}/net_info", self.rpc_url.trim_end_matches('/'));

        let response = self.client
            .get(&url)
            .send()
            .await?;

        let net_info: NetInfoResponse = response.json().await?;

        let count = net_info.result.n_peers.parse::<u64>()?;

        Ok(count)
    }
}
