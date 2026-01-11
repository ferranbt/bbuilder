use fetcher_api::{FetcherProgressApiServer, ProgressMessage};
use jsonrpsee::core::SubscriptionResult;
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::ProgressTracker;

/// Progress tracker that sends updates to a WebSocket broadcast channel
pub struct WebSocketProgressTracker {
    total: Option<u64>,
    tx: Arc<broadcast::Sender<ProgressMessage>>,
}

impl WebSocketProgressTracker {
    pub fn new(tx: Arc<broadcast::Sender<ProgressMessage>>) -> Self {
        Self { total: None, tx }
    }
}

impl ProgressTracker for WebSocketProgressTracker {
    fn set_total(&mut self, total: u64) {
        self.total = Some(total);
        let _ = self.tx.send(ProgressMessage::SetTotal { total });
    }

    fn update(&mut self, downloaded: u64) {
        let _ = self.tx.send(ProgressMessage::Update {
            downloaded,
            total: self.total,
        });
    }

    fn finish(&mut self) {
        let _ = self.tx.send(ProgressMessage::Finished);
    }
}

/// Implementation of the Fetcher Progress RPC server
pub struct FetcherProgressServer {
    progress_tx: Arc<broadcast::Sender<ProgressMessage>>,
}

impl FetcherProgressServer {
    pub fn new(progress_tx: Arc<broadcast::Sender<ProgressMessage>>) -> Self {
        Self { progress_tx }
    }
}

#[jsonrpsee::core::async_trait]
impl FetcherProgressApiServer for FetcherProgressServer {
    async fn subscribe_progress(
        &self,
        pending: jsonrpsee::PendingSubscriptionSink,
    ) -> SubscriptionResult {
        let sink = match pending.accept().await {
            Ok(sink) => sink,
            Err(err) => {
                tracing::error!("failed to accept subscription: {err}");
                return Err(err.to_string().into());
            }
        };

        let mut rx = self.progress_tx.subscribe();

        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(msg) => {
                        let subscription_msg =
                            jsonrpsee::SubscriptionMessage::from_json(&msg).unwrap();
                        let result = sink.send(subscription_msg).await;
                        if result.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Progress subscriber lagged by {} messages", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
            tracing::debug!("Progress subscription ended");
        });

        Ok(())
    }
}

/// Start the WebSocket progress server
pub async fn start_progress_server(
    addr: std::net::SocketAddr,
    progress_tx: Arc<broadcast::Sender<ProgressMessage>>,
) -> Result<jsonrpsee::server::ServerHandle, Box<dyn std::error::Error>> {
    use jsonrpsee::server::Server;

    let server = Server::builder().build(addr).await?;

    let progress_server = FetcherProgressServer::new(progress_tx);
    let handle = server.start(progress_server.into_rpc());

    tracing::info!("Progress WebSocket server listening on {}", addr);

    Ok(handle)
}
