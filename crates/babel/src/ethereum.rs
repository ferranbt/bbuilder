use crate::Babel;
use async_trait::async_trait;
use serde_json::json;

/// Ethereum node implementation (supports execution layer clients like Geth, Reth, etc.)
pub struct EthereumBabel {
    rpc_url: String,
    client: reqwest::Client,
}

impl EthereumBabel {
    pub fn new(rpc_url: String) -> Self {
        Self {
            rpc_url,
            client: reqwest::Client::new(),
        }
    }

    async fn rpc_call(&self, method: &str, params: serde_json::Value) -> eyre::Result<serde_json::Value> {
        let response = self.client
            .post(&self.rpc_url)
            .json(&json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params,
                "id": 1
            }))
            .send()
            .await?;

        let json: serde_json::Value = response.json().await?;

        if let Some(error) = json.get("error") {
            return Err(eyre::eyre!("RPC error: {}", error));
        }

        json.get("result")
            .cloned()
            .ok_or_else(|| eyre::eyre!("No result in RPC response"))
    }
}

#[async_trait]
impl Babel for EthereumBabel {
    async fn peer_count(&self) -> eyre::Result<u64> {
        let result = self.rpc_call("net_peerCount", json!([])).await?;

        // Result is a hex string like "0x19"
        let hex_str = result.as_str()
            .ok_or_else(|| eyre::eyre!("Expected string result"))?;

        // Remove "0x" prefix and parse
        let count = u64::from_str_radix(hex_str.trim_start_matches("0x"), 16)?;

        Ok(count)
    }
}
