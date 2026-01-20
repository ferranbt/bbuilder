use cosmos_keys::{generate_cometbft_key, generate_tendermint_key};
use serde::{Deserialize, Serialize};
use spec::{
    Arg, Artifacts, Babel, ComputeResource, Deployment, DeploymentExtension, Manifest, Pod, Spec,
    Volume,
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

        let keys = generate_tendermint_key().serialize()?;
        let val_keys = generate_cometbft_key().serialize()?;

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
            .artifact(Artifacts::File(spec::File{
                name: "genesis".to_string(),
                target_path: "/data/heimdall/config/genesis.json".to_string(),
                content: "https://storage.googleapis.com/amoy-heimdallv2-genesis/migrated_dump-genesis.json".to_string(),
            }))
            .artifact(Artifacts::File(spec::File{
                name: "client.toml".to_string(),
                target_path: "/data/heimdall/config/client.toml".to_string(),
                content: client_config.render().to_string(),
            }))
            .artifact(Artifacts::File(spec::File{
                name: "app.toml".to_string(),
                target_path: "/data/heimdall/config/app.toml".to_string(),
                content: app_config.to_string(),
            }))
            .artifact(Artifacts::File(spec::File{
                name: "config.toml".to_string(),
                target_path: "/data/heimdall/config/config.toml".to_string(),
                content: config_config.to_string(),
            }))
            .artifact(Artifacts::File(spec::File{
                name: "node_key.json".to_string(),
                target_path: "/data/heimdall/config/node_key.json".to_string(),
                content: keys,
            }))
            .artifact(Artifacts::File(spec::File{
                name: "priv_validator_key.json".to_string(),
                target_path: "/data/heimdall/config/priv_validator_key.json".to_string(),
                content: val_keys,
            }))
            .artifact(Artifacts::File(spec::File{
                name: "priv_validator_state.json".to_string(),
                target_path: "/data/heimdall/data/priv_validator_state.json".to_string(),
                content: val_keys_state.to_string(),
            }));

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
            .artifact(Artifacts::File(spec::File {
                name: "config".to_string(),
                target_path: "/data/config.toml".to_string(),
                content: config.render(),
            }))
            .artifact(Artifacts::File(spec::File {
                name: "genesis.json".to_string(),
                target_path: "/data/genesis.json".to_string(),
                content: bor_genesis(chain),
            }));

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
