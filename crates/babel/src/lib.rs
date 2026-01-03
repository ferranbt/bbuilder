use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Core trait for blockchain node health checks
#[async_trait]
pub trait Babel: Send + Sync {
    /// Get comprehensive status
    async fn status(&self) -> eyre::Result<Status>;
}

/// Status response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub peers: u64,
    pub current_block_number: u64,
    pub is_syncing: bool,
    pub latest_block_number: Option<u64>,
    pub is_ready: bool,
    pub is_healthy: bool,
}

pub mod cosmos;
pub mod ethereum;
pub mod ethereum_beacon;
pub mod metrics;
pub mod server;
mod utils;

pub use cosmos::CosmosBabel;
pub use ethereum::EthereumBabel;
pub use ethereum_beacon::EthereumBeaconBabel;
pub use server::BabelServer;
