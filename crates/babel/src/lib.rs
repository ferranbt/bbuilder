use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Core trait for blockchain node health checks
#[async_trait]
pub trait Babel: Send + Sync {
    /// Get the number of connected peers for this node
    async fn peer_count(&self) -> eyre::Result<u64>;

    /// Get comprehensive health status
    async fn health_status(&self) -> eyre::Result<HealthStatus> {
        Ok(HealthStatus {
            peers: self.peer_count().await?,
        })
    }
}

/// Health status response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub peers: u64,
}

pub mod cosmos;
pub mod ethereum;
pub mod ethereum_beacon;
pub mod server;

pub use cosmos::CosmosBabel;
pub use ethereum::EthereumBabel;
pub use ethereum_beacon::EthereumBeaconBabel;
pub use server::BabelServer;
