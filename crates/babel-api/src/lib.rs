use jsonrpsee::proc_macros::rpc;
use serde::{Deserialize, Serialize};

/// Status response
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Status {
    pub peers: u64,
    pub current_block_number: u64,
    pub is_syncing: bool,
    pub latest_block_number: Option<u64>,
    pub is_ready: bool,
    pub is_healthy: bool,
}

/// JSON-RPC API for Babel status
#[rpc(server, client)]
pub trait BabelApi {
    /// Get comprehensive status
    #[method(name = "status")]
    async fn status(&self) -> Result<Status, jsonrpsee::types::ErrorObjectOwned>;
}
