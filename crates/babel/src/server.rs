use crate::{metrics::BabelMetrics, Babel, Status};
use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Json, Router};
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
        let prometheus_handle = builder
            .install_recorder()
            .expect("failed to install Prometheus recorder");

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
            .route("/status", get(status_handler))
            .route("/metrics", get(move || metrics_handler(prometheus_handle)))
            .with_state(self.state)
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
                    state.metrics.peers_total.set(status.peers as f64);

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

async fn status_handler(State(state): State<AppState>) -> Result<Json<Status>, AppError> {
    // Return cached status
    let status = state
        .cached_status
        .read()
        .await
        .clone()
        .ok_or_else(|| eyre::eyre!("Status not yet available"))?;

    Ok(Json(status))
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
