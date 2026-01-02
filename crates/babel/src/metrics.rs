use metrics_derive::Metrics;

#[derive(Metrics, Clone)]
#[metrics(scope = "babel")]
pub struct BabelMetrics {
    /// Total number of peers connected
    #[metric(describe = "Total number of connected peers")]
    pub peers_total: metrics::Gauge,
}
