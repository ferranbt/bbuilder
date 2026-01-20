use spec::{Dep, Deployment, Manifest};

pub use catalog_berachain::BerachainDeployment;
pub use catalog_ethereum::EthereumDeployment;
pub use catalog_polygon::PolygonDeployment;
pub use catalog_tempo::TempoDeployment;

pub fn apply(dep: Dep) -> eyre::Result<Manifest> {
    match dep.module.as_str() {
        "ethereum" => EthereumDeployment::default().apply(&dep),
        "polygon" => PolygonDeployment::default().apply(&dep),
        "berachain" => BerachainDeployment::default().apply(&dep),
        "tempo" => TempoDeployment::default().apply(&dep),
        _ => Err(eyre::eyre!("Unknown module: {}", dep.module)),
    }
}
