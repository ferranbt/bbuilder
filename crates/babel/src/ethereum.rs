use crate::{Babel, Status};
use alloy_provider::ext::NetApi;
use alloy_provider::{Provider, ProviderBuilder, RootProvider};
use alloy_rpc_types_eth::SyncStatus;
use alloy_transport::BoxTransport;
use async_trait::async_trait;

/// Ethereum node implementation (supports execution layer clients like Geth, Reth, etc.)
pub struct EthereumBabel {
    provider: RootProvider<BoxTransport>,
}

impl EthereumBabel {
    pub async fn new(rpc_url: String) -> eyre::Result<Self> {
        let provider: alloy_provider::RootProvider<alloy_transport::BoxTransport> =
            ProviderBuilder::new()
                .on_builtin(&rpc_url)
                .await
                .map_err(|e| eyre::eyre!("Failed to create provider: {}", e))?;

        Ok(Self { provider: provider })
    }
}

#[async_trait]
impl Babel for EthereumBabel {
    async fn status(&self) -> eyre::Result<Status> {
        let peers = self.provider.net_peer_count().await?;

        let (current_block_number, is_syncing, latest_block_number) =
            match self.provider.syncing().await? {
                SyncStatus::Info(sync_status) => (
                    sync_status.current_block.to::<u64>(),
                    false,
                    Some(sync_status.highest_block.to::<u64>()),
                ),
                SyncStatus::None => {
                    let block_num = self.provider.get_block_number().await?;
                    (block_num, true, None)
                }
            };

        Ok(Status {
            peers,
            current_block_number,
            is_syncing,
            latest_block_number,
            is_ready: peers > 0 && !is_syncing,
            is_healthy: true,
        })
    }
}
