use serde::Deserialize;
use spec::{
    Arg, Artifacts, Babel, ComputeResource, DEFAULT_JWT_TOKEN, Deployment, DeploymentExtension,
    Manifest, Pod, Port, Spec, Volume,
};

#[derive(Default, Clone)]
pub enum Chains {
    #[default]
    Mainnet,
    Sepolia,
}

#[derive(Default, Deserialize)]
pub struct EthereumDeployment {}

#[derive(Debug, Deserialize)]
pub struct EthDeploymentInput {
    pub el_node: ELNode,
    pub cl_node: CLNode,
}

impl Deployment for EthereumDeployment {
    type Input = EthDeploymentInput;
    type Chains = Chains;

    fn manifest(&self, chain: Chains, input: EthDeploymentInput) -> eyre::Result<Manifest> {
        let mut manifest = Manifest::new("eth".to_string());

        let el_node = match input.el_node {
            ELNode::Reth(reth) => reth.spec(chain.clone()),
        }?;
        manifest.add_spec("el".to_string(), el_node);

        let cl_node = match input.cl_node {
            CLNode::Lighthouse(lighthouse) => lighthouse.spec(chain.clone()),
            CLNode::Prysm(prysm) => prysm.spec(chain.clone()),
        }?;

        // Add Babel sidecar to CL pod
        manifest.add_spec("cl".to_string(), cl_node);

        Ok(manifest)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ELNode {
    Reth(Reth),
}

#[derive(Debug, Default, Deserialize)]
pub struct Reth {}

impl ComputeResource for Reth {
    type Chains = Chains;

    fn spec(&self, chain: Chains) -> eyre::Result<Pod> {
        let chain_arg = match chain {
            Chains::Mainnet => "mainnet",
            Chains::Sepolia => "sepolia",
        };

        let node = Spec::builder()
            .image("ghcr.io/paradigmxyz/reth")
            .tag("v1.4.8")
            .volume(Volume {
                name: "data".to_string(),
                path: "/data".to_string(),
            })
            .arg("node")
            .arg2("--chain", chain_arg)
            .arg("--full")
            .arg2("--color", "never")
            .arg2(
                "--authrpc.port",
                Arg::Port {
                    name: "authrpc".to_string(),
                    preferred: 8551,
                },
            )
            .arg2("--authrpc.addr", "0.0.0.0")
            .arg2("--authrpc.jwtsecret", "/data/jwt_secret")
            .arg2(
                "--http.port",
                Arg::Port {
                    name: "http".to_string(),
                    preferred: 8545,
                },
            )
            .arg2("--http.addr", "0.0.0.0")
            .arg("--http")
            .arg2("--metrics", "0.0.0.0:9090")
            .arg2("--datadir", "/data")
            .port(Port {
                port: 9090,
                name: "metrics".to_string(),
            })
            .with_babel(Babel::new(
                "ethereum",
                Arg::Ref {
                    name: "el-node".to_string(),
                    port: "http".to_string(),
                },
            ))
            .artifact(Artifacts::File(spec::File {
                name: "jwt".to_string(),
                target_path: "/data/jwt_secret".to_string(),
                content: DEFAULT_JWT_TOKEN.to_string(),
            }));

        Ok(Pod::default().with_spec("node", node))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CLNode {
    Prysm(Prysm),
    Lighthouse(Lighthouse),
}

#[derive(Debug, Default, Deserialize)]
pub struct Lighthouse {}

impl ComputeResource for Lighthouse {
    type Chains = Chains;

    fn spec(&self, chain: Chains) -> eyre::Result<Pod> {
        let chain_arg = match chain {
            Chains::Mainnet => "mainnet",
            Chains::Sepolia => "sepolia",
        };

        let node = Spec::builder()
            .image("sigp/lighthouse")
            .tag("v8.0.0-rc.2")
            .entrypoint(["lighthouse"])
            .volume(Volume {
                name: "data".to_string(),
                path: "/data".to_string(),
            })
            .arg("bn")
            .arg2("--network", chain_arg)
            .arg2(
                "--execution-endpoint",
                Arg::Ref {
                    name: "el-node".to_string(),
                    port: "authrpc".to_string(),
                },
            )
            .arg2("--execution-jwt", "/data/jwt_secret")
            .arg2(
                "--http-port",
                Arg::Port {
                    name: "http".to_string(),
                    preferred: 5052,
                },
            )
            .arg2("--http-address", "0.0.0.0")
            .arg("--http")
            .arg2("--datadir", "/data")
            .with_babel(Babel::new(
                "ethereum_beacon",
                Arg::Ref {
                    name: "cl-node".to_string(),
                    port: "http".to_string(),
                },
            ))
            .artifact(Artifacts::File(spec::File {
                name: "jwt".to_string(),
                target_path: "/data/jwt_secret".to_string(),
                content: DEFAULT_JWT_TOKEN.to_string(),
            }));

        Ok(Pod::default().with_spec("node", node))
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct Prysm {}

impl ComputeResource for Prysm {
    type Chains = Chains;

    fn spec(&self, chain: Chains) -> eyre::Result<Pod> {
        let chain_arg = match chain {
            Chains::Mainnet => "--mainnet",
            Chains::Sepolia => "--sepolia",
        };

        let node = Spec::builder()
            .image("gcr.io/prysmaticlabs/prysm/beacon-chain")
            .tag("v6.0.0")
            .arg(chain_arg)
            .arg2(
                "--datadir",
                Arg::Dir {
                    name: "prysm_data".to_string(),
                    path: "/data".to_string(),
                },
            )
            .arg2(
                "--execution-endpoint",
                Arg::Ref {
                    name: "el-node".to_string(),
                    port: "authrpc".to_string(),
                },
            )
            .arg2("--jwt-secret", "/data/jwt_secret".to_string())
            .arg2("--grpc-gateway-host", "0.0.0.0")
            .arg2(
                "--grpc-gateway-port",
                Arg::Port {
                    name: "http".to_string(),
                    preferred: 5052,
                },
            )
            .arg2("--monitoring-host", "0.0.0.0")
            .arg2(
                "--monitoring-port",
                Arg::Port {
                    name: "metrics".to_string(),
                    preferred: 8080,
                },
            )
            .arg("--accept-terms-of-use")
            .with_babel(Babel::new(
                "ethereum_beacon",
                Arg::Ref {
                    name: "cl-node".to_string(),
                    port: "http".to_string(),
                },
            ))
            .artifact(Artifacts::File(spec::File {
                name: "jwt".to_string(),
                target_path: "/data/jwt_secret".to_string(),
                content: DEFAULT_JWT_TOKEN.to_string(),
            }));

        Ok(Pod::default().with_spec("node", node))
    }
}
