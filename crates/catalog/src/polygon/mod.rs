use base64::{Engine as _, engine::general_purpose};
use ed25519_dalek::SigningKey;
use k256::ecdsa::SigningKey as kSigningKey;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};
use spec::{
    Artifacts, Capabilities, ChainSpec, ComputeResource, Deployment, Manifest, Pod, Spec, Volume,
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

    fn capabilities(&self) -> Capabilities<Chains> {
        Capabilities {
            chains: vec![
                ChainSpec {
                    chain: Chains::Mainnet,
                    min_version: "v1.4.8".to_string(),
                },
                ChainSpec {
                    chain: Chains::Amoy,
                    min_version: "v1.4.8".to_string(),
                },
            ],
            volumes: vec![Volume {
                name: "data".to_string(),
            }],
        }
    }

    fn spec(&self, chain: Chains) -> eyre::Result<Pod> {
        let app_config = include_str!("heimdall/app.toml");
        let config_config = include_str!("heimdall/config.toml");
        let client_config = HeimdallClientConfigFile {
            chain: chain.cosmos_chain_id().to_string(),
        };

        let keys = generate_tendermint_key();
        let val_keys = generate_cometbft_key();

        let val_keys_state = "{
  \"height\": \"0\",
  \"round\": 0,
  \"step\": 0
}";

        let node = Spec::builder()
            .image("0xpolygon/heimdall-v2")
            .entrypoint(["/usr/bin/heimdalld"])
            .tag("0.2.16")
            .arg("start")
            .arg2("--home", "/data/heimdall")
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

#[derive(Serialize, Deserialize)]
struct PrivKeyWrapper {
    priv_key: PrivKey,
}

#[derive(Serialize, Deserialize)]
struct PrivKey {
    #[serde(rename = "type")]
    key_type: String,
    value: String,
}

fn generate_tendermint_key() -> String {
    let signing_key = SigningKey::generate(&mut OsRng);
    let private_bytes = signing_key.to_bytes();
    let public_bytes = signing_key.verifying_key().to_bytes();

    let mut combined = Vec::with_capacity(64);
    combined.extend_from_slice(&private_bytes);
    combined.extend_from_slice(&public_bytes);

    let encoded = general_purpose::STANDARD.encode(&combined);

    let key_wrapper = PrivKeyWrapper {
        priv_key: PrivKey {
            key_type: "tendermint/PrivKeyEd25519".to_string(),
            value: encoded,
        },
    };

    serde_json::to_string(&key_wrapper).unwrap()
}

#[derive(Serialize, Deserialize)]
struct ValidatorKey {
    address: String,
    pub_key: PubKey2,
    priv_key: PrivKey2,
}

#[derive(Serialize, Deserialize)]
struct PubKey2 {
    #[serde(rename = "type")]
    key_type: String,
    value: String,
}

#[derive(Serialize, Deserialize)]
struct PrivKey2 {
    #[serde(rename = "type")]
    key_type: String,
    value: String,
}

fn generate_cometbft_key() -> String {
    // Generate secp256k1 key pair
    let signing_key = kSigningKey::random(&mut OsRng);
    let verifying_key = signing_key.verifying_key();

    // Get private key bytes (32 bytes)
    let private_bytes = signing_key.to_bytes();
    let priv_base64 = general_purpose::STANDARD.encode(&private_bytes);

    // Get public key bytes (uncompressed, 65 bytes)
    let public_bytes = verifying_key.to_encoded_point(false);
    let pub_base64 = general_purpose::STANDARD.encode(public_bytes.as_bytes());

    // Generate address: first 20 bytes of keccak256(public_key)
    let hash = Keccak256::digest(&public_bytes.as_bytes()[1..]); // Skip the 0x04 prefix
    let address_bytes = &hash[12..]; // Take last 20 bytes
    let address = hex::encode_upper(address_bytes);

    let validator_key = ValidatorKey {
        address,
        pub_key: PubKey2 {
            key_type: "cometbft/PubKeySecp256k1eth".to_string(),
            value: pub_base64,
        },
        priv_key: PrivKey2 {
            key_type: "cometbft/PrivKeySecp256k1eth".to_string(),
            value: priv_base64,
        },
    };

    serde_json::to_string_pretty(&validator_key).unwrap()
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

    fn capabilities(&self) -> Capabilities<Chains> {
        Capabilities {
            chains: vec![],
            volumes: vec![],
        }
    }

    fn spec(&self, chain: Chains) -> eyre::Result<Pod> {
        let config = BorConfig {
            chain: chain.name().to_string(),
            data_dir: "/data".to_string(),
        };

        let node = Spec::builder()
            .image("0xpolygon/bor")
            .tag("1.1.0")
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

    fn capabilities(&self) -> Vec<ChainSpec<Chains>> {
        vec![
            ChainSpec {
                chain: Chains::Mainnet,
                min_version: "".to_string(),
            },
            ChainSpec {
                chain: Chains::Amoy,
                min_version: "".to_string(),
            },
        ]
    }

    fn manifest(&self, chain: Chains, input: PolygonDeploymentInput) -> eyre::Result<Manifest> {
        let mut manifest = Manifest::new("polygon".to_string());
        manifest.add_spec("heimdall".to_string(), input.heimdall.spec(chain.clone())?);
        manifest.add_spec("bor".to_string(), input.bor.spec(chain)?);

        Ok(manifest)
    }
}
