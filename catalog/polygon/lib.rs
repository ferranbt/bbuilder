use serde::{Deserialize, Serialize};
use spec::{
    Arg, Artifacts, Babel, ComputeResource, Deployment, DeploymentExtension, Generated, Manifest,
    Pod, Spec, Volume,
};
use template::Template;

#[derive(Default, Clone)]
pub enum Chains {
    #[default]
    Mainnet,
    Amoy,
}

impl Chains {
    fn cosmos_chain_id(&self) -> &str {
        match self {
            Chains::Mainnet => "heimdallv2-137",
            Chains::Amoy => "heimdallv2-80002",
        }
    }

    fn name(&self) -> &str {
        match self {
            Chains::Mainnet => "mainnet",
            Chains::Amoy => "amoy",
        }
    }
}

#[derive(Default, Deserialize)]
pub struct Heimdall {}

#[derive(Template, Serialize)]
#[template(path = "heimdall/client.toml")]
struct HeimdallClientConfigFile {
    chain: String,
}

impl ComputeResource for Heimdall {
    type Chains = Chains;

    fn spec(&self, chain: Chains) -> eyre::Result<Pod> {
        let app_config = include_str!("heimdall/app.toml");
        let config_config = include_str!("heimdall/config.toml");
        let client_config = HeimdallClientConfigFile {
            chain: chain.cosmos_chain_id().to_string(),
        };

        let val_keys_state = "{
  \"height\": \"0\",
  \"round\": 0,
  \"step\": 0
}";

        let node = Spec::builder()
            .image("0xpolygon/heimdall-v2")
            .entrypoint(["/usr/bin/heimdalld"])
            .tag("0.2.16")
            .volume(Volume {
                name: "data".to_string(),
                path: "/data".to_string(),
            })
            .arg("start")
            .arg2("--home", "/data/heimdall")
            .arg2(
                "--api.address",
                Arg::Port {
                    name: "http".to_string(),
                    preferred: 1317,
                },
            )
            .with_babel(Babel::new(
                "cosmos",
                Arg::Ref {
                    name: "heimdall-node".to_string(),
                    port: "http".to_string(),
                },
            ))
            .artifact(Artifacts::File(spec::File::remote(
                "genesis",
                "/data/heimdall/config/genesis.json",
                "https://storage.googleapis.com/amoy-heimdallv2-genesis/migrated_dump-genesis.json",
            )))
            .artifact(Artifacts::File(spec::File::inline(
                "client.toml",
                "/data/heimdall/config/client.toml",
                client_config.render(),
            )))
            .artifact(Artifacts::File(spec::File::inline(
                "app.toml",
                "/data/heimdall/config/app.toml",
                app_config,
            )))
            .artifact(Artifacts::File(spec::File::inline(
                "config.toml",
                "/data/heimdall/config/config.toml",
                config_config,
            )))
            .artifact(Artifacts::File(spec::File::generated(
                "node_key.json",
                "/data/heimdall/config/node_key.json",
                Generated::Ed25519TendermintNodeKey,
            )))
            .artifact(Artifacts::File(spec::File::generated(
                "priv_validator_key.json",
                "/data/heimdall/config/priv_validator_key.json",
                Generated::Secp256k1CometBftValidatorKey,
            )))
            .artifact(Artifacts::File(spec::File::inline(
                "priv_validator_state.json",
                "/data/heimdall/data/priv_validator_state.json",
                val_keys_state,
            )));

        Ok(Pod::default().with_spec("node", node))
    }
}

#[derive(Template, Serialize)]
#[template(path = "bor/config.toml")]
pub struct BorConfig {
    chain: String,
    data_dir: String,
}

#[derive(Default, Deserialize)]
pub struct Bor {}

fn bor_genesis(chain: Chains) -> String {
    let filename = match chain {
        Chains::Mainnet => "genesis-mainnet-v1",
        Chains::Amoy => "genesis-testnet-v4.json",
    };

    format!(
        "https://raw.githubusercontent.com/0xPolygon/bor/master/builder/files/{}.json",
        filename
    )
}

impl ComputeResource for Bor {
    type Chains = Chains;

    fn spec(&self, chain: Chains) -> eyre::Result<Pod> {
        let config = BorConfig {
            chain: chain.name().to_string(),
            data_dir: "/data".to_string(),
        };

        let node = Spec::builder()
            .image("0xpolygon/bor")
            .tag("1.1.0")
            .volume(Volume {
                name: "data".to_string(),
                path: "/data".to_string(),
            })
            .arg("server")
            .arg2("--config", "/data/config.toml")
            .artifact(Artifacts::File(spec::File::inline(
                "config",
                "/data/config.toml",
                config.render(),
            )))
            .artifact(Artifacts::File(spec::File::remote(
                "genesis.json",
                "/data/genesis.json",
                bor_genesis(chain),
            )));

        Ok(Pod::default().with_spec("bor", node))
    }
}

#[derive(Default, Deserialize)]
pub struct PolygonDeploymentInput {
    pub heimdall: Heimdall,
    pub bor: Bor,
}

#[derive(Default, Deserialize)]
pub struct PolygonDeployment {}

impl Deployment for PolygonDeployment {
    type Input = PolygonDeploymentInput;
    type Chains = Chains;

    fn manifest(&self, chain: Chains, input: PolygonDeploymentInput) -> eyre::Result<Manifest> {
        let mut manifest = Manifest::new("polygon".to_string());

        let heimdall_pod = input.heimdall.spec(chain.clone())?;
        manifest.add_spec("heimdall".to_string(), heimdall_pod);
        manifest.add_spec("bor".to_string(), input.bor.spec(chain)?);

        Ok(manifest)
    }
}
