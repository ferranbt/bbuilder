use jsonrpsee::proc_macros::rpc;
use serde::{Deserialize, Serialize};

/// Progress message sent over WebSocket
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProgressMessage {
    /// Total size is known
    SetTotal { total: u64 },
    /// Progress update
    Update { downloaded: u64, total: Option<u64> },
    /// Download finished
    Finished,
}

/// JSON-RPC API for fetcher progress tracking
#[rpc(server, client)]
pub trait FetcherProgressApi {
    /// Subscribe to download progress updates via WebSocket
    #[subscription(name = "subscribeProgress", item = ProgressMessage)]
    async fn subscribe_progress(&self);
}
