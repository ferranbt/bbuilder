use crate::{metrics::BabelMetrics, Babel, Status};
use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Router};
use metrics_exporter_prometheus::PrometheusHandle;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
struct AppState {
    babel: Arc<dyn Babel>,
    metrics: BabelMetrics,
    cached_status: Arc<RwLock<Option<Status>>>,
}

pub struct BabelServer {
    state: AppState,
    prometheus_handle: PrometheusHandle,
}

impl BabelServer {
    pub fn new(babel: impl Babel + 'static, nodename: Option<String>) -> Self {
        // Setup Prometheus exporter
        let builder = metrics_exporter_prometheus::PrometheusBuilder::new();
        let prometheus_handle = builder.install_recorder().unwrap_or_else(|_| {
            // In tests, recorder might already be installed
            #[cfg(test)]
            {
                metrics_exporter_prometheus::PrometheusBuilder::new()
                    .build_recorder()
                    .handle()
            }
            #[cfg(not(test))]
            panic!("failed to install Prometheus recorder")
        });

        let metrics = if let Some(nodename) = nodename {
            BabelMetrics::new_with_labels(&[("nodename", nodename)])
        } else {
            BabelMetrics::default()
        };

        let state = AppState {
            babel: Arc::new(babel),
            metrics,
            cached_status: Arc::new(RwLock::new(None)),
        };

        Self {
            state,
            prometheus_handle,
        }
    }

    pub fn router(self) -> Router {
        let prometheus_handle = self.prometheus_handle.clone();

        Router::new()
            .route("/ready", get(ready_handler))
            .route("/healthy", get(healthy_handler))
            .route("/metrics", get(move || metrics_handler(prometheus_handle)))
            .with_state(self.state)
    }

    pub fn cached_status(&self) -> Arc<RwLock<Option<Status>>> {
        self.state.cached_status.clone()
    }

    pub async fn serve(self, addr: &str) -> eyre::Result<()> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        tracing::info!("Babel server listening on {}", addr);

        // Spawn periodic status polling task
        let state = self.state.clone();
        tokio::spawn(async move {
            Self::status_polling_loop(state).await;
        });

        // Serve with graceful shutdown
        axum::serve(listener, self.router())
            .with_graceful_shutdown(shutdown_signal())
            .await?;

        Ok(())
    }

    async fn status_polling_loop(state: AppState) {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(1));

        loop {
            interval.tick().await;

            match state.babel.status().await {
                Ok(status) => {
                    // Update metrics
                    state.metrics.update_metrics(&status);

                    // Update cached status
                    *state.cached_status.write().await = Some(status);
                }
                Err(e) => {
                    tracing::error!("Failed to fetch status: {}", e);
                }
            }
        }
    }
}

async fn ready_handler(State(state): State<AppState>) -> Result<StatusCode, AppError> {
    let status = state
        .cached_status
        .read()
        .await
        .clone()
        .ok_or_else(|| eyre::eyre!("Status not yet available"))?;

    if status.is_ready {
        Ok(StatusCode::OK)
    } else {
        Ok(StatusCode::SERVICE_UNAVAILABLE)
    }
}

async fn healthy_handler(State(state): State<AppState>) -> Result<StatusCode, AppError> {
    let status = state
        .cached_status
        .read()
        .await
        .clone()
        .ok_or_else(|| eyre::eyre!("Status not yet available"))?;

    if status.is_healthy {
        Ok(StatusCode::OK)
    } else {
        Ok(StatusCode::SERVICE_UNAVAILABLE)
    }
}

async fn metrics_handler(prometheus_handle: PrometheusHandle) -> String {
    prometheus_handle.render()
}

struct AppError(eyre::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Error: {}", self.0),
        )
            .into_response()
    }
}

impl<E> From<E> for AppError
where
    E: Into<eyre::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            tracing::info!("Received Ctrl+C, shutting down gracefully");
        },
        _ = terminate => {
            tracing::info!("Received SIGTERM, shutting down gracefully");
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Babel, Status};
    use async_trait::async_trait;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    // Mock Babel implementation for testing
    struct MockBabel {
        status: Arc<RwLock<Status>>,
    }

    impl MockBabel {
        fn new(status: Status) -> Self {
            Self {
                status: Arc::new(RwLock::new(status)),
            }
        }
    }

    #[async_trait]
    impl Babel for MockBabel {
        async fn status(&self) -> eyre::Result<Status> {
            Ok(self.status.read().await.clone())
        }
    }

    struct TestContext {
        rest_client: axum_test::TestServer,
        rpc_url: String,
        _rpc_handle: jsonrpsee::server::ServerHandle,
    }

    async fn setup_server_with_status(status: Status) -> TestContext {
        let mock_babel = MockBabel::new(status);
        let server = BabelServer::new(mock_babel, None);

        let state = server.state.clone();
        tokio::spawn(async move {
            BabelServer::status_polling_loop(state).await;
        });

        let cached_status = server.cached_status();
        let rpc_addr = "127.0.0.1:0".parse().unwrap();
        let (rpc_handle, rpc_local_addr) = crate::rpc::start_rpc_server(rpc_addr, cached_status)
            .await
            .unwrap();
        let rpc_url = format!("http://{}", rpc_local_addr);

        let router = server.router();
        let rest_client = axum_test::TestServer::new(router).unwrap();

        // Wait for status polling to run at least once
        tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;

        TestContext {
            rest_client,
            rpc_url,
            _rpc_handle: rpc_handle,
        }
    }

    #[tokio::test]
    async fn test_healthy_endpoint() {
        // Test healthy status
        let ctx = setup_server_with_status(Status {
            is_healthy: true,
            ..Default::default()
        })
        .await;

        let response = ctx.rest_client.get("/healthy").await;
        assert_eq!(response.status_code(), axum::http::StatusCode::OK);

        // Test unhealthy status
        let ctx = setup_server_with_status(Status::default()).await;

        let response = ctx.rest_client.get("/healthy").await;
        assert_eq!(
            response.status_code(),
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[tokio::test]
    async fn test_ready_endpoint() {
        // Test ready status
        let ctx = setup_server_with_status(Status {
            is_ready: true,
            ..Default::default()
        })
        .await;

        let response = ctx.rest_client.get("/ready").await;
        assert_eq!(response.status_code(), axum::http::StatusCode::OK);

        // Test not ready status
        let ctx = setup_server_with_status(Status::default()).await;

        let response = ctx.rest_client.get("/ready").await;
        assert_eq!(
            response.status_code(),
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[tokio::test]
    async fn test_rpc_status() {
        use babel_api::BabelApiClient;
        use jsonrpsee::http_client::HttpClientBuilder;

        let ctx = setup_server_with_status(Status {
            peers: 10,
            current_block_number: 500,
            is_syncing: true,
            latest_block_number: Some(1000),
            is_healthy: true,
            ..Default::default()
        })
        .await;

        let client = HttpClientBuilder::default().build(&ctx.rpc_url).unwrap();

        let status = client.status().await.unwrap();
        assert_eq!(status.peers, 10);
        assert_eq!(status.current_block_number, 500);
        assert_eq!(status.is_syncing, true);
        assert_eq!(status.latest_block_number, Some(1000));
        assert_eq!(status.is_ready, false);
        assert_eq!(status.is_healthy, true);
    }
}
