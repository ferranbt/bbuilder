use serde::{Deserialize, Serialize};
use spec::{
    Arg, Artifacts, Babel, ComputeResource, Deployment, DeploymentExtension, Manifest, Pod, Spec,
    Volume,
};
use template::Template;

fn bera_chain_file(chain_id: u64, path: &str) -> String {
    format!(
        "https://raw.githubusercontent.com/berachain/beacon-kit/refs/heads/main/testing/networks/{}/{}",
        chain_id, path,
    )
}

fn get_network_file(chain_id: u64, filename: &str) -> &'static str {
    match (chain_id, filename) {
        (80069, "el-bootnodes.txt") => include_str!("config/networks/80069/el-bootnodes.txt"),
        (80069, "el-peers.txt") => include_str!("config/networks/80069/el-peers.txt"),
        (80094, "el-bootnodes.txt") => include_str!("config/networks/80094/el-bootnodes.txt"),
        (80094, "el-peers.txt") => include_str!("config/networks/80094/el-peers.txt"),
        _ => panic!("Unknown network file: {} for chain {}", filename, chain_id),
    }
}

#[derive(Default, Clone)]
pub enum Chains {
    #[default]
    Mainnet,
    Bepolia,
}

impl Chains {
    fn chain_id(&self) -> u64 {
        match self {
            Chains::Mainnet => 80094,
            Chains::Bepolia => 80069,
        }
    }
}

#[derive(Default, Deserialize)]
pub struct BerachainDeploymentInput {
    pub beacon_kit: BeaconKit,
    pub bera_reth: BeraReth,
}

#[derive(Default, Deserialize)]
pub struct BerachainDeployment {}

impl Deployment for BerachainDeployment {
    type Input = BerachainDeploymentInput;
    type Chains = Chains;

    fn manifest(&self, chain: Chains, input: BerachainDeploymentInput) -> eyre::Result<Manifest> {
        let mut manifest = Manifest::new("berachain".to_string());

        let beaconkit_pod = input.beacon_kit.spec(chain.clone())?;
        manifest.add_spec("beaconkit".to_string(), beaconkit_pod);

        let berareth_pod = input.bera_reth.spec(chain)?;
        manifest.add_spec("berareth".to_string(), berareth_pod);

        Ok(manifest)
    }
}


#[derive(Template, Serialize)]
#[template(path = "config/config.toml")]
struct BeaconKitConfigFile {}

#[derive(Template, Serialize)]
#[template(path = "config/app.toml")]
struct BeaconKitAppFile {
    rpc_dial_url: String,
}

#[derive(Default, Deserialize)]
pub struct BeaconKit {}

impl ComputeResource for BeaconKit {
    type Chains = Chains;

    fn spec(&self, chain: Chains) -> eyre::Result<Pod> {
        let chain_id = chain.chain_id();

        let config_file = BeaconKitConfigFile {};
        let app_file = BeaconKitAppFile {
            rpc_dial_url: "http://localhost:8551".to_string(),
        };

        let bootnodes = get_network_file(chain_id, "el-bootnodes.txt");
        let peers = get_network_file(chain_id, "el-peers.txt");

        let node = Spec::builder()
            .image("ghcr.io/berachain/beacon-kit")
            .tag("v1.3.4-rc1")
            .volume(Volume {
                name: "data".to_string(),
                path: "/data".to_string(),
            })
            .arg("start")
            .arg2("--home", "/data")
            .arg2(
                "--api.address",
                Arg::Port {
                    name: "http".to_string(),
                    preferred: 1317,
                },
            )
            .env("EL_BOOTNODES", bootnodes.trim())
            .env("EL_PEERS", peers.trim())
            .with_babel(Babel::new(
                "cosmos",
                Arg::Ref {
                    name: "beaconkit-node".to_string(),
                    port: "http".to_string(),
                },
            ))
            .artifact(Artifacts::File(spec::File {
                name: "genesis".to_string(),
                target_path: "/data/genesis.json".to_string(),
                content: bera_chain_file(chain_id, "genesis.json"),
            }))
            .artifact(Artifacts::File(spec::File {
                name: "kzg-trusted-setup".to_string(),
                target_path: "/data/kzg-trusted-setup.json".to_string(),
                content: bera_chain_file(chain_id, "kzg-trusted-setup.json"),
            }))
            .artifact(Artifacts::File(spec::File {
                name: "config".to_string(),
                target_path: "/data/config.toml".to_string(),
                content: config_file.render().to_string(),
            }))
            .artifact(Artifacts::File(spec::File {
                name: "app".to_string(),
                target_path: "/data/app.toml".to_string(),
                content: app_file.render().to_string(),
            }));

        Ok(Pod::default().with_spec("node", node))
    }
}

#[derive(Default, Deserialize)]
pub struct BeraReth {}

impl ComputeResource for BeraReth {
    type Chains = Chains;

    fn spec(&self, chain: Chains) -> eyre::Result<Pod> {
        let chain_id = chain.chain_id();

        let node = Spec::builder()
            .image("ghcr.io/berachain/bera-reth")
            .tag("v1.3.0")
            .volume(Volume {
                name: "data".to_string(),
                path: "/data".to_string(),
            })
            .arg2("--chain", "/data/genesis.json")
            .arg2(
                "--http.port",
                Arg::Port {
                    name: "http".to_string(),
                    preferred: 8545,
                },
            )
            .arg2("--http.addr", "0.0.0.0")
            .arg("--http")
            .with_babel(Babel::new(
                "ethereum",
                Arg::Ref {
                    name: "berareth-reth".to_string(),
                    port: "http".to_string(),
                },
            ))
            .artifact(Artifacts::File(spec::File {
                name: "eth-genesis".to_string(),
                target_path: "/data/eth-genesis.json".to_string(),
                content: bera_chain_file(chain_id, "eth-genesis.json"),
            }));

        Ok(Pod::default().with_spec("reth", node))
    }
}
