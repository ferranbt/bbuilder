use crate::Status;
use metrics_derive::Metrics;

#[derive(Metrics, Clone)]
#[metrics(scope = "babel")]
pub struct BabelMetrics {
    /// Total number of peers connected
    #[metric(describe = "Total number of connected peers")]
    pub peers_total: metrics::Gauge,

    /// Current block number
    #[metric(describe = "Current block number")]
    pub current_block_number: metrics::Gauge,

    /// Is the node syncing
    #[metric(describe = "Whether the node is currently syncing (1 = syncing, 0 = not syncing)")]
    pub is_syncing: metrics::Gauge,

    /// Latest block number (if syncing)
    #[metric(describe = "Latest block number when syncing")]
    pub latest_block_number: metrics::Gauge,

    /// Is the node ready
    #[metric(describe = "Whether the node is ready (1 = ready, 0 = not ready)")]
    pub is_ready: metrics::Gauge,

    /// Is the node healthy
    #[metric(describe = "Whether the node is healthy (1 = healthy, 0 = not healthy)")]
    pub is_healthy: metrics::Gauge,
}

impl BabelMetrics {
    pub fn update_metrics(&self, status: &Status) {
        self.peers_total.set(status.peers as f64);
        self.current_block_number
            .set(status.current_block_number as f64);
        self.is_syncing
            .set(if status.is_syncing { 1.0 } else { 0.0 });
        self.latest_block_number
            .set(status.latest_block_number.map(|n| n as f64).unwrap_or(0.0));
        self.is_ready.set(if status.is_ready { 1.0 } else { 0.0 });
        self.is_healthy
            .set(if status.is_healthy { 1.0 } else { 0.0 });
    }
}
