use serde::Deserialize;
use spec::{Arg, Babel, ComputeResource, Deployment, DeploymentExtension, Manifest, Pod, Spec, Volume};

#[derive(Default, Clone)]
pub enum Chains {
    #[default]
    Mainnet,
}

#[derive(Default, Deserialize)]
pub struct TempoDeploymentInput {
    pub tempo: Tempo,
}

#[derive(Default, Deserialize)]
pub struct TempoDeployment {}

impl Deployment for TempoDeployment {
    type Input = TempoDeploymentInput;
    type Chains = Chains;

    fn manifest(&self, chain: Chains, input: TempoDeploymentInput) -> eyre::Result<Manifest> {
        let mut manifest = Manifest::new("tempo".to_string());

        let tempo_pod = input.tempo.spec(chain)?;
        manifest.add_spec("tempo".to_string(), tempo_pod);

        Ok(manifest)
    }
}

#[derive(Default, Deserialize)]
pub struct Tempo {}

impl ComputeResource for Tempo {
    type Chains = Chains;

    fn spec(&self, _chain: Chains) -> eyre::Result<Pod> {
        let node = Spec::builder()
            .image("ghcr.io/tempo-xyz/tempo")
            .tag("1.0.1")
            .volume(Volume {
                name: "data".to_string(),
                path: "/data".to_string(),
            })
            .arg("node")
            .arg("-vvv")
            .arg("--follow")
            .arg2("--datadir", "/data")
            .arg2("--port", "30303")
            .arg2("--discovery.addr", "0.0.0.0")
            .arg2("--discovery.port", "30303")
            .arg("--http")
            .arg2("--http.addr", "0.0.0.0")
            .arg2(
                "--http.port",
                Arg::Port {
                    name: "http".to_string(),
                    preferred: 8545,
                },
            )
            .arg2("--http.api", "eth,net,web3,txpool,trace")
            .arg("--ws")
            .arg2("--ws.addr", "0.0.0.0")
            .arg2(
                "--ws.port",
                Arg::Port {
                    name: "ws".to_string(),
                    preferred: 8546,
                },
            )
            .arg2("--ws.api", "eth,net,web3,txpool,trace")
            .arg2(
                "--metrics",
                Arg::Port {
                    name: "metrics".to_string(),
                    preferred: 9000,
                },
            )
            .with_babel(Babel::new(
                "ethereum",
                Arg::Ref {
                    name: "tempo-node".to_string(),
                    port: "http".to_string(),
                },
            ));

        Ok(Pod::default().with_spec("node", node))
    }
}
