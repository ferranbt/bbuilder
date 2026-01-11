use babel_api::{BabelApiServer, Status};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Implementation of the Babel RPC server
pub struct BabelRpcServer {
    cached_status: Arc<RwLock<Option<Status>>>,
}

impl BabelRpcServer {
    pub fn new(cached_status: Arc<RwLock<Option<Status>>>) -> Self {
        Self { cached_status }
    }
}

#[jsonrpsee::core::async_trait]
impl BabelApiServer for BabelRpcServer {
    async fn status(&self) -> Result<Status, jsonrpsee::types::ErrorObjectOwned> {
        self.cached_status
            .read()
            .await
            .clone()
            .ok_or_else(|| {
                jsonrpsee::types::ErrorObjectOwned::owned(
                    1,
                    "Status not yet available",
                    None::<()>,
                )
            })
    }
}

/// Start the RPC server
pub async fn start_rpc_server(
    addr: std::net::SocketAddr,
    cached_status: Arc<RwLock<Option<Status>>>,
) -> eyre::Result<(jsonrpsee::server::ServerHandle, std::net::SocketAddr)> {
    use jsonrpsee::server::Server;

    let server = Server::builder().build(addr).await?;
    let local_addr = server.local_addr()?;

    let rpc_server = BabelRpcServer::new(cached_status);
    let handle = server.start(rpc_server.into_rpc());

    tracing::info!("Babel RPC server listening on {}", local_addr);

    Ok((handle, local_addr))
}
