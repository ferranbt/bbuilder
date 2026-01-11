use async_trait::async_trait;

// Re-export Status from babel-api
pub use babel_api::Status;

/// Core trait for blockchain node health checks
#[async_trait]
pub trait Babel: Send + Sync {
    /// Get comprehensive status
    async fn status(&self) -> eyre::Result<Status>;
}

pub mod cosmos;
pub mod ethereum;
pub mod ethereum_beacon;
pub mod metrics;
pub mod rpc;
pub mod server;
mod utils;

pub use cosmos::CosmosBabel;
pub use ethereum::EthereumBabel;
pub use ethereum_beacon::EthereumBeaconBabel;
pub use server::BabelServer;
