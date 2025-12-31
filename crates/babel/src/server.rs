use crate::{Babel, HealthStatus};
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use std::sync::Arc;

pub struct BabelServer {
    babel: Arc<dyn Babel>,
}

impl BabelServer {
    pub fn new(babel: impl Babel + 'static) -> Self {
        Self {
            babel: Arc::new(babel),
        }
    }

    pub fn router(self) -> Router {
        Router::new()
            .route("/health", get(health_handler))
            .route("/peers", get(peers_handler))
            .with_state(self.babel)
    }

    pub async fn serve(self, addr: &str) -> eyre::Result<()> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        tracing::info!("Babel server listening on {}", addr);

        axum::serve(listener, self.router())
            .await?;

        Ok(())
    }
}

async fn health_handler(
    State(babel): State<Arc<dyn Babel>>,
) -> Result<Json<HealthStatus>, AppError> {
    let status = babel.health_status().await?;
    Ok(Json(status))
}

async fn peers_handler(
    State(babel): State<Arc<dyn Babel>>,
) -> Result<Json<PeersResponse>, AppError> {
    let count = babel.peer_count().await?;
    Ok(Json(PeersResponse { peers: count }))
}

#[derive(serde::Serialize)]
struct PeersResponse {
    peers: u64,
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
