use crate::Babel;
use async_trait::async_trait;
use serde::Deserialize;

/// Ethereum Beacon (Consensus Layer) node implementation (uses Beacon API)
pub struct EthereumBeaconBabel {
    api_url: String,
    client: reqwest::Client,
}

#[derive(Deserialize)]
struct PeerCountResponse {
    data: PeerCountData,
}

#[derive(Deserialize)]
struct PeerCountData {
    connected: String,
}

impl EthereumBeaconBabel {
    pub fn new(api_url: String) -> Self {
        Self {
            api_url,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Babel for EthereumBeaconBabel {
    async fn peer_count(&self) -> eyre::Result<u64> {
        // Beacon API endpoint: /eth/v1/node/peer_count
        let url = format!("{}/eth/v1/node/peer_count", self.api_url.trim_end_matches('/'));

        let response = self.client
            .get(&url)
            .send()
            .await?;

        let peer_count: PeerCountResponse = response.json().await?;

        let count = peer_count.data.connected.parse::<u64>()?;

        Ok(count)
    }
}
